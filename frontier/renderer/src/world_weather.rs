//! Game-loop integration for Frontier's world-space atmosphere. Simulation and
//! instrument scans run on a worker. The renderer and game use one published pair.
use frontier_runtime::weather::{
    self, radar, regional, Climate, Regime, Simulation, Snapshot, SolarClock, STEP_SECONDS,
};
use glam::{DVec3, Vec3};
use std::time::{Duration, Instant};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
};

#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub regime: Regime,
    pub age_seconds: f64,
    pub day: u32,
    pub hour: f64,
    pub prevailing_wind: Vec3,
    pub center: Option<[f64; 2]>,
    pub day_length: f64,
    pub latitude_deg: f32,
    pub checkpoint: Option<PathBuf>,
}

impl Config {
    pub fn start_time(&self) -> f64 {
        f64::from(self.day) * 86400. + self.hour * 3600.
    }
}

/// Presentation time is capped at the latest complete solver state. A slow
/// worker accumulates at most one step of credit, including at high speeds.
pub struct Clock {
    pub seconds: f64,
    pub solar: SolarClock,
    pub achieved: f32,
    credit: f64,
}

impl Clock {
    pub fn new(seconds: f64, solar: SolarClock) -> Self {
        Self {
            seconds,
            solar,
            achieved: 0.,
            credit: 0.,
        }
    }

    pub fn tick(&mut self, elapsed: f64, speed: f32, day_length: f64, latest: Option<f64>) {
        self.solar.retime(self.seconds, day_length);
        if speed == 0. {
            self.credit = 0.;
            self.achieved = 0.;
            return;
        }
        let before = self.seconds;
        let delta = elapsed * f64::from(speed);
        if let Some(latest) = latest {
            let budget = self.credit + delta;
            let advanced = budget.min((latest - self.seconds).max(0.));
            self.seconds += advanced;
            self.credit = (budget - advanced).min(f64::from(STEP_SECONDS));
        } else {
            self.seconds += delta;
            self.credit = 0.;
        }
        if elapsed > 0. {
            let rate = ((self.seconds - before) / elapsed) as f32;
            let weight = (1. - (-elapsed * 2.).exp()) as f32;
            self.achieved += (rate - self.achieved) * weight;
        }
    }

    pub fn solar_day(&self) -> f64 {
        self.solar.day(self.seconds)
    }
}

pub(crate) struct Frame {
    pub previous: Arc<Snapshot>,
    pub current: Arc<Snapshot>,
    pub mosaic: Option<Arc<radar::Datalink>>,
    pub solver_ms: f64,
    pub solar: SolarClock,
    pub packed_volume: [Arc<[[u8; 2]]>; 2],
}

impl Frame {
    pub fn sky_light(&self, clock: &Clock) -> crate::celestial::Light {
        self.current.regional.as_ref().map_or_else(
            || crate::celestial::Light::at(clock.solar_day()),
            |r| {
                crate::celestial::Light::regional(
                    clock.solar_day(),
                    clock.seconds,
                    r.current.latitude_deg,
                )
            },
        )
    }

    pub fn fraction(&self, seconds: f64) -> f32 {
        let gap = self.current.time - self.previous.time;
        if gap == 0. {
            1.
        } else {
            ((seconds - self.previous.time) / gap).clamp(0., 1.) as f32
        }
    }

    pub fn interpolate_wind(&self, position: DVec3, fraction: f32) -> Vec3 {
        let t = self.previous.time + f64::from(fraction) * (self.current.time - self.previous.time);
        weather::gust(&self.previous, position, t)
            .lerp(weather::gust(&self.current, position, t), fraction)
    }

    pub fn density_ratio(&self, position: DVec3, fraction: f32) -> f32 {
        let Some(regional) = &self.current.regional else {
            return 1.;
        };
        let time =
            self.previous.time + f64::from(fraction) * (self.current.time - self.previous.time);
        let a = self.previous.sample(position);
        let b = self.current.sample(position);
        let temperature = a.temperature_k + (b.temperature_k - a.temperature_k) * fraction;
        let vapor = a.vapor + (b.vapor - a.vapor) * fraction;
        let standard = 288.15 - 0.0065 * (position.y as f32).clamp(-500., 11_000.);
        (regional.pressure_ratio(position, time) * standard / (temperature * (1. + 0.61 * vapor)))
            .clamp(0.75, 1.25)
    }

    pub fn report(&self, position: DVec3) -> String {
        let m = self.current.metrics;
        let air = self.current.sample(position);
        let regional = self.current.regional.as_ref().map_or_else(|| String::from("null"), |r| {
            let pressure = r.pressure_ratio(position, self.current.time) * 1013.25;
            let upper_position = DVec3::new(position.x, 10_000., position.z);
            let wind = r.sample(upper_position, self.current.time)
                .map_or_else(|| String::from("null"), |air| format!("{:?}", air.wind.to_array()));
            format!("{{\"reference_latitude_deg\":{},\"regional_step_seconds\":{},\"pressure_hpa\":{pressure},\"upper_wind_mps\":{wind}}}",
                r.current.latitude_deg, regional::STEP)
        });
        format!(
            "{{\"seconds\":{},\"solver_step_ms\":{},\"origin_m\":{:?},\"resolved_at_view\":{},\"wind_mps\":{:?},\"turbulence_mps\":{},\"local_rain_mm_h\":{},\"cloud_cells\":{},\"rain_cells\":{},\"cloud_base_m\":{},\"cloud_top_m\":{},\"peak_rain_mm_h\":{},\"rain_core_m\":{:?},\"updraft_mps\":{},\"downdraft_mps\":{},\"datalink_age_s\":{},\"regional\":{regional}}}",
            self.current.time, self.solver_ms, self.current.origin.to_array(),
            air.resolved, air.wind.to_array(), air.turbulence, air.rain_rate_mm_h(),
            m.cloud_cells, m.rain_cells, m.cloud_base_m, m.cloud_top_m,
            m.peak_rain_mm_h, m.rain_core_m, m.updraft_mps, m.downdraft_mps,
            self.mosaic.as_ref().map_or("null".into(), |m| (self.current.time - m.scan_time).to_string())
        )
    }
}

fn sample_surface(
    world: &frontier_world::World,
    x: f64,
    z: f64,
    coarse: bool,
) -> regional::Surface {
    use frontier_world::terrain::SeededBiome;
    let Some(terrain) = world
        .fbm_terrain()
        .filter(|t| t.params().recipe.has_static_water())
    else {
        return regional::Surface {
            ground: world.ground_height_m(x, z) as f32,
            ..Default::default()
        };
    };
    if coarse {
        let [ground, warmth, moisture] = terrain.sample_climate_surface(x, z);
        let water = (0.5 - ground / 180.).clamp(0., 1.);
        return regional::Surface {
            ground: ground.max(0.) as f32,
            warmth: warmth as f32,
            moisture: moisture as f32,
            water: water as f32,
            roughness: (0.25 + moisture * 0.42 - water * 0.21) as f32,
        };
    }
    let f = terrain.sample_fields(x, z);
    // The seeded fields already include an elevation adjustment. Recover a
    // sea-level climate normal before the solver applies its own lapse rate.
    let warmth = (f.temperature + 0.00019 * f.ground_height_m.max(0.)).clamp(0., 1.);
    let roughness = match f.biome {
        SeededBiome::Woodland => 0.85,
        SeededBiome::Mountains => 0.68,
        SeededBiome::Desert => 0.23,
        SeededBiome::Wetland => 0.45,
        SeededBiome::Ocean => 0.06,
        _ => 0.40,
    };
    regional::Surface {
        ground: f.water_height_m.unwrap_or(f.ground_height_m) as f32,
        warmth: warmth as f32,
        moisture: f.moisture as f32,
        water: f32::from(f.water_height_m.is_some()),
        roughness,
    }
}

struct Worker {
    simulation: Simulation,
    follow_world: Option<frontier_world::World>,
    station: DVec3,
    checkpoint: Option<PathBuf>,
    next_checkpoint: f64,
    pending: Option<radar::Datalink>,
    mosaic: Option<Arc<radar::Datalink>>,
    next_scan: f64,
    current: Arc<Snapshot>,
    previous: Arc<Snapshot>,
    last_step_ms: f64,
    packed_volume: [Arc<[[u8; 2]]>; 2],
}

impl Worker {
    fn new(world: &frontier_world::World, config: Config, center: DVec3) -> io::Result<Self> {
        let climate = Climate {
            seed: world.seed(),
            regime: config.regime,
            prevailing_wind: config.prevailing_wind,
        };
        let regional = config.regime == Regime::Regional;
        let mut simulation = if regional {
            Simulation::new_regional(
                climate,
                regional::Settings {
                    seed: climate.seed,
                    latitude_deg: config.latitude_deg,
                    wind_offset: climate.prevailing_wind,
                    initial_time: config.start_time(),
                    center,
                },
                |x, z| sample_surface(world, x, z, true),
                &|x, z| sample_surface(world, x, z, false),
            )
        } else {
            Simulation::new(climate, center, config.start_time(), |x, z| {
                world.ground_height_m(x, z) as f32
            })
        };
        simulation.set_solar_clock(SolarClock::new(config.start_time(), config.day_length));
        let resumed = if let Some(path) = &config.checkpoint {
            match File::open(path) {
                Ok(file) => {
                    simulation
                        .read_checkpoint(BufReader::new(file))
                        .map_err(|e| {
                            io::Error::new(
                                e.kind(),
                                format!("weather checkpoint {}: {e}", path.display()),
                            )
                        })?;
                    true
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => false,
                Err(e) => return Err(e),
            }
        } else {
            false
        };
        let current = Arc::new(simulation.snapshot());
        let mut worker = Self {
            simulation,
            follow_world: regional.then(|| world.clone()),
            station: center,
            checkpoint: config.checkpoint,
            next_checkpoint: current.time + 600.,
            pending: Some(if regional {
                radar::Datalink::scan_at(&current, center)
            } else {
                radar::Datalink::scan(&current)
            }),
            mosaic: None,
            current: current.clone(),
            previous: current.clone(),
            next_scan: current.time + radar::DATALINK_SWEEP_SECONDS,
            last_step_ms: 0.,
            packed_volume: [Arc::new([]), Arc::new([])],
        };
        let age_until = if resumed {
            worker.current.time
        } else {
            worker.current.time + config.age_seconds
        };
        while worker.simulation.time() + 0.01 < age_until {
            worker.advance(false);
        }
        worker.current = Arc::new(worker.simulation.snapshot());
        worker.previous = worker.current.clone();
        let pixels = Arc::from(worker.current.volume_rg8());
        worker.packed_volume = [Arc::clone(&pixels), pixels];
        Ok(worker)
    }

    fn save(&self) {
        let Some(path) = &self.checkpoint else {
            return;
        };
        if let Err(error) = write_checkpoint(&self.simulation, path) {
            eprintln!("Weather checkpoint {}: {error}", path.display());
        }
    }

    fn follow(&mut self, view: DVec3) -> bool {
        let Some(world) = &self.follow_world else {
            return false;
        };
        if self
            .simulation
            .follow(view, &|x, z| sample_surface(world, x, z, false))
        {
            // Both volume textures and the local map interpolate on a common
            // origin. This snapshot preserves world cells from before the step.
            self.current = Arc::new(self.simulation.snapshot());
            self.packed_volume[1] = Arc::from(self.current.volume_rg8());
            return true;
        }
        false
    }

    fn advance(&mut self, publish: bool) {
        let start = Instant::now();
        self.simulation.advance();
        self.last_step_ms = start.elapsed().as_secs_f64() * 1000.;
        let time = self.simulation.time();
        if self
            .pending
            .as_ref()
            .is_some_and(|scan| scan.scan_time + radar::DATALINK_LATENCY_SECONDS <= time)
        {
            self.mosaic = self.pending.take().map(Arc::new);
        }
        if publish && self.checkpoint.is_some() && time >= self.next_checkpoint {
            self.save();
            self.next_checkpoint = time + 600.;
        }
        if publish || time >= self.next_scan {
            let snapshot = Arc::new(self.simulation.snapshot());
            if time >= self.next_scan {
                self.pending = Some(if snapshot.regional.is_some() {
                    radar::Datalink::scan_at(&snapshot, self.station)
                } else {
                    radar::Datalink::scan(&snapshot)
                });
                self.next_scan += radar::DATALINK_SWEEP_SECONDS;
            }
            if publish {
                self.packed_volume[0] =
                    std::mem::replace(&mut self.packed_volume[1], Arc::from(snapshot.volume_rg8()));
                self.previous = std::mem::replace(&mut self.current, snapshot);
            }
        }
    }

    fn frame(&self) -> Frame {
        Frame {
            previous: self.previous.clone(),
            current: self.current.clone(),
            mosaic: self.mosaic.clone(),
            solver_ms: self.last_step_ms,
            solar: self.simulation.solar_clock().unwrap(),
            packed_volume: self.packed_volume.clone(),
        }
    }
}

fn write_checkpoint(simulation: &Simulation, path: &Path) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        let mut output = BufWriter::new(file);
        simulation.write_checkpoint(&mut output)?;
        output.flush()?;
        output.get_ref().sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

enum Command {
    Step(SolarClock, DVec3),
    Follow(DVec3),
    Instrument(radar::Request),
}

pub(crate) struct Session {
    frame: Arc<Mutex<Arc<Frame>>>,
    instrument: Arc<Mutex<Option<Arc<radar::Image>>>>,
    sender: Option<mpsc::SyncSender<Command>>,
    worker: Option<std::thread::JoinHandle<()>>,
    last_instrument: Instant,
    requested_step: Option<f64>,
}

impl Session {
    pub fn new(
        world: &frontier_world::World,
        config: Config,
        center: DVec3,
        realtime: bool,
    ) -> std::io::Result<Self> {
        let start = Instant::now();
        let age_seconds = config.age_seconds;
        let mut worker = Worker::new(world, config, center)?;
        eprintln!(
            "Weather initialized at {:.0}s after {:.2}s: {:?}",
            age_seconds,
            start.elapsed().as_secs_f64(),
            worker.current.metrics
        );
        let frame = Arc::new(Mutex::new(Arc::new(worker.frame())));
        let instrument = Arc::new(Mutex::new(None));
        let mut session = Self {
            frame,
            instrument,
            sender: None,
            worker: None,
            last_instrument: Instant::now() - Duration::from_secs(2),
            requested_step: None,
        };
        if realtime {
            let (sender, receive) = mpsc::sync_channel(1);
            let frame = session.frame.clone();
            let instrument = session.instrument.clone();
            session.sender = Some(sender);
            session.worker = Some(
                std::thread::Builder::new()
                    .name("frontier atmosphere".into())
                    .spawn(move || {
                        while let Ok(command) = receive.recv() {
                            match command {
                                Command::Step(solar, view) => {
                                    worker.simulation.set_solar_clock(solar);
                                    worker.follow(view);
                                    worker.advance(true);
                                    *frame.lock().unwrap() = Arc::new(worker.frame());
                                }
                                Command::Follow(view) => {
                                    if worker.follow(view) {
                                        worker.previous = worker.current.clone();
                                        worker.packed_volume[0] = worker.packed_volume[1].clone();
                                        *frame.lock().unwrap() = Arc::new(worker.frame());
                                    }
                                }
                                Command::Instrument(request) => {
                                    let image = radar::render(
                                        Some(&worker.current),
                                        worker.mosaic.as_deref(),
                                        request,
                                        Vec3::ZERO,
                                    );
                                    *instrument.lock().unwrap() = Some(Arc::new(image));
                                }
                            }
                        }
                        worker.save();
                    })?,
            );
        }
        Ok(session)
    }

    pub fn frame(&self) -> Arc<Frame> {
        self.frame.lock().unwrap().clone()
    }

    pub fn request_step(&mut self, frame: &Frame, clock: &Clock, speed: f32, view: DVec3) {
        if speed == 0. && frame.current.regional.is_some() {
            let center = frame.current.origin + (weather::EXTENT.as_dvec3() * 0.5);
            let d = view - center;
            // Use a slightly wider threshold than the rounded solver recenter.
            if d.x.abs().max(d.z.abs()) >= 5. * f64::from(weather::CELL.x) {
                if let Some(tx) = &self.sender {
                    let _ = tx.try_send(Command::Follow(view));
                }
            }
        }
        if speed > 0.
            && clock.seconds >= frame.current.time
            && self.requested_step != Some(frame.current.time)
            && self
                .sender
                .as_ref()
                .is_some_and(|tx| tx.try_send(Command::Step(clock.solar, view)).is_ok())
        {
            self.requested_step = Some(frame.current.time);
        }
    }

    pub fn request_instrument(&mut self, request: radar::Request) {
        if self.last_instrument.elapsed() >= Duration::from_millis(450) {
            if let Some(tx) = &self.sender {
                if tx.try_send(Command::Instrument(request)).is_ok() {
                    self.last_instrument = Instant::now();
                }
            }
        }
    }

    pub fn instrument(&self) -> Option<Arc<radar::Image>> {
        self.instrument.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_pauses_and_never_outruns_weather() {
        let mut clock = Clock::new(0., SolarClock::new(0., 1200.));
        clock.tick(0.5, 64., 1200., Some(2.));
        assert_eq!(clock.seconds, 2.);
        clock.tick(1., 0., 3600., Some(4.));
        assert_eq!(clock.seconds, 2.);
        assert_eq!(clock.solar_day(), 2. / 1200.);
        clock.tick(0.125, 1., 3600., Some(4.));
        assert_eq!(clock.seconds, 2.125);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
