//! Dependency-free immediate settings overlay. Geometry is rebuilt only while
//! visible; input coordinates and drawing share one layout, including DPI scaling.
use crate::settings::Settings;
use bytemuck::{Pod, Zeroable};
use std::time::{Duration, Instant};
use winit::keyboard::KeyCode;
const W: f32 = 760.;
const H: f32 = 640.;
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Tab {
    Display,
    World,
}
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
}
pub struct Menu {
    pub open: bool,
    pub settings: Settings,
    pub worlds: crate::worlds::Editor,
    tab: Tab,
    cursor: [f32; 2],
    dragging: Option<usize>,
    editing: Option<usize>,
    text: String,
    changed: Option<Instant>,
}
impl Menu {
    pub fn new() -> Self {
        Self {
            open: false,
            settings: Settings::load(),
            worlds: crate::worlds::Editor::new(),
            tab: Tab::Display,
            cursor: [0.; 2],
            dragging: None,
            editing: None,
            text: String::new(),
            changed: None,
        }
    }
    fn transform(width: u32, height: u32) -> (f32, f32, f32) {
        let scale = (width as f32 / (W + 32.))
            .min(height as f32 / (H + 32.))
            .max(0.1);
        (
            scale,
            (width as f32 - W * scale) * 0.5,
            (height as f32 - H * scale) * 0.5,
        )
    }
    pub fn cursor(&mut self, x: f32, y: f32, width: u32, height: u32) {
        let (s, ox, oy) = Self::transform(width, height);
        self.cursor = [(x - ox) / s, (y - oy) / s];
        if let Some(row) = self.dragging {
            self.slide(row);
        }
    }
    fn value(&self, row: usize) -> f64 {
        match row {
            0 => self.settings.quality,
            1 => self.settings.native_radius,
            2 => self.settings.memory_mib as f64,
            _ => self.settings.haze,
        }
    }
    fn range(row: usize) -> (f64, f64) {
        match row {
            0 => (0., 100.),
            1 => (2., 32.),
            2 => (64., 1024.),
            _ => (0., 2.),
        }
    }
    fn set(&mut self, row: usize, value: f64) {
        match row {
            0 => self.settings.quality = value.round(),
            1 => self.settings.native_radius = (value * 2.).round() / 2.,
            2 => self.settings.memory_mib = ((value / 16.).round() as usize) * 16,
            _ => self.settings.haze = (value * 100.).round() / 100.,
        }
        self.settings.validate();
        self.changed = Some(Instant::now());
    }
    fn slide(&mut self, row: usize) {
        let (a, b) = Self::range(row);
        self.set(
            row,
            a + (b - a) * ((self.cursor[0] - 310.) / 290.).clamp(0., 1.) as f64,
        );
    }
    pub fn click(&mut self, pressed: bool) -> bool {
        if !pressed {
            self.dragging = None;
            return false;
        }
        self.commit_text();
        let [x, y] = self.cursor;
        if (18. ..54.).contains(&y) {
            if (340. ..520.).contains(&x) {
                self.tab = Tab::Display;
            }
            if (536. ..732.).contains(&x) {
                self.tab = Tab::World;
            }
            return false;
        }
        if self.tab == Tab::World {
            return self.world_click(x, y);
        }
        if (588.0..734.0).contains(&x) && (584.0..624.0).contains(&y) {
            return true;
        }
        if (88.0..124.0).contains(&y) {
            for i in 0..3 {
                if (28. + i as f32 * 236. ..250. + i as f32 * 236.).contains(&x) {
                    let ray = self.settings.ray;
                    self.settings = Settings::preset(i);
                    self.settings.ray = ray;
                    self.changed = Some(Instant::now());
                }
            }
        }
        for row in 0..4 {
            let cy = 166. + row as f32 * 66.;
            if (cy - 12. ..cy + 28.).contains(&y) {
                if (310.0..600.0).contains(&x) {
                    self.dragging = Some(row);
                    self.slide(row);
                }
                if (620.0..730.0).contains(&x) {
                    self.editing = Some(row);
                    self.text = format!("{}", self.value(row));
                }
            }
        }
        if (28.0..428.0).contains(&x) && (584.0..624.0).contains(&y) {
            self.settings.ray = !self.settings.ray;
            self.changed = Some(Instant::now());
        }
        if (438.0..482.0).contains(&y) {
            for i in 0..4 {
                if (28. + i as f32 * 180. ..198. + i as f32 * 180.).contains(&x) {
                    match i {
                        0 => self.settings.ambient = !self.settings.ambient,
                        1 => self.settings.shadows = !self.settings.shadows,
                        2 => self.settings.antialias = !self.settings.antialias,
                        _ => self.settings.transitions = !self.settings.transitions,
                    }
                    self.changed = Some(Instant::now());
                }
            }
        }
        false
    }
    fn commit_text(&mut self) {
        if let Some(row) = self.editing.take() {
            if row == 4 {
                self.worlds.seed = self.text.clone();
                self.worlds.status.clear();
                return;
            }
            if row == 5 {
                self.worlds.centimeters = self.text.clone();
                self.worlds.status.clear();
                return;
            }
            if row == 6 {
                self.worlds.world_scale = self.text.clone();
                self.worlds.status.clear();
                return;
            }
            if let Ok(v) = self.text.parse::<f64>() {
                if v.is_finite() {
                    let (a, b) = Self::range(row);
                    self.set(row, v.clamp(a, b));
                }
            }
        }
    }
    pub fn key(&mut self, key: KeyCode, text: Option<&str>) {
        if self.editing.is_none() {
            return;
        }
        match key {
            KeyCode::Enter => self.commit_text(),
            KeyCode::Backspace => {
                self.text.pop();
            }
            KeyCode::Delete => self.text.clear(),
            _ => {
                if let Some(t) = text {
                    for c in t.chars() {
                        let row = self.editing.unwrap();
                        let limit = if row == 4 { 20 } else { 16 };
                        if (c.is_ascii_digit() || (c == '.' && row != 4)) && self.text.len() < limit
                        {
                            self.text.push(c);
                        }
                    }
                }
            }
        }
    }
    pub fn ready(&mut self, force: bool) -> Option<Settings> {
        if force {
            self.commit_text();
            self.dragging = None;
        }
        if self
            .changed
            .is_some_and(|t| force || t.elapsed() > Duration::from_millis(200))
        {
            self.changed = None;
            Some(self.settings)
        } else {
            None
        }
    }
    fn vertices(&self, width: u32, height: u32, stats: &[String]) -> Vec<Vertex> {
        let mut p = Painter {
            vertices: Vec::with_capacity(30000),
            width,
            height,
            transform: Self::transform(width, height),
        };
        p.rect(0., 0., W, H, [0.018, 0.028, 0.044, 0.98]);
        p.rect(0., 0., W, 4., [0.22, 0.72, 0.54, 1.]);
        p.text(28., 25., "SETTINGS", 3., [0.88, 0.94, 0.94, 1.]);
        for (tab, x, w, label) in [
            (Tab::Display, 340., 180., "DISPLAY"),
            (Tab::World, 536., 196., "WORLD"),
        ] {
            p.rect(
                x,
                18.,
                w,
                36.,
                if self.tab == tab {
                    [0.12, 0.35, 0.28, 1.]
                } else {
                    [0.07, 0.12, 0.16, 1.]
                },
            );
            p.text(x + 20., 29., label, 2., [0.8, 0.94, 0.9, 1.]);
        }
        if self.tab == Tab::World {
            self.world_vertices(&mut p);
            return p.vertices;
        }
        p.text(
            28.,
            60.,
            "ESC TO RESUME - CHANGES APPLY AUTOMATICALLY",
            1.6,
            [0.46, 0.62, 0.68, 1.],
        );
        for (i, name) in ["LAPTOP", "BALANCED", "HIGH DETAIL"].iter().enumerate() {
            let x = 28. + i as f32 * 236.;
            p.rect(x, 88., 222., 36., [0.07, 0.12, 0.16, 1.]);
            p.text(x + 16., 99., name, 2., [0.7, 0.86, 0.85, 1.]);
        }
        let names = [
            "TERRAIN DETAIL",
            "NATIVE RADIUS - M",
            "TERRAIN CACHE - MIB",
            "DISTANCE HAZE",
        ];
        for row in 0..4 {
            let y = 166. + row as f32 * 66.;
            p.text(28., y, &names[row], 2., [0.78, 0.84, 0.86, 1.]);
            let (a, b) = Self::range(row);
            let t = ((self.value(row) - a) / (b - a)) as f32;
            p.rect(310., y + 4., 290., 8., [0.09, 0.15, 0.20, 1.]);
            p.rect(310., y + 4., 290. * t, 8., [0.13, 0.53, 0.39, 1.]);
            p.rect(306. + 290. * t, y - 2., 8., 20., [0.53, 0.95, 0.77, 1.]);
            p.rect(620., y - 10., 110., 34., [0.06, 0.1, 0.14, 1.]);
            let value = if self.editing == Some(row) {
                format!("{}_", self.text)
            } else {
                format!("{:.1}", self.value(row))
            };
            p.text(630., y, &value, 2., [0.87, 0.94, 0.94, 1.]);
        }
        p.text(
            28.,
            416.,
            "MAX DETAIL TARGETS 1 PIXEL - CACHE MAY LIMIT QUALITY",
            1.6,
            [0.47, 0.64, 0.68, 1.],
        );
        for (i, (name, on)) in [
            ("AMBIENT", self.settings.ambient),
            ("SHADOWS", self.settings.shadows),
            ("AA", self.settings.antialias),
            ("LOD FADE", self.settings.transitions),
        ]
        .into_iter()
        .enumerate()
        {
            let x = 28. + i as f32 * 180.;
            p.rect(x, 442., 170., 34., [0.06, 0.11, 0.15, 1.]);
            p.text(
                x + 10.,
                453.,
                &format!("{} {}", if on { "+" } else { "-" }, name),
                1.8,
                if on {
                    [0.5, 0.9, 0.72, 1.]
                } else {
                    [0.43, 0.5, 0.57, 1.]
                },
            );
        }
        p.rect(28., 584., 400., 40., [0.07, 0.16, 0.18, 1.]);
        p.text(
            40.,
            597.,
            if self.settings.ray {
                "RENDERER: RAY CASTING"
            } else {
                "RENDERER: MESH"
            },
            2.,
            [0.55, 0.95, 0.78, 1.],
        );
        for (i, line) in stats.iter().take(4).enumerate() {
            p.text(
                28.,
                499. + i as f32 * 20.,
                line,
                1.6,
                [0.59, 0.72, 0.77, 1.],
            );
        }

        p.rect(588., 584., 146., 40., [0.12, 0.40, 0.30, 1.]);
        p.text(605., 598., "RESUME", 2.4, [0.88, 0.97, 0.91, 1.]);
        p.vertices
    }
    fn world_click(&mut self, x: f32, y: f32) -> bool {
        if self.worlds.busy {
            return false;
        }
        if (584. ..624.).contains(&y) && (588. ..734.).contains(&x) {
            return true;
        }
        if (100. ..144.).contains(&y) {
            if (28. ..370.).contains(&x) {
                self.worlds.procedural = false;
                self.worlds.status.clear();
            }
            if (388. ..732.).contains(&x) {
                self.worlds.procedural = true;
                self.worlds.status.clear();
            }
        }
        if self.worlds.procedural {
            if (194. ..234.).contains(&y) {
                if (28. ..478.).contains(&x) {
                    self.editing = Some(4);
                    self.text = self.worlds.seed.clone();
                }
                if (500. ..732.).contains(&x) {
                    self.worlds.randomize();
                }
            }
            if (294. ..334.).contains(&y) && (28. ..238.).contains(&x) {
                self.editing = Some(5);
                self.text = self.worlds.centimeters.clone();
            }
            if (384. ..424.).contains(&y) && (28. ..238.).contains(&x) {
                self.editing = Some(6);
                self.text = self.worlds.world_scale.clone();
            }
        } else if (182. ..224.).contains(&y)
            && (28. ..732.).contains(&x)
            && !self.worlds.surveys.is_empty()
        {
            self.worlds.selected = (self.worlds.selected + 1) % self.worlds.surveys.len();
            self.worlds.status.clear();
        }
        if (584. ..624.).contains(&y) && (28. ..550.).contains(&x) {
            self.worlds.apply();
        }
        false
    }
    fn world_vertices(&self, p: &mut Painter) {
        let text = [0.78, 0.86, 0.88, 1.];
        let muted = [0.48, 0.64, 0.68, 1.];
        p.text(
            28.,
            66.,
            "CHOOSE A WORLD, THEN PRESS APPLY WORLD",
            1.6,
            muted,
        );
        for (procedural, x, label) in [
            (false, 28., "REAL WORLDS"),
            (
                true,
                388.,
                if self.worlds.atlas {
                    "ATLAS WORLD"
                } else {
                    "PROCEDURAL WORLD"
                },
            ),
        ] {
            p.rect(
                x,
                100.,
                344.,
                44.,
                if self.worlds.procedural == procedural {
                    [0.12, 0.35, 0.28, 1.]
                } else {
                    [0.07, 0.12, 0.16, 1.]
                },
            );
            p.text(x + 18., 116., label, 2., text);
        }
        if self.worlds.procedural {
            p.text(28., 170., "WORLD SEED", 2., text);
            p.rect(28., 194., 450., 40., [0.06, 0.1, 0.14, 1.]);
            let seed = if self.editing == Some(4) {
                format!("{}_", self.text)
            } else {
                self.worlds.seed.clone()
            };
            p.text(40., 207., &seed, 2., text);
            p.rect(500., 194., 232., 40., [0.07, 0.16, 0.18, 1.]);
            p.text(514., 208., "NEW RANDOM SEED", 1.9, [0.55, 0.95, 0.78, 1.]);
            p.text(28., 270., "VOXEL SIZE - CENTIMETERS", 2., text);
            p.rect(28., 294., 210., 40., [0.06, 0.1, 0.14, 1.]);
            let cm = if self.editing == Some(5) {
                format!("{}_", self.text)
            } else {
                self.worlds.centimeters.clone()
            };
            p.text(40., 307., &cm, 2., text);
            p.text(258., 308., "E.G. 25, 10, 6.25, 5, 2, 1 CM", 1.7, muted);
            p.text(28., 360., "WORLD SCALE - LANDSCAPE COMPRESSION", 2., text);
            p.rect(28., 384., 210., 40., [0.06, 0.1, 0.14, 1.]);
            let world_scale = if self.editing == Some(6) {
                format!("{}_", self.text)
            } else {
                self.worlds.world_scale.clone()
            };
            p.text(40., 397., &world_scale, 2., text);
            p.text(258., 390., "1 GAME METER = N TERRAIN METERS", 1.6, muted);
            p.text(
                258.,
                411.,
                "1 = FULL SIZE, 10 = 1:10; RANGE 1-100",
                1.5,
                muted,
            );
            p.text(
                28.,
                450.,
                "SCALES DISTANCES AND HEIGHTS; PLAYER / VOXEL CM STAY FIXED",
                1.5,
                muted,
            );
        } else {
            p.text(28., 162., "IMPORTED REAL WORLD - CLICK TO CYCLE", 1.7, text);
            p.rect(28., 182., 704., 42., [0.07, 0.16, 0.18, 1.]);
            if let Some(entry) = self.worlds.surveys.get(self.worlds.selected) {
                p.text(40., 197., &entry.name, 2., text);
                p.text(
                    28.,
                    270.,
                    &format!(
                        "VOXEL SIZE: {} CM - FIXED BY THE SURVEY",
                        entry.grid.meters() * 100.
                    ),
                    1.8,
                    text,
                );
            } else {
                p.text(40., 197., "NO IMPORTED WORLDS FOUND", 2., text);
            }
            p.text(
                28.,
                326.,
                "REAL WORLDS USE THEIR MEASURED HEIGHTS AND COLORS",
                1.6,
                muted,
            );
            p.text(
                28.,
                359.,
                "SEED AND VOXEL SIZE APPLY ONLY TO PROCEDURAL WORLDS",
                1.6,
                muted,
            );
        }
        p.text(28., 480., &self.worlds.status, 1.4, [0.9, 0.78, 0.5, 1.]);
        p.text(
            28.,
            521.,
            "APPLY REPLACES THE CURRENT SESSION AND RESPAWNS YOU",
            1.6,
            muted,
        );
        p.rect(28., 584., 522., 40., [0.12, 0.40, 0.30, 1.]);
        p.text(
            48.,
            598.,
            if self.worlds.busy {
                "PREPARING WORLD..."
            } else {
                "APPLY WORLD"
            },
            2.4,
            [0.88, 0.97, 0.91, 1.],
        );
        p.rect(588., 584., 146., 40., [0.07, 0.16, 0.18, 1.]);
        p.text(605., 598., "RESUME", 2.4, text);
    }
}
struct Painter {
    vertices: Vec<Vertex>,
    width: u32,
    height: u32,
    transform: (f32, f32, f32),
}
impl Painter {
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        let (s, ox, oy) = self.transform;
        for [u, v] in [[0., 0.], [1., 0.], [1., 1.], [0., 0.], [1., 1.], [0., 1.]] {
            self.vertices.push(Vertex {
                position: [
                    2. * (ox + (x + u * w) * s) / self.width as f32 - 1.,
                    1. - 2. * (oy + (y + v * h) * s) / self.height as f32,
                ],
                color,
            });
        }
    }
    fn text(&mut self, x: f32, y: f32, text: &str, size: f32, color: [f32; 4]) {
        for (i, c) in text.chars().enumerate() {
            let rows = glyph(c.to_ascii_uppercase());
            for (row, bits) in rows.into_iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << (4 - col)) != 0 {
                        self.rect(
                            x + (i as f32 * 6. + col as f32) * size,
                            y + row as f32 * size,
                            size,
                            size,
                            color,
                        );
                    }
                }
            }
        }
    }
}
// Original 5x7 bitmap glyphs: no font engine, external assets or texture uploads.
fn glyph(c: char) -> [u8; 7] {
    match c {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 18, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        '.' => [0, 0, 0, 0, 0, 6, 6],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '+' => [0, 4, 4, 31, 4, 4, 0],
        '/' => [1, 1, 2, 4, 8, 16, 16],
        ':' => [0, 6, 6, 0, 6, 6, 0],
        '=' => [0, 0, 31, 0, 31, 0, 0],
        '%' => [17, 2, 4, 8, 16, 17, 0],
        '_' => [0, 0, 0, 0, 0, 0, 31],
        ';' => [0, 6, 6, 0, 6, 4, 8],
        _ => [0; 7],
    }
}
pub struct Renderer {
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    capacity: u64,
    fingerprint: Option<u64>,
    count: u32,
}
impl Renderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("settings UI"),
            source: wgpu::ShaderSource::Wgsl(include_str!("menu.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("settings UI"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 24,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x2,1=>Float32x4],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let capacity = 4 * 1024 * 1024;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bounded settings UI vertices"),
            size: capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            buffer,
            capacity,
            fingerprint: None,
            count: 0,
        }
    }
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        menu: &Menu,
        width: u32,
        height: u32,
        stats: &[String],
    ) {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        for v in [
            menu.settings.quality,
            menu.settings.native_radius,
            menu.settings.haze,
        ] {
            v.to_bits().hash(&mut hash);
        }
        menu.settings.memory_mib.hash(&mut hash);
        [
            menu.settings.ambient,
            menu.settings.shadows,
            menu.settings.antialias,
            menu.settings.transitions,
        ]
        .hash(&mut hash);
        menu.editing.hash(&mut hash);
        menu.text.hash(&mut hash);
        menu.tab.hash(&mut hash);
        menu.worlds.hash(&mut hash);
        stats.hash(&mut hash);
        width.hash(&mut hash);
        height.hash(&mut hash);
        let fingerprint = hash.finish();
        if self.fingerprint != Some(fingerprint) {
            let vertices = menu.vertices(width, height, stats);
            let bytes = bytemuck::cast_slice(&vertices);
            assert!(bytes.len() as u64 <= self.capacity);
            queue.write_buffer(&self.buffer, 0, bytes);
            self.count = vertices.len() as u32;
            self.fingerprint = Some(fingerprint);
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("settings"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.buffer.slice(..));
            pass.draw(0..self.count, 0..1);
        }
        queue.submit(Some(encoder.finish()));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn world_fields_require_apply_and_preserve_full_u64_seeds() {
        let mut menu = Menu::new();
        menu.cursor = [600., 30.];
        menu.click(true);
        assert!(menu.tab == Tab::World);
        menu.worlds.procedural = true;
        menu.cursor = [100., 210.];
        menu.click(true);
        menu.key(KeyCode::Delete, None);
        menu.key(KeyCode::Digit1, Some("18446744073709551615"));
        menu.key(KeyCode::Enter, None);
        menu.cursor = [100., 310.];
        menu.click(true);
        menu.key(KeyCode::Delete, None);
        menu.key(KeyCode::Digit5, Some("5"));
        menu.key(KeyCode::Enter, None);
        menu.cursor = [100., 400.];
        menu.click(true);
        menu.key(KeyCode::Delete, None);
        menu.key(KeyCode::Digit1, Some("10"));
        menu.key(KeyCode::Enter, None);
        assert!(menu.worlds.pending.is_none());
        menu.cursor = [100., 600.];
        menu.click(true);
        assert_eq!(
            menu.worlds.pending.take(),
            Some(crate::worlds::Request::Procedural {
                seed: u64::MAX,
                grid: microvoxel::GridScale::new(1, 20).unwrap(),
                world_scale: 10,
            })
        );
        menu.click(true);
        assert!(menu.worlds.pending.is_none());
        assert!(!menu.vertices(1280, 720, &[]).is_empty());
    }
    #[test]
    fn renderer_toggle_and_quality_presets_preserve_the_selected_backend() {
        let mut menu = Menu::new();
        menu.settings.ray = false;
        menu.cursor = [100., 600.];
        assert!(!menu.click(true));
        assert!(menu.ready(true).unwrap().ray);
        menu.cursor = [100., 100.];
        menu.click(true);
        assert!(menu.ready(true).unwrap().ray);
        menu.cursor = [100., 600.];
        menu.click(true);
        assert!(!menu.ready(true).unwrap().ray);
    }
    #[test]
    fn menu_input_and_drawing_share_the_same_scaled_coordinates() {
        let mut m = Menu::new();
        let (s, x, y) = Menu::transform(1280, 720);
        m.cursor(x + 599. * s, y + 170. * s, 1280, 720);
        m.click(true);
        assert_eq!(m.settings.quality, 100.);
        m.click(false);
        assert!(m.ready(true).is_some());
        assert!(!m.vertices(1280, 720, &[]).is_empty());
    }
}
