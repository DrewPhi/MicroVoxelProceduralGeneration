//! Versioned, atomic edit saves. Procedural terrain and render caches are not saved.
use crate::{
    generation::Generator,
    grid::{ChunkPos, CHUNK_VOLUME},
    materials::{Appearance, MaterialId, Voxel, MATERIAL_SCHEMA},
    storage::{Brick, EditStore},
    world::World,
    Result,
};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
const MAGIC: &[u8; 8] = b"MVWORLD1";
static TEMP_ID: AtomicU64 = AtomicU64::new(0);
fn u32read(r: &mut impl Read) -> std::io::Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn u64read(r: &mut impl Read) -> std::io::Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}
fn u16read(r: &mut impl Read) -> std::io::Result<u16> {
    let mut b = [0; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}
fn voxelread(r: &mut impl Read) -> std::io::Result<Voxel> {
    Ok(Voxel::new(MaterialId(u16read(r)?), Appearance(u16read(r)?)))
}
fn voxelwrite(w: &mut impl Write, v: Voxel) -> std::io::Result<()> {
    w.write_all(&v.material.0.to_le_bytes())?;
    w.write_all(&v.appearance.0.to_le_bytes())
}
impl<G: Generator> World<G> {
    pub fn save_edits(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let temp = path.with_extension(format!(
            "tmp.{}.{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| -> Result<()> {
            let mut w = std::io::BufWriter::new(
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temp)?,
            );
            w.write_all(MAGIC)?;
            for value in [1u32, 16, 32, MATERIAL_SCHEMA, self.identity().version] {
                w.write_all(&value.to_le_bytes())?;
            }
            for value in [
                self.identity().seed,
                self.identity().definition,
                self.materials().fingerprint(),
                self.revision(),
                self.edits.bricks.len() as u64,
            ] {
                w.write_all(&value.to_le_bytes())?;
            }
            for (key, brick) in &self.edits.bricks {
                for n in [key.x, key.y, key.z] {
                    w.write_all(&n.to_le_bytes())?;
                }
                match brick {
                    Brick::Uniform(v) => {
                        w.write_all(&[1])?;
                        voxelwrite(&mut w, *v)?;
                    }
                    _ => {
                        w.write_all(&[0])?;
                        w.write_all(&(brick.len() as u32).to_le_bytes())?;
                        for (index, v) in brick.entries() {
                            w.write_all(&index.to_le_bytes())?;
                            voxelwrite(&mut w, v)?;
                        }
                    }
                }
            }
            w.flush()?;
            w.get_ref().sync_all()?;
            drop(w);
            fs::rename(&temp, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
    pub fn load_edits(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let mut r = std::io::BufReader::new(File::open(path)?);
        let mut magic = [0; 8];
        r.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err("unrecognized world save".into());
        }
        let header = [
            u32read(&mut r)?,
            u32read(&mut r)?,
            u32read(&mut r)?,
            u32read(&mut r)?,
            u32read(&mut r)?,
        ];
        if header != [1, 16, 32, MATERIAL_SCHEMA, self.identity().version] {
            return Err("incompatible format, grid, materials, or generator version".into());
        }
        if u64read(&mut r)? != self.identity().seed
            || u64read(&mut r)? != self.identity().definition
            || u64read(&mut r)? != self.materials().fingerprint()
        {
            return Err("save belongs to a different procedural/material baseline".into());
        }
        let revision = u64read(&mut r)?;
        let count = u64read(&mut r)?;
        if count > 1_000_000 {
            return Err("world save exceeds brick limit".into());
        }
        let mut store = EditStore::default();
        let mut keys = BTreeSet::new();
        let mut records = 0usize;
        for _ in 0..count {
            let key = ChunkPos {
                x: u64read(&mut r)? as i64,
                y: u64read(&mut r)? as i64,
                z: u64read(&mut r)? as i64,
            };
            if !keys.insert(key) {
                return Err("duplicate brick".into());
            }
            let mut tag = [0];
            r.read_exact(&mut tag)?;
            let entries = match tag[0] {
                0 => {
                    let count = u32read(&mut r)? as usize;
                    if count == 0 || count > CHUNK_VOLUME {
                        return Err("invalid brick record count".into());
                    }
                    let mut entries = Vec::with_capacity(count);
                    let mut seen = BTreeSet::new();
                    for _ in 0..count {
                        let i = u16read(&mut r)?;
                        if i as usize >= CHUNK_VOLUME || !seen.insert(i) {
                            return Err("invalid or duplicate cell index".into());
                        }
                        entries.push((i, voxelread(&mut r)?));
                    }
                    entries
                }
                1 => {
                    let v = voxelread(&mut r)?;
                    (0..CHUNK_VOLUME).map(|i| (i as u16, v)).collect()
                }
                _ => return Err("unknown brick encoding".into()),
            };
            records += entries.len();
            if records > 16_000_000 {
                return Err("world save exceeds cell limit".into());
            }
            for (index, value) in entries {
                let p = key.cell(index).ok_or("coordinate overflow")?;
                if self.materials().get(value.material).is_none() || self.baseline_at(p).is_none() {
                    return Err("unknown material or unprovided coordinate in save".into());
                }
                if self.baseline_at(p) != Some(value) {
                    store.set(p, Some(value));
                }
            }
        }
        let mut trailing = [0];
        if r.read(&mut trailing)? != 0 {
            return Err("trailing data in save".into());
        }
        store.compact(&keys);
        self.replace_edits(store, revision);
        Ok(())
    }
}
