//! Offline growth fixture using the runtime grammar and resource accounting.
//! Supplies finite real pond water directly to soil for accelerated validation;
//! this bypasses bowl transport, not the plant's energy, water or geometry rules.
use frontier_world::{
    bowl::Bowl, desert, garden::Garden, pockets::Pockets, Cell, FbmParams, FbmRecipe, World,
    VOXEL_SIZE,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or("output/recursive-plant/world.bowl".into());
    let seconds = std::env::args()
        .nth(2)
        .unwrap_or("2000".into())
        .parse::<usize>()?;
    let params = FbmParams {
        recipe: FbmRecipe::Desert,
        ..Default::default()
    };
    let mut world = World::fbm(19, params)?;
    let o = desert::oasis(19, 0, 0);
    let mut top = Cell::from_meters([o.x + 60., 0., o.z]);
    top.y = (world.ground_height_m(top.x as f64 * VOXEL_SIZE, top.z as f64 * VOXEL_SIZE)
        / VOXEL_SIZE) as i64
        - 1;
    let mut garden = Garden::default();
    let mut water = Bowl::default();
    garden.dig(&mut world, top)?;
    garden.plant(&mut world, 0)?;
    garden.cover(&mut world, 0)?;
    let pockets = Pockets::migrate(&mut world, &garden)?;
    garden.sand = 0;
    garden.soil = 0;
    let mut refills = 0;
    for _ in 0..seconds {
        if water.moisture.values().sum::<u32>() < 8 {
            let x = o.x + ((refills % 32) as f64 - 16.) * 0.125;
            let z = o.z + ((refills / 32) as f64 - 16.) * 0.125;
            let n = water.scoop(&mut world, [x, o.stage + 0.5, z], [0., -1., 0.])?;
            water.units -= n;
            water.emitted += u64::from(n);
            *water.moisture.entry(top).or_default() += u32::from(n);
            refills += 1;
        }
        for _ in 0..4 {
            garden.advance(&mut world, &mut water, 0.25, [0.2, 0.95, 0.24]);
        }
    }
    assert!(water.conserved());
    assert_eq!(garden.water_accounted(), water.plant_transferred);
    let plant = &garden.plants[0];
    let max_y = plant.tissue.keys().map(|c| c.y).max().unwrap();
    let height = (max_y - garden.holes[0].seed.y) as f64 * VOXEL_SIZE;
    eprintln!("seconds={seconds} cells={} leaves={} height_m={height:.2} active_tips={} energy={:.2} water_units={}",garden.cells(),plant.tissue.values().filter(|t|**t==frontier_world::garden::Tissue::Leaf).count(),plant.active_tips(),plant.energy,water.scooped);
    let path = std::path::Path::new(&output);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let save = format!(
        "DESERT_BOWL1 19 {} {} {}\n{}{}{}",
        desert::VERSION,
        params.horizontal_scale.to_bits(),
        params.amplitude.to_bits(),
        water.snapshot(19),
        garden.snapshot(),
        pockets.snapshot()
    );
    std::fs::write(path, save)?;
    let cells = plant
        .tissue
        .iter()
        .map(|(c, t)| format!("{} {} {} {}\n", c.x, c.y, c.z, *t as u8))
        .collect::<String>();
    std::fs::write(path.with_extension("cells"), cells)?;
    println!(
        "plant_base_m={:?}; saved {}",
        garden.holes[0].seed.center_m(),
        path.display()
    );
    Ok(())
}
