//! Conservative current-frame software occlusion. A tiny coverage grid stores
//! only fully covered cells and the furthest depth of guaranteed-solid prisms.
//! No previous-frame visibility is reused when the camera moves.
use glam::{Mat4, Vec4};
use microvoxel::render::lod::MeshKey;
const W: usize = 64;
const H: usize = 36;
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: MeshKey,
    pub low: [f64; 3],
    pub high: [f64; 3],
    pub solid_top: f64,
}
struct Node {
    low: [f64; 3],
    high: [f64; 3],
    children: Option<[usize; 2]>,
    leaf: usize,
}
#[derive(Default)]
pub struct Visibility {
    entries: Vec<Entry>,
    nodes: Vec<Node>,
    pub occluded_nodes: usize,
}
fn build(indices: &mut [usize], entries: &[Entry], nodes: &mut Vec<Node>, depth: usize) -> usize {
    let index = nodes.len();
    let low = std::array::from_fn(|a| {
        indices
            .iter()
            .map(|i| entries[*i].low[a])
            .fold(f64::INFINITY, f64::min)
    });
    let high = std::array::from_fn(|a| {
        indices
            .iter()
            .map(|i| entries[*i].high[a])
            .fold(f64::NEG_INFINITY, f64::max)
    });
    nodes.push(Node {
        low,
        high,
        children: None,
        leaf: indices[0],
    });
    if indices.len() > 1 {
        let axis = if depth % 2 == 0 { 0 } else { 2 };
        let mid = indices.len() / 2;
        indices.select_nth_unstable_by(mid, |a, b| {
            entries[*a].low[axis].total_cmp(&entries[*b].low[axis])
        });
        let (a, b) = indices.split_at_mut(mid);
        let a = build(a, entries, nodes, depth + 1);
        let b = build(b, entries, nodes, depth + 1);
        nodes[index].children = Some([a, b]);
    }
    index
}
fn project(low: [f64; 3], high: [f64; 3], eye: [f64; 3], vp: Mat4) -> Option<Vec<[f32; 3]>> {
    let mut points = Vec::with_capacity(8);
    for c in 0..8 {
        let p = Vec4::new(
            (if c & 1 == 0 { low[0] } else { high[0] } - eye[0]) as f32,
            (if c & 2 == 0 { low[1] } else { high[1] } - eye[1]) as f32,
            (if c & 4 == 0 { low[2] } else { high[2] } - eye[2]) as f32,
            1.,
        );
        let p = vp * p;
        if p.w <= 0.001 || p.z <= 0. {
            return None;
        }
        let p = p / p.w;
        points.push([p.x * 0.5 + 0.5, 0.5 - p.y * 0.5, p.z]);
    }
    Some(points)
}
fn rectangle(p: &[[f32; 3]]) -> [usize; 4] {
    let lo = [
        p.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min),
        p.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min),
    ];
    let hi = [
        p.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max),
        p.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max),
    ];
    [
        (lo[0] * W as f32).floor().clamp(0., W as f32 - 1.) as usize,
        (lo[1] * H as f32).floor().clamp(0., H as f32 - 1.) as usize,
        (hi[0] * W as f32).floor().clamp(0., W as f32 - 1.) as usize,
        (hi[1] * H as f32).floor().clamp(0., H as f32 - 1.) as usize,
    ]
}
fn cross(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}
fn hull(points: &[[f32; 3]]) -> Vec<[f32; 2]> {
    let mut p: Vec<_> = points.iter().map(|p| [p[0], p[1]]).collect();
    p.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    p.dedup();
    let mut h = Vec::new();
    for &v in &p {
        while h.len() >= 2 && cross(h[h.len() - 2], h[h.len() - 1], v) <= 0. {
            h.pop();
        }
        h.push(v);
    }
    let lower = h.len();
    for &v in p.iter().rev().skip(1) {
        while h.len() > lower && cross(h[h.len() - 2], h[h.len() - 1], v) <= 0. {
            h.pop();
        }
        h.push(v);
    }
    h.pop();
    h
}
fn inside(h: &[[f32; 2]], p: [f32; 2]) -> bool {
    h.len() >= 3 && (0..h.len()).all(|i| cross(h[i], h[(i + 1) % h.len()], p) > 1e-7)
}
impl Visibility {
    pub fn select(
        &mut self,
        entries: Vec<Entry>,
        eye: [f64; 3],
        vp: Mat4,
        limit: f32,
        occlusion: bool,
    ) -> Vec<MeshKey> {
        self.occluded_nodes = 0;
        if entries != self.entries {
            self.entries = entries;
            self.nodes.clear();
            if !self.entries.is_empty() {
                build(
                    &mut (0..self.entries.len()).collect::<Vec<_>>(),
                    &self.entries,
                    &mut self.nodes,
                    0,
                );
            }
        }
        if self.nodes.is_empty() {
            return vec![];
        }
        let mut depth = [1f32; W * H];
        let mut stack = vec![0usize];
        let mut visible = Vec::new();
        let distance = |node: &Node| {
            (0..3)
                .map(|i| {
                    (node.low[i] - eye[i])
                        .max(0.)
                        .max(eye[i] - node.high[i])
                        .powi(2)
                })
                .sum::<f64>()
        };
        while let Some(index) = stack.pop() {
            let n = &self.nodes[index];
            if distance(n) > f64::from(limit).powi(2) {
                continue;
            }
            let low = std::array::from_fn(|i| n.low[i] - eye[i]);
            let high = std::array::from_fn(|i| n.high[i] - eye[i]);
            if !crate::gpu::in_frustum(vp, low, high) {
                continue;
            }
            if occlusion {
                if let Some(p) = project(n.low, n.high, eye, vp) {
                    let [x0, y0, x1, y1] = rectangle(&p);
                    let near = p.iter().map(|p| p[2]).fold(1., f32::min);
                    if (y0..=y1).all(|y| (x0..=x1).all(|x| depth[y * W + x] + 0.000001 < near)) {
                        self.occluded_nodes += 1;
                        continue;
                    }
                }
            }
            if let Some([a, b]) = n.children {
                if distance(&self.nodes[a]) < distance(&self.nodes[b]) {
                    stack.extend([b, a]);
                } else {
                    stack.extend([a, b]);
                }
            } else {
                let e = &self.entries[n.leaf];
                visible.push(e.key);
                if occlusion && e.solid_top > 0. {
                    if let Some(p) = project(
                        [e.low[0], 0., e.low[2]],
                        [e.high[0], e.solid_top, e.high[2]],
                        eye,
                        vp,
                    ) {
                        let far = p.iter().map(|p| p[2]).fold(0., f32::max) + 0.000001;
                        let h = hull(&p);
                        let [x0, y0, x1, y1] = rectangle(&p);
                        for y in y0..=y1 {
                            for x in x0..=x1 {
                                if [[x, y], [x + 1, y], [x, y + 1], [x + 1, y + 1]].iter().all(
                                    |p| {
                                        inside(&h, [p[0] as f32 / W as f32, p[1] as f32 / H as f32])
                                    },
                                ) {
                                    depth[y * W + x] = depth[y * W + x].min(far);
                                }
                            }
                        }
                    }
                }
            }
        }
        visible
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    #[test]
    fn solid_prism_hides_a_distant_box_but_preserves_its_silhouette() {
        use microvoxel::render::lod::TileKey;
        let key = |x| MeshKey {
            tile: TileKey { level: 1, x, z: 0 },
            neighbors: [1; 4],
            revision: 0,
        };
        let wall = Entry {
            key: key(0),
            low: [-1., 0., 5.],
            high: [1., 5., 6.],
            solid_top: 5.,
        };
        let hidden = Entry {
            key: key(1),
            low: [-0.1, 2.8, 20.],
            high: [0.1, 3.2, 21.],
            solid_top: 0.,
        };
        let vp = Mat4::perspective_rh(65f32.to_radians(), 1., 0.025, 100.)
            * Mat4::look_to_rh(Vec3::ZERO, Vec3::Z, Vec3::Y);
        let mut culler = Visibility::default();
        assert_eq!(
            culler.select(
                vec![wall.clone(), hidden.clone()],
                [0., 3., 0.],
                vp,
                100.,
                true
            ),
            vec![key(0)]
        );
        let high = Entry {
            low: [-0.1, 13., 20.],
            high: [0.1, 14., 21.],
            ..hidden
        };
        assert_eq!(
            culler
                .select(vec![wall, high], [0., 3., 0.], vp, 100., true)
                .len(),
            2
        );
    }
    #[test]
    fn convex_coverage_keeps_edges_and_near_plane_conservative() {
        let h = hull(&[[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]]);
        assert!(inside(&h, [0.5, 0.5]));
        assert!(!inside(&h, [0., 0.5]));
        assert!(project(
            [-1.; 3],
            [1.; 3],
            [0.; 3],
            Mat4::perspective_rh(1., 1., 0.1, 100.)
        )
        .is_none());
    }
}
