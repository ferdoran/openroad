//! Edge-crossing tests for nav mesh movement.
//!
//! Blocking is decided in 2D: a movement step is a segment, a wall is an edge,
//! and the step is blocked when the two properly cross.
//!
//! The flag byte's two block bits name a direction relative to the edge's own
//! `src`->`dst` sense. Acting on that direction turns out to buy nothing here,
//! and the attempt actively broke movement. Measured over the whole v1.188
//! corpus:
//!
//! - All 1,586,920 blocked terrain edges carry `BlockSrc2Dst` only, and all of
//!   them have `assoc_cell.1 == -1`. A blocked terrain edge separates a
//!   walkable cell from *void*; there is no far side to stand on, so the mover
//!   is always on the `src` side and the directional answer is by construction
//!   the same as [`NavMeshEdgeFlag::is_blocked`].
//! - Object nav meshes have no one-way outline edges at all (all 71,395 blocked
//!   ones are blocked both ways), and 26 one-way inline edges out of 209,000.
//!
//! Blocking *at this level* is therefore the plain, non-directional
//! [`NavMeshEdgeFlag::is_blocked`] — the two block bits only.
//!
//! The `Underpass` bit is deliberately *not* handled here. It is a directional
//! boundary — "passthrough from outside, blocked from inside" — and this module
//! has no notion of which side is "inside". Its one legitimate use, holding a
//! mover on the elevated surface it stands on (a bridge railing, a stair side),
//! is applied one level up in [`object_crossing`](super::NavMeshRaycast), where
//! the mover is known to be inside the object's outline so the direction is
//! fixed. See `docs/formats/nvm-jmxvnvm.md`. (An earlier revision *did* treat
//! `Underpass` as an unconditional wall here; blocking it in both directions
//! also killed the outside→in passthrough and trapped movers — hence keeping it
//! out of the non-directional test.)
//!
//! The module is deliberately free of ECS and asset types so it can be tested
//! without PK2 data.

use bevy::math::Vec2;

use crate::assets::nvm::nav_mesh_edge_flag::NavMeshEdgeFlag;

/// One nav edge reduced to what a blocking test needs, in whichever 2D frame
/// the caller works in — region-local for terrain edges, object-local for
/// object edges. Mixing frames within one call is a bug.
pub struct EdgeProbe<'a> {
    pub src: Vec2,
    pub dst: Vec2,
    pub flag: &'a NavMeshEdgeFlag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeCross {
    Blocked,
    Passable,
}

/// Which side of the directed line `src`→`dst` the point `p` lies on, as a
/// signed area: positive to the left, negative to the right, ~0 on the line.
pub fn side_of(src: Vec2, dst: Vec2, p: Vec2) -> f32 {
    (dst.x - src.x) * (p.y - src.y) - (dst.y - src.y) * (p.x - src.x)
}

/// Does moving `a`→`b` cross `edge`, and if so, is that crossing blocked?
/// `None` means the step doesn't touch this edge at all.
pub fn classify_crossing(a: Vec2, b: Vec2, edge: &EdgeProbe) -> Option<EdgeCross> {
    if !segments_intersect(a, b, edge.src, edge.dst) {
        return None;
    }
    Some(if edge.flag.is_blocked() {
        EdgeCross::Blocked
    } else {
        EdgeCross::Passable
    })
}

/// Do the 2D segments `p1p2` and `p3p4` properly cross? Uses the signed-area
/// (orientation) test on both endpoint pairs. Collinear/just-touching cases are
/// treated as non-crossing, which is fine for blocking walls.
pub fn segments_intersect(p1: Vec2, p2: Vec2, p3: Vec2, p4: Vec2) -> bool {
    let d1 = side_of(p3, p4, p1);
    let d2 = side_of(p3, p4, p2);
    let d3 = side_of(p1, p2, p3);
    let d4 = side_of(p1, p2, p4);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flag bits, per the table on `NavMeshEdgeFlag`.
    const BLOCK_DST_TO_SRC: u8 = 1;
    const BLOCK_SRC_TO_DST: u8 = 2;
    const BLOCKED: u8 = 3;
    const GLOBAL: u8 = 8;
    const UNDERPASS: u8 = 16;

    /// A vertical edge along x = 0, running from y = -10 up to y = +10, so its
    /// left half-plane is x < 0. Every test below crosses it horizontally.
    fn probe(flag: &NavMeshEdgeFlag) -> EdgeProbe<'_> {
        EdgeProbe {
            src: Vec2::new(0.0, -10.0),
            dst: Vec2::new(0.0, 10.0),
            flag,
        }
    }

    /// A step crossing the edge left→right, and its mirror image.
    const FROM_LEFT: (Vec2, Vec2) = (Vec2::new(-5.0, 0.0), Vec2::new(5.0, 0.0));
    const FROM_RIGHT: (Vec2, Vec2) = (Vec2::new(5.0, 0.0), Vec2::new(-5.0, 0.0));

    fn cross(flag: u8, step: (Vec2, Vec2)) -> Option<EdgeCross> {
        let flag = NavMeshEdgeFlag(flag);
        classify_crossing(step.0, step.1, &probe(&flag))
    }

    #[test]
    fn side_of_signs() {
        // Edge points "up" (+y), so its left side is -x.
        let (src, dst) = (Vec2::new(0.0, -10.0), Vec2::new(0.0, 10.0));
        assert!(side_of(src, dst, Vec2::new(-1.0, 0.0)) > 0.0);
        assert!(side_of(src, dst, Vec2::new(1.0, 0.0)) < 0.0);
        assert_eq!(side_of(src, dst, Vec2::new(0.0, 5.0)), 0.0);
    }

    #[test]
    fn side_of_is_antisymmetric() {
        let (src, dst) = (Vec2::new(1.0, 2.0), Vec2::new(4.0, -3.0));
        let p = Vec2::new(-2.0, 7.0);
        assert_eq!(side_of(src, dst, p), -side_of(dst, src, p));
    }

    #[test]
    fn no_crossing_returns_none() {
        let flag = NavMeshEdgeFlag(BLOCKED);
        // Parallel to the edge, well clear of it.
        assert_eq!(
            classify_crossing(Vec2::new(-5.0, -5.0), Vec2::new(-5.0, 5.0), &probe(&flag)),
            None
        );
    }

    #[test]
    fn blocked_edges_stop_both_directions() {
        assert_eq!(cross(BLOCKED, FROM_LEFT), Some(EdgeCross::Blocked));
        assert_eq!(cross(BLOCKED, FROM_RIGHT), Some(EdgeCross::Blocked));
    }

    #[test]
    fn passable_edge_crosses() {
        assert_eq!(cross(GLOBAL, FROM_LEFT), Some(EdgeCross::Passable));
        assert_eq!(cross(0, FROM_RIGHT), Some(EdgeCross::Passable));
    }

    /// Either block bit alone blocks both ways.
    ///
    /// This is the shape the data actually has: every blocked terrain edge
    /// carries `BlockSrc2Dst` alone with no cell on its far side, so the mover
    /// is always on the blocked side and a directional reading would give the
    /// same answer — while getting the polarity wrong turns every wall in the
    /// world into a one-way membrane.
    #[test]
    fn a_single_block_bit_still_blocks_both_ways() {
        for flag in [BLOCK_SRC_TO_DST, BLOCK_DST_TO_SRC] {
            assert_eq!(cross(flag, FROM_LEFT), Some(EdgeCross::Blocked));
            assert_eq!(cross(flag, FROM_RIGHT), Some(EdgeCross::Blocked));
        }
    }

    /// Underpass is not a blocking flag. All 626 Underpass outline edges in the
    /// data carry no block bits; treating the flag as a wall trapped movers
    /// inside buildings and trees.
    #[test]
    fn underpass_alone_does_not_block() {
        assert_eq!(cross(UNDERPASS, FROM_LEFT), Some(EdgeCross::Passable));
        assert_eq!(cross(UNDERPASS, FROM_RIGHT), Some(EdgeCross::Passable));
    }

    /// ...but an Underpass edge that *does* carry block bits still blocks.
    #[test]
    fn underpass_with_block_bits_still_blocks() {
        assert_eq!(
            cross(UNDERPASS | BLOCKED, FROM_LEFT),
            Some(EdgeCross::Blocked)
        );
    }

    #[test]
    fn zero_length_edge_never_crosses() {
        let flag = NavMeshEdgeFlag(BLOCKED);
        let p = EdgeProbe {
            src: Vec2::ZERO,
            dst: Vec2::ZERO,
            flag: &flag,
        };
        assert_eq!(classify_crossing(FROM_LEFT.0, FROM_LEFT.1, &p), None);
    }

    #[test]
    fn zero_length_step_never_crosses() {
        assert_eq!(cross(BLOCKED, (Vec2::ZERO, Vec2::ZERO)), None);
    }

    /// Touching an endpoint is documented as non-crossing; movers sliding along
    /// a wall must not be stopped by it.
    #[test]
    fn touching_an_endpoint_is_not_a_crossing() {
        // Step ends exactly on the edge.
        assert_eq!(
            cross(BLOCKED, (Vec2::new(-5.0, 0.0), Vec2::new(0.0, 0.0))),
            None
        );
        // Step passes through the edge's own endpoint.
        assert_eq!(
            cross(BLOCKED, (Vec2::new(-5.0, 10.0), Vec2::new(5.0, 10.0))),
            None
        );
    }
}
