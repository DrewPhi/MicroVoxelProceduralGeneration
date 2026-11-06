//! Disposable, versioned completed-geometry cache. Never a world/save authority.
//! Explicit little-endian fields; no Rust memory images and no decompression.
use crate::adaptive_geometry::{
    AffineStats, FaceAppearance, GeometryTile, NativeBin, NativeQuad, ProxyTriangle, TileKey,
};
use crate::mesh::{Mesh, Vertex};
use frontier_runtime::occlusion::Aabb;
use frontier_world::{Cell, World, SOURCE_VERSION, VOXEL_SIZE};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Write},
    path::PathBuf,
    time::Instant,
};

const MAGIC: &[u8; 8] = b"FVDC0002";
// Bump when geometry, certificate, appearance or ownership construction changes.
const REPRESENTATION_VERSION: u64 = 2;
const MAX_FILES: usize = 16384;
const MAX_EDITS: usize = 4096;
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DiskCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub invalid: u64,
    pub writes: u64,
    pub errors: u64,
    pub evictions: u64,
    pub read_bytes: u64,
    pub written_bytes: u64,
    pub read_ms: f64,
    pub write_ms: f64,
    pub resident_bytes: u64,
    pub writeback_queued_entries: usize,
    pub writeback_peak_queued_entries: usize,
    pub writeback_queued_payload_bytes: usize,
    pub writeback_peak_queued_payload_bytes: usize,
    pub writeback_skipped_writes: u64,
    pub writeback_encode_buffer_cap_bytes: usize,
    pub writeback_encode_buffer_reserved_bytes: usize,
    pub writeback_peak_encode_buffer_reserved_bytes: usize,
}
/// Camera-independent filename calculation, cloneable without copying the
/// bounded source/edit identity or touching the IO worker's lock.
#[derive(Clone, Copy)]
pub(crate) struct DiskCacheKeyer {
    prefix_hash: u64,
}
impl DiskCacheKeyer {
    pub(crate) fn name(self, key: TileKey) -> String {
        format!(
            "{:016x}.fvdc",
            checksum_from(self.prefix_hash, &key_suffix(key))
        )
    }
}
fn key_suffix(key: TileKey) -> [u8; 72] {
    let words = [
        key.min[0] as u64,
        key.min[1] as u64,
        key.width_cells as u64,
        key.step_cells as u64,
        key.edge_steps[0] as u64,
        key.edge_steps[1] as u64,
        key.edge_steps[2] as u64,
        key.edge_steps[3] as u64,
        key.source_revision,
    ];
    std::array::from_fn(|i| words[i / 8].to_le_bytes()[i % 8])
}
pub(crate) struct DiskCache {
    root: PathBuf,
    _lock: fs::File,
    budget: u64,
    max_entry: usize,
    identity: Vec<u8>,
    revision: u64,
    files: HashMap<String, (u64, u64)>,
    clock: u64,
    serial: u64,
    stats: DiskCacheStats,
    removed_names: Vec<String>,
}
impl DiskCache {
    pub fn new(
        root: PathBuf,
        budget_bytes: u64,
        max_entry_bytes: usize,
        world: &World,
    ) -> io::Result<Self> {
        if budget_bytes == 0 || max_entry_bytes < 1024 || max_entry_bytes as u64 > budget_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid derived-cache budgets",
            ));
        }
        let identity = source_identity(world)?;
        fs::create_dir_all(&root)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("cache.lock"))?;
        lock.try_lock()
            .map_err(|e| io::Error::other(format!("derived-cache directory in use: {e}")))?;
        let mut this = Self {
            root,
            _lock: lock,
            budget: budget_bytes,
            max_entry: max_entry_bytes,
            identity,
            revision: world.revision(),
            files: HashMap::new(),
            clock: 0,
            serial: 0,
            stats: DiskCacheStats::default(),
            removed_names: Vec::new(),
        };
        for (i, item) in fs::read_dir(&this.root)?.enumerate() {
            if i >= MAX_FILES {
                return Err(io::Error::other("derived-cache directory entry limit"));
            }
            let item = item?;
            let name = item.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".fvdc") && !name.ends_with(".fvtmp") {
                continue;
            }
            let meta = item.metadata()?;
            if !meta.is_file() {
                continue;
            }
            if name.ends_with(".fvtmp") || meta.len() > max_entry_bytes as u64 {
                fs::remove_file(item.path())?;
                continue;
            }
            this.clock += 1;
            this.stats.resident_bytes = this.stats.resident_bytes.saturating_add(meta.len());
            this.files.insert(name, (meta.len(), this.clock));
        }
        this.make_room(0)?;
        Ok(this)
    }
    pub fn stats(&self) -> DiskCacheStats {
        self.stats
    }
    pub(crate) fn keyer(&self) -> DiskCacheKeyer {
        DiskCacheKeyer {
            prefix_hash: checksum(&self.identity),
        }
    }
    pub(crate) fn index_names(&self) -> HashSet<String> {
        self.files.keys().cloned().collect()
    }
    pub(crate) fn contains_name(&self, name: &str) -> bool {
        self.files.contains_key(name)
    }
    pub(crate) fn max_entry_bytes(&self) -> usize {
        self.max_entry
    }
    /// Removals from the most recent load/store, including corruption and LRU
    /// eviction. Drain after every operation. At most MAX_FILES names can occur;
    /// the next operation discards any undrained events to keep memory bounded.
    pub(crate) fn take_removed_names(&mut self) -> Vec<String> {
        std::mem::take(&mut self.removed_names)
    }
    fn key_bytes(&self, key: TileKey) -> Vec<u8> {
        let mut b = self.identity.clone();
        b.extend_from_slice(&key_suffix(key));
        b
    }
    fn name(&self, key: TileKey) -> String {
        self.keyer().name(key)
    }
    fn remove(&mut self, name: &str) -> io::Result<()> {
        match fs::remove_file(self.root.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        if let Some((n, _)) = self.files.remove(name) {
            self.stats.resident_bytes -= n;
            self.removed_names.push(name.to_owned());
        }
        Ok(())
    }
    fn make_room(&mut self, additional: u64) -> io::Result<()> {
        while self.stats.resident_bytes.saturating_add(additional) > self.budget
            || self.files.len() >= MAX_FILES - 1
        {
            let name = self
                .files
                .iter()
                .min_by_key(|(name, (_, stamp))| (*stamp, *name))
                .map(|(n, _)| n.clone())
                .ok_or_else(|| io::Error::other("cache budget"))?;
            self.remove(&name)?;
            self.stats.evictions += 1;
        }
        Ok(())
    }
    pub fn load(&mut self, key: TileKey) -> Option<GeometryTile> {
        self.removed_names.clear();
        let started = Instant::now();
        let result = self.load_inner(key);
        self.stats.read_ms += started.elapsed().as_secs_f64() * 1000.;
        if result.is_some() {
            self.stats.hits += 1;
        } else {
            self.stats.misses += 1;
        }
        result
    }
    fn load_inner(&mut self, key: TileKey) -> Option<GeometryTile> {
        if key.source_revision != self.revision {
            return None;
        }
        let transient_budget = crate::build_memory::reservation(key)?;
        let name = self.name(key);
        if !self.files.contains_key(&name) {
            return None;
        }
        let result = (|| {
            // Read a bounded amount even if another process replaces/grows a file.
            use std::io::Read;
            let mut file = fs::File::open(self.root.join(&name))?;
            let n = file.metadata()?.len();
            // Compact scalar vectors decode to no more payload bytes than
            // their encoded fields. Reserve both allocations before reading.
            if n > self.max_entry as u64 || n > (transient_budget / 2) as u64 {
                return Err(io::Error::other("oversized cache entry"));
            }
            let mut b = vec![0; n as usize];
            file.read_exact(&mut b)?;
            if file.read(&mut [0; 1])? != 0 {
                return Err(io::Error::other("cache entry grew"));
            }
            self.stats.read_bytes += b.len() as u64;
            if b.len() > self.max_entry || b.len() < 16 || &b[..8] != MAGIC {
                return Err(io::Error::other("cache header"));
            }
            let expected = u64::from_le_bytes(b[8..16].try_into().unwrap());
            if checksum(&b[16..]) != expected {
                return Err(io::Error::other("cache checksum"));
            }
            let identity = self.key_bytes(key);
            if !b[16..].starts_with(&identity) {
                return Err(io::Error::other("cache identity"));
            }
            decode(&b[16 + identity.len()..], key).ok_or_else(|| io::Error::other("cache payload"))
        })();
        match result {
            Ok(tile) => {
                self.clock += 1;
                if let Some(v) = self.files.get_mut(&name) {
                    v.1 = self.clock;
                }
                Some(tile)
            }
            Err(_) => {
                self.stats.invalid += 1;
                if self.remove(&name).is_err() {
                    self.stats.errors += 1;
                }
                None
            }
        }
    }
    pub fn store(&mut self, tile: &GeometryTile) -> bool {
        self.removed_names.clear();
        let started = Instant::now();
        let result = self.store_inner(tile);
        self.stats.write_ms += started.elapsed().as_secs_f64() * 1000.;
        if result.is_err() {
            self.stats.errors += 1;
        }
        result.is_ok()
    }
    fn store_inner(&mut self, tile: &GeometryTile) -> io::Result<()> {
        if tile.key.source_revision != self.revision {
            return Err(io::Error::other("snapshot revision"));
        }
        // Each scalar is <=8 encoded bytes; vector element widths are fixed.
        let predicted = encoded_size(tile)
            .and_then(|n| n.checked_add(self.identity.len() + 80 + 16))
            .ok_or_else(|| io::Error::other("size overflow"))?;
        let transient_budget = crate::build_memory::reservation(tile.key)
            .ok_or_else(|| io::Error::other("unsupported cache tile key"))?;
        if predicted > self.max_entry || predicted > transient_budget / 2 {
            return Err(io::Error::other("entry budget"));
        }
        let mut b = Vec::with_capacity(predicted);
        b.extend_from_slice(MAGIC);
        put(&mut b, 0);
        b.extend_from_slice(&self.key_bytes(tile.key));
        encode(tile, &mut b);
        if b.len() > self.max_entry {
            return Err(io::Error::other("entry budget"));
        }
        let sum = checksum(&b[16..]);
        b[8..16].copy_from_slice(&sum.to_le_bytes());
        let name = self.name(tile.key);
        // Remove an old entry before reserving temp space: budget includes temp writes.
        self.remove(&name)?;
        self.make_room(b.len() as u64)?;
        self.serial += 1;
        let temp = self
            .root
            .join(format!("{}-{}.fvtmp", std::process::id(), self.serial));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(&b)?;
            file.sync_all()?;
            fs::rename(&temp, self.root.join(&name))?;
            Ok::<_, io::Error>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result?;
        self.clock += 1;
        self.files.insert(name, (b.len() as u64, self.clock));
        self.stats.resident_bytes += b.len() as u64;
        self.stats.written_bytes += b.len() as u64;
        self.stats.writes += 1;
        Ok(())
    }
}
fn source_identity(w: &World) -> io::Result<Vec<u8>> {
    let mut b = vec![];
    put(&mut b, REPRESENTATION_VERSION);
    put(&mut b, SOURCE_VERSION as u64);
    put(&mut b, w.seed());
    put(&mut b, VOXEL_SIZE.to_bits());
    put(&mut b, w.revision());
    match w.topology_base_cells() {
        Some(v) => {
            put(&mut b, 1);
            put(&mut b, v as u64);
        }
        None => {
            put(&mut b, 0);
            put(&mut b, 0);
        }
    }
    if let Some(t) = w.fbm_terrain() {
        let p = t.params();
        put(&mut b, 1);
        put(&mut b, p.recipe as u64);
        put(&mut b, t.source_version() as u64);
        put(&mut b, t.appearance_version() as u64);
        for v in [p.horizontal_scale, p.amplitude, p.roughness] {
            put(&mut b, v.to_bits());
        }
        put(&mut b, p.octaves as u64);
    } else {
        put(&mut b, 0);
    }
    let mut edits: Vec<_> = w.edit_cells().take(MAX_EDITS + 1).collect();
    if edits.len() > MAX_EDITS {
        return Err(io::Error::other("derived-cache edit identity limit"));
    }
    edits.sort_by_key(|c| (c.x, c.y, c.z));
    put(&mut b, edits.len() as u64);
    for c in edits {
        for v in [c.x, c.y, c.z] {
            put(&mut b, v as u64);
        }
        put(&mut b, w.material(c) as u64);
    }
    Ok(b)
}
fn checksum(b: &[u8]) -> u64 {
    checksum_from(0xcbf29ce484222325, b)
}
fn checksum_from(seed: u64, b: &[u8]) -> u64 {
    b.iter()
        .fold(seed, |h, v| (h ^ *v as u64).wrapping_mul(0x100000001b3))
}

fn put(b: &mut Vec<u8>, v: u64) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn p32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn f(b: &mut Vec<u8>, v: f64) {
    put(b, v.to_bits());
}
fn opt(b: &mut Vec<u8>, v: Option<f64>) {
    put(b, v.is_some() as u64);
    if let Some(v) = v {
        f(b, v);
    }
}
fn encoded_size(t: &GeometryTile) -> Option<usize> {
    [
        (t.mesh.vertices.len(), 36),
        (t.mesh.indices.len(), 4),
        (t.native_quads.len(), 32),
        (t.proxy_triangles.len(), 64),
        (t.native_bins.len(), 20),
        (t.face_profiles.len(), 80),
        (t.face_profile_indices.len(), 4),
    ]
    .into_iter()
    .try_fold(1024usize, |n, (len, size)| {
        n.checked_add(len.checked_mul(size)?)
    })
}
fn encode(t: &GeometryTile, b: &mut Vec<u8>) {
    for v in [t.mesh.origin.x, t.mesh.origin.y, t.mesh.origin.z] {
        put(b, v as u64);
    }
    put(b, t.mesh.exposed_native_faces as u64);
    put(b, t.sampled_columns as u64);
    put(b, t.exact as u64);
    for bounds in [t.bounds, t.conservative_surface_bounds] {
        for p in bounds {
            for v in p {
                f(b, v);
            }
        }
    }
    put(b, t.guaranteed_solid_box.is_some() as u64);
    if let Some(a) = t.guaranteed_solid_box {
        for p in [a.min, a.max] {
            for v in p {
                f(b, v);
            }
        }
    }
    f(b, t.empirical_error_m);
    for v in [
        t.certified_error_m,
        t.best_certified_vertical_error_m,
        t.global_error_m,
        t.affine_error_m,
    ] {
        opt(b, v);
    }
    for range in [t.affine_height_range_m, t.exact_height_range_m] {
        put(b, range.is_some() as u64);
        if let Some(r) = range {
            for v in r {
                f(b, v);
            }
        }
    }
    for v in [
        t.affine_stats.quad_bounds,
        t.affine_stats.unavailable_quads,
        t.affine_stats.noise_cells,
        t.affine_stats.full_range_fallbacks,
    ] {
        put(b, v);
    }
    f(b, t.affine_stats.cpu_ms);
    for v in t.appearance.area_m2 {
        f(b, v);
    }
    for p in t.appearance.color_area {
        for v in p {
            f(b, v);
        }
    }
    put(b, t.appearance.exhaustive as u64);
    put(b, t.mesh.vertices.len() as u64);
    for v in &t.mesh.vertices {
        for a in [v.position, v.color, v.normal] {
            for v in a {
                p32(b, v.to_bits());
            }
        }
    }
    put(b, t.mesh.indices.len() as u64);
    for v in &t.mesh.indices {
        p32(b, *v);
    }
    put(b, t.native_quads.len() as u64);
    for q in &t.native_quads {
        for v in q.anchor.into_iter().chain(q.extent) {
            p32(b, v.to_bits());
        }
        for v in [q.axis_sign, q.material, q.profile_index] {
            p32(b, v);
        }
    }
    put(b, t.native_bins.len() as u64);
    for q in &t.native_bins {
        for v in [q.axis_sign, q.first_quad, q.quad_count] {
            p32(b, v);
        }
        for v in [q.min_plane, q.max_plane] {
            p32(b, v.to_bits());
        }
    }
    put(b, t.proxy_triangles.len() as u64);
    for triangle in &t.proxy_triangles {
        for position in triangle.positions {
            for v in position {
                p32(b, v.to_bits());
            }
        }
        for v in triangle.normal.into_iter().chain(triangle.color) {
            p32(b, v.to_bits());
        }
        p32(b, triangle.profile_index);
    }
    put(b, t.face_profiles.len() as u64);
    for p in &t.face_profiles {
        for a in p {
            for v in a {
                p32(b, v.to_bits());
            }
        }
    }
    put(b, t.face_profile_indices.len() as u64);
    for v in &t.face_profile_indices {
        p32(b, *v);
    }
}
struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn u(&mut self) -> Option<u64> {
        let v = u64::from_le_bytes(self.0.get(..8)?.try_into().ok()?);
        self.0 = &self.0[8..];
        Some(v)
    }
    fn n(&mut self) -> Option<usize> {
        usize::try_from(self.u()?).ok()
    }
    fn u32(&mut self) -> Option<u32> {
        let v = u32::from_le_bytes(self.0.get(..4)?.try_into().ok()?);
        self.0 = &self.0[4..];
        Some(v)
    }
    fn f(&mut self) -> Option<f64> {
        let v = f64::from_bits(self.u()?);
        v.is_finite().then_some(v)
    }
    fn f32(&mut self) -> Option<f32> {
        let v = f32::from_bits(self.u32()?);
        v.is_finite().then_some(v)
    }
    fn flag(&mut self) -> Option<bool> {
        match self.u()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
    fn opt(&mut self) -> Option<Option<f64>> {
        if self.flag()? {
            Some(Some(self.f()?))
        } else {
            Some(None)
        }
    }
    fn arr<const N: usize>(&mut self) -> Option<[f64; N]> {
        let mut a = [0.; N];
        for v in &mut a {
            *v = self.f()?;
        }
        Some(a)
    }
    fn a32<const N: usize>(&mut self) -> Option<[f32; N]> {
        let mut a = [0.; N];
        for v in &mut a {
            *v = self.f32()?;
        }
        Some(a)
    }
    fn vec<T>(
        &mut self,
        width: usize,
        mut read: impl FnMut(&mut Self) -> Option<T>,
    ) -> Option<Vec<T>> {
        let n = self.n()?;
        if n > self.0.len() / width {
            return None;
        }
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(read(self)?);
        }
        Some(v)
    }
}
fn decode(b: &[u8], key: TileKey) -> Option<GeometryTile> {
    let mut r = Reader(b);
    let origin = Cell {
        x: r.u()? as i64,
        y: r.u()? as i64,
        z: r.u()? as i64,
    };
    let exposed_native_faces = r.n()?;
    let sampled_columns = r.n()?;
    let exact = r.flag()?;
    let bounds = [r.arr()?, r.arr()?];
    let conservative_surface_bounds = [r.arr()?, r.arr()?];
    let guaranteed_solid_box = if r.flag()? {
        Some(Aabb {
            min: r.arr()?,
            max: r.arr()?,
        })
    } else {
        None
    };
    let empirical_error_m = r.f()?;
    let certified_error_m = r.opt()?;
    let best_certified_vertical_error_m = r.opt()?;
    let global_error_m = r.opt()?;
    let affine_error_m = r.opt()?;
    let affine_height_range_m = if r.flag()? { Some(r.arr()?) } else { None };
    let exact_height_range_m = if r.flag()? { Some(r.arr()?) } else { None };
    let affine_stats = AffineStats {
        quad_bounds: r.u()?,
        unavailable_quads: r.u()?,
        noise_cells: r.u()?,
        full_range_fallbacks: r.u()?,
        cpu_ms: r.f()?,
    };
    let area_m2 = r.arr()?;
    let mut color_area = [[0.; 3]; 5];
    for a in &mut color_area {
        *a = r.arr()?;
    }
    let appearance = FaceAppearance {
        area_m2,
        color_area,
        exhaustive: r.flag()?,
    };
    let vertices = r.vec(36, |r| {
        Some(Vertex {
            position: r.a32()?,
            color: r.a32()?,
            normal: r.a32()?,
        })
    })?;
    let indices = r.vec(4, |r| r.u32())?;
    let native_quads = r.vec(32, |r| {
        Some(NativeQuad {
            anchor: r.a32()?,
            extent: r.a32()?,
            axis_sign: r.u32()?,
            material: r.u32()?,
            profile_index: r.u32()?,
        })
    })?;
    let native_bins = r.vec(20, |r| {
        Some(NativeBin {
            axis_sign: r.u32()?,
            first_quad: r.u32()?,
            quad_count: r.u32()?,
            min_plane: r.f32()?,
            max_plane: r.f32()?,
        })
    })?;
    let proxy_triangles = r.vec(64, |r| {
        Some(ProxyTriangle {
            positions: [r.a32()?, r.a32()?, r.a32()?],
            normal: r.a32()?,
            color: r.a32()?,
            profile_index: r.u32()?,
        })
    })?;
    let face_profiles = r.vec(80, |r| {
        let mut p = [[0.; 4]; 5];
        for a in &mut p {
            *a = r.a32()?;
        }
        Some(p)
    })?;
    let face_profile_indices = r.vec(4, |r| r.u32())?;
    let bytes = vertices.capacity() * std::mem::size_of::<Vertex>()
        + indices.capacity() * 4
        + native_quads.capacity() * std::mem::size_of::<NativeQuad>()
        + native_bins.capacity() * std::mem::size_of::<NativeBin>()
        + proxy_triangles.capacity() * std::mem::size_of::<ProxyTriangle>()
        + face_profiles.capacity() * 80
        + face_profile_indices.capacity() * 4;
    let t = GeometryTile {
        key,
        mesh: Mesh {
            vertices,
            indices,
            origin,
            exposed_native_faces,
        },
        native_quads,
        proxy_triangles,
        native_bins,
        bounds,
        conservative_surface_bounds,
        guaranteed_solid_box,
        sampled_columns,
        empirical_error_m,
        certified_error_m,
        best_certified_vertical_error_m,
        global_error_m,
        affine_error_m,
        affine_height_range_m,
        exact_height_range_m,
        affine_stats,
        appearance,
        face_profiles,
        face_profile_indices,
        exact,
        bytes,
    };
    if !r.0.is_empty() || !validate(&t) {
        return None;
    }
    Some(t)
}
fn validate(t: &GeometryTile) -> bool {
    let bbox = |b: [[f64; 3]; 2]| (0..3).all(|a| b[0][a] <= b[1][a]);
    bbox(t.bounds)
        && (t.proxy_triangles.is_empty()
            || (!t.exact
                && t.native_quads.is_empty()
                && t.mesh.vertices.is_empty()
                && t.mesh.indices.is_empty()
                && t.face_profile_indices.is_empty()))
        && t.proxy_triangles
            .iter()
            .all(|p| (p.profile_index as usize) < t.face_profiles.len())
        && bbox(t.conservative_surface_bounds)
        && t.guaranteed_solid_box.is_none_or(|b| b.valid())
        && [
            t.certified_error_m,
            t.best_certified_vertical_error_m,
            t.global_error_m,
            t.affine_error_m,
        ]
        .into_iter()
        .flatten()
        .all(|v| v >= 0.)
        && [t.affine_height_range_m, t.exact_height_range_m]
            .into_iter()
            .flatten()
            .all(|r| r[0] <= r[1])
        && t.appearance.area_m2.into_iter().all(|v| v >= 0.)
        && t.affine_stats.cpu_ms >= 0.
        && valid_bins(t)
        && t.empirical_error_m >= 0.
        && t.mesh.indices.len() % 3 == 0
        && t.mesh
            .indices
            .iter()
            .all(|v| (*v as usize) < t.mesh.vertices.len())
        && t.face_profile_indices.len() == t.mesh.vertices.len()
        && t.face_profile_indices
            .iter()
            .all(|v| (*v as usize) < t.face_profiles.len())
        && t.native_quads.iter().all(|q| {
            q.axis_sign & !7 == 0
                && q.axis_sign & 3 < 3
                && q.material <= 4
                && (q.profile_index as usize) < t.face_profiles.len()
                && q.extent.iter().all(|v| *v >= 0.)
        })
        && t.native_bins.iter().all(|b| {
            b.axis_sign & !7 == 0
                && b.axis_sign & 3 < 3
                && b.min_plane <= b.max_plane
                && (b.first_quad as usize)
                    .checked_add(b.quad_count as usize)
                    .is_some_and(|end| end <= t.native_quads.len())
        })
}

fn valid_bins(t: &GeometryTile) -> bool {
    if t.native_quads.is_empty() {
        return t.native_bins.is_empty();
    }
    let mut next = 0usize;
    for b in &t.native_bins {
        if b.first_quad as usize != next || b.quad_count == 0 || b.axis_sign & 3 > 2 {
            return false;
        }
        let Some(end) = next
            .checked_add(b.quad_count as usize)
            .filter(|end| *end <= t.native_quads.len())
        else {
            return false;
        };
        for q in &t.native_quads[next..end] {
            let plane = q.anchor[(b.axis_sign & 3) as usize];
            if q.axis_sign != b.axis_sign || plane < b.min_plane || plane > b.max_plane {
                return false;
            }
        }
        next = end;
    }
    next == t.native_quads.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adaptive_geometry::build;
    use frontier_world::{FbmParams, FbmRecipe, Material};
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "fvdc-test-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn world() -> World {
        World::fbm(
            7,
            FbmParams {
                amplitude: 16.,
                octaves: 2,
                ..Default::default()
            },
        )
        .unwrap()
    }
    fn key(step: u32) -> TileKey {
        TileKey {
            min: [-16, -16],
            width_cells: 32,
            step_cells: step,
            edge_steps: [step; 4],
            source_revision: 0,
        }
    }
    fn cache(p: &Temp, w: &World) -> DiskCache {
        DiskCache::new(p.0.clone(), 4 * 1024 * 1024, 1024 * 1024, w).unwrap()
    }
    fn bytes(t: &GeometryTile) -> Vec<u8> {
        let mut b = vec![];
        encode(t, &mut b);
        b
    }
    #[test]
    fn keyer_names_preserve_format_and_report_index_deltas() {
        let w = world();
        let dir = Temp::new();
        let mut c = cache(&dir, &w);
        let keyer = c.keyer();
        let t = build(&w, key(4), || false).unwrap();
        assert_eq!(
            keyer.name(t.key),
            format!("{:016x}.fvdc", checksum(&c.key_bytes(t.key)))
        );
        assert!(c.store(&t));
        let name = keyer.name(t.key);
        assert!(c.contains_name(&name));
        assert!(c.index_names().contains(&name));
        assert!(c.take_removed_names().is_empty());
        assert!(c.store(&t));
        assert_eq!(c.take_removed_names(), vec![name.clone()]);
        fs::write(dir.0.join(&name), b"corrupt").unwrap();
        assert!(c.load(t.key).is_none());
        assert_eq!(c.take_removed_names(), vec![name]);
        assert_eq!(c.max_entry_bytes(), 1024 * 1024);
    }
    #[test]
    fn packed_proxy_restart_preserves_every_instance_field_and_rejects_bad_profiles() {
        let w = World::fbm(7, frontier_world::FbmParams::default()).unwrap();
        let dir = Temp::new();
        let k = TileKey {
            min: [0, -512],
            width_cells: 128,
            step_cells: 8,
            edge_steps: [8; 4],
            source_revision: 0,
        };
        let mut t = build(&w, k, || false).unwrap();
        crate::adaptive_geometry::pack_proxy_triangles(&mut t).unwrap();
        assert!(!t.proxy_triangles.is_empty());
        let mut c = cache(&dir, &w);
        assert!(c.store(&t));
        drop(c);
        let mut c = cache(&dir, &w);
        let restored = c.load(k).unwrap();
        assert_eq!(bytes(&restored), bytes(&t));
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&restored.proxy_triangles),
            bytemuck::cast_slice::<_, u8>(&t.proxy_triangles)
        );
        t.proxy_triangles[0].profile_index = u32::MAX;
        assert!(decode(&bytes(&t), k).is_none());
        t.proxy_triangles[0].profile_index = 0;
        t.proxy_triangles[0].positions[0][0] = f32::NAN;
        assert!(decode(&bytes(&t), k).is_none());
    }
    #[test]
    fn cold_warm_restart_exact_and_proxy_parity() {
        let w = world();
        let dir = Temp::new();
        for step in [1, 4] {
            let t = build(&w, key(step), || false).unwrap();
            assert!(validate(&t));
            let mut cold = cache(&dir, &w);
            assert!(cold.load(t.key).is_none());
            assert!(cold.store(&t));
            drop(cold);
            let mut warm = cache(&dir, &w);
            let restored = warm.load(t.key).unwrap();
            assert_eq!(bytes(&t), bytes(&restored));
            assert_eq!(restored.triangle_count(), t.triangle_count());
            assert_eq!(warm.stats().hits, 1);
        }
    }
    #[test]
    fn identity_covers_recipe_seed_params_topology_and_edit_content() {
        let w = world();
        let original = source_identity(&w).unwrap();
        assert_ne!(
            original,
            source_identity(&World::fbm(8, w.fbm_terrain().unwrap().params()).unwrap()).unwrap()
        );
        for p in [
            FbmParams {
                recipe: FbmRecipe::Regional,
                ..w.fbm_terrain().unwrap().params()
            },
            FbmParams {
                amplitude: 17.,
                ..w.fbm_terrain().unwrap().params()
            },
        ] {
            assert_ne!(
                original,
                source_identity(&World::fbm(7, p).unwrap()).unwrap()
            );
        }
        assert_ne!(
            original,
            source_identity(&World::fbm_topology(7, w.fbm_terrain().unwrap().params()).unwrap())
                .unwrap()
        );
        let mut a = w.clone();
        let mut b = w.clone();
        let c = Cell { x: 0, y: 500, z: 0 };
        a.set(c, Material::Stone);
        b.set(c, Material::Wood);
        let mut ai = source_identity(&a).unwrap();
        let mut bi = source_identity(&b).unwrap();
        // Revision numbers are globally assigned; prove edit content itself
        // participates even if two persisted snapshots share a revision.
        ai[32..40].fill(0);
        bi[32..40].fill(0);
        assert_ne!(ai, bi);
        let dir = Temp::new();
        let t = build(&w, key(4), || false).unwrap();
        assert!(cache(&dir, &w).store(&t));
        assert!(cache(&dir, &a).load(t.key).is_none());
        assert!(cache(
            &dir,
            &World::fbm(8, w.fbm_terrain().unwrap().params()).unwrap()
        )
        .load(t.key)
        .is_none());
    }
    #[test]
    fn corruption_oversize_truncation_schema_are_misses_and_regenerate() {
        let w = world();
        let dir = Temp::new();
        let t = build(&w, key(4), || false).unwrap();
        let mut c = cache(&dir, &w);
        for kind in 0..4 {
            assert!(c.store(&t));
            let path = dir.0.join(c.name(t.key));
            let mut b = fs::read(&path).unwrap();
            match kind {
                0 => {
                    let n = b.len();
                    b[n - 1] ^= 128;
                }
                1 => b.resize(c.max_entry + 1, 0),
                2 => b.truncate(17),
                _ => b[7] = b'1',
            }
            fs::write(&path, b).unwrap();
            assert!(c.load(t.key).is_none());
            assert!(!path.exists());
            assert_eq!(bytes(&build(&w, t.key, || false).unwrap()), bytes(&t));
        }
        assert_eq!(c.stats().invalid, 4);
    }
    #[test]
    fn malformed_lengths_and_indices_are_rejected_without_allocation_explosion() {
        let w = world();
        let mut t = build(&w, key(4), || false).unwrap();
        t.mesh.indices[0] = u32::MAX;
        assert!(decode(&bytes(&t), t.key).is_none());
        assert!(decode(&[255; 4096], t.key).is_none());
        let mut r = Reader(&[255; 8]);
        assert!(r.vec(4, |r| r.u32()).is_none());
    }
    #[test]
    fn invalid_floats_ranges_and_profile_addresses_fail_validation() {
        let w = world();
        let mut t = build(&w, key(4), || false).unwrap();
        let saved = t.bounds;
        t.bounds[0][0] = f64::NAN;
        assert!(decode(&bytes(&t), t.key).is_none());
        t.bounds = saved;
        t.bounds[0][0] = t.bounds[1][0] + 1.;
        assert!(decode(&bytes(&t), t.key).is_none());
        t.bounds = saved;
        let cert = t.certified_error_m;
        t.certified_error_m = Some(-1.);
        assert!(decode(&bytes(&t), t.key).is_none());
        t.certified_error_m = cert;
        let range = t.affine_height_range_m;
        t.affine_height_range_m = Some([1., -1.]);
        assert!(decode(&bytes(&t), t.key).is_none());
        t.affine_height_range_m = range;
        t.face_profile_indices[0] = u32::MAX;
        assert!(decode(&bytes(&t), t.key).is_none());
        let mut t = build(&w, key(1), || false).unwrap();
        t.native_quads[0].profile_index = u32::MAX;
        assert!(decode(&bytes(&t), t.key).is_none());
        t.native_quads[0].profile_index = 0;
        t.native_bins[0].min_plane = f32::INFINITY;
        assert!(decode(&bytes(&t), t.key).is_none());
    }
    #[test]
    fn key_reservation_rejects_large_record_before_reading() {
        let w = world();
        let dir = Temp::new();
        let mut k = key(4);
        k.width_cells = 4;
        let t = build(&w, k, || false).unwrap();
        let mut c = DiskCache::new(dir.0.clone(), 64 * 1024 * 1024, 32 * 1024 * 1024, &w).unwrap();
        assert!(c.store(&t));
        let path = dir.0.join(c.name(k));
        let oversized = (crate::build_memory::reservation(k).unwrap() / 2 + 1) as u64;
        assert!(oversized < c.max_entry as u64);
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(oversized)
            .unwrap();
        assert!(c.load(k).is_none());
        assert_eq!(c.stats().read_bytes, 0);
        assert_eq!(c.stats().invalid, 1);
        assert!(!path.exists());
    }
    #[test]
    fn bounded_disk_lru_startup_and_temp_cleanup() {
        let w = world();
        let dir = Temp::new();
        let t = build(&w, key(4), || false).unwrap();
        let mut c = cache(&dir, &w);
        assert!(c.store(&t));
        let size = c.stats().resident_bytes;
        drop(c);
        fs::write(dir.0.join("interrupted.fvtmp"), b"partial").unwrap();
        let mut c = DiskCache::new(dir.0.clone(), size + 2048, size as usize + 2048, &w).unwrap();
        assert!(!dir.0.join("interrupted.fvtmp").exists());
        let mut t = t;
        for x in [32, 64, 96] {
            t.key.min[0] = x;
            assert!(c.store(&t));
            assert!(c.stats().resident_bytes <= c.budget);
        }
        assert!(c.stats().evictions > 0);
        assert!(c.load(key(4)).is_none());
        assert!(DiskCache::new(dir.0.clone(), 100, 100, &w).is_err());
    }
    #[test]
    fn exclusive_directory_lock_released_on_drop() {
        let dir = Temp::new();
        let w = world();
        let c = cache(&dir, &w);
        assert!(DiskCache::new(dir.0.clone(), 4 * 1024 * 1024, 1024 * 1024, &w).is_err());
        drop(c);
        assert!(DiskCache::new(dir.0.clone(), 4 * 1024 * 1024, 1024 * 1024, &w).is_ok());
    }
    #[test]
    fn disk_cache_cpu_measurement() {
        let w = World::fbm(7, FbmParams::default()).unwrap();
        let dir = Temp::new();
        let mut c = cache(&dir, &w);
        for step in [1, 8] {
            let k = TileKey {
                min: [-128, -512],
                width_cells: 128,
                step_cells: step,
                edge_steps: [step; 4],
                source_revision: 0,
            };
            let start = Instant::now();
            let t = build(&w, k, || false).unwrap();
            let build_ms = start.elapsed().as_secs_f64() * 1000.;
            assert!(c.store(&t));
            let write_ms = c.stats().write_ms;
            let entry_bytes = fs::metadata(dir.0.join(c.name(k))).unwrap().len();
            drop(c);
            let mut restart = cache(&dir, &w);
            let restored = restart.load(k).unwrap();
            assert_eq!(bytes(&restored), bytes(&t));
            println!("diskcache step={step} build_ms={build_ms:.3} write_ms={:.3} warm_read_ms={:.3} entry_bytes={} decoded_bytes={}",write_ms,restart.stats().read_ms,entry_bytes,restored.bytes);
            drop(restart);
            c = cache(&dir, &w);
        }
    }
}
