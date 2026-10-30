//! Exact GPU color tiles: raw fallback, one constant, or an 8-bit local palette.
//! Packed values include lighting metadata; canonical RGB is never approximated.
use std::collections::HashMap;
pub fn pack(colors: &[u32]) -> Vec<u32> {
    let raw = || {
        let mut out = vec![0, 0, colors.len() as u32, 0];
        out.extend_from_slice(colors);
        out
    };
    if colors.is_empty() {
        return raw();
    }
    if colors.iter().all(|c| *c == colors[0]) && colors.len() > 1 {
        return vec![1, 1, colors.len() as u32, 0, colors[0]];
    }
    let mut palette = Vec::new();
    let mut lookup = HashMap::new();
    let mut indices = Vec::with_capacity((colors.len() + 3) / 4);
    for (i, &color) in colors.iter().enumerate() {
        let id = if let Some(&id) = lookup.get(&color) {
            id
        } else {
            if palette.len() == 256 {
                return raw();
            }
            let id = palette.len() as u32;
            palette.push(color);
            lookup.insert(color, id);
            id
        };
        if i % 4 == 0 {
            indices.push(0u32);
        }
        *indices.last_mut().unwrap() |= id << ((i % 4) * 8);
    }
    if palette.len() + indices.len() >= colors.len() {
        return raw();
    }
    let mut out = vec![2, palette.len() as u32, colors.len() as u32, 0];
    out.extend(palette);
    out.extend(indices);
    out
}
pub fn unpack(words: &[u32], index: usize) -> u32 {
    assert!(index < (words[2] as usize));
    match words[0] {
        1 => words[4],
        2 => {
            let p = words[4 + words[1] as usize + index / 4];
            words[4 + ((p >> ((index % 4) * 8)) & 255) as usize]
        }
        _ => words[4 + index],
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_colors_and_light_survive_all_encodings() {
        for data in [
            vec![0xabcdef42; 1_000_000],
            (0..4096).map(|i| (i % 17) * 1297289).collect(),
            (0..4096).map(|i| i * 7919).collect(),
        ] {
            let words = pack(&data);
            for (i, &value) in data.iter().enumerate() {
                assert_eq!(unpack(&words, i), value);
            }
            if data.len() == 1_000_000 {
                assert_eq!(words.len() * 4, 20);
            }
        }
    }
}
