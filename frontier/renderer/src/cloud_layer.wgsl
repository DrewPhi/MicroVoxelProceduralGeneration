// Forward ray interval in the cloud deck, clipped to the atmospheric horizon.
// A horizontal ray inside the deck can remain in cloud for its entire range.
fn cloud_interval(eye_y: f32, direction_y: f32) -> vec2<f32> {
    let horizon = 24000.;
    if abs(direction_y) < 0.000001 {
        return vec2(0., select(0., 12000., eye_y >= 1720. && eye_y <= 2400.));
    }
    let base = (1720. - eye_y) / direction_y;
    let top = (2400. - eye_y) / direction_y;
    let start = clamp(min(base, top), 0., horizon);
    return vec2(start, clamp(max(base, top), start, min(horizon, start + 12000.)));
}
