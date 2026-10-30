//! Physical world dimensions and the fixed authoritative grid. No global scale.
use crate::CellPos;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GridScale {
    numerator: u32,
    denominator: u32,
}
impl Default for GridScale {
    fn default() -> Self {
        Self {
            numerator: 1,
            denominator: 16,
        }
    }
}
impl GridScale {
    pub fn new(numerator: u32, denominator: u32) -> Result<Self, String> {
        if numerator == 0 || denominator == 0 {
            return Err("cell size must be positive".into());
        }
        let (mut a, mut b) = (numerator, denominator);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        let s = Self {
            numerator: numerator / a,
            denominator: denominator / a,
        };
        if !(0.001..=1.).contains(&s.meters()) {
            return Err("supported cell sizes are 1 mm through 1 m".into());
        }
        Ok(s)
    }
    pub fn ratio(self) -> (u32, u32) {
        (self.numerator, self.denominator)
    }
    pub fn meters(self) -> f64 {
        self.numerator as f64 / self.denominator as f64
    }
    pub fn cells(self, meters: f64) -> Option<i64> {
        let v = (meters / self.meters()).floor();
        (v.is_finite() && v.abs() < (i64::MAX / 32) as f64).then_some(v as i64)
    }
    pub fn position(self, point: [f64; 3]) -> Option<CellPos> {
        Some(CellPos::new(
            self.cells(point[0])?,
            self.cells(point[1])?,
            self.cells(point[2])?,
        ))
    }
    pub fn point(self, p: CellPos) -> [f64; 3] {
        [
            p.x as f64 * self.meters(),
            p.y as f64 * self.meters(),
            p.z as f64 * self.meters(),
        ]
    }
}
impl std::str::FromStr for GridScale {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        if let Some((a, b)) = s.split_once('/') {
            return Self::new(
                a.parse().map_err(|_| "invalid numerator")?,
                b.parse().map_err(|_| "invalid denominator")?,
            );
        }
        let value: f64 = s
            .parse()
            .map_err(|_| "expected meters per voxel, e.g. 0.0625 or 1/100")?;
        if !value.is_finite() || !(0.001..=1.).contains(&value) {
            return Err("cell size must be 0.001..1 meters".into());
        }
        Self::new((value * 1_000_000.).round() as u32, 1_000_000)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorldSpec {
    pub grid: GridScale,
    pub extent_m: u32,
    pub ceiling_m: u32,
}
impl Default for WorldSpec {
    fn default() -> Self {
        Self {
            grid: GridScale::default(),
            extent_m: 10_000,
            ceiling_m: 2048,
        }
    }
}
impl WorldSpec {
    pub fn validate(self) -> Result<Self, String> {
        if self.extent_m == 0
            || self.extent_m > 10_000
            || self.ceiling_m < 1024
            || self.ceiling_m > 2048
        {
            return Err("world extent must be 1..10000 m and vertical limit 1024..2048 m".into());
        }
        Ok(self)
    }
    pub fn extent_cells(self) -> i64 {
        (self.extent_m as f64 / self.grid.meters()).ceil() as i64
    }
    pub fn ceiling_cells(self) -> i64 {
        (self.ceiling_m as f64 / self.grid.meters()).ceil() as i64
    }
    pub fn fingerprint(self) -> u64 {
        let (n, d) = self.grid.ratio();
        crate::painting::hash(n as u64 | ((d as u64) << 32))
            ^ crate::painting::hash(self.extent_m as u64 | ((self.ceiling_m as u64) << 32))
    }
    /// Fixed 64 m ownership pages. Native storage/render subdivision is separate.
    pub fn page(self, p: CellPos) -> [i64; 2] {
        let (n, d) = self.grid.ratio();
        [p.x, p.z].map(|v| ((v as i128 * n as i128).div_euclid(64 * d as i128)) as i64)
    }
}
