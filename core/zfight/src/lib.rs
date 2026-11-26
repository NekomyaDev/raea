//! Faces lying in one plane over each other (docs/make.feature MK8, the clip check FLOOR3 ran in Blender): two
//! triangles facing the same way within about a degree, their planes within 0.2 mm, overlapping by more than a
//! millimetre. The depth buffer can't order them, so the picture flickers between them as the camera moves
//! (z-fighting): between two parts (a rug's top in the floor's, two wall slabs overlapping in line) or inside one model
//! (a generated mesh's doubled faces). Faces meeting along an edge, and faces back to back (a box resting on another),
//! are left alone.

use std::collections::HashMap;

use bevy::math::{Vec2, Vec3};

/// Planes this close (metres) are one plane.
const PLANE: f32 = 0.0002;
/// Normals this aligned (the cosine) face the same way.
const PARALLEL: f32 = 0.9998;
/// Overlapping by less than this (metres) is meeting at an edge, not one face over another.
const INSIDE: f32 = 0.001;
/// A model with more faces than this isn't checked against itself (the check would cost more than the render).
const SELF_MAX: usize = 400_000;

struct Face {
    n: Vec3,
    d: f32,
    tri: [Vec3; 3],
}

fn face(t: &[Vec3; 3]) -> Option<Face> {
    let c = (t[1] - t[0]).cross(t[2] - t[0]);
    let l = c.length();
    if l < 1e-10 {
        return None;
    }
    let n = c / l;
    Some(Face { n, d: n.dot(t[0]), tri: *t })
}

type Key = (i32, i32, i32, i64);

fn key(n: Vec3, d: f32) -> Key {
    ((n.x * 100.0).round() as i32, (n.y * 100.0).round() as i32, (n.z * 100.0).round() as i32, (d / PLANE).floor() as i64)
}

/// The keys a face's twin may sit under: its own and every neighbour (a normal or plane on a bucket's edge).
fn near(k: Key) -> impl Iterator<Item = Key> {
    (-1..=1).flat_map(move |a| (-1..=1).flat_map(move |b| (-1..=1).flat_map(move |c| (-1..=1).map(move |e| (k.0 + a, k.1 + b, k.2 + c, k.3 + e as i64)))))
}

/// A point of a plane with normal `n` as two numbers, in an orthonormal frame of the plane.
fn flat_in(n: Vec3) -> impl Fn(Vec3) -> Vec2 {
    let u = n.any_orthonormal_vector();
    let v = n.cross(u);
    move |p| Vec2::new(p.dot(u), p.dot(v))
}

/// Whether two coplanar faces overlap by more than INSIDE: separating axes in the plane, in an orthonormal frame.
fn overlap(a: &Face, b: &Face) -> bool {
    let point = flat_in(a.n);
    let flat = |t: &[Vec3; 3]| t.map(&point);
    let (p, q) = (flat(&a.tri), flat(&b.tri));
    for tri in [&p, &q] {
        for k in 0..3 {
            let e = tri[(k + 1) % 3] - tri[k];
            let axis = Vec2::new(-e.y, e.x).normalize_or_zero();
            if axis == Vec2::ZERO {
                continue;
            }
            let span = |t: &[Vec2; 3]| {
                let d = t.map(|x| x.dot(axis));
                (d[0].min(d[1]).min(d[2]), d[0].max(d[1]).max(d[2]))
            };
            let (a0, a1) = span(&p);
            let (b0, b1) = span(&q);
            if a1.min(b1) - a0.max(b0) < INSIDE {
                return false;
            }
        }
    }
    true
}

/// What two coplanar faces share: its area, and its middle. One triangle cut by the other's three edges.
fn shared(a: &Face, b: &Face) -> (f32, Vec3) {
    // the same frame flat_in makes, to carry the middle back into the world
    let u = a.n.any_orthonormal_vector();
    let v = a.n.cross(u);
    let flat = flat_in(a.n);
    let mut poly: Vec<Vec2> = a.tri.iter().map(|p| flat(*p)).collect();
    let q = b.tri.map(flat);
    // b's winding in this frame, so its inside is on one known side of each edge
    let turn = (q[1] - q[0]).perp_dot(q[2] - q[0]).signum();
    for k in 0..3 {
        let (e0, e1) = (q[k], q[(k + 1) % 3]);
        let side = |p: Vec2| (e1 - e0).perp_dot(p - e0) * turn;
        let mut cut = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let (p0, p1) = (poly[i], poly[(i + 1) % poly.len()]);
            let (s0, s1) = (side(p0), side(p1));
            if s0 >= 0.0 {
                cut.push(p0);
            }
            if (s0 > 0.0 && s1 < 0.0) || (s0 < 0.0 && s1 > 0.0) {
                cut.push(p0 + (p1 - p0) * (s0 / (s0 - s1)));
            }
        }
        poly = cut;
        if poly.len() < 3 {
            return (0.0, Vec3::ZERO);
        }
    }
    let mut area = 0.0;
    let mut mid = Vec2::ZERO;
    for i in 1..poly.len() - 1 {
        let t = 0.5 * (poly[i] - poly[0]).perp_dot(poly[i + 1] - poly[0]).abs();
        area += t;
        mid += (poly[0] + poly[i] + poly[i + 1]) / 3.0 * t;
    }
    if area <= 0.0 {
        return (0.0, Vec3::ZERO);
    }
    let mid = mid / area;
    (area, u * mid.x + v * mid.y + a.n * a.d)
}

fn same_plane(a: &Face, b: &Face) -> bool {
    a.n.dot(b.n) >= PARALLEL && (a.n.dot(b.tri[0]) - a.d).abs() <= PLANE
}

/// One face over another: how many pairs, the area they share, and the middle of the largest shared patch. `b` None: within `a` itself.
pub fn overlaps(a: &[[Vec3; 3]], b: Option<&[[Vec3; 3]]>) -> Option<(usize, f32, Vec3)> {
    let o = sorted(a, b);
    o.fight.or(o.twins).or(o.bases)
}

/// Faces in one plane over each other, by whether they show.
#[derive(Default, Debug)]
pub struct Overlaps {
    /// One face over a different one: the picture flickers between them.
    pub fight: Option<(usize, f32, Vec3)>,
    /// Between two parts, the same triangle in both (a slab built into two neighbouring chunks): nothing to see where
    /// they look alike, since either draws the same.
    pub twins: Option<(usize, f32, Vec3)>,
    /// Between two parts, faces looking straight down in the plane both parts stand on (two boxes on one floor, one
    /// set into the other): their undersides, which the floor hides.
    pub bases: Option<(usize, f32, Vec3)>,
}

/// Corners this close (metres) are one corner.
const CORNER: f32 = 0.0005;

fn twin(a: &Face, b: &Face) -> bool {
    a.tri.iter().all(|p| b.tri.iter().any(|q| p.distance(*q) <= CORNER))
}

/// The lowest corner of a part's faces.
fn low(faces: &[Face]) -> f32 {
    let mut y = f32::INFINITY;
    for f in faces {
        for p in &f.tri {
            y = y.min(p.y);
        }
    }
    y
}

/// As `overlaps`, with the pairs that don't show kept apart (between two parts only: inside one model every pair counts).
pub fn sorted(a: &[[Vec3; 3]], b: Option<&[[Vec3; 3]]>) -> Overlaps {
    if b.is_none() && a.len() > SELF_MAX {
        return Overlaps::default();
    }
    let fa: Vec<Face> = a.iter().filter_map(face).collect();
    let fb: Vec<Face> = b.map(|b| b.iter().filter_map(face).collect()).unwrap_or_default();
    let others = if b.is_some() { &fb } else { &fa };
    let (low_a, low_b) = (low(&fa), low(&fb));
    let mut buckets: HashMap<Key, Vec<usize>> = HashMap::new();
    for (i, f) in others.iter().enumerate() {
        buckets.entry(key(f.n, f.d)).or_default().push(i);
    }
    let mut out = Overlaps::default();
    // the largest patch each kind has seen, for where to point
    let mut most = [0.0f32; 3];
    let mut add = |kind: usize, to: &mut Option<(usize, f32, Vec3)>, f: &Face, g: &Face| {
        let (area, mid) = shared(f, g);
        if area <= 0.0 {
            return;
        }
        let e = to.get_or_insert((0, 0.0, mid));
        e.0 += 1;
        e.1 += area;
        if area > most[kind] {
            most[kind] = area;
            e.2 = mid;
        }
    };
    for (i, f) in fa.iter().enumerate() {
        let mut seen: Vec<usize> = Vec::new();
        for k in near(key(f.n, f.d)) {
            for &j in buckets.get(&k).into_iter().flatten() {
                // within one model, each pair once
                if (b.is_none() && j <= i) || seen.contains(&j) {
                    continue;
                }
                seen.push(j);
                let g = &others[j];
                if same_plane(f, g) && overlap(f, g) {
                    let y = f.tri[0].y;
                    let base = b.is_some() && f.n.y <= -0.999 && (y - low_a).abs() <= 0.001 && (y - low_b).abs() <= 0.001;
                    if base {
                        add(0, &mut out.bases, f, g);
                    } else if b.is_some() && twin(f, g) {
                        add(1, &mut out.twins, f, g);
                    } else {
                        add(2, &mut out.fight, f, g);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(y: f32, x0: f32, x1: f32, up: bool) -> Vec<[Vec3; 3]> {
        let (a, b, c, d) = (Vec3::new(x0, y, 0.0), Vec3::new(x1, y, 0.0), Vec3::new(x1, y, 1.0), Vec3::new(x0, y, 1.0));
        if up { vec![[a, c, b], [a, d, c]] } else { vec![[a, b, c], [a, c, d]] }
    }

    #[test]
    fn faces_in_one_plane_over_each_other_fight_and_neighbours_do_not() {
        // a floor's top and a rug's top in the same plane, facing up: they fight
        let floor = quad(0.0, -1.0, 1.0, true);
        let rug = quad(0.0001, 0.0, 0.5, true);
        let (n, area, _) = overlaps(&rug, Some(&floor)).expect("the rug's top fights the floor's");
        // the rug's whole top, 0.5 m², and no more (the floor's triangles are bigger); pointed at inside the rug
        let at = overlaps(&rug, Some(&floor)).unwrap().2;
        assert!(n >= 2 && (area - 0.5).abs() < 1e-4 && at.x > 0.0 && at.x < 0.5 && at.z > 0.0 && at.z < 1.0, "{n} {area} {at}");
        // a strip 2 mm wide along a metre: 20 cm²
        let (_, strip, _) = overlaps(&quad(0.0, 0.998, 2.0, true), Some(&floor)).expect("a strip over the floor's edge");
        assert!((strip - 0.002).abs() < 1e-5, "{strip}");
        // 1 cm above it, it doesn't
        assert!(overlaps(&quad(0.01, 0.0, 0.5, true), Some(&floor)).is_none());
        // back to back (a box resting on another: one top up, the other's bottom down) doesn't fight
        assert!(overlaps(&quad(0.0, 0.0, 0.5, false), Some(&floor)).is_none());
        // side by side in one plane, meeting at an edge, doesn't
        assert!(overlaps(&quad(0.0, 1.0, 2.0, true), Some(&floor)).is_none());
        // a model with one face doubled flickers inside itself; a plain quad doesn't
        let mut doubled = quad(0.5, 0.0, 1.0, true);
        doubled.push(doubled[0]);
        assert_eq!(overlaps(&doubled, None).map(|o| o.0), Some(1));
        assert!(overlaps(&quad(0.5, 0.0, 1.0, true), None).is_none());
    }

    #[test]
    fn the_same_faces_in_two_parts_and_undersides_on_one_floor_are_kept_apart_from_a_fight() {
        // a slab built into two chunks: the same triangles in both
        let slab = quad(8.8, 0.0, 4.0, true);
        let o = sorted(&slab, Some(&slab));
        assert!(o.fight.is_none() && o.twins.map(|t| t.0) == Some(2), "{o:?}");
        // a rug's top in the floor's is a different face over it: a fight
        let o = sorted(&quad(0.0001, 0.0, 0.5, true), Some(&quad(0.0, -1.0, 1.0, true)));
        assert!(o.fight.is_some() && o.twins.is_none() && o.bases.is_none(), "{o:?}");
        // a door set into a wall, both standing on the floor: their undersides overlap in the floor's plane
        let mut wall = quad(0.0, -1.0, 1.0, false);
        wall.extend(quad(2.0, -1.0, 1.0, true));
        let mut door = quad(0.0, -0.4, 0.4, false);
        door.extend(quad(1.0, -0.4, 0.4, true));
        let o = sorted(&door, Some(&wall));
        assert!(o.fight.is_none() && o.bases.is_some(), "{o:?}");
        // the same underside held up off the ground (a soffit under a beam's) still fights
        let mut beam = quad(2.0, -1.0, 1.0, false);
        beam.extend(quad(0.0, -1.0, 1.0, true));
        let mut soffit = quad(2.0, -0.4, 0.4, false);
        soffit.extend(quad(1.5, -0.4, 0.4, true));
        assert!(sorted(&soffit, Some(&beam)).fight.is_some());
    }
}
