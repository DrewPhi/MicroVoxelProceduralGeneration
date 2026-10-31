//! Headless experimental landscape compiler. Does not change the playable world.
use microvoxel::generation::landscape_experiment::{Config, Landscape};
use std::{
    fs,
    io::{BufWriter, Write},
    path::PathBuf,
    time::Instant,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut c = Config::default();
    let mut output = PathBuf::from("output/landscape-milestone/seed42");
    let mut baseline = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seed" => c.seed = args.next().ok_or("missing seed")?.parse()?,
            "--side" => c.side = args.next().ok_or("missing side")?.parse()?,
            "--extent-km" => {
                c.extent = args.next().ok_or("missing extent")?.parse::<f64>()? * 1000.
            }
            "--steps" => c.steps = args.next().ok_or("missing steps")?.parse()?,
            "--dt" => c.dt = args.next().ok_or("missing dt")?.parse()?,
            "--atlas" => c.atlas = true,
            "--climate" => {
                c.atlas = true;
                c.climate = true;
            }
            "--wind-deg" => {
                c.climate_settings.wind_degrees = args.next().ok_or("missing wind")?.parse()?
            }
            "--latitude" => {
                c.climate_settings.latitude_at_z0 =
                    args.next().ok_or("missing latitude")?.parse()?
            }
            "--boundary-humidity" => {
                c.climate_settings.boundary_humidity =
                    args.next().ok_or("missing humidity")?.parse()?
            }
            "--raster" => c.irregular = false,
            "--baseline" => baseline = true,
            "--output" => output = args.next().ok_or("missing output")?.into(),
            _ => return Err(format!("unknown option {arg}").into()),
        }
    }
    if !(3..=2049).contains(&c.side)
        || !(1..=2000).contains(&c.steps)
        || !c.dt.is_finite()
        || c.dt <= 0.
        || c.dt > 100_000.
        || !c.extent.is_finite()
        || !(10_000. ..=1_000_000.).contains(&c.extent)
        || !c.climate_settings.valid()
    {
        return Err("invalid size/steps/dt".into());
    }
    if baseline && (c.extent != 100_000. || c.atlas) {
        return Err("baseline comparison is defined only for the 100 km fixture".into());
    }
    fs::create_dir_all(&output)?;
    let start = Instant::now();
    let mut l = Landscape::new(c);
    if baseline {
        let terrain = microvoxel::generation::ProceduralTerrain::with_spec(
            c.seed,
            Default::default(),
            microvoxel::WorldSpec {
                extent_m: 100_000,
                ..Default::default()
            },
        )?;
        for (i, p) in l.positions.iter().enumerate() {
            l.height[i] = terrain
                .sample_m(p[0].min(99_999.), p[1].min(99_999.))
                .ok_or("baseline sample outside world")?
                .height_m;
        }
        l.initial.clone_from(&l.height);
        l.uplift.fill(0.);
        l.route();
    } else {
        l.evolve();
    }
    let diagnostic = l.diagnostics();
    let diagnostic = if baseline {
        diagnostic.split("uplifted_m3=").next().unwrap()
    } else {
        &diagnostic
    };
    let mut summary = format!(
        "model={}\n{}elapsed_seconds={:.3}\n",
        if baseline {
            "generator6"
        } else {
            "coupled_evolution"
        },
        diagnostic,
        start.elapsed().as_secs_f64()
    );
    if let Some(h) = &l.hydrology {
        summary.push_str(&format!("climate_model=ocean_transport_v1\nlatitude_at_z0={}\nwind_degrees={}\nboundary_humidity={}\npermanent_lakes={}\nseasonal_closed_basins={}\nrunoff_m3_year={:.6}\nwater_export_m3_year={:.6}\nextra_lake_evaporation_m3_year={:.6}\nwater_budget_residual_m3_year={:.6}\n",
            c.climate_settings.latitude_at_z0,c.climate_settings.wind_degrees,c.climate_settings.boundary_humidity,
            h.lakes.iter().filter(|l|l.permanent).count(),h.lakes.iter().filter(|l|!l.permanent).count(),
            h.runoff_m3_year,h.export_m3_year,h.extra_evaporation_m3_year,h.residual()));
        let mut climate = BufWriter::new(fs::File::create(output.join("climate.bin"))?);
        for cell in &l.climate {
            for v in cell.fields() {
                climate.write_all(&v.to_le_bytes())?;
            }
        }
        let mut flow = BufWriter::new(fs::File::create(output.join("flow.bin"))?);
        let mut watersheds = BufWriter::new(fs::File::create(output.join("watershed.bin"))?);
        let mut lake_ids = BufWriter::new(fs::File::create(output.join("lake-id.bin"))?);
        for i in 0..l.height.len() {
            flow.write_all(
                &(h.discharge[i] / microvoxel::generation::atlas_hydrology::SECONDS_PER_YEAR)
                    .to_le_bytes(),
            )?;
            flow.write_all(
                &microvoxel::generation::atlas_hydrology::channel_width(h.discharge[i])
                    .to_le_bytes(),
            )?;
            watersheds.write_all(&h.watershed[i].to_le_bytes())?;
            lake_ids.write_all(&h.lake_id[i].to_le_bytes())?;
        }
        let mut lakes = BufWriter::new(fs::File::create(output.join("lakes.csv"))?);
        writeln!(lakes,"id,outlet_node,spill_m,permanent,inflow_m3_year,extra_evaporation_m3_year,outflow_m3_year,storage_m3")?;
        for (i, lake) in h.lakes.iter().enumerate() {
            writeln!(
                lakes,
                "{i},{},{},{},{},{},{},{}",
                lake.outlet,
                lake.spill_m,
                lake.permanent,
                lake.inflow_m3_year,
                lake.extra_evaporation_m3_year,
                lake.outflow_m3_year,
                lake.storage_m3
            )?;
        }
    }
    println!("{summary}");
    fs::write(output.join("summary.txt"), summary)?;
    if c.atlas {
        fs::write(
            output.join("ocean.bin"),
            l.ocean_mask()
                .iter()
                .map(|&v| u8::from(v))
                .collect::<Vec<_>>(),
        )?;
    }
    let mut water = BufWriter::new(fs::File::create(output.join("water.bin"))?);
    for level in l.water_surface() {
        water.write_all(&level.to_le_bytes())?;
    }
    let mut nodes = BufWriter::new(fs::File::create(output.join("nodes.bin"))?);
    for i in 0..l.height.len() {
        for v in [
            l.positions[i][0],
            l.positions[i][1],
            l.height[i],
            l.initial[i],
            l.uplift[i],
            l.area[i],
            l.sediment[i],
            l.receiver[i] as f64,
        ] {
            nodes.write_all(&v.to_le_bytes())?;
        }
    }
    let mut triangles = BufWriter::new(fs::File::create(output.join("triangles.bin"))?);
    for t in &l.triangles {
        for v in t {
            triangles.write_all(&v.to_le_bytes())?;
        }
    }
    Ok(())
}
