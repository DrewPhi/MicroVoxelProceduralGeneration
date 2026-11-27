//! Finite meshless terrain benchmark. Run with --help for the experimental modes.
#[path = "cached_heightfield/data.rs"]
mod data;
#[path = "cached_heightfield/stream.rs"]
mod stream;

use bytemuck::{Pod, Zeroable};
use data::Surface;
use frontier_world::{raycast, FbmParams, FbmRecipe, FbmTerrain, World};
use glam::{DQuat, DVec2, DVec3, IVec2, Vec3, Vec3Swizzles};
use std::{
    collections::HashMap,
    error::Error,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Instant,
};
use wgpu::util::DeviceExt;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const SOURCE: &str = concat!(
    include_str!("cached_heightfield.wgsl"),
    "\n",
    include_str!("cached_heightfield/footprint.wgsl"),
);
const EYE: DVec3 = DVec3::new(-43.625, 75., -289.5625);
const INNER_CELLS: u32 = 512;
const TARGET: DVec3 = DVec3::new(0., 490., 3300.);

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct View {
    eye: [f32; 4],
    forward: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    grid: [f32; 4],
    size: [u32; 4],
    cache: [i32; 4],
    inner: [f32; 4],
    inner_cache: [i32; 4],
    old_eye: [f32; 4],
    old_forward: [f32; 4],
    old_right: [f32; 4],
    old_up: [f32; 4],
}
impl View {
    fn ray(self, x: u32, y: u32) -> (DVec3, DVec3) {
        let xyz = |a: [f32; 4]| Vec3::from_array([a[0], a[1], a[2]]).as_dvec3();
        let u = 2. * (f64::from(x) + 0.5) / f64::from(self.size[0]) - 1.;
        let v = 2. * (f64::from(y) + 0.5) / f64::from(self.size[1]) - 1.;
        (
            xyz(self.eye),
            xyz(self.forward) + u * xyz(self.right) - v * xyz(self.up),
        )
    }
}
struct Options {
    width: u32,
    height: u32,
    cells: usize,
    trials: u32,
    frames: u32,
    warmup: u32,
    seed: u64,
    route: String,
    modes: Vec<String>,
    bands: u32,
    stride: u32,
    native: bool,
    spiky_fixture: bool,
    snapshot: Option<PathBuf>,
    snapshot_origin: Option<DVec2>,
    snapshot_eye: Option<DVec3>,
    snapshot_target: Option<DVec3>,
    snapshot_far: Option<f64>,
    stream: bool,
    nested: bool,
    history: bool,
    shift_window: bool,
    audit_native: bool,
    bilinear: bool,
    level: bool,
    shaded: bool,
    minmax: bool,
    instrument: bool,
    capture_motion: bool,
    allow_software: bool,
    oracle: u32,
    probes: Vec<[u32; 2]>,
    output: PathBuf,
}

fn options() -> Result<Option<Options>> {
    let mut o = Options {
        width: 640,
        height: 360,
        cells: 1024,
        trials: 2,
        frames: 20,
        warmup: 4,
        seed: 7,
        route: "still".into(),
        modes: vec!["dick".into()],
        bands: 64,
        stride: 1,
        native: false,
        spiky_fixture: false,
        snapshot: None,
        snapshot_origin: None,
        snapshot_eye: None,
        snapshot_target: None,
        snapshot_far: None,
        stream: false,
        nested: false,
        history: false,
        shift_window: false,
        audit_native: false,
        bilinear: false,
        level: false,
        shaded: false,
        minmax: false,
        instrument: false,
        capture_motion: false,
        allow_software: false,
        oracle: 0,
        probes: Vec::new(),
        output: "output/cached-heightfield".into(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" => {
                println!("cached_heightfield [--width 640 --height 360 --cells 1024 --frames 20 --warmup 4 --trials 2 --seed 7] [--modes max,dick,dda,vertical,adaptive,silhouette,filtered,packet4,ssaa4,ssaa64] [--route still|grazing|walk|turn|high|low|bank|banked|revisit|teleport] [--level --bands 64] [--mip-step 1|2 --minmax --bilinear --native --spiky-fixture --stream --nested --history --shift-window --audit-native --shaded --instrument --capture-motion --allow-software --oracle 0 --probe X,Y --output DIR]");
                println!("Production ground snapshot: --native --ground-snapshot FILE --origin X,Z --eye X,Y,Z --target X,Y,Z --far RADIAL_METRES");
                return Ok(None);
            }
            "--bilinear" => o.bilinear = true,
            "--native" => o.native = true,
            "--spiky-fixture" => o.spiky_fixture = true,
            "--history" => o.history = true,
            "--shift-window" => o.shift_window = true,
            "--audit-native" => o.audit_native = true,
            "--nested" => {
                o.nested = true;
                o.stream = true;
                o.bilinear = true;
            }
            "--stream" => {
                o.stream = true;
                o.bilinear = true;
            }
            "--level" => o.level = true,
            "--shaded" => o.shaded = true,
            "--minmax" => o.minmax = true,
            "--instrument" => o.instrument = true,
            "--capture-motion" => o.capture_motion = true,
            "--allow-software" => o.allow_software = true,
            name => {
                let value = args.next().ok_or_else(|| format!("{name} needs a value"))?;
                match name {
                    "--width" => o.width = value.parse()?,
                    "--height" => o.height = value.parse()?,
                    "--cells" => o.cells = value.parse()?,
                    "--trials" => o.trials = value.parse()?,
                    "--frames" => o.frames = value.parse()?,
                    "--warmup" => o.warmup = value.parse()?,
                    "--seed" => o.seed = value.parse()?,
                    "--route" => o.route = value,
                    "--modes" => o.modes = value.split(',').map(str::to_owned).collect(),
                    "--bands" => o.bands = value.parse()?,
                    "--mip-step" => o.stride = value.parse()?,
                    "--oracle" => o.oracle = value.parse()?,
                    "--ground-snapshot" => o.snapshot = Some(value.into()),
                    "--origin" => o.snapshot_origin = Some(DVec2::from_array(position(&value)?)),
                    "--eye" => o.snapshot_eye = Some(DVec3::from_array(position(&value)?)),
                    "--target" => o.snapshot_target = Some(DVec3::from_array(position(&value)?)),
                    "--far" => o.snapshot_far = Some(value.parse()?),
                    "--probe" => {
                        let (x, y) = value.split_once(',').ok_or("probe needs X,Y")?;
                        o.probes.push([x.parse()?, y.parse()?]);
                    }
                    "--output" => o.output = value.into(),
                    _ => return Err(format!("unknown option {name}").into()),
                }
            }
        }
    }
    let max_cells = if o.radial_clip() && o.native {
        8192
    } else {
        4096
    };
    if !(32..=max_cells).contains(&o.cells)
        || !o.cells.is_power_of_two()
        || !(1..=1920).contains(&o.width)
        || !(1..=1080).contains(&o.height)
        || !(1..=8).contains(&o.trials)
        || !(1..=240).contains(&o.frames)
        || !(1..=1080).contains(&o.bands)
        || ![1, 2].contains(&o.stride)
        || o.oracle > 512
        || o.probes.len() > 32
        || o.probes.iter().any(|p| p[0] >= o.width || p[1] >= o.height)
        || o.warmup > 60
        || (o.bilinear && o.native)
        || o.modes.iter().any(|m| {
            !matches!(m.as_str(), "max" | "dick" | "dda" | "vertical") && footprint(m).is_none()
        })
        || !matches!(
            o.route.as_str(),
            "still"
                | "grazing"
                | "walk"
                | "turn"
                | "high"
                | "low"
                | "bank"
                | "banked"
                | "revisit"
                | "teleport"
        )
    {
        return Err("unsupported dimensions or experiment options".into());
    }
    if o.modes.iter().any(|m| m == "vertical")
        && (!o.level
            || o.radial_clip()
            || o.nested
            || o.history
            || matches!(o.route.as_str(), "bank" | "banked")
            || o.stride != 1)
    {
        return Err(
            "vertical mode requires an unbanked level camera, an axial clip, one cache and mip step 1".into(),
        );
    }
    if o.stream && (o.cells < 128 || o.minmax || o.native) {
        return Err("streaming needs at least 128 cells and bilinear maximum bounds".into());
    }
    if (o.shift_window || o.audit_native) && !o.stream {
        return Err("window shifting and the GPU source audit require --stream or --nested".into());
    }
    if o.history && !o.bilinear {
        return Err("history validation requires bilinear geometry".into());
    }
    if o.native && o.cells < 256 {
        return Err("native fixture requires at least 256 cells per side".into());
    }
    if o.modes.iter().any(|m| footprint(m).is_some()) && (o.stream || o.history || !o.shaded) {
        return Err("pixel footprint experiments require finite terrain and --shaded".into());
    }
    if o.modes
        .iter()
        .any(|m| footprint(m).is_some_and(|samples| samples.tile_proof || samples.adaptive))
        && !o.native
    {
        return Err("footprint proofs require --native columns".into());
    }
    if o.spiky_fixture
        && (o.stream
            || !matches!(o.route.as_str(), "still" | "walk" | "bank")
            || (o.native && o.cells != 8192))
    {
        return Err("the spiky fixture uses a 512 m finite window (8192 native columns per side) with a still, walk or bank route".into());
    }
    match (
        o.snapshot_origin,
        o.snapshot_eye,
        o.snapshot_target,
        o.snapshot_far,
    ) {
        (Some(origin), Some(eye), Some(target), Some(far)) if o.snapshot.is_some() => {
            let local_eye = eye.xz() - origin;
            let direction = target - eye;
            if !o.native
                || o.spiky_fixture
                || o.route != "still"
                || o.level
                || !far.is_finite()
                || far <= 0.025
                || origin.fract() != DVec2::ZERO
                || direction.xz().length_squared() < 1e-12
                || local_eye.min_element() < far
                || local_eye.max_element() + far >= o.cells as f64 / 16.
            {
                return Err("ground snapshot needs native still mode, a whole-metre origin and a radial camera contained within the snapshot".into());
            }
        }
        (None, None, None, None) if o.snapshot.is_none() => (),
        _ => return Err(
            "ground snapshot needs all of --ground-snapshot, --origin, --eye, --target and --far"
                .into(),
        ),
    }
    Ok(Some(o))
}

fn position<const N: usize>(text: &str) -> Result<[f64; N]> {
    let values = text
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<Vec<f64>, _>>()?;
    if values.len() != N || values.iter().any(|x| !x.is_finite()) {
        return Err(format!("expected {N} comma-separated finite coordinates").into());
    }
    values
        .try_into()
        .map_err(|_| "invalid coordinate count".into())
}

struct Sampling {
    axis: u32,
    adaptive: bool,
    tile_proof: bool,
    silhouette_only: bool,
    filter_faces: bool,
}

fn footprint(mode: &str) -> Option<Sampling> {
    match mode {
        "adaptive" | "silhouette" | "filtered" | "packet4" | "ssaa4" | "ssaa64" => Some(Sampling {
            axis: if mode == "ssaa64" { 8 } else { 2 },
            adaptive: matches!(mode, "adaptive" | "silhouette" | "filtered"),
            tile_proof: matches!(mode, "adaptive" | "packet4"),
            silhouette_only: matches!(mode, "silhouette" | "filtered"),
            filter_faces: mode == "filtered",
        }),
        _ => None,
    }
}

impl Options {
    fn radial_clip(&self) -> bool {
        self.spiky_fixture || self.snapshot.is_some()
    }

    fn terrain_params(&self) -> FbmParams {
        FbmParams {
            recipe: if self.spiky_fixture {
                FbmRecipe::Legacy
            } else {
                FbmRecipe::Alpine
            },
            ..Default::default()
        }
    }

    fn view(&self, s: &Surface, terrain: &FbmTerrain, frame: u32) -> View {
        let t = f64::from(frame) / f64::from(self.frames.saturating_sub(1).max(1));
        let (mut eye, target, mut yaw, mut roll) = match self.route.as_str() {
            "grazing" => (
                DVec3::new(-1156., 995., 2460.),
                DVec3::new(900., 650., 3900.),
                0.,
                0.,
            ),
            "walk" => (EYE + DVec3::new(0., 0., 12. * t), TARGET, 0.21 * t, 0.),
            "turn" => (EYE, TARGET, 0.85 * t, 0.),
            "high" => (
                DVec3::new(-350. + 1100. * t, 900., 620. + 1350. * t),
                TARGET,
                0.18 * t,
                0.,
            ),
            "low" => (
                EYE + DVec3::new(74. * t, 4. * t, 1100. * t),
                TARGET,
                0.2 * t,
                0.,
            ),
            "banked" => (DVec3::new(150., 615., 1305.), TARGET, 0.15, 0.59),
            "bank" => (
                DVec3::new(-300. + 900. * t, 540. + 150. * t, 580. + 1450. * t),
                TARGET,
                0.3 * t,
                0.59 * (t * std::f64::consts::PI).sin(),
            ),
            "teleport" => {
                let shift = if t >= 0.5 {
                    DVec3::new(18000., 0., 2300.)
                } else {
                    DVec3::ZERO
                };
                (EYE + shift, TARGET + shift, 0., 0.)
            }
            "revisit" => (
                EYE + DVec3::new(
                    460. * (t * std::f64::consts::TAU).sin(),
                    0.,
                    900. * (t * std::f64::consts::PI).sin(),
                ),
                TARGET,
                0.,
                0.,
            ),
            _ => (EYE, TARGET, 0., 0.),
        };
        let mut target = target;
        let far = if let (Some(snapshot_eye), Some(snapshot_target), Some(far)) =
            (self.snapshot_eye, self.snapshot_target, self.snapshot_far)
        {
            eye = snapshot_eye;
            target = snapshot_target;
            far as f32
        } else if self.spiky_fixture {
            let slope = DVec3::new(
                -8.731370115810932e-8,
                -0.04993761703372,
                -0.9987523555755615,
            );
            eye = DVec3::new(0., terrain.height_m(0., 0.) + 1.7, 0.);
            yaw = 0.;
            roll = 0.;
            if self.route == "walk" {
                eye.z -= 5. * t;
                eye.y = terrain.height_m(0., eye.z) + 1.7;
            }
            if self.route == "bank" {
                eye += DVec3::new(10. * t, 29. - 12. * t, -35. * t);
                roll = 0.26 * (t * std::f64::consts::PI).sin();
                yaw = 0.10 * (2. * t - 1.);
            }
            target = eye + slope;
            300.
        } else if self.native {
            eye = EYE + DVec3::new(0., 0., 4. * t * f64::from(u8::from(self.route == "walk")));
            eye.y = terrain.height_m(eye.x, eye.z) + 1.7;
            target = eye + DVec3::new(0.3, -2., 8.);
            10.
        } else {
            if matches!(self.route.as_str(), "walk" | "low") {
                eye.y =
                    terrain.height_m(eye.x, eye.z) + if self.route == "walk" { 1.7 } else { 26. };
            }
            6000.
        };
        let mut delta = target - eye;
        if self.level {
            delta.y = 0.;
        }
        let forward = (DQuat::from_rotation_y(yaw) * delta.normalize()).as_vec3();
        let roll = glam::Quat::from_axis_angle(forward, roll as f32);
        let right = roll * forward.cross(Vec3::Y).normalize();
        let up = right.cross(forward).normalize();
        let tan = 0.5773502691896258f32;
        View {
            eye: eye.as_vec3().extend(far).to_array(),
            forward: forward.extend(0.).to_array(),
            right: (right * (tan * self.width as f32 / self.height as f32))
                .extend(0.)
                .to_array(),
            up: (up * tan).extend(0.).to_array(),
            grid: [
                s.origin.x as f32,
                s.origin.y as f32,
                (1. / s.pitch) as f32,
                s.n as f32,
            ],
            size: [self.width, self.height, s.n.ilog2(), self.bands],
            cache: [0; 4],
            inner: [0.; 4],
            inner_cache: [0; 4],
            old_eye: [0.; 4],
            old_forward: [0.; 4],
            old_right: [0.; 4],
            old_up: [0.; 4],
        }
    }
}
fn elapsed(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}
fn summary(mut times: Vec<f64>) -> [f64; 3] {
    times.sort_by(f64::total_cmp);
    if times.is_empty() {
        return [0.; 3];
    }
    [0.5, 0.95, 0.99].map(|q| times[((times.len() - 1) as f64 * q).round() as usize])
}
fn fetch(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Result<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, move |v| {
        let _ = tx.send(v);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv()??;
    let data = slice.get_mapped_range().to_vec();
    buffer.unmap();
    Ok(data)
}
fn readback(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}
fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let mut png = png::Encoder::new(fs::File::create(path)?, width, height);
    png.set_color(png::ColorType::Rgba);
    png.set_depth(png::BitDepth::Eight);
    png.write_header()?.write_image_data(rgba)?;
    Ok(())
}
fn upload(
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    mip: u32,
    side: u32,
    channels: u32,
    values: &[f32],
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: mip,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(values),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(side * channels * 4),
            rows_per_image: Some(side),
        },
        wgpu::Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
    );
}
fn oracle(
    o: &Options,
    s: &Surface,
    inner: Option<&Surface>,
    view: View,
    pixels: &[[f32; 2]],
) -> Result<String> {
    if o.oracle == 0 && o.probes.is_empty() {
        return Ok("null".into());
    }
    let n = (o.oracle as f64).sqrt().floor() as u32;
    let world = o
        .snapshot
        .is_none()
        .then(|| World::fbm(o.seed, o.terrain_params()))
        .transpose()?;
    let mut sample_errors = Vec::new();
    let mut native_errors = Vec::new();
    let mut sample_mask = 0;
    let mut native_mask = 0;
    let mut native_faces = 0;
    let mut sample_max = 0f64;
    let normals = [[0, 1, 0], [-1, 0, 0], [1, 0, 0], [0, 0, -1], [0, 0, 1]];
    let regular = (0..n).flat_map(|j| {
        (0..n).map(move |i| {
            [
                (2 * i + 1) * o.width / (2 * n),
                (2 * j + 1) * o.height / (2 * n),
            ]
        })
    });
    let mut targeted = Vec::new();
    for (index, [x, y]) in regular.chain(o.probes.iter().copied()).enumerate() {
        let (eye, d) = view.ray(x, y);
        let [time, face] = pixels[(x + y * o.width) as usize];
        let got = (time >= 0.).then_some(f64::from(time));
        let scale = if o.radial_clip() { 1. / d.length() } else { 1. };
        let far = f64::from(view.eye[3]) * scale;
        let near = 0.025 * scale;
        let reference = if let Some(inner) = inner {
            inner.reference(eye, d, near, far).or_else(|| {
                let start = inner.clip(eye, d, near, far).map_or(near, |(_, end)| end);
                s.reference(eye, d, start, far)
            })
        } else {
            s.reference(eye, d, near, far)
        };
        let native = world.as_ref().and_then(|world| {
            raycast(
                world,
                (eye + d * near).to_array(),
                d.to_array(),
                (far - near) * d.length(),
            )
        });
        if index >= (n * n) as usize {
            let number = |v: Option<f64>| v.map_or("null".to_owned(), |x| x.to_string());
            let native_depth = native.map(|h| h.distance + near * d.length());
            targeted.push(format!(
                    r#"{{"pixel":[{x},{y}],"gpu_radial_m":{},"sampled_radial_m":{},"native_radial_m":{}}}"#,
                    number(got.map(|v|v*d.length())),number(reference.map(|v|v*d.length())),number(native_depth)
                ));
        }
        sample_mask += usize::from(got.is_some() != reference.is_some());
        if world.is_some() {
            native_mask += usize::from(got.is_some() != native.is_some());
        }
        if let (Some(a), Some(b)) = (got, reference) {
            let e = (a - b).abs() * d.length();
            sample_max = sample_max.max(e);
            sample_errors.push(e);
        }
        if let (Some(a), Some(b)) = (got, native) {
            native_errors.push(((a - near) * d.length() - b.distance).abs());
            if o.native && b.distance > 0. {
                native_faces += usize::from(normals[face as usize] != b.normal);
            }
        }
    }
    let probes = targeted.join(",");
    let native_counts = if world.is_some() {
        format!(
            r#""native_coverage_disagreements":{native_mask},"native_error_m_p50_p95_p99":{:?},"native_face_disagreements":{native_faces}"#,
            summary(native_errors)
        )
    } else {
        r#""native_coverage_disagreements":null,"native_error_m_p50_p95_p99":null,"native_face_disagreements":null"#.into()
    };
    Ok(format!(
        r#"{{"rays":{},"sampled_coverage_disagreements":{sample_mask},"sampled_error_m_p50_p95_p99":{:?},"sampled_max_m":{sample_max},{native_counts},"probes":[{probes}]}}"#,
        (n * n) as usize + o.probes.len(),
        summary(sample_errors)
    ))
}

fn read_heights(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    offset: IVec2,
    n: usize,
) -> Result<Vec<f32>> {
    let row = ((n + 1) * 4).next_multiple_of(256);
    let read = readback(device, (row * (n + 1)) as u64);
    let mut cmd = device.create_command_encoder(&Default::default());
    cmd.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &read,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row as u32),
                rows_per_image: Some(n as u32 + 1),
            },
        },
        wgpu::Extent3d {
            width: n as u32 + 1,
            height: n as u32 + 1,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([cmd.finish()]);
    let bytes = fetch(device, &read)?;
    let address = |p: usize, axis: usize| {
        if p == n {
            n
        } else {
            ((p as i32 + offset[axis]) & (n as i32 - 1)) as usize
        }
    };
    let mut heights = Vec::with_capacity((n + 1) * (n + 1));
    for z in 0..=n {
        for x in 0..=n {
            let offset = address(z, 1) * row + address(x, 0) * 4;
            heights.push(f32::from_le_bytes(bytes[offset..offset + 4].try_into()?));
        }
    }
    Ok(heights)
}

fn sample_parity(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    view: View,
    reference: &Surface,
) -> Result<String> {
    let n = reference.n;
    let heights = read_heights(
        device,
        queue,
        texture,
        IVec2::new(view.cache[0], view.cache[1]),
        n,
    )?;
    let mut errors = Vec::new();
    let mut floor_disagreement = 0;
    for iz in 0..=64 {
        let z = iz * n / 64;
        for ix in 0..=64 {
            let x = ix * n / 64;
            let got = heights[z * (n + 1) + x];
            let expected = reference.points[z * (n + 1) + x];
            errors.push(f64::from((expected - got).abs()));
            floor_disagreement += usize::from((expected * 16.).floor() != (got * 16.).floor());
        }
    }
    let max = errors.iter().copied().fold(0., f64::max);
    Ok(format!(
        r#"{{"points":{},"absolute_error_m_p50_p95_p99":{:?},"max_error_m":{max},"floor_to_voxel_disagreements":{floor_disagreement}}}"#,
        errors.len(),
        summary(errors)
    ))
}

fn native_source_audit(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    terrain: &FbmTerrain,
    make_texture: impl Fn(u32, u32, wgpu::TextureFormat) -> wgpu::Texture,
) -> Result<String> {
    let n = 512u32;
    let pitch = 0.0625;
    let heights = make_texture(n + 1, 1, wgpu::TextureFormat::R32Float);
    let maxima = make_texture(n, n.ilog2() + 1, wgpu::TextureFormat::R32Float);
    let mut cache = stream::Stream::new(device, n, terrain.seed(), pitch, &heights, &maxima)
        .at_native_centers();
    let mut regions = Vec::new();
    for eye in [DVec2::new(EYE.x, EYE.z), DVec2::new(-900., 3000.)] {
        let base = cache.target(eye);
        cache.update(device, queue, base)?;
        let values = read_heights(device, queue, &heights, base, n as usize)?;
        let mut mismatches = 0u32;
        let mut max_cells = 0i64;
        for z in 0..n {
            for x in 0..n {
                let got = (values[(z * (n + 1) + x) as usize] * 16.).floor() as i64;
                let exact = terrain.height_cells(
                    i64::from(base.x) + i64::from(x),
                    i64::from(base.y) + i64::from(z),
                );
                let delta = (got - exact).abs();
                mismatches += u32::from(delta > 0);
                max_cells = max_cells.max(delta);
            }
        }
        regions.push(format!(r#"{{"origin_cells":{:?},"tested":{},"height_disagreements":{mismatches},"max_cell_difference":{max_cells}}}"#,base.to_array(),n*n));
    }
    Ok(format!(
        r#"{{"voxel_m":{pitch},"regions":[{}]}}"#,
        regions.join(",")
    ))
}

fn read_ground_snapshot(path: &Path, cells: usize) -> Result<(Vec<f32>, Vec<f32>)> {
    let mut file = fs::File::open(path)?;
    let heights = cells * cells;
    let colors = (cells / 16).pow(2);
    if file.metadata()?.len() != ((heights + colors) * 4) as u64 {
        return Err("ground snapshot length differs from the selected --cells".into());
    }
    let mut heights = vec![0f32; heights];
    let mut colors = vec![0f32; colors];
    for plane in [&mut heights, &mut colors] {
        file.read_exact(bytemuck::cast_slice_mut(plane))?;
        if cfg!(target_endian = "big") {
            for value in plane {
                *value = f32::from_bits(u32::from_le(value.to_bits()));
            }
        }
    }
    if heights
        .iter()
        .any(|h| !h.is_finite() || (h * 16.).fract() != 0.)
        || colors.iter().any(|c| !(0. ..=16_777_215.).contains(c))
    {
        return Err("ground snapshot contains invalid heights or colors".into());
    }
    Ok((heights, colors))
}

fn main() -> Result<()> {
    let start = Instant::now();
    let Some(o) = options()? else {
        return Ok(());
    };
    fs::create_dir_all(&o.output)?;
    let terrain = FbmTerrain::new(o.seed, o.terrain_params())?;
    let timer = Instant::now();
    let origin = if let Some(origin) = o.snapshot_origin {
        origin
    } else if o.spiky_fixture {
        DVec2::new(-256., -384.)
    } else if o.native {
        DVec2::new(-44., -293.) - DVec2::splat(o.cells as f64 * 0.0625 * 0.35).floor()
    } else {
        DVec2::new(-8192., -4096.)
    };
    let pitch = if o.native {
        0.0625
    } else if o.spiky_fixture {
        512. / o.cells as f64
    } else {
        16384. / o.cells as f64
    };
    let (surface, albedo) = if let Some(path) = &o.snapshot {
        let (points, colors) = read_ground_snapshot(path, o.cells)?;
        (
            Surface {
                origin,
                n: o.cells,
                pitch,
                bilinear: false,
                points,
            },
            colors,
        )
    } else if o.stream {
        (
            Surface {
                origin,
                n: o.cells,
                pitch,
                bilinear: true,
                points: Vec::new(),
            },
            Vec::new(),
        )
    } else {
        (
            Surface::build(&terrain, origin, o.cells, pitch, o.bilinear),
            Vec::new(),
        )
    };
    let generation_ms = elapsed(timer);
    let timer = Instant::now();
    let levels = (!o.stream).then(|| surface.hierarchy(o.minmax));
    let hierarchy_ms = elapsed(timer);

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..Default::default()
    });
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok_or("no Vulkan adapter")?;
    let info = adapter.get_info();
    eprintln!("Adapter {info:?}");
    if info.device_type == wgpu::DeviceType::Cpu && !o.allow_software {
        return Err("software Vulkan needs --allow-software".into());
    }
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            required_features: wgpu::Features::TIMESTAMP_QUERY,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        },
        None,
    ))?;
    let texture = |side, mips, format| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | if o.stream {
                    wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::STORAGE_BINDING
                } else {
                    wgpu::TextureUsages::empty()
                },
            view_formats: &[],
        })
    };
    let timer = Instant::now();
    let n = o.cells as u32;
    let channels = if o.minmax { 2 } else { 1 };
    let format = if o.minmax {
        wgpu::TextureFormat::Rg32Float
    } else {
        wgpu::TextureFormat::R32Float
    };
    let maximum = texture(n, n.ilog2() + 1, format);
    let sample = texture(
        if o.bilinear {
            n + 1
        } else if o.snapshot.is_some() {
            n / 16
        } else {
            1
        },
        1,
        wgpu::TextureFormat::R32Float,
    );
    if let Some(levels) = &levels {
        for (i, level) in levels.iter().enumerate() {
            upload(&queue, &maximum, i as u32, n >> i, channels, level);
        }
        if o.bilinear {
            upload(&queue, &sample, 0, n + 1, 1, &surface.points);
        }
    }
    if o.snapshot.is_some() {
        upload(&queue, &sample, 0, n / 16, 1, &albedo);
    }
    drop(levels);
    queue.submit([]);
    device.poll(wgpu::Maintain::Wait);
    let upload_ms = elapsed(timer);
    let mut cache = o
        .stream
        .then(|| stream::Stream::new(&device, n, o.seed, pitch, &sample, &maximum));
    let terrain_bytes = 4 * u64::from(channels) * (4 * u64::from(n) * u64::from(n) - 1) / 3
        + if o.bilinear {
            4 * u64::from(n + 1) * u64::from(n + 1)
        } else if o.snapshot.is_some() {
            (albedo.len() * 4) as u64
        } else {
            4
        };
    let inner_size = if o.nested { INNER_CELLS } else { 1 };
    let inner_maximum = texture(
        inner_size,
        inner_size.ilog2() + 1,
        wgpu::TextureFormat::R32Float,
    );
    let inner_sample = texture(
        inner_size + u32::from(o.nested),
        1,
        wgpu::TextureFormat::R32Float,
    );
    let inner_pitch = pitch / 16.;
    let mut inner_cache = o.nested.then(|| {
        stream::Stream::new(
            &device,
            inner_size,
            o.seed,
            inner_pitch,
            &inner_sample,
            &inner_maximum,
        )
    });
    let terrain_bytes = terrain_bytes
        + 4 * (4 * u64::from(inner_size).pow(2) - 1) / 3
        + 4 * u64::from(inner_size + u32::from(o.nested)).pow(2);
    let buffer = |size| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    };
    let pixels = u64::from(o.width) * u64::from(o.height);
    let depths = buffer(pixels * 8);
    let rgb = buffer(if o.shaded { pixels * 4 } else { 4 });
    let work = buffer(if o.instrument { pixels * 16 } else { 16 });
    let history_bytes = if o.history { pixels * 8 } else { 8 };
    let history = buffer(history_bytes);
    let depth_read = readback(&device, pixels * 8);
    let rgb_read = o.shaded.then(|| readback(&device, pixels * 4));
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::bytes_of(&o.view(&surface, &terrain, 0)),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("cached heightfield"),
        source: wgpu::ShaderSource::Wgsl(SOURCE.into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &std::array::from_fn::<_, 9, _>(|i| wgpu::BindGroupLayoutEntry {
            binding: i as u32,
            visibility: wgpu::ShaderStages::COMPUTE,
            count: None,
            ty: if matches!(i, 1 | 2 | 6 | 7) {
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                }
            } else {
                wgpu::BindingType::Buffer {
                    ty: if i == 0 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage { read_only: false }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                }
            },
        }),
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(
                    &maximum.create_view(&Default::default()),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(
                    &sample.create_view(&Default::default()),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: depths.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: rgb.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: work.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(
                    &inner_maximum.create_view(&Default::default()),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(
                    &inner_sample.create_view(&Default::default()),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: history.as_entire_binding(),
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let pipeline = |entry, method, sample_mode: Option<Sampling>| {
        let samples = sample_mode.unwrap_or(Sampling {
            axis: 2,
            adaptive: false,
            tile_proof: false,
            silhouette_only: false,
            filter_faces: false,
        });
        let constants = HashMap::from([
            ("METHOD".into(), f64::from(method)),
            ("BILINEAR".into(), f64::from(o.bilinear)),
            ("MINMAX".into(), f64::from(o.minmax)),
            ("INSTRUMENT".into(), f64::from(o.instrument)),
            ("LEVEL_STEP".into(), f64::from(o.stride)),
            ("STREAM".into(), f64::from(o.stream)),
            ("NESTED".into(), f64::from(o.nested)),
            ("HISTORY".into(), f64::from(o.history)),
            ("RADIAL_CLIP".into(), f64::from(o.radial_clip())),
            ("GROUND_ALBEDO".into(), f64::from(o.snapshot.is_some())),
            ("FOOTPRINT_SAMPLES".into(), f64::from(samples.axis)),
            ("ADAPTIVE_FOOTPRINT".into(), f64::from(samples.adaptive)),
            ("TILE_PROOF".into(), f64::from(samples.tile_proof)),
            ("SILHOUETTE_ONLY".into(), f64::from(samples.silhouette_only)),
            ("FILTER_FACES".into(), f64::from(samples.filter_faces)),
        ]);
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &constants,
                ..Default::default()
            },
            cache: None,
        })
    };
    let pipes: Vec<_> = o
        .modes
        .iter()
        .map(|s| match s.as_str() {
            "max" => pipeline("independent", 0u32, None),
            "dick" => pipeline("independent", 1u32, None),
            "vertical" => pipeline("vertical", 1u32, None),
            _ => pipeline("independent", 2u32, None),
        })
        .collect();
    let shade: Vec<_> = o
        .modes
        .iter()
        .map(|mode| {
            o.shaded.then(|| match footprint(mode) {
                Some(samples) => pipeline("shade_footprint", 2u32, Some(samples)),
                None => pipeline("shade_image", 1u32, None),
            })
        })
        .collect();
    let retain = o.history.then(|| pipeline("save_history", 1u32, None));
    let query_count = 2 * (1 + u32::from(o.shaded) + u32::from(o.history));
    let query = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: None,
        ty: wgpu::QueryType::Timestamp,
        count: query_count,
    });
    let resolve = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(query_count) * 8,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let time_read = readback(&device, u64::from(query_count) * 8);
    let mut visibility = vec![Vec::new(); pipes.len()];
    let mut gpu = visibility.clone();
    let mut wall = visibility.clone();
    let mut first = None;
    let mut motion_unresolved = vec![0; pipes.len()];
    let mut updates = (0..pipes.len()).map(|_| Vec::new()).collect::<Vec<_>>();
    for trial in 0..o.trials {
        for j in 0..pipes.len() {
            let i = if trial % 2 == 0 {
                j
            } else {
                pipes.len() - 1 - j
            };
            let mode = &o.modes[i];
            let folder = o.output.join(mode);
            fs::create_dir_all(&folder)?;
            let groups = o.width * o.height.div_ceil(o.bands);
            let mut previous: Option<View> = None;
            for f in 0..o.frames + o.warmup {
                let timer = Instant::now();
                let frame = f.saturating_sub(o.warmup);
                let mut view = o.view(&surface, &terrain, frame);
                let mut update_gpu = 0.;
                if let Some(cache) = &mut cache {
                    let base =
                        cache.target(DVec2::new(f64::from(view.eye[0]), f64::from(view.eye[2])));
                    let base = base + IVec2::splat(64 * i32::from(o.shift_window));
                    if let Some(update) = cache.update(&device, &queue, base)? {
                        update_gpu = update.gpu_ms;
                        updates[i].push((trial, frame, "outer", base, update));
                    }
                    view.cache = [base.x, base.y, 0, 0];
                    view.grid[0] = (f64::from(base.x) * pitch) as f32;
                    view.grid[1] = (f64::from(base.y) * pitch) as f32;
                }
                if let Some(cache) = &mut inner_cache {
                    let base =
                        cache.target(DVec2::new(f64::from(view.eye[0]), f64::from(view.eye[2])));
                    let base = base + IVec2::splat(64 * i32::from(o.shift_window));
                    if let Some(update) = cache.update(&device, &queue, base)? {
                        update_gpu += update.gpu_ms;
                        updates[i].push((trial, frame, "inner", base, update));
                    }
                    view.inner_cache = [base.x, base.y, inner_size.ilog2() as i32, 0];
                    view.inner = [
                        (f64::from(base.x) * inner_pitch) as f32,
                        (f64::from(base.y) * inner_pitch) as f32,
                        (1. / inner_pitch) as f32,
                        inner_size as f32,
                    ];
                }
                if o.history {
                    if let Some(old) = previous {
                        let displacement = Vec3::from_array([
                            view.eye[0] - old.eye[0],
                            view.eye[1] - old.eye[1],
                            view.eye[2] - old.eye[2],
                        ]);
                        let dot = (0..3)
                            .map(|i| view.forward[i] * old.forward[i])
                            .sum::<f32>();
                        if displacement.length() < view.eye[3] * 0.125 && dot > 0.7 {
                            view.old_eye = old.eye;
                            view.old_forward = old.forward;
                            view.old_right = old.right;
                            view.old_up = old.up;
                        }
                    }
                    previous = Some(view);
                }
                queue.write_buffer(&uniform, 0, bytemuck::bytes_of(&view));
                let mut cmd = device.create_command_encoder(&Default::default());
                for (pass, pipe) in std::iter::once(&pipes[i])
                    .chain(shade[i].iter())
                    .chain(retain.iter())
                    .enumerate()
                {
                    let mut compute = cmd.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: None,
                        timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                            query_set: &query,
                            beginning_of_pass_write_index: Some(pass as u32 * 2),
                            end_of_pass_write_index: Some(pass as u32 * 2 + 1),
                        }),
                    });
                    compute.set_pipeline(pipe);
                    compute.set_bind_group(0, &bind, &[]);
                    if pass == 0 && mode == "vertical" {
                        compute.dispatch_workgroups(groups.div_ceil(64), 1, 1);
                    } else {
                        compute.dispatch_workgroups(o.width.div_ceil(8), o.height.div_ceil(8), 1);
                    }
                }
                cmd.resolve_query_set(&query, 0..query_count, &resolve, 0);
                cmd.copy_buffer_to_buffer(&resolve, 0, &time_read, 0, u64::from(query_count) * 8);
                let final_frame = f + 1 == o.frames + o.warmup && trial + 1 == o.trials;
                let capture = trial + 1 == o.trials && f >= o.warmup && o.capture_motion;
                if final_frame || capture {
                    cmd.copy_buffer_to_buffer(&depths, 0, &depth_read, 0, pixels * 8);
                    if let Some(read) = &rgb_read {
                        cmd.copy_buffer_to_buffer(&rgb, 0, read, 0, pixels * 4);
                    }
                }
                queue.submit([cmd.finish()]);
                let ticks = fetch(&device, &time_read)?;
                let mut ms = bytemuck::cast_slice::<u8, u64>(&ticks)
                    .chunks_exact(2)
                    .map(|v| {
                        v[1].saturating_sub(v[0]) as f64
                            * f64::from(queue.get_timestamp_period())
                            * 1e-6
                    });
                let trace = ms.next().unwrap();
                let total = trace + ms.sum::<f64>() + update_gpu;
                first.get_or_insert_with(|| elapsed(start));
                if f >= o.warmup {
                    visibility[i].push(trace);
                    gpu[i].push(total);
                    wall[i].push(elapsed(timer));
                }
                if final_frame || capture {
                    let data = fetch(&device, &depth_read)?;
                    let hits = bytemuck::cast_slice::<u8, [f32; 2]>(&data);
                    let mut unresolved = hits.iter().filter(|h| h[0] == -2.).count();
                    if let Some(read) = &rgb_read {
                        let mut colors = fetch(&device, read)?;
                        let coverage = if footprint(mode).is_some() {
                            let mask: Vec<_> = colors
                                .chunks_exact_mut(4)
                                .zip(hits)
                                .map(|(pixel, hit)| {
                                    let value = pixel[3];
                                    if pixel == [255, 0, 255, 0] && hit[0] != -2. {
                                        unresolved += 1;
                                    }
                                    pixel[3] = 255;
                                    value
                                })
                                .collect();
                            Some(mask)
                        } else if o.radial_clip() {
                            Some(
                                hits.iter()
                                    .map(|hit| if hit[0] >= 0. { 255 } else { 0 })
                                    .collect(),
                            )
                        } else {
                            None
                        };
                        if final_frame {
                            save_png(&folder.join("terrain.png"), o.width, o.height, &colors)?;
                            if let Some(mask) = &coverage {
                                fs::write(folder.join("coverage.u8"), mask)?;
                            }
                        }
                        if capture {
                            let frames = folder.join("frames");
                            fs::create_dir_all(&frames)?;
                            save_png(
                                &frames.join(format!("{frame:04}.png")),
                                o.width,
                                o.height,
                                &colors,
                            )?;
                            if let Some(mask) = &coverage {
                                fs::write(frames.join(format!("{frame:04}.u8")), mask)?;
                            }
                        }
                    }
                    motion_unresolved[i] = motion_unresolved[i].max(unresolved);
                    if !final_frame {
                        continue;
                    }
                    let distances: Vec<f32> = hits.iter().map(|p| p[0]).collect();
                    fs::write(folder.join("depth.f32"), bytemuck::cast_slice(&distances))?;
                    let counts = if o.instrument {
                        let size = if mode == "vertical" {
                            u64::from(groups)
                        } else {
                            pixels
                        };
                        let read = readback(&device, size * 16);
                        let mut cmd = device.create_command_encoder(&Default::default());
                        cmd.copy_buffer_to_buffer(&work, 0, &read, 0, size * 16);
                        queue.submit([cmd.finish()]);
                        let values = fetch(&device, &read)?;
                        let totals = bytemuck::cast_slice::<u8, [u32; 4]>(&values)
                            .iter()
                            .fold([0u64; 4], |v, p| {
                                std::array::from_fn(|i| v[i] + u64::from(p[i]))
                            });
                        format!("{totals:?}")
                    } else {
                        "null".into()
                    };
                    let active_origin =
                        DVec2::new(f64::from(view.grid[0]), f64::from(view.grid[1]));
                    let check_surface = (o.stream && (o.oracle > 0 || !o.probes.is_empty()))
                        .then(|| Surface::build(&terrain, active_origin, o.cells, pitch, true));
                    let check_inner =
                        (o.nested && (o.oracle > 0 || !o.probes.is_empty())).then(|| {
                            let origin =
                                DVec2::new(f64::from(view.inner[0]), f64::from(view.inner[1]));
                            let mut near = Surface::build(
                                &terrain,
                                origin,
                                inner_size as usize,
                                inner_pitch,
                                true,
                            );
                            near.blend_parent(
                                check_surface.as_ref().unwrap(),
                                DVec2::new(f64::from(view.eye[0]), f64::from(view.eye[2])),
                            );
                            near
                        });
                    let check = oracle(
                        &o,
                        check_surface.as_ref().unwrap_or(&surface),
                        check_inner.as_ref(),
                        view,
                        hits,
                    )?;
                    let parity = if let Some(reference) = &check_surface {
                        sample_parity(&device, &queue, &sample, view, reference)?
                    } else {
                        "null".into()
                    };
                    let cache_updates=updates[i].iter().map(|(trial,frame,tier,base,u)| format!(
                        r#"{{"trial":{trial},"frame":{frame},"tier":{tier:?},"origin_cells":{:?},"new_samples":{},"halo_samples":{},"bounds_nodes":{},"gpu_ms":{},"wall_ms":{}}}"#,
                        base.to_array(), u.new_samples, u.halo_samples, u.bounds_nodes, u.gpu_ms, u.wall_ms
                    )).collect::<Vec<_>>().join(",");
                    let count = visibility[i].len();
                    let trace = summary(std::mem::take(&mut visibility[i]));
                    let total = summary(std::mem::take(&mut gpu[i]));
                    let frame_wall = summary(std::mem::take(&mut wall[i]));
                    let rss = fs::read_to_string("/proc/self/status")
                        .ok()
                        .and_then(|s| {
                            s.lines()
                                .find(|l| l.starts_with("VmHWM:"))?
                                .split_whitespace()
                                .nth(1)?
                                .parse::<u64>()
                                .ok()
                        })
                        .unwrap_or(0);
                    let measures = if footprint(mode).is_some() {
                        r#"["trace_bounds_fetches","tested_leaf_intervals","extra_rays","certified_sky_pixels"]"#
                    } else {
                        r#"["bounds_fetches","tested_leaf_intervals","valid_history_caps","tested_history_depths"]"#
                    };
                    let fields = [
                        format!(
                            r#""adapter":{:?},"driver":{:?},"device_type":{:?}"#,
                            info.name,
                            info.driver_info,
                            format!("{:?}", info.device_type)
                        ),
                        format!(
                            r#""mode":{mode:?},"route":{:?},"seed":{},"eye":{:?},"forward":{:?},"right":{:?},"up":{:?},"far":{}"#,
                            o.route,
                            o.seed,
                            view.eye,
                            view.forward,
                            view.right,
                            view.up,
                            view.eye[3]
                        ),
                        format!(
                            r#""width":{},"height":{},"cells":{},"mip_step":{},"bands":{},"samples":{count},"trials":{}"#,
                            o.width, o.height, o.cells, o.stride, o.bands, o.trials
                        ),
                        format!(
                            r#""pitch_m":{pitch},"origin":{:?},"bilinear":{},"native":{},"minmax":{},"level":{},"shaded":{}"#,
                            active_origin.to_array(),
                            o.bilinear,
                            o.native,
                            o.minmax,
                            o.level,
                            o.shaded
                        ),
                        format!(
                            r#""visibility_ms_p50_p95_p99":{trace:?},"gpu_ms_p50_p95_p99":{total:?},"frame_wall_ms_p50_p95_p99":{frame_wall:?}"#
                        ),
                        format!(
                            r#""height_generation_ms":{generation_ms},"bounds_generation_ms":{hierarchy_ms},"upload_ms":{upload_ms},"first_completed_frame_ms":{}"#,
                            first.unwrap()
                        ),
                        format!(
                            r#""resident_terrain_bytes":{terrain_bytes},"host_sample_bytes":{},"peak_rss_kib":{rss},"depth_bytes":{},"color_bytes":{},"instrumentation_bytes":{}"#,
                            surface.points.len() * 4,
                            pixels * 8,
                            if o.shaded { pixels * 4 } else { 4 },
                            if o.instrument { pixels * 16 } else { 16 }
                        ),
                        format!(
                            r#""stream":{},"nested":{},"inner_pitch_m":{},"inner_bounds":{:?},"sample_parity":{parity},"cache_updates":[{cache_updates}]"#,
                            o.stream,
                            o.nested,
                            if o.nested { inner_pitch } else { 0. },
                            view.inner
                        ),
                        format!(r#""history":{},"history_bytes":{history_bytes}"#, o.history),
                        format!(
                            r#""spiky_fixture":{},"ground_snapshot":{},"radial_clip":{},"footprint_samples_per_axis":{}"#,
                            o.spiky_fixture,
                            o.snapshot.is_some(),
                            o.radial_clip(),
                            footprint(mode).map_or(1, |samples| samples.axis)
                        ),
                        format!(
                            r#""counters":{measures},"counter_totals":{counts},"unresolved":{unresolved},"captured_max_unresolved":{},"oracle":{check}"#,
                            motion_unresolved[i]
                        ),
                    ];
                    let report = format!("{{{}}}\n", fields.join(","));
                    fs::write(folder.join("report.json"), &report)?;
                    println!("{report}");
                }
            }
        }
    }
    if o.audit_native {
        let audit = native_source_audit(&device, &queue, &terrain, texture)?;
        fs::write(o.output.join("native-source-audit.json"), audit + "\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn shader_validates() {
        let m = naga::front::wgsl::parse_str(super::SOURCE)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(super::SOURCE)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&m)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(super::SOURCE)));
    }
}
