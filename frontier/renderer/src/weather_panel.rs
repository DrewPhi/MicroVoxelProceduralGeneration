use crate::{settings::VisualSettings, world_weather::Frame, Camera};
use egui::{Color32, Pos2, Stroke, Vec2};
use frontier_runtime::weather::{regional, CELL, EXTENT, SIZE};
use glam::DVec3;
use std::time::{Duration, Instant};

pub struct Observation<'a> {
    pub achieved: f32,
    pub frame: Option<&'a Frame>,
    pub fraction: f32,
    pub seconds: f64,
    pub solar_day: f64,
    pub camera: Camera,
    pub aspect: f32,
}

impl Observation<'_> {
    pub fn clock_label(&self) -> String {
        let minutes = (self.solar_day.rem_euclid(1.) * 1440.).floor() as u32;
        format!(
            "Day {}  ·  {:02}:{:02}",
            self.solar_day.floor() as u64,
            minutes / 60,
            minutes % 60
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Product {
    #[default]
    Clouds,
    Rain,
    Datalink,
    Pressure,
    Jet,
    RegionalRain,
    Slice,
}

impl Product {
    fn is_regional(self) -> bool {
        matches!(self, Self::Pressure | Self::Jet | Self::RegionalRain)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MapRegion {
    origin: DVec3,
    extent: Vec2,
}

impl MapRegion {
    fn world(self, u: f32, v: f32) -> DVec3 {
        self.origin
            + DVec3::new(
                f64::from(u * self.extent.x),
                0.,
                f64::from(v * self.extent.y),
            )
    }

    fn unit(self, position: DVec3) -> Vec2 {
        let p = (position - self.origin).as_vec3();
        Vec2::new(p.x / self.extent.x, p.z / self.extent.y)
    }

    fn regional(state: &regional::Snapshot, camera: Camera, zoom: u8) -> Self {
        let span =
            Vec2::new(regional::SIZE[0] as f32, regional::SIZE[2] as f32) * regional::SPACING;
        let extent = span / (1u32 << zoom) as f32;
        let eye = (camera.position - state.origin).as_vec3();
        let step = regional::SPACING;
        let position = |p: f32, full: f32, view: f32| {
            ((p - view * 0.5) / step)
                .round()
                .mul_add(step, 0.)
                .clamp(0., full - view) as f64
        };
        Self {
            origin: state.origin
                + DVec3::new(
                    position(eye.x, span.x, extent.x),
                    0.,
                    position(eye.z, span.y, extent.y),
                ),
            extent,
        }
    }
}

#[derive(Default)]
pub struct Panel {
    pub open: bool,
    product: Product,
    texture: Option<egui::TextureHandle>,
    zoom: u8,
    last: Option<(Instant, f64, Product, MapRegion)>,
    slice_pose: Option<(DVec3, f32)>,
}

pub fn time_controls(ui: &mut egui::Ui, settings: &mut VisualSettings) {
    ui.horizontal_wrapped(|ui| {
        for (label, speed) in [("Pause", 0.), ("1×", 1.), ("16×", 16.), ("64×", 64.)] {
            if ui
                .selectable_label(settings.time_speed == speed, label)
                .clicked()
            {
                settings.time_speed = speed;
            }
        }
        ui.add(
            egui::Slider::new(&mut settings.time_speed, 0. ..=64.)
                .step_by(0.25)
                .suffix("×")
                .text("Speed"),
        );
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut settings.day_night, "Day/night cycle");
        egui::ComboBox::from_id_salt("solar day length")
            .selected_text(if settings.day_length == 86400. {
                "24 hours".into()
            } else {
                format!("{} min", settings.day_length / 60.)
            })
            .show_ui(ui, |ui| {
                for (label, seconds) in [
                    ("24 hours", 86400.),
                    ("1 hour", 3600.),
                    ("20 minutes", 1200.),
                    ("10 minutes", 600.),
                    ("5 minutes", 300.),
                ] {
                    ui.selectable_value(&mut settings.day_length, seconds, label);
                }
            });
        ui.label("Day length");
    });
}

fn paint_map(frame: &Frame, fraction: f32, product: Product) -> Vec<Color32> {
    let [width, height, depth] = SIZE;
    let pixels = |a: u8, b: u8| f32::from(a) + f32::from(i16::from(b) - i16::from(a)) * fraction;
    let mut map = Vec::with_capacity(width * depth);
    for z in 0..depth {
        for x in 0..width {
            let world = frame.current.origin
                + DVec3::new(
                    (x as f64 + 0.5) * f64::from(CELL.x),
                    0.,
                    (z as f64 + 0.5) * f64::from(CELL.z),
                );
            let h = frame.current.ground(world);
            let ground = (h.max(0.) / 2200.).min(1.);
            let line = if (h / 250.).fract() < 0.10 && h > 10. {
                7.
            } else {
                0.
            };
            let base = [
                13. + ground * 32. + line,
                27. + ground * 32. + line,
                37. + ground * 22. + line,
            ];
            if product == Product::Datalink {
                let rgb = frame
                    .mosaic
                    .as_deref()
                    .and_then(|scan| scan.sample(world))
                    .map_or_else(
                        || {
                            if (x + z) % 4 == 0 {
                                [47., 56., 62.]
                            } else {
                                base
                            }
                        },
                        |z| rain_color(10. * z.max(0.001).log10(), base),
                    );
                map.push(to_color(rgb));
                continue;
            }
            let mut liquid = 0_f32;
            if product == Product::Rain {
                let z = frame
                    .previous
                    .interpolate_column_echo(&frame.current, x, z, fraction);
                map.push(to_color(rain_color(10. * z.max(0.001).log10(), base)));
                continue;
            }
            let mut top = 0;
            for y in 0..height {
                let index = (z * height + y) * width + x;
                let previous = frame.packed_volume[0][index];
                let current = frame.packed_volume[1][index];
                let cloud = pixels(previous[0], current[0]);
                liquid = liquid.max(cloud);
                if cloud >= 3. {
                    top = y;
                }
            }
            let rgb = {
                let opacity = (1. - (-liquid * 0.047).exp()).clamp(0., 1.);
                let altitude = (top as f32 * CELL.y / 5600.).clamp(0., 1.);
                let bright = [
                    104. + 108. * altitude,
                    156. + 76. * altitude,
                    202. + 43. * altitude,
                ];
                std::array::from_fn(|i| base[i] + (bright[i] - base[i]) * opacity)
            };
            map.push(to_color(rgb));
        }
    }
    map
}

fn paint_regional(
    regional: &regional::Frame,
    seconds: f64,
    region: MapRegion,
    product: Product,
) -> Vec<Color32> {
    let [width, _, depth] = regional::SIZE;
    let blend = regional.fraction(seconds);
    let state = &regional.current;
    (0..width * depth)
        .map(|i| {
            let point = region.world(
                (i % width) as f32 / width as f32 + 0.5 / width as f32,
                (i / width) as f32 / depth as f32 + 0.5 / depth as f32,
            );
            let p = (point - state.origin).as_vec3() / regional::SPACING;
            let index = (p.z as usize).min(depth - 1) * width + (p.x as usize).min(width - 1);
            let a = regional.previous.columns()[index];
            let b = state.columns()[index];
            let mix = |a, b| a + (b - a) * blend;
            let colors = match product {
                Product::Pressure => {
                    let pressure = mix(a.pressure_hpa, b.pressure_hpa);
                    let t = ((pressure - 995.) / 36.).clamp(0., 1.);
                    let contour = f32::from((pressure / 4.).fract() < 0.17) * 23.;
                    [
                        49. + 151. * t + contour,
                        126. + 25. * t + contour,
                        194. - 130. * t + contour,
                    ]
                }
                Product::Jet => {
                    let t = (mix(a.jet_mps, b.jet_mps) / 65.).clamp(0., 1.);
                    [21. + t * 220., 30. + t * 142., 62. + t * 56.]
                }
                Product::RegionalRain => {
                    let rate: f32 = mix(a.rain_mm_h, b.rain_mm_h);
                    let cover = 1. - (-mix(a.cloud_water, b.cloud_water) * 1750.).exp();
                    rain_color(
                        10. * (200. * rate.powf(1.6)).max(0.001).log10(),
                        [20. + cover * 65., 32. + cover * 77., 46. + cover * 96.],
                    )
                }
                _ => unreachable!(),
            };
            to_color(colors)
        })
        .collect()
}

const SLICE_SIZE: [usize; 2] = [120, 60];
const SLICE_RANGE: f64 = 12_000.;
const SLICE_TOP: f64 = 10_000.;

fn paint_slice(frame: &Frame, fraction: f32, view: Camera) -> Vec<Color32> {
    let [width, height] = SLICE_SIZE;
    let heading = DVec3::new(f64::from(view.yaw.sin()), 0., f64::from(view.yaw.cos()));
    (0..width * height)
        .map(|i| {
            let x = i % width;
            let y = i / width;
            let distance = (x as f64 + 0.5) / width as f64 * SLICE_RANGE;
            let altitude = (1. - (y as f64 + 0.5) / height as f64) * SLICE_TOP;
            let mut position = view.position + distance * heading;
            position.y = altitude;
            let resolved = frame.current.contains_horizontal(position);
            if resolved && altitude < f64::from(frame.current.ground(position)) {
                return Color32::from_rgb(73, 57, 51);
            }
            let air = frame.previous.sample(position);
            let next = frame.current.sample(position);
            let cloud = (air.cloud + (next.cloud - air.cloud) * fraction) * 1200.;
            let echo = air.reflectivity() + (next.reflectivity() - air.reflectivity()) * fraction;
            let line = f32::from(y % 12 == 0 || x % 30 == 0) * 12.;
            let color = [
                14. + cloud.clamp(0., 1.) * 150. + line,
                26. + cloud.clamp(0., 1.) * 154. + line,
                38. + cloud.clamp(0., 1.) * 170. + line,
            ];
            to_color(rain_color(10. * echo.max(0.001).log10(), color))
        })
        .collect()
}

fn to_color(v: [f32; 3]) -> Color32 {
    Color32::from_rgb(v[0] as u8, v[1] as u8, v[2] as u8)
}

fn rain_color(dbz: f32, base: [f32; 3]) -> [f32; 3] {
    if dbz < 18. {
        base
    } else if dbz < 34. {
        [38., 174., 105.]
    } else if dbz < 43. {
        [237., 202., 75.]
    } else if dbz < 50. {
        [237., 82., 67.]
    } else {
        [215., 94., 186.]
    }
}

impl Panel {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        observation: &Observation<'_>,
        settings: &mut VisualSettings,
    ) {
        if !self.open {
            return;
        }
        let mut open = self.open;
        let screen = ctx.screen_rect();
        let side = (screen.width() * 0.49)
            .min(screen.height() * 0.60)
            .clamp(170., 520.);
        egui::Window::new("Weather radar")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_pos(Pos2::new((screen.right() - side - 32.).max(0.), 46.))
            .default_width(side)
            .max_width((screen.width() * 0.58).max(180.))
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(observation.clock_label()).strong());
                time_controls(ui, settings);
                ui.separator();
                let Some(frame) = observation.frame else {
                    ui.label("Launch with --weather regional to see the weather volume here.");
                    return;
                };
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.product, Product::Clouds, "Live clouds");
                    ui.selectable_value(&mut self.product, Product::Rain, "Live rain");
                    ui.selectable_value(&mut self.product, Product::Datalink, "Datalink radar");
                    ui.selectable_value(&mut self.product, Product::Slice, "Vertical slice");
                });
                let regional = frame.current.regional.as_ref();
                if regional.is_some() {
                    ui.horizontal_wrapped(|ui| {
                        ui.selectable_value(&mut self.product, Product::Pressure, "Pressure");
                        ui.selectable_value(&mut self.product, Product::Jet, "Jet stream");
                        ui.selectable_value(
                            &mut self.product,
                            Product::RegionalRain,
                            "Regional rain",
                        );
                        if self.product.is_regional() {
                            ui.add(egui::Slider::new(&mut self.zoom, 0..=5).text("Zoom"));
                        }
                    });
                }
                ui.small(format!("Achieved time speed {:.1}×", observation.achieved));
                let regional = regional.filter(|_| self.product.is_regional());
                let region = regional.map_or(
                    MapRegion {
                        origin: frame.current.origin,
                        extent: Vec2::new(EXTENT.x, EXTENT.z),
                    },
                    |r| MapRegion::regional(&r.current, observation.camera, self.zoom),
                );
                let view = observation.camera;
                let slice_pose = (self.product == Product::Slice).then(|| {
                    (
                        (view.position / 80.).round() * 80.,
                        (view.yaw * 32.).round() / 32.,
                    )
                });
                if self.last.is_none_or(|(at, time, product, old_region)| {
                    product != self.product
                        || region != old_region
                        || (time != observation.seconds || slice_pose != self.slice_pose)
                            && at.elapsed() >= Duration::from_millis(125)
                }) {
                    let data = if self.product == Product::Slice {
                        egui::ColorImage {
                            size: SLICE_SIZE,
                            pixels: paint_slice(frame, observation.fraction, view),
                        }
                    } else if let Some(regional) = regional {
                        egui::ColorImage {
                            size: [regional::SIZE[0], regional::SIZE[2]],
                            pixels: paint_regional(
                                regional,
                                observation.seconds,
                                region,
                                self.product,
                            ),
                        }
                    } else {
                        egui::ColorImage {
                            size: [SIZE[0], SIZE[2]],
                            pixels: paint_map(frame, observation.fraction, self.product),
                        }
                    };
                    if let Some(texture) = &mut self.texture {
                        texture.set(data, egui::TextureOptions::NEAREST);
                    } else {
                        self.texture = Some(ctx.load_texture(
                            "Weather volume map",
                            data,
                            egui::TextureOptions::NEAREST,
                        ));
                    }
                    self.last = Some((Instant::now(), observation.seconds, self.product, region));
                    self.slice_pose = slice_pose;
                }
                let size = ui
                    .available_width()
                    .clamp(150., (screen.height() * 0.58).max(150.));
                let texture = self.texture.as_ref().unwrap();
                if self.product == Product::Slice {
                    ui.horizontal(|ui| {
                        let mut axis = egui::vec2(28., size * 0.55);
                        axis.y = axis.y.max(100.);
                        let (labels, _) = ui.allocate_exact_size(axis, egui::Sense::hover());
                        for height in [10, 8, 6, 4, 2, 0] {
                            let point = Pos2::new(
                                labels.right(),
                                labels.bottom() - height as f32 / 10. * labels.height(),
                            );
                            ui.painter().text(
                                point,
                                egui::Align2::RIGHT_CENTER,
                                height,
                                egui::FontId::proportional(11.),
                                Color32::LIGHT_GRAY,
                            );
                        }
                        ui.image((texture.id(), Vec2::new((size - 36.).max(100.), axis.y)));
                    });
                    ui.small(
                        "Altitude in km MSL · looking 12 km ahead · white clouds, colored rain",
                    );
                    return;
                }
                let rect = ui
                    .add(egui::Image::new((
                        texture.id(),
                        Vec2::new(size, size * region.extent.y / region.extent.x),
                    )))
                    .rect;
                let painter = ui.painter().with_clip_rect(rect);
                let grid =
                    Stroke::new(0.65_f32, Color32::from_rgba_unmultiplied(151, 184, 196, 62));
                for i in 1..4 {
                    let x = rect.left() + rect.width() * i as f32 / 4.;
                    let y = rect.top() + rect.height() * i as f32 / 4.;
                    painter.line_segment(
                        [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                        grid,
                    );
                    painter.line_segment(
                        [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                        grid,
                    );
                }
                painter.text(
                    rect.center_top() + Vec2::new(0., 6.),
                    egui::Align2::CENTER_TOP,
                    "N",
                    egui::FontId::proportional(15.),
                    Color32::WHITE,
                );
                let view = observation.camera;
                let unit = region.unit(view.position);
                let viewing_map = (0. ..=1.).contains(&unit.x) && (0. ..=1.).contains(&unit.y);
                if let Some(r) = regional.filter(|_| self.product == Product::Jet) {
                    for y in 1..8 {
                        for x in 1..10 {
                            let u = x as f32 / 10.;
                            let v = y as f32 / 8.;
                            let p = region.world(u, v) + DVec3::Y * 10_000.;
                            let Some(air) = r.sample(p, observation.seconds) else {
                                continue;
                            };
                            let wind = Vec2::new(air.wind.x, air.wind.z);
                            let magnitude = wind.length();
                            if magnitude < 2. {
                                continue;
                            }
                            let origin = rect.min + Vec2::new(u, v) * rect.size();
                            let direction = wind / magnitude;
                            let tip = origin + direction * (4. + magnitude.min(60.) * 0.13);
                            let stroke = Stroke::new(1.1_f32, Color32::from_rgb(227, 234, 246));
                            let side = Vec2::new(-direction.y, direction.x);
                            painter.line_segment([origin, tip], stroke);
                            painter.line_segment([tip, tip - direction * 3.5 + side * 2.2], stroke);
                            painter.line_segment([tip, tip - direction * 3.5 - side * 2.2], stroke);
                        }
                    }
                }
                if viewing_map {
                    let point = rect.min + unit * rect.size();
                    let fov = ((view.fov_y.to_radians() * 0.5).tan() * observation.aspect).atan();
                    let ray = |yaw: f32| Vec2::new(yaw.sin(), yaw.cos());
                    let gold = Color32::from_rgb(255, 223, 132);
                    let half_length = rect.width() * (view.far / region.extent.x).clamp(0.035, 1.);
                    painter.add(egui::Shape::convex_polygon(
                        vec![
                            point,
                            point + ray(view.yaw - fov) * half_length,
                            point + ray(view.yaw + fov) * half_length,
                        ],
                        Color32::from_rgba_unmultiplied(251, 213, 86, 29),
                        Stroke::new(1_f32, gold),
                    ));
                    painter.circle_filled(point, 4., gold);
                    painter.line_segment(
                        [point, point + ray(view.yaw) * 16.],
                        Stroke::new(2_f32, gold),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    ui.small(format!("{:.1} km across", region.extent.x / 1000.));
                    if viewing_map {
                        ui.small("Gold shows your view");
                    } else {
                        ui.small("View is outside the simulated region");
                    }
                    if self.product == Product::Datalink {
                        ui.small(frame.mosaic.as_deref().map_or("Awaiting scan".into(), |m| {
                            format!(
                                "{:.1} min old",
                                (observation.seconds - m.scan_time).max(0.) / 60.
                            )
                        }));
                    }
                });
                ui.small(match self.product {
                    Product::Clouds => "Blue to white · increasing cloud-top height",
                    Product::Rain => "Green, yellow, red, magenta · increasing rain echoes",
                    Product::Datalink => "Delayed ground-station returns · hatching marks gaps",
                    Product::Pressure => "Blue to amber · 995–1031 hPa · 4 hPa contours",
                    Product::Jet => "Upper-level wind speed · 0–65 m/s · arrows at 10 km MSL",
                    Product::RegionalRain => {
                        "Gray-blue cloud cover · color shows surface rain intensity"
                    }
                    Product::Slice => unreachable!(),
                });
            });
        self.open = open;
    }
}
