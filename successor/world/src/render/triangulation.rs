//! Conforming greedy rectangles. Only rectangles with actual T-junctions or
//! independently meshed tile borders need explicit triangles. Others stay quads.
use super::surface::{Quad, Surface, AXES};
use std::collections::{HashMap, VecDeque};
type Point = [i64; 3];
type Line = (usize, i64, i64);
fn line(p: Point, axis: usize) -> Line {
    (axis, p[(axis + 1) % 3], p[(axis + 2) % 3])
}
fn corners(q: &Quad, step: i64) -> [Point; 4] {
    let [_, u, v] = AXES[q.face as usize];
    let a = q.origin.map(i64::from);
    let mut b = a;
    b[u] += q.size[0] as i64 * step;
    let mut c = b;
    c[v] += q.size[1] as i64 * step;
    let mut d = a;
    d[v] = c[v];
    [a, b, c, d]
}
fn at_border(q: &Quad, step: i64, edge: i64) -> bool {
    corners(q, step)
        .iter()
        .any(|p| p[0] == 0 || p[0] == edge || p[2] == 0 || p[2] == edge)
}
/// Points on axis-aligned lines, shared across the main mesh and its finer
/// stitch parts. Sorting once avoids testing every edge against every vertex.
pub struct Junctions {
    lines: HashMap<Line, Vec<i64>>,
}
impl Junctions {
    pub fn new<'a>(parts: impl Iterator<Item = (&'a Surface, i64)>) -> Self {
        let mut lines: HashMap<Line, Vec<i64>> = HashMap::new();
        for (surface, step) in parts {
            for q in &surface.quads {
                for p in corners(q, step) {
                    for axis in 0..3 {
                        lines.entry(line(p, axis)).or_default().push(p[axis]);
                    }
                }
            }
        }
        for points in lines.values_mut() {
            points.sort_unstable();
            points.dedup();
        }
        Self { lines }
    }
    fn edge(
        &self,
        a: Point,
        b: Point,
        step: i64,
        extent: i64,
        neighbors: [i64; 4],
        constant: bool,
    ) -> Vec<Point> {
        let axis = (0..3)
            .find(|&i| a[i] != b[i])
            .expect("nonempty rectangle edge");
        let low = a[axis].min(b[axis]);
        let high = a[axis].max(b[axis]);
        let mut points = vec![low, high];
        if let Some(on_line) = self.lines.get(&line(a, axis)) {
            let first = on_line.partition_point(|v| *v <= low);
            let last = on_line.partition_point(|v| *v < high);
            points.extend_from_slice(&on_line[first..last]);
        }
        // Every independently meshed border uses the same finer grid spacing.
        // Only that edge refines; other edges retain their own junction list.
        let mut spacing = if constant { i64::MAX / 2 } else { step };
        let mut border = false;
        for (fixed, plane, n) in [
            (0, 0, neighbors[0]),
            (0, extent, neighbors[1]),
            (2, 0, neighbors[2]),
            (2, extent, neighbors[3]),
        ] {
            if a[fixed] == plane && b[fixed] == plane {
                spacing = spacing.min(n);
                border = true;
            }
        }
        if border {
            points.extend((low..=high).step_by(spacing as usize));
        }
        points.sort_unstable();
        points.dedup();
        if a[axis] > b[axis] {
            points.reverse();
        }
        points
            .into_iter()
            .map(|v| {
                let mut p = a;
                p[axis] = v;
                p
            })
            .collect()
    }
}
pub struct Prepared {
    pub order: Vec<usize>,
    pub factors: Vec<u32>,
    pub regular: u32,
    pub vertices: Vec<[u32; 3]>,
}
impl Prepared {
    pub fn quads_only(surface: &Surface) -> Self {
        Self {
            order: (0..surface.quads.len()).collect(),
            factors: vec![1; surface.quads.len()],
            regular: surface.quads.len() as u32,
            vertices: Vec::new(),
        }
    }
    pub fn bytes(&self) -> usize {
        4 + self.vertices.len().max(1) * if self.compact_vertices() { 8 } else { 12 }
    }
    fn compact_vertices(&self) -> bool {
        self.vertices
            .iter()
            .all(|v| v[0] < 65536 && v[1] < 1 << 24 && v[2] < 1 << 24)
    }
    /// Exact 16-bit record + two 24-bit coordinates. Wide/adversarial parts
    /// retain a lossless 3-word fallback; never truncate fine-grid junctions.
    pub fn vertex_words(&self) -> Vec<u32> {
        let compact = self.compact_vertices();
        let mut words = Vec::with_capacity(self.bytes() / 4);
        words.push(u32::from(compact));
        for v in &self.vertices {
            if compact {
                words.extend([(v[0] << 16) | (v[1] & 65535), (v[2] << 8) | (v[1] >> 16)]);
            } else {
                words.extend(v);
            }
        }
        if self.vertices.is_empty() {
            words.extend([0, 0]);
        }
        words
    }
}

#[cfg(test)]
mod packing_tests {
    use super::*;
    #[test]
    fn compact_junctions_are_exact_and_wide_references_fall_back() {
        let mut p = Prepared {
            order: vec![],
            factors: vec![],
            regular: 0,
            vertices: vec![[65535, 1 << 23, (1 << 24) - 1], [7, 129000, 1]],
        };
        let words = p.vertex_words();
        assert_eq!(words[0], 1);
        assert_eq!(words.len() * 4, p.bytes());
        let decoded: Vec<_> = words[1..]
            .chunks_exact(2)
            .map(|v| [v[0] >> 16, (v[0] & 65535) | ((v[1] & 255) << 16), v[1] >> 8])
            .collect();
        assert_eq!(decoded, p.vertices);
        p.vertices.push([65536, 1, 1]);
        let words = p.vertex_words();
        assert_eq!(words[0], 0);
        assert_eq!(words.len() * 4, p.bytes());
        assert_eq!(&words[1..], p.vertices.concat());
    }
}
/// Match each finer neighbor midpoint without changing the original patch
/// diagonal. Splitting a whole polygon around its center would change relief.
pub fn prepare_coherent(
    surface: &Surface,
    step: i64,
    extent: i64,
    neighbors: [i64; 4],
) -> Prepared {
    let corners = [[0, 0], [2, 0], [2, 2], [0, 2]];
    let mut out = Prepared {
        order: Vec::new(),
        factors: Vec::new(),
        regular: 0,
        vertices: Vec::new(),
    };
    let mut split = Vec::new();
    for (index, q) in surface.quads.iter().enumerate() {
        let x = q.origin[0] as i64;
        let z = q.origin[2] as i64;
        // UV u follows Z; UV v follows X.
        let edges = [
            x == 0 && neighbors[0] < step,
            z + step == extent && neighbors[3] < step,
            x + step == extent && neighbors[1] < step,
            z == 0 && neighbors[2] < step,
        ];
        if !edges.iter().any(|b| *b) {
            out.order.push(index);
            continue;
        }
        let mut triangles = vec![
            [corners[0], corners[1], corners[2]],
            [corners[0], corners[2], corners[3]],
        ];
        for edge in 0..4 {
            if !edges[edge] {
                continue;
            }
            let a = corners[edge];
            let b = corners[(edge + 1) % 4];
            let middle = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
            let mut next = Vec::new();
            for tri in triangles {
                if let Some(i) = (0..3).find(|&i| tri[i] == a && tri[(i + 1) % 3] == b) {
                    let opposite = tri[(i + 2) % 3];
                    next.push([a, middle, opposite]);
                    next.push([middle, b, opposite]);
                } else {
                    next.push(tri);
                }
            }
            triangles = next;
        }
        split.push((index, triangles));
    }
    out.regular = out.order.len() as u32;
    out.factors.resize(out.order.len(), 1);
    for (index, triangles) in split {
        let record = out.order.len() as u32;
        out.order.push(index);
        out.factors.push(2);
        for p in triangles.into_iter().flatten() {
            out.vertices.push([record, p[0], p[1]]);
        }
    }
    out
}

pub fn prepare(
    surface: &Surface,
    step: i64,
    extent: i64,
    neighbors: [i64; 4],
    junctions: &Junctions,
) -> Prepared {
    let mut result = Prepared {
        order: Vec::new(),
        factors: Vec::new(),
        regular: 0,
        vertices: Vec::new(),
    };
    let mut split = Vec::new();
    for (index, q) in surface.quads.iter().enumerate() {
        if q.size == [1, 1] && (!at_border(q, step, extent) || step == 1) {
            result.order.push(index);
            continue;
        }
        let c = corners(q, step);
        let edges: [Vec<Point>; 4] = std::array::from_fn(|i| {
            junctions.edge(
                c[i],
                c[(i + 1) % 4],
                step,
                extent,
                neighbors,
                q.constant_color,
            )
        });
        if edges.iter().all(|e| e.len() == 2) {
            result.order.push(index);
            continue;
        }
        let [_, u, v] = AXES[q.face as usize];
        // A clipped water plane can introduce a native-grid junction, not
        // merely a half-step one. Preserve it exactly instead of rounding two
        // perimeter vertices onto the same UV and degenerating the polygon.
        let factor = edges
            .iter()
            .flatten()
            .flat_map(|p| [p[u] - c[0][u], p[v] - c[0][v]])
            .map(|d| step / (1i64 << d.unsigned_abs().trailing_zeros().min(step.trailing_zeros())))
            .max()
            .unwrap_or(1);
        let mut polygon = Vec::new();
        for edge in edges {
            for p in &edge[..edge.len() - 1] {
                let uv = [
                    (p[u] - c[0][u]) * factor / step,
                    (p[v] - c[0][v]) * factor / step,
                ];
                assert!(uv.iter().all(|v| (0..=u32::MAX as i64).contains(v)));
                polygon.push([uv[0] as u32, uv[1] as u32]);
            }
        }
        split.push((index, factor as u32, triangulate(&polygon)));
    }
    result.regular = result.order.len() as u32;
    result.factors.resize(result.order.len(), 1);
    for (index, factor, triangles) in split {
        let record = result.order.len() as u32;
        result.order.push(index);
        result.factors.push(factor);
        for p in triangles {
            result.vertices.push([record, p[0], p[1]]);
        }
    }
    result
}
fn cross(a: [u32; 2], b: [u32; 2], c: [u32; 2]) -> i64 {
    (b[0] as i64 - a[0] as i64) * (c[1] as i64 - a[1] as i64)
        - (b[1] as i64 - a[1] as i64) * (c[0] as i64 - a[0] as i64)
}
/// A convex rectangle perimeter with collinear junction vertices. Ear clipping
/// preserves those vertices without introducing off-grid center points. The
/// diagonal check prevents leaving an untriangulated collinear boundary tail.
fn triangulate(polygon: &[[u32; 2]]) -> Vec<[u32; 2]> {
    let n = polygon.len();
    let mut count = n;
    let mut previous: Vec<_> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<_> = (0..n).map(|i| (i + 1) % n).collect();
    let mut alive = vec![true; n];
    let mut rows = HashMap::new();
    let mut columns = HashMap::new();
    for p in polygon {
        *rows.entry(p[1]).or_insert(0usize) += 1;
        *columns.entry(p[0]).or_insert(0usize) += 1;
    }
    let mut ears: VecDeque<_> = (0..n)
        .filter(|&i| cross(polygon[previous[i]], polygon[i], polygon[next[i]]) > 0)
        .collect();
    let mut triangles = Vec::with_capacity((n - 2) * 3);
    while count > 3 {
        let i = ears.pop_front().expect("convex rectangle has an ear");
        if !alive[i] {
            continue;
        }
        let left = previous[i];
        let right = next[i];
        let [a, b, c] = [polygon[left], polygon[i], polygon[right]];
        if cross(a, b, c) <= 0 {
            continue;
        }
        // Every vertex lies on the original rectangle perimeter. A non-axis
        // diagonal intersects it only at its endpoints. An axis diagonal must
        // not skip another remaining boundary point; row/column counts test
        // that in O(1), including long one-cell-wide cliff faces.
        if (a[0] == c[0] && columns[&a[0]] > 2) || (a[1] == c[1] && rows[&a[1]] > 2) {
            ears.push_back(i);
            continue;
        }
        triangles.extend_from_slice(&[a, b, c]);
        alive[i] = false;
        count -= 1;
        *rows.get_mut(&b[1]).unwrap() -= 1;
        *columns.get_mut(&b[0]).unwrap() -= 1;
        next[left] = right;
        previous[right] = left;
        ears.push_back(left);
        ears.push_back(right);
    }
    let a = alive.iter().position(|v| *v).unwrap();
    let b = next[a];
    let c = next[b];
    assert!(cross(polygon[a], polygon[b], polygon[c]) > 0);
    triangles.extend_from_slice(&[polygon[a], polygon[b], polygon[c]]);
    triangles
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_water_junctions_do_not_round_to_half_step_uvs() {
        let coarse = Surface {
            quads: vec![Quad {
                origin: [0, 64, 0],
                face: 2,
                size: [2, 2],
                color_offset: 0,
                water_level: 0,
                slope_light: false,
                constant_color: false,
            }],
            ..Default::default()
        };
        let thin = Surface {
            quads: vec![Quad {
                origin: [0, 67, 0],
                face: 3,
                size: [1, 16],
                color_offset: 0,
                water_level: 0,
                slope_light: false,
                constant_color: true,
            }],
            ..Default::default()
        };
        let junctions = Junctions::new([(&coarse, 16), (&thin, 1)].into_iter());
        let prepared = prepare(&coarse, 16, 512, [16; 4], &junctions);
        assert!(prepared.factors.contains(&16));
        let mut area = 0;
        for triangle in prepared.vertices.chunks_exact(3) {
            let points: Vec<_> = triangle.iter().map(|p| [p[1], p[2]]).collect();
            let a = cross(points[0], points[1], points[2]);
            assert!(a > 0);
            area += a;
        }
        assert_eq!(area, 32 * 32 * 2);
    }
    #[test]
    fn collinear_perimeters_keep_every_edge_vertex_and_exact_area() {
        for [w, h] in [
            [1, 1],
            [1, 32],
            [32, 1],
            [2, 2],
            [17, 23],
            [32, 32],
            [1, 4096],
        ] {
            let mut polygon = Vec::new();
            for x in 0..w {
                polygon.push([x, 0]);
            }
            for y in 0..h {
                polygon.push([w, y]);
            }
            for x in (1..=w).rev() {
                polygon.push([x, h]);
            }
            for y in (1..=h).rev() {
                polygon.push([0, y]);
            }
            let triangles = triangulate(&polygon);
            assert_eq!(triangles.len(), 3 * (polygon.len() - 2));
            let area: i64 = triangles
                .chunks_exact(3)
                .map(|t| cross(t[0], t[1], t[2]))
                .sum();
            assert_eq!(area, 2 * w as i64 * h as i64);
            let mut edges = std::collections::BTreeMap::new();
            for t in triangles.chunks_exact(3) {
                assert!(cross(t[0], t[1], t[2]) > 0);
                for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    let e = if a < b { (a, b) } else { (b, a) };
                    *edges.entry(e).or_insert(0) += 1;
                }
            }
            for ((a, b), count) in edges {
                assert!(count == 1 || count == 2);
                if count == 1 {
                    assert_eq!(a[0].abs_diff(b[0]) + a[1].abs_diff(b[1]), 1);
                }
            }
        }
    }
    #[test]
    fn interior_t_junctions_split_without_splitting_unrelated_rectangles() {
        let surface = Surface {
            quads: vec![
                Quad {
                    constant_color: false,
                    origin: [5, 10, 5],
                    face: 0,
                    size: [4, 4],
                    color_offset: 0,
                    water_level: 0,
                    slope_light: false,
                },
                Quad {
                    constant_color: false,
                    origin: [9, 10, 7],
                    face: 0,
                    size: [2, 2],
                    color_offset: 16,
                    water_level: 0,
                    slope_light: false,
                },
            ],
            ..Surface::default()
        };
        let junctions = Junctions::new([(&surface, 1)].into_iter());
        let p = prepare(&surface, 1, 32, [1; 4], &junctions);
        assert_eq!(p.regular, 1);
        assert_eq!(p.order, [1, 0]);
        assert_eq!(p.vertices.len(), 9);
    }
}

#[cfg(test)]
mod coherent_preparation_tests {
    use super::*;
    use std::collections::BTreeMap;
    fn quad(origin: [i32; 3]) -> Quad {
        Quad {
            origin,
            face: 6,
            size: [1, 1],
            color_offset: 0,
            water_level: 0,
            slope_light: false,
            constant_color: true,
        }
    }
    fn undirected(a: [u32; 2], b: [u32; 2]) -> ([u32; 2], [u32; 2]) {
        if a < b {
            (a, b)
        } else {
            (b, a)
        }
    }
    fn height(p: [u32; 2], h: [f64; 4]) -> f64 {
        let u = p[0] as f64 * 0.5;
        let v = p[1] as f64 * 0.5;
        if u >= v {
            h[0] + (h[1] - h[0]) * u + (h[2] - h[1]) * v
        } else {
            h[0] + (h[2] - h[3]) * u + (h[3] - h[0]) * v
        }
    }
    #[test]
    fn every_finer_neighbor_edge_pattern_preserves_area_winding_and_original_diagonal() {
        let surface = Surface {
            quads: vec![quad([0, 0, 0])],
            ..Default::default()
        };
        // A one-cell extent exercises even all four exterior sides together.
        // Ordinary32-cell tiles are covered separately below.
        for mask in 0u32..16 {
            let neighbors = std::array::from_fn(|side| if mask & (1 << side) != 0 { 4 } else { 8 });
            let prepared = prepare_coherent(&surface, 8, 8, neighbors);
            assert_eq!(prepared.order, [0]);
            if mask == 0 {
                assert_eq!(prepared.regular, 1);
                assert_eq!(prepared.factors, [1]);
                assert!(prepared.vertices.is_empty());
                continue;
            }
            assert_eq!(prepared.regular, 0);
            assert_eq!(prepared.factors, [2]);
            assert_eq!(
                prepared.vertices.len(),
                3 * (2 + mask.count_ones() as usize)
            );
            let mut edges = BTreeMap::<_, usize>::new();
            let mut area = 0;
            let heights = [3., 17., 5., 41.];
            for tri in prepared.vertices.chunks_exact(3) {
                assert!(tri.iter().all(|p| p[0] == 0 && p[1] <= 2 && p[2] <= 2));
                let p: [[u32; 2]; 3] = std::array::from_fn(|i| [tri[i][1], tri[i][2]]);
                let signed = cross(p[0], p[1], p[2]);
                assert!(signed > 0, "winding mask={mask}");
                area += signed;
                // No new diagonal crosses the original(u==v) crease.
                assert!(p.iter().all(|p| p[0] >= p[1]) || p.iter().all(|p| p[0] <= p[1]));
                // Nonplanar corner heights have a strong diagonal crease.
                // Each output triangle must reproduce its original plane.
                let original = if p.iter().all(|p| p[0] >= p[1]) {
                    [[0, 0], [2, 0], [2, 2]]
                } else {
                    [[0, 0], [2, 2], [0, 2]]
                };
                for vertex in p {
                    let denom = cross(original[0], original[1], original[2]) as f64;
                    let interpolated = (cross(vertex, original[1], original[2]) as f64
                        * height(original[0], heights)
                        + cross(original[0], vertex, original[2]) as f64
                            * height(original[1], heights)
                        + cross(original[0], original[1], vertex) as f64
                            * height(original[2], heights))
                        / denom;
                    assert_eq!(height(vertex, heights), interpolated);
                }
                for i in 0..3 {
                    *edges.entry(undirected(p[i], p[(i + 1) % 3])).or_default() += 1;
                }
            }
            assert_eq!(area, 8, "exact UV area mask={mask}");
            assert_eq!(
                edges[&undirected([0, 0], [2, 2])],
                2,
                "original diagonal must remain shared"
            );
            let corners = [[0, 0], [2, 0], [2, 2], [0, 2]];
            let edge_to_side = [0, 3, 1, 2];
            let mut expected = BTreeMap::new();
            for edge in 0..4 {
                let a = corners[edge];
                let b = corners[(edge + 1) % 4];
                if mask & (1 << edge_to_side[edge]) != 0 {
                    let mid = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
                    expected.insert(undirected(a, mid), 1);
                    expected.insert(undirected(mid, b), 1);
                } else {
                    expected.insert(undirected(a, b), 1);
                }
            }
            let boundary: BTreeMap<_, _> = edges
                .iter()
                .filter(|(_, n)| **n == 1)
                .map(|(e, n)| (*e, *n))
                .collect();
            assert_eq!(
                boundary, expected,
                "neighbor midpoint ownership mask={mask}"
            );
            assert!(edges.values().all(|&n| n == 1 || n == 2));
        }
    }
    #[test]
    fn only_matching_external_edges_split_and_record_order_and_uv_factor_are_exact() {
        let surface = Surface {
            quads: vec![
                quad([0, 0, 8]),
                quad([8, 0, 8]),
                quad([16, 0, 24]),
                quad([24, 0, 16]),
            ],
            ..Default::default()
        };
        // Firstquad touches-X; thirdquad touches+Z; lastquad touches+X.
        // Only-X and+Z neighbors are finer.
        let prepared = prepare_coherent(&surface, 8, 32, [4, 8, 8, 4]);
        assert_eq!(prepared.regular, 2);
        assert_eq!(prepared.order, [1, 3, 0, 2]);
        assert_eq!(prepared.factors, [1, 1, 2, 2]);
        assert_eq!(prepared.vertices.len(), 18);
        for record in [2u32, 3] {
            let vertices: Vec<_> = prepared
                .vertices
                .iter()
                .filter(|v| v[0] == record)
                .collect();
            assert_eq!(vertices.len(), 9);
            let q = &surface.quads[prepared.order[record as usize]];
            let world: Vec<_> = vertices
                .iter()
                .map(|v| {
                    [
                        q.origin[0] as i64 + v[2] as i64 * 8 / 2,
                        q.origin[2] as i64 + v[1] as i64 * 8 / 2,
                    ]
                })
                .collect();
            let expected = if record == 2 { [0, 12] } else { [20, 32] };
            assert!(
                world.contains(&expected),
                "halfstep UV must map to neighbor's exact native coordinate"
            );
        }
        let words = prepared.vertex_words();
        assert_eq!(words[0], 1);
        let decoded: Vec<_> = words[1..]
            .chunks_exact(2)
            .map(|p| [p[0] >> 16, (p[0] & 65535) | ((p[1] & 255) << 16), p[1] >> 8])
            .collect();
        assert_eq!(decoded, prepared.vertices);
    }
}
