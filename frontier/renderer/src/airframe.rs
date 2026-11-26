//! Geometry shared by the voxel model and the aerodynamic sections. Metres,
//! with body coordinates +X toward the left wing, +Y up, +Z forward.

pub(crate) const AILERON_HINGE_Z: f32 = -0.53;
pub(crate) const ELEVATOR_HINGE_Z: f32 = -4.79;
pub(crate) const RUDDER_HINGE_Z: f32 = -5.19;
pub(crate) const FLAP_SPLIT: f32 = 0.57;
pub(crate) const CONTROL_TIP: f32 = 0.96;
pub(crate) const AILERON_TRAVEL: f32 = 0.32;
pub(crate) fn elevator_deflection(pitch: f32) -> f32 {
    pitch * if pitch >= 0. { 0.47 } else { 0.35 }
}
pub(crate) const RUDDER_TRAVEL: f32 = 0.46;
pub(crate) const FLAP_TRAVEL: f32 = std::f32::consts::FRAC_PI_6;
pub(crate) const PROP_Z: f32 = 2.60;

pub(crate) struct Planform {
    pub span: f32,
    pub center: f32,
    pub chord: f32,
    pub root: f32,
    pub rise: f32,
    pub hinge: f32,
    pub control_root: f32,
}

impl Planform {
    pub fn section(&self, u: f32) -> (f32, f32, f32) {
        (
            self.chord * (1. - 0.33 * u.powi(8)),
            self.center - 0.09 * u * u,
            self.root + self.rise * u,
        )
    }
}

pub(crate) const WING: Planform = Planform {
    span: 5.5,
    center: 0.02,
    chord: 1.72,
    root: 0.85,
    rise: 0.24,
    hinge: AILERON_HINGE_Z,
    control_root: 0.14,
};
pub(crate) const TAIL: Planform = Planform {
    span: 1.82,
    center: -4.54,
    chord: 1.10,
    root: 0.18,
    rise: 0.06,
    hinge: ELEVATOR_HINGE_Z,
    control_root: 0.10,
};

pub(crate) const FIN_ROOT: f32 = 3. / 16.;
pub(crate) const FIN_HEIGHT: f32 = 19. / 16.;

pub(crate) fn fin_section(u: f32) -> (f32, f32) {
    ((-61. - 19. * u) / 16., (-89. + 4. * u) / 16.)
}
