//! Menu world choices and validated generation requests; no graphics resources.
use microvoxel::{generation::Terrain, GridScale};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Request {
    Atlas {
        seed: u64,
        grid: GridScale,
        world_scale: u32,
    },
    Procedural {
        seed: u64,
        grid: GridScale,
        world_scale: u32,
    },
    Survey(PathBuf),
}
#[derive(Clone, Debug, Hash)]
pub struct SurveyEntry {
    pub name: String,
    pub path: PathBuf,
    pub grid: GridScale,
}
impl SurveyEntry {
    fn read(path: PathBuf) -> Option<Self> {
        let mut h = [0u8; 24];
        std::fs::File::open(&path).ok()?.read_exact(&mut h).ok()?;
        if &h[..8] != b"MVEARTH1" {
            return None;
        }
        let n = u32::from_le_bytes(h[16..20].try_into().ok()?);
        let d = u32::from_le_bytes(h[20..24].try_into().ok()?);
        let grid = GridScale::new(n, d).ok()?;
        let name = if path.file_stem()?.to_str()? == "volcan" {
            "VOLCAN MOUNTAIN".into()
        } else {
            path.file_stem()?
                .to_str()?
                .replace('_', " ")
                .to_uppercase()
                .chars()
                .take(42)
                .collect()
        };
        Some(Self { name, path, grid })
    }
}
#[derive(Hash)]
pub struct Editor {
    pub atlas: bool,
    pub procedural: bool,
    pub seed: String,
    pub centimeters: String,
    pub world_scale: String,
    pub surveys: Vec<SurveyEntry>,
    pub selected: usize,
    pub busy: bool,
    pub status: String,
    pub pending: Option<Request>,
}
impl Editor {
    pub fn new() -> Self {
        let mut paths: Vec<_> = std::fs::read_dir("output/earth")
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "mvearth"))
            .collect();
        paths.sort();
        let surveys = paths
            .into_iter()
            .filter_map(SurveyEntry::read)
            .take(16)
            .collect();
        Self {
            atlas: false,
            procedural: true,
            seed: "42".into(),
            centimeters: "6.25".into(),
            world_scale: "1".into(),
            surveys,
            selected: 0,
            busy: false,
            status: String::new(),
            pending: None,
        }
    }
    pub fn select_current(&mut self, request: &Request) {
        self.procedural = !matches!(request, Request::Survey(_));
        if self.procedural {
            self.atlas = matches!(request, Request::Atlas { .. });
        }
        match request {
            Request::Atlas {
                seed,
                grid,
                world_scale,
            }
            | Request::Procedural {
                seed,
                grid,
                world_scale,
            } => {
                self.seed = seed.to_string();
                self.centimeters = (grid.meters() * 100.).to_string();
                self.world_scale = world_scale.to_string();
            }
            Request::Survey(path) => {
                if let Some(i) = self.surveys.iter().position(|e| e.path == *path) {
                    self.selected = i;
                } else if let Some(entry) = SurveyEntry::read(path.clone()) {
                    self.selected = self.surveys.len();
                    self.surveys.push(entry);
                }
            }
        }
    }
    pub fn randomize(&mut self) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        self.seed = microvoxel::painting::hash(
            time ^ COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        )
        .to_string();
        self.status.clear();
    }
    pub fn request(&self) -> Result<Request, String> {
        if !self.procedural {
            return self
                .surveys
                .get(self.selected)
                .map(|e| Request::Survey(e.path.clone()))
                .ok_or_else(|| "NO IMPORTED REAL WORLD IS AVAILABLE".into());
        }
        let seed = self
            .seed
            .parse::<u64>()
            .map_err(|_| "SEED MUST BE A WHOLE NUMBER FROM 0 TO 18446744073709551615")?;
        let cm = self
            .centimeters
            .parse::<f64>()
            .map_err(|_| "ENTER VOXEL SIZE IN CENTIMETERS")?;
        let cells = 100. / cm;
        if !cm.is_finite() || !(4. ..=128.).contains(&cells) || (cells - cells.round()).abs() > 1e-7
        {
            return Err("USE 100 / N CM WITH N = 4..128; E.G. 25, 6.25, 5, 1".into());
        }
        let world_scale = self
            .world_scale
            .parse::<u32>()
            .map_err(|_| "WORLD SCALE MUST BE A WHOLE NUMBER FROM 1 TO 100")?;
        microvoxel::generation::ProceduralTerrain::validate_world_scale(world_scale)
            .map_err(|_| "WORLD SCALE MUST BE A WHOLE NUMBER FROM 1 TO 100")?;
        let grid = GridScale::new(1, cells.round() as u32)?;
        Ok(if self.atlas {
            Request::Atlas {
                seed,
                grid,
                world_scale,
            }
        } else {
            Request::Procedural {
                seed,
                grid,
                world_scale,
            }
        })
    }
    pub fn apply(&mut self) {
        if self.busy {
            return;
        }
        match self.request() {
            Ok(request) => {
                self.pending = Some(request);
                self.busy = true;
                self.status = if self.atlas && self.procedural {
                    "PREPARING ATLAS - FIRST BUILD MAY TAKE 3 MINUTES"
                } else {
                    "PREPARING WORLD - PLEASE WAIT"
                }
                .into();
            }
            Err(error) => self.status = error,
        }
    }
    pub fn for_terrain(terrain: &Terrain, earth_path: Option<&Path>) -> Request {
        use microvoxel::generation::Generator;
        if terrain.survey().is_some() {
            Request::Survey(
                earth_path
                    .unwrap_or(Path::new("output/earth/volcan.mvearth"))
                    .into(),
            )
        } else if terrain.is_atlas() {
            Request::Atlas {
                seed: terrain.identity().seed,
                grid: terrain.spec().grid,
                world_scale: terrain.world_scale(),
            }
        } else {
            Request::Procedural {
                seed: terrain.identity().seed,
                grid: terrain.spec().grid,
                world_scale: terrain.world_scale(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atlas_selection_survives_field_updates_and_survey_round_trip() {
        let mut editor = Editor::new();
        editor.select_current(&Request::Atlas {
            seed: 42,
            grid: GridScale::default(),
            world_scale: 10,
        });
        editor.seed = "73".into();
        editor.centimeters = "5".into();
        editor.world_scale = "20".into();
        assert_eq!(
            editor.request().unwrap(),
            Request::Atlas {
                seed: 73,
                grid: GridScale::new(1, 20).unwrap(),
                world_scale: 20,
            }
        );
        editor.select_current(&Request::Survey("missing.mvearth".into()));
        assert!(editor.atlas);
        editor.procedural = true;
        editor.apply();
        assert!(matches!(
            editor.pending,
            Some(Request::Atlas { seed: 73, .. })
        ));
        assert!(editor.status.contains("3 MINUTES"));
    }
    #[test]
    fn scale_validation_and_source_selection_are_independent_of_voxel_size() {
        let mut e = Editor::new();
        e.world_scale = "10".into();
        let request = e.request().unwrap();
        assert!(
            matches!(request, Request::Procedural { world_scale: 10, grid, .. }
            if grid == GridScale::default())
        );
        e.select_current(&request);
        assert_eq!(e.world_scale, "10");
        for invalid in ["0", "101", "-1", "2.5", "NaN", "", "4294967296"] {
            e.world_scale = invalid.into();
            e.apply();
            assert!(!e.busy && e.pending.is_none());
        }
    }
    #[test]
    fn seed_and_centimeters_are_exact_and_invalid_requests_do_not_start_work() {
        let mut e = Editor::new();
        e.procedural = true;
        e.seed = u64::MAX.to_string();
        e.centimeters = "6.25".into();
        assert_eq!(
            e.request().unwrap(),
            Request::Procedural {
                seed: u64::MAX,
                grid: GridScale::default(),
                world_scale: 1,
            }
        );
        e.centimeters = "0.78125".into();
        assert!(e.request().is_ok());
        for bad in ["0", "-1", "NaN", "7", "0.1", "100"] {
            e.centimeters = bad.into();
            e.apply();
            assert!(!e.busy);
            assert!(e.pending.is_none());
        }
        e.centimeters = "5".into();
        e.seed = "18446744073709551616".into();
        assert!(e.request().is_err());
        e.seed = "0".into();
        e.apply();
        assert!(e.busy);
        assert_eq!(
            e.pending.take().unwrap(),
            Request::Procedural {
                seed: 0,
                grid: GridScale::new(1, 20).unwrap(),
                world_scale: 1,
            }
        );
    }
    #[test]
    fn real_world_ignores_procedural_fields_and_repeated_apply_is_bounded() {
        let mut e = Editor::new();
        e.surveys = vec![SurveyEntry {
            name: "TEST".into(),
            path: "test.mvearth".into(),
            grid: GridScale::new(1, 20).unwrap(),
        }];
        e.procedural = false;
        e.seed = "bad".into();
        e.centimeters = "bad".into();
        e.world_scale = "bad".into();
        e.apply();
        assert_eq!(
            e.pending.take(),
            Some(Request::Survey("test.mvearth".into()))
        );
        e.apply();
        assert!(e.pending.is_none());
    }
}
