//! Keep cell-addressed checkpoints tied to the grid that produced them.
use frontier_world::{voxel_grid, VoxelGrid};

const GRID_HEADER: &[u8; 8] = b"FVGRID01";

pub(crate) fn header(format: &[u8; 8]) -> Vec<u8> {
    header_for(format, voxel_grid())
}

fn header_for(format: &[u8; 8], grid: VoxelGrid) -> Vec<u8> {
    let mut bytes = Vec::new();
    if grid != VoxelGrid::DEFAULT {
        bytes.extend(GRID_HEADER);
        bytes.extend(grid.size_m().to_le_bytes());
    }
    bytes.extend(format);
    bytes
}

pub(crate) fn payload(bytes: &[u8]) -> Result<&[u8], String> {
    payload_for(bytes, voxel_grid())
}

fn payload_for(bytes: &[u8], grid: VoxelGrid) -> Result<&[u8], String> {
    let (saved_size, payload) = if let Some(rest) = bytes.strip_prefix(GRID_HEADER) {
        let (size, payload) = rest.split_first_chunk::<8>().ok_or("truncated save grid")?;
        (f64::from_le_bytes(*size), payload)
    } else {
        (VoxelGrid::DEFAULT.size_m(), bytes)
    };
    if saved_size.to_bits() != grid.size_m().to_bits() {
        return Err(format!(
            "save uses {saved_size} m voxels, but this world uses {} m voxels",
            grid.size_m()
        ));
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_rejects_other_grids_and_preserves_legacy_bytes() {
        let format = b"FVWATER3";
        let decimal = VoxelGrid::new(0.03).unwrap();
        let legacy = header_for(format, VoxelGrid::DEFAULT);
        let resized = header_for(format, decimal);

        assert_eq!(&legacy, format);
        assert_eq!(payload_for(&legacy, VoxelGrid::DEFAULT).unwrap(), format);
        assert_eq!(payload_for(&resized, decimal).unwrap(), format);
        assert!(payload_for(&legacy, decimal).is_err());
        assert!(payload_for(&resized, VoxelGrid::DEFAULT).is_err());
        assert!(payload_for(GRID_HEADER, decimal).is_err());
    }
}
