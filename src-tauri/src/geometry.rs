//! Geometry processing (H3) — turns the flat polygon list into an
//! electrical net graph.
//!
//! Pipeline:
//!   1. A uniform-grid spatial index buckets polygons by their bbox so
//!      we only narrow-phase test pairs that share a cell (avoids the
//!      O(n²) all-pairs blow-up on large layouts).
//!   2. A narrow-phase polygon-touch test (bbox reject → edge-segment
//!      intersection → containment) decides if two polygons abut/overlap.
//!   3. Two polygons are *connected* when they touch AND either sit on
//!      the same layer or one is a via-role layer (contacts/vias bridge
//!      layers). Poly-over-diffusion is intentionally NOT a connection —
//!      that's a transistor, extracted later in H4.
//!   4. Union-Find merges connected polygons into nets.
//!
//! Output [`Nets`] maps each polygon to a compacted net id and lists the
//! members of each net — the seed of the H4 transistor extractor and the
//! "click a wire → whole net lights up" interaction.

use std::collections::HashMap;

use crate::gds::{self, Polygon};
use crate::tech::Tech;

/// Connected-component result over the flattened polygon list.
#[derive(Debug, Default)]
pub struct Nets {
    /// `net_of[i]` = net id of polygon `i` (ids are compacted 0..count).
    pub net_of: Vec<u32>,
    /// `members[net]` = polygon indices belonging to that net.
    pub members: Vec<Vec<u32>>,
}

impl Nets {
    pub fn count(&self) -> usize {
        self.members.len()
    }
}

/// Build the net graph for `polys` under technology `tech`.
pub fn build_nets(polys: &[Polygon], tech: &Tech) -> Nets {
    let n = polys.len();
    let mut uf = UnionFind::new(n);

    if n > 1 {
        let grid = Grid::build(polys);
        // A given pair can share several cells; test each pair once.
        let mut tested: HashMap<(u32, u32), ()> = HashMap::new();
        for bucket in grid.cells.values() {
            for a in 0..bucket.len() {
                for b in (a + 1)..bucket.len() {
                    let (i, j) = (bucket[a], bucket[b]);
                    let key = if i < j { (i as u32, j as u32) } else { (j as u32, i as u32) };
                    if tested.insert(key, ()).is_some() {
                        continue;
                    }
                    if connectable(&polys[i], &polys[j], tech) {
                        uf.union(i, j);
                    }
                }
            }
        }
    }

    // Compact roots → 0..count net ids.
    let mut net_of = vec![0u32; n];
    let mut root_to_net: HashMap<usize, u32> = HashMap::new();
    let mut members: Vec<Vec<u32>> = Vec::new();
    for i in 0..n {
        let r = uf.find(i);
        let net = *root_to_net.entry(r).or_insert_with(|| {
            members.push(Vec::new());
            (members.len() - 1) as u32
        });
        net_of[i] = net;
        members[net as usize].push(i as u32);
    }

    Nets { net_of, members }
}

/// Two polygons are electrically connected if their geometry touches and
/// the technology allows it across their layers.
fn connectable(a: &Polygon, b: &Polygon, tech: &Tech) -> bool {
    let same_layer = a.layer == b.layer;
    let bridges =
        same_layer || tech.is_via(a.layer, a.datatype) || tech.is_via(b.layer, b.datatype);
    if !bridges {
        return false;
    }
    polygons_touch(&a.points, &b.points)
}

// ---- narrow phase: do two polygon rings touch / overlap? ----

fn polygons_touch(a: &[[f64; 2]], b: &[[f64; 2]]) -> bool {
    if a.len() < 3 || b.len() < 3 {
        return false;
    }
    let ba = gds::polygon_bbox(a);
    let bb = gds::polygon_bbox(b);
    if !bbox_overlap(&ba, &bb) {
        return false;
    }
    // Any edge crossing (incl. touching endpoints / collinear overlap)?
    let na = a.len();
    let nb = b.len();
    for i in 0..na {
        let a0 = a[i];
        let a1 = a[(i + 1) % na];
        for j in 0..nb {
            let b0 = b[j];
            let b1 = b[(j + 1) % nb];
            if segments_intersect(a0, a1, b0, b1) {
                return true;
            }
        }
    }
    // No edge crossing → one ring may be fully inside the other.
    gds::point_in_polygon(b, a[0]) || gds::point_in_polygon(a, b[0])
}

fn bbox_overlap(a: &gds::Bbox, b: &gds::Bbox) -> bool {
    a.min[0] <= b.max[0] && a.max[0] >= b.min[0] && a.min[1] <= b.max[1] && a.max[1] >= b.min[1]
}

/// Orientation of ordered triple (p, q, r): >0 ccw, <0 cw, 0 collinear.
fn orient(p: [f64; 2], q: [f64; 2], r: [f64; 2]) -> f64 {
    (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
}

/// Is point `q` on segment `pr`, given the three are collinear?
fn on_segment(p: [f64; 2], q: [f64; 2], r: [f64; 2]) -> bool {
    q[0] <= p[0].max(r[0]) + EPS
        && q[0] >= p[0].min(r[0]) - EPS
        && q[1] <= p[1].max(r[1]) + EPS
        && q[1] >= p[1].min(r[1]) - EPS
}

const EPS: f64 = 1e-9;

/// Standard segment-intersection test that returns true for proper
/// crossings, shared endpoints, and collinear overlaps — exactly the
/// cases that mean "these polygons touch".
fn segments_intersect(p1: [f64; 2], p2: [f64; 2], p3: [f64; 2], p4: [f64; 2]) -> bool {
    let d1 = orient(p3, p4, p1);
    let d2 = orient(p3, p4, p2);
    let d3 = orient(p1, p2, p3);
    let d4 = orient(p1, p2, p4);

    if ((d1 > EPS && d2 < -EPS) || (d1 < -EPS && d2 > EPS))
        && ((d3 > EPS && d4 < -EPS) || (d3 < -EPS && d4 > EPS))
    {
        return true;
    }
    // Collinear / endpoint-touch cases.
    if d1.abs() <= EPS && on_segment(p3, p1, p4) {
        return true;
    }
    if d2.abs() <= EPS && on_segment(p3, p2, p4) {
        return true;
    }
    if d3.abs() <= EPS && on_segment(p1, p3, p2) {
        return true;
    }
    if d4.abs() <= EPS && on_segment(p1, p4, p2) {
        return true;
    }
    false
}

// ---- uniform-grid spatial index ----

struct Grid {
    cells: HashMap<(i32, i32), Vec<usize>>,
}

impl Grid {
    /// Bucket every polygon into the grid cells its bbox covers. Cell
    /// size targets ~`TARGET` cells along the longer scene axis.
    fn build(polys: &[Polygon]) -> Grid {
        const TARGET: f64 = 64.0;
        let scene = gds::bbox(polys);
        let span_x = (scene.max[0] - scene.min[0]).max(1.0);
        let span_y = (scene.max[1] - scene.min[1]).max(1.0);
        let cell = (span_x.max(span_y) / TARGET).max(1.0);
        let origin = scene.min;

        let cell_of = |v: f64, o: f64| ((v - o) / cell).floor() as i32;

        let mut cells: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (idx, p) in polys.iter().enumerate() {
            if p.points.len() < 3 {
                continue;
            }
            let bb = gds::polygon_bbox(&p.points);
            let ix0 = cell_of(bb.min[0], origin[0]);
            let ix1 = cell_of(bb.max[0], origin[0]);
            let iy0 = cell_of(bb.min[1], origin[1]);
            let iy1 = cell_of(bb.max[1], origin[1]);
            for ix in ix0..=ix1 {
                for iy in iy0..=iy1 {
                    cells.entry((ix, iy)).or_default().push(idx);
                }
            }
        }
        Grid { cells }
    }
}

// ---- union-find (disjoint set, path compression + union by size) ----

struct UnionFind {
    parent: Vec<usize>,
    size: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind {
            parent: (0..n).collect(),
            size: vec![1; n],
        }
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]]; // path halving
            x = self.parent[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        let (big, small) = if self.size[ra] >= self.size[rb] { (ra, rb) } else { (rb, ra) };
        self.parent[small] = big;
        self.size[big] += self.size[small];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poly(layer: i16, datatype: i16, x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
        Polygon {
            layer,
            datatype,
            points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
        }
    }

    #[test]
    fn segments_intersect_cases() {
        // Proper crossing.
        assert!(segments_intersect([0.0, 0.0], [2.0, 2.0], [0.0, 2.0], [2.0, 0.0]));
        // Shared endpoint.
        assert!(segments_intersect([0.0, 0.0], [1.0, 0.0], [1.0, 0.0], [1.0, 1.0]));
        // Collinear overlap.
        assert!(segments_intersect([0.0, 0.0], [2.0, 0.0], [1.0, 0.0], [3.0, 0.0]));
        // Disjoint.
        assert!(!segments_intersect([0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]));
    }

    #[test]
    fn same_layer_touching_merges_into_one_net() {
        // Two abutting metal1 rectangles share an edge → one net.
        let polys = vec![
            poly(4, 0, 0.0, 0.0, 10.0, 10.0),
            poly(4, 0, 10.0, 0.0, 20.0, 10.0),
        ];
        let nets = build_nets(&polys, Tech::default_tech());
        assert_eq!(nets.count(), 1);
        assert_eq!(nets.net_of[0], nets.net_of[1]);
    }

    #[test]
    fn disjoint_same_layer_stays_separate() {
        let polys = vec![
            poly(4, 0, 0.0, 0.0, 10.0, 10.0),
            poly(4, 0, 100.0, 100.0, 110.0, 110.0),
        ];
        let nets = build_nets(&polys, Tech::default_tech());
        assert_eq!(nets.count(), 2);
        assert_ne!(nets.net_of[0], nets.net_of[1]);
    }

    #[test]
    fn different_layers_need_a_via_to_connect() {
        // metal1 (layer 4) overlapping metal2 (layer 5) but NO via → 2 nets.
        let no_via = vec![
            poly(4, 0, 0.0, 0.0, 10.0, 10.0),
            poly(5, 0, 5.0, 5.0, 15.0, 15.0),
        ];
        let nets = build_nets(&no_via, Tech::default_tech());
        assert_eq!(nets.count(), 2);

        // Add a via (layer 7) overlapping both → all three merge.
        let with_via = vec![
            poly(4, 0, 0.0, 0.0, 10.0, 10.0),
            poly(5, 0, 5.0, 5.0, 15.0, 15.0),
            poly(7, 0, 4.0, 4.0, 11.0, 11.0),
        ];
        let nets = build_nets(&with_via, Tech::default_tech());
        assert_eq!(nets.count(), 1);
        assert_eq!(nets.net_of[0], nets.net_of[2]);
        assert_eq!(nets.net_of[1], nets.net_of[2]);
    }

    #[test]
    fn poly_over_diffusion_is_not_a_connection() {
        // poly (layer 1) crossing active/diffusion (layer 2): a transistor,
        // not a wire — must stay two separate nets.
        let polys = vec![
            poly(1, 0, 5.0, 0.0, 7.0, 10.0),
            poly(2, 0, 0.0, 3.0, 12.0, 6.0),
        ];
        let nets = build_nets(&polys, Tech::default_tech());
        assert_eq!(nets.count(), 2);
    }

    #[test]
    fn contained_polygon_connects_same_layer() {
        // Small rect fully inside a big one, same layer, no edge crossing.
        let polys = vec![
            poly(4, 0, 0.0, 0.0, 100.0, 100.0),
            poly(4, 0, 40.0, 40.0, 60.0, 60.0),
        ];
        let nets = build_nets(&polys, Tech::default_tech());
        assert_eq!(nets.count(), 1);
    }
}
