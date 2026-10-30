//! Reproducible still-camera exports; coordinates snap to canonical voxel cells.
use crate::runtime::Runtime;
#[derive(Clone, Copy, Debug)]
pub struct Viewpoint {
    pub meters: [f64; 3],
    pub yaw: f64,
    pub pitch: f64,
}
impl std::str::FromStr for Viewpoint {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, String> {
        let values: Vec<f64> = text
            .split(',')
            .map(|v| {
                v.parse()
                    .map_err(|_| "--camera needs X,Y,Z,YAW,PITCH (meters/degrees)".to_string())
            })
            .collect::<Result<_, _>>()?;
        if values.len() != 5 || values.iter().any(|v| !v.is_finite()) {
            return Err("--camera needs five finite numbers".into());
        }
        if !(0.0..10000.).contains(&values[0])
            || !(0.0..2048.).contains(&values[1])
            || !(0.0..10000.).contains(&values[2])
            || values[4].abs() > 89.
        {
            return Err("Camera must be inside the world; pitch must be -89..89 degrees".into());
        }
        Ok(Self {
            meters: [values[0], values[1], values[2]],
            yaw: values[3].to_radians(),
            pitch: values[4].to_radians(),
        })
    }
}
impl Viewpoint {
    /// Use the same reproducible X/Z/yaw/pitch at actual player height.
    pub fn ground(self, runtime: &mut Runtime) -> microvoxel::Result<()> {
        if !runtime
            .player
            .place_above(&runtime.world, runtime.camera.cell.x, runtime.camera.cell.z)
        {
            return Err("No clear player placement at capture coordinates".into());
        }
        // Settle the actual voxel shape and let the existing idle controller
        // align head/body with the requested look direction before drawing it.
        let input = microvoxel::player::PlayerInput {
            yaw: self.yaw,
            pitch: self.pitch,
            ..Default::default()
        };
        for _ in 0..60 {
            runtime.player.advance(&runtime.world, 1. / 60., input)?;
        }
        if runtime.flying {
            runtime.toggle_flight();
        }
        runtime.set_view(runtime.view);
        runtime.refresh_terrain();
        println!(
            "Ground capture: player {:?}, eye {:?} m",
            runtime.player.root,
            runtime.camera.eye()
        );
        Ok(())
    }
    pub fn apply(self, runtime: &mut Runtime) {
        if !runtime.flying {
            runtime.toggle_flight();
        }
        runtime.camera.cell = microvoxel::CellPos::new(
            (self.meters[0] / runtime.world.spec().grid.meters()).round() as i64,
            (self.meters[1] / runtime.world.spec().grid.meters()).round() as i64,
            (self.meters[2] / runtime.world.spec().grid.meters()).round() as i64,
        );
        runtime.camera.yaw = self.yaw;
        runtime.camera.pitch = self.pitch;
        runtime.camera.reverse = false;
        runtime.clear_controls();
        runtime.refresh_terrain();
        println!(
            "Capture camera {:?} m; yaw {:.2}, pitch {:.2} degrees",
            runtime.camera.eye(),
            self.yaw.to_degrees(),
            self.pitch.to_degrees()
        );
    }
}
pub fn size(text: &str) -> Result<[u32; 2], String> {
    let (w, h) = text.split_once('x').ok_or("--size needs WIDTHxHEIGHT")?;
    let w = w.parse().map_err(|_| "Invalid image width")?;
    let h = h.parse().map_err(|_| "Invalid image height")?;
    if !(320..=4096).contains(&w) || !(240..=4096).contains(&h) {
        return Err("Image dimensions must be 320..4096 by 240..4096".into());
    }
    Ok([w, h])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn camera_and_image_size_reject_invalid_exports() {
        assert!("1,2,3,90,-10".parse::<Viewpoint>().is_ok());
        for s in ["1,2,3", "NaN,2,3,0,0", "10001,2,3,0,0", "1,2,3,0,90"] {
            assert!(s.parse::<Viewpoint>().is_err());
        }
        assert_eq!(size("2560x1440").unwrap(), [2560, 1440]);
        assert!(size("99999x99999").is_err());
    }
}
