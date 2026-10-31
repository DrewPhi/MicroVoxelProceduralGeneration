//! Lawson edge flips for the experiment's bounded, nondegenerate point set.
//! Not a general-purpose exact-predicate triangulation library. Near-cocircular
//! edges are retained within a scale-relative f64 tolerance for deterministic ties.
use std::collections::{HashMap, VecDeque};
type Key = (u32, u32);
const NONE: usize = usize::MAX;

pub(super) fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}
fn key(a: u32, b: u32) -> Key {
    (a.min(b), a.max(b))
}
fn keys(t: [u32; 3]) -> [Key; 3] {
    [key(t[0], t[1]), key(t[1], t[2]), key(t[2], t[0])]
}
fn insert(map: &mut HashMap<Key, [usize; 2]>, t: [u32; 3], id: usize) {
    for k in keys(t) {
        let slot = map.entry(k).or_insert([NONE; 2]);
        if slot[0] == NONE {
            slot[0] = id;
        } else {
            assert_eq!(slot[1], NONE);
            slot[1] = id;
        }
    }
}
fn remove(map: &mut HashMap<Key, [usize; 2]>, t: [u32; 3], id: usize) {
    for k in keys(t) {
        let slot = map.get_mut(&k).unwrap();
        if slot[0] == id {
            slot[0] = slot[1];
            slot[1] = NONE;
        } else {
            assert_eq!(slot[1], id);
            slot[1] = NONE;
        }
        if slot[0] == NONE {
            map.remove(&k);
        }
    }
}
fn incircle(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let mut p = [a, b, c].map(|v| [v[0] - d[0], v[1] - d[1]]);
    let scale = p.iter().flatten().fold(0f64, |m, v| m.max(v.abs()));
    if scale == 0. {
        return false;
    }
    for v in &mut p {
        v[0] /= scale;
        v[1] /= scale;
    }
    let [a, b, c] = p;
    let norm = |v: [f64; 2]| v[0] * v[0] + v[1] * v[1];
    let cross = |u: [f64; 2], v: [f64; 2]| u[0] * v[1] - u[1] * v[0];
    norm(a) * cross(b, c) - norm(b) * cross(a, c) + norm(c) * cross(a, b) > 1e-12
}
pub(super) fn legalize(points: &[[f64; 2]], triangles: &mut [[u32; 3]]) {
    let mut map = HashMap::with_capacity(triangles.len() * 2);
    for (i, &t) in triangles.iter().enumerate() {
        insert(&mut map, t, i);
    }
    let mut initial: Vec<_> = map.keys().copied().collect();
    initial.sort_unstable();
    let mut queue: VecDeque<_> = initial.into();
    let mut flips = 0;
    while let Some(k) = queue.pop_front() {
        let Some(&[i, j]) = map.get(&k) else {
            continue;
        };
        if j == NONE {
            continue;
        }
        let t = triangles[i];
        let u = triangles[j];
        // Choose the shared edge in t's counterclockwise orientation.
        let index = (0..3).find(|&q| key(t[q], t[(q + 1) % 3]) == k).unwrap();
        let (a, b, c) = (t[index], t[(index + 1) % 3], t[(index + 2) % 3]);
        let d = *u.iter().find(|&&v| v != a && v != b).unwrap();
        let p = |v: u32| points[v as usize];
        if orient(p(c), p(d), p(b)) <= 0. || orient(p(d), p(c), p(a)) <= 0. {
            continue;
        }
        if !incircle(p(a), p(b), p(c), p(d)) {
            continue;
        }
        remove(&mut map, t, i);
        remove(&mut map, u, j);
        triangles[i] = [c, d, b];
        triangles[j] = [d, c, a];
        insert(&mut map, triangles[i], i);
        insert(&mut map, triangles[j], j);
        queue.extend([key(a, c), key(c, b), key(b, d), key(d, a)]);
        flips += 1;
        assert!(
            flips < triangles.len() * 100,
            "triangulation failed to converge"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flips_illegal_diagonal_and_preserves_winding() {
        let p = [[0., 0.], [2., 0.], [2., 2.], [0., 1.]];
        let mut t = [[0, 1, 2], [0, 2, 3]];
        legalize(&p, &mut t);
        assert!(t.iter().all(|t| t.contains(&1) && t.contains(&3)));
        assert!(t
            .iter()
            .all(|t| orient(p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]) > 0.));
        let before = t;
        legalize(&p, &mut t);
        assert_eq!(before, t);
    }
}
