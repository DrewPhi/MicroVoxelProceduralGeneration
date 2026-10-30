//! Checksummed, versioned write-ahead edit journal. A torn final frame is ignored.
use crate::{
    generation::Generator,
    grid::CellPos,
    materials::{Appearance, MaterialId, Voxel},
    world::{Edit, EditAction, World},
    Result,
};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};
const MAGIC: &[u8; 8] = b"MVJOURN1";
const VERSION: u32 = 1;
const MAX_FRAME: usize = 65536 * 34 + 12;
fn hash(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in bytes {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}
fn read_u32(b: &[u8], at: &mut usize) -> Option<u32> {
    let v = u32::from_le_bytes(b.get(*at..*at + 4)?.try_into().ok()?);
    *at += 4;
    Some(v)
}
fn read_u64(b: &[u8], at: &mut usize) -> Option<u64> {
    let v = u64::from_le_bytes(b.get(*at..*at + 8)?.try_into().ok()?);
    *at += 8;
    Some(v)
}
fn read_i64(b: &[u8], at: &mut usize) -> Option<i64> {
    Some(read_u64(b, at)? as i64)
}
fn read_u16(b: &[u8], at: &mut usize) -> Option<u16> {
    let v = u16::from_le_bytes(b.get(*at..*at + 2)?.try_into().ok()?);
    *at += 2;
    Some(v)
}
fn header<G: Generator>(world: &World<G>) -> Vec<u8> {
    let mut b = MAGIC.to_vec();
    b.extend(VERSION.to_le_bytes());
    let id = world.identity();
    for v in [
        id.seed,
        id.definition,
        id.version as u64,
        world.spec().fingerprint(),
        world.materials().fingerprint(),
    ] {
        b.extend(v.to_le_bytes());
    }
    let c = hash(&b);
    b.extend(c.to_le_bytes());
    b
}
fn encode(revision: u64, actions: &[EditAction]) -> Vec<u8> {
    let mut b = Vec::with_capacity(12 + actions.len() * 34);
    b.extend(revision.to_le_bytes());
    b.extend((actions.len() as u32).to_le_bytes());
    for action in actions {
        let (tag, pos, voxel, expected) = match *action {
            EditAction::Set(e) => (0, e.pos, Some(e.value), e.expected),
            EditAction::Reset { pos, expected } => (1, pos, None, expected),
        };
        b.push(tag);
        for n in [pos.x, pos.y, pos.z] {
            b.extend(n.to_le_bytes());
        }
        if let Some(v) = voxel {
            b.extend(v.material.0.to_le_bytes());
            b.extend(v.appearance.0.to_le_bytes());
        }
        b.push(u8::from(expected.is_some()));
        if let Some(v) = expected {
            b.extend(v.material.0.to_le_bytes());
            b.extend(v.appearance.0.to_le_bytes());
        }
    }
    b
}
fn decode(b: &[u8]) -> Result<(u64, Vec<EditAction>)> {
    let mut at = 0;
    let rev = read_u64(b, &mut at).ok_or("short journal frame")?;
    let count = read_u32(b, &mut at).ok_or("short journal frame")? as usize;
    if count > 65536 {
        return Err("journal transaction too large".into());
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let tag = *b.get(at).ok_or("short journal action")?;
        at += 1;
        let pos = CellPos::new(
            read_i64(b, &mut at).ok_or("short x")?,
            read_i64(b, &mut at).ok_or("short y")?,
            read_i64(b, &mut at).ok_or("short z")?,
        );
        let value = if tag == 0 {
            Some(Voxel::new(
                MaterialId(read_u16(b, &mut at).ok_or("short material")?),
                Appearance(read_u16(b, &mut at).ok_or("short appearance")?),
            ))
        } else {
            None
        };
        let present = *b.get(at).ok_or("short expected flag")?;
        at += 1;
        if present > 1 {
            return Err("invalid expected flag".into());
        }
        let expected = if present == 1 {
            Some(Voxel::new(
                MaterialId(read_u16(b, &mut at).ok_or("short expected material")?),
                Appearance(read_u16(b, &mut at).ok_or("short expected appearance")?),
            ))
        } else {
            None
        };
        out.push(match (tag, value) {
            (0, Some(value)) => EditAction::Set(Edit {
                pos,
                value,
                expected,
            }),
            (1, None) => EditAction::Reset { pos, expected },
            _ => return Err("invalid journal action".into()),
        });
    }
    if at != b.len() {
        return Err("trailing journal action data".into());
    }
    Ok((rev, out))
}
fn open_checked<G: Generator>(world: &World<G>, path: &Path) -> Result<File> {
    let expected = header(world);
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)?;
    if f.metadata()?.len() == 0 {
        f.write_all(&expected)?;
        f.sync_all()?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            File::open(parent)?.sync_all()?;
        }
    } else {
        let mut got = vec![0; expected.len()];
        f.read_exact(&mut got)?;
        if got != expected {
            return Err("journal belongs to a different source, grid, or material schema".into());
        }
    }
    Ok(f)
}
/// Scan valid frames. Invalid or incomplete final bytes are a torn tail; callers
/// truncate them before appending. A complete valid frame with wrong sequence fails.
fn frames(f: &mut File, start: u64) -> Result<(Vec<(u64, Vec<EditAction>)>, u64)> {
    f.seek(SeekFrom::Start(start))?;
    let mut good = start;
    let mut out = Vec::new();
    loop {
        let mut len_buf = [0; 4];
        if f.read_exact(&mut len_buf).is_err() {
            break;
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len < 12 || len > MAX_FRAME {
            break;
        }
        let mut body = vec![0; len];
        if f.read_exact(&mut body).is_err() {
            break;
        }
        let mut check = [0; 8];
        if f.read_exact(&mut check).is_err() {
            break;
        }
        if u64::from_le_bytes(check) != hash(&body) {
            break;
        }
        let decoded = decode(&body)?;
        good = f.stream_position()?;
        out.push(decoded);
    }
    Ok((out, good))
}
impl<G: Generator> World<G> {
    /// Durably records a validated transaction before publishing it in memory.
    pub fn apply_journaled(
        &mut self,
        path: impl AsRef<Path>,
        actions: &[EditAction],
    ) -> Result<crate::world::ChangeSet> {
        if actions.is_empty() {
            return Ok(self.apply_actions(actions)?);
        }
        // Preflight on an isolated copy of override metadata; the shared bricks
        // make this proportional to edited metadata rather than world volume.
        let mut probe = self.validation_copy();
        let preview = probe.apply_actions(actions)?;
        if preview.changed_cells.is_empty() {
            return Ok(self.apply_actions(actions)?);
        }
        let mut file = open_checked(self, path.as_ref())?;
        let (old, good) = frames(&mut file, header(self).len() as u64)?;
        let mut last = 0;
        for (rev, _) in old {
            if rev <= last {
                return Err("journal revisions are not increasing".into());
            }
            last = rev;
        }
        if last > self.revision() {
            return Err("journal has unrecovered transactions".into());
        }
        file.set_len(good)?;
        file.seek(SeekFrom::Start(good))?;
        let body = encode(preview.revision, actions);
        file.write_all(&(body.len() as u32).to_le_bytes())?;
        file.write_all(&body)?;
        file.write_all(&hash(&body).to_le_bytes())?;
        file.sync_all()?;
        Ok(self.apply_actions(actions)?)
    }
    /// Replays only revisions newer than the currently loaded save/snapshot.
    /// Returns the number of committed transactions recovered.
    pub fn recover_journal(&mut self, path: impl AsRef<Path>) -> Result<usize> {
        let mut file = open_checked(self, path.as_ref())?;
        let (frames, good) = frames(&mut file, header(self).len() as u64)?;
        let mut previous = 0;
        let mut recovered = 0;
        for (rev, actions) in frames {
            if rev <= previous {
                return Err("journal revisions are not increasing".into());
            }
            previous = rev;
            if rev <= self.revision() {
                continue;
            }
            if rev != self.revision().checked_add(1).ok_or("revision overflow")? {
                return Err("journal revision gap".into());
            }
            let changes = self.apply_actions(&actions)?;
            if changes.revision != rev {
                return Err("journal transaction does not match revision".into());
            }
            recovered += 1;
        }
        file.set_len(good)?;
        Ok(recovered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{generation::GeneratorIdentity, grid::Bounds, snapshot::QueryError, world::Lookup};
    struct Flat;
    impl Generator for Flat {
        fn identity(&self) -> GeneratorIdentity {
            GeneratorIdentity {
                seed: 17,
                version: 1,
                definition: 3,
            }
        }
        fn voxel(&self, p: CellPos) -> Option<Voxel> {
            (p.x.abs() < 100 && p.y.abs() < 100 && p.z.abs() < 100)
                .then_some(Voxel::new(MaterialId::ROCK, Appearance(42)))
        }
    }
    fn path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "voxel-journal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    #[test]
    fn snapshot_isolation_unknown_bounds_and_reset_recovery() {
        let mut world = World::new(Flat);
        let p = CellPos::new(-1, 2, -1);
        let before = world.read_snapshot();
        let path = path();
        let air = EditAction::Set(Edit::set(p, Voxel::AIR));
        world.apply_journaled(&path, &[air]).unwrap();
        assert_eq!(
            before.voxel_at(p),
            Lookup::Known(Voxel::new(MaterialId::ROCK, Appearance(42)))
        );
        assert_eq!(world.read_snapshot().voxel_at(p), Lookup::Known(Voxel::AIR));
        let bounds = Bounds {
            min: CellPos::new(98, 0, 0),
            max: CellPos::new(102, 1, 1),
        };
        assert_eq!(
            world
                .fill_voxels(bounds)
                .unwrap()
                .iter()
                .filter(|v| **v == Lookup::Unknown)
                .count(),
            2
        );
        let huge = Bounds {
            min: CellPos::new(0, 0, 0),
            max: CellPos::new(33, 33, 33),
        };
        assert_eq!(world.fill_voxels(huge), Err(QueryError::TooLarge));
        let reset = EditAction::Reset {
            pos: p,
            expected: Some(Voxel::AIR),
        };
        world.apply_journaled(&path, &[reset]).unwrap();
        assert_eq!(world.storage().override_count(), 0);
        let mut recovered = World::new(Flat);
        assert_eq!(recovered.recover_journal(&path).unwrap(), 2);
        assert_eq!(
            recovered.voxel_at(p),
            Lookup::Known(Voxel::new(MaterialId::ROCK, Appearance(42)))
        );
        assert_eq!(recovered.storage().override_count(), 0);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn torn_tail_is_discarded_before_next_commit() {
        let path = path();
        let p = CellPos::new(0, 0, 0);
        let mut world = World::new(Flat);
        world
            .apply_journaled(&path, &[EditAction::Set(Edit::set(p, Voxel::AIR))])
            .unwrap();
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(&[10, 0, 0, 0, 1, 2]).unwrap();
        drop(f);
        let mut recovered = World::new(Flat);
        assert_eq!(recovered.recover_journal(&path).unwrap(), 1);
        assert_eq!(recovered.voxel_at(p), Lookup::Known(Voxel::AIR));
        recovered
            .apply_journaled(
                &path,
                &[EditAction::Reset {
                    pos: p,
                    expected: None,
                }],
            )
            .unwrap();
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn mismatched_header_is_rejected_without_overwrite() {
        let path = path();
        let p = CellPos::new(0, 0, 0);
        let mut world = World::new(Flat);
        world
            .apply_journaled(&path, &[EditAction::Set(Edit::set(p, Voxel::AIR))])
            .unwrap();
        let original = std::fs::read(&path).unwrap();
        let mut changed =
            World::with_materials(Flat, crate::materials::MaterialRegistry::default());
        // Alter the persisted identity itself: no append or recovery may rewrite it.
        let mut bytes = original.clone();
        bytes[12] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        assert!(changed.recover_journal(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(changed
            .apply_journaled(&path, &[EditAction::Set(Edit::set(p, Voxel::AIR))])
            .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_file(path).unwrap();
    }
}
