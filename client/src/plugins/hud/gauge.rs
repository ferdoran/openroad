//! The one `CIFGauge` recipe, exported so nobody re-derives it from a comment.
//!
//! **Idea.** A vanilla `CIFGauge` with `Style=0` draws its art at 1:1
//! texel-to-pixel scale and expresses "fill" by *cropping* along X — it never
//! resizes the bitmap. `docs/re/ui/hp-mp-gauge-widget.md` §3b/§3c proves this
//! from PK2 pixels alone: `delete_time_gauge.ddj` (280x8) carries 7 bands of
//! exactly 40 texels and its overlay's 6 engraved ticks sit pixel-exactly on
//! those band boundaries, an alignment only a 1:1 draw survives. Corpus-wide,
//! 33 of 59 de-duplicated `CIFGauge` arts equal their rect's exact pixel size
//! and 17 more declare `w=h=0` (sized from the texture).
//!
//! So a gauge is **three** nodes, not two, and the middle one is the whole
//! point (#630):
//!
//! ```text
//! track  authored rect, Overflow::clip()   <- never moves
//!  crop  plain Node, width = fill px, clip <- the ONLY node the fill drives
//!   art  ImageNode at the art's native px  <- never resized, so never squashed
//! ```
//!
//! Writing the percentage onto the `ImageNode` instead (what four of our seven
//! gauges did) stretches the bitmap: `tw_hp.ddj`'s 1-texel rounded corners then
//! ride the fill front at every HP value, and `ub_sp_bar`'s 2-px border columns
//! travel inward as SP drains.

use bevy::prelude::*;

/// Edge the fill grows from. `[S]` — settled, but behind one name so a future
/// screenshot flips every site in a single edit: the gold->red ramp runs left
/// to right and `HAlign=2` appears on **no** `CIFGauge` in the corpus, so the
/// crop is left-anchored (`docs/re/ui/hp-mp-gauge-widget.md` §3c).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GaugeAnchor {
    Left,
    Right,
}

/// The anchor every gauge uses. One constant, four call sites.
pub const GAUGE_ANCHOR: GaugeAnchor = GaugeAnchor::Left;

/// Fill width in *whole pixels* for `fraction` of a `track_w`-px-wide gauge.
///
/// Rounding is a **choice, not a finding**: static data cannot say whether the
/// original floors, rounds or ceils `f * rect.w` (`hp-mp-gauge-widget.md` §7
/// leaves it `[U]`). We round, so a gauge reads full at 99.6 % and empty only
/// at true zero, and we do it in one place so all sites agree.
pub fn gauge_fill_width(fraction: f32, track_w: f32) -> Val {
    Val::Px((fraction.clamp(0.0, 1.0) * track_w).round())
}

/// The art node of a gauge built by [`gauge`], so a site that repaints its art
/// (the target window swaps monster/NPC bars) can reach the `ImageNode`
/// without owning the recipe's node layout.
#[derive(Component, Default, Clone)]
pub struct GaugeArt;

/// The gauge as a BSN scene, for the `bsn!`-built windows: the returned scene's
/// root **is** the crop node, so a site adds its own fill marker next to it
/// (`MiniInfoHpFill gauge(..)`), exactly the way `label()` is used.
pub fn gauge(art: Handle<Image>, width: f32, height: f32) -> impl Scene {
    let (left, right) = anchor_edges();
    let fill = gauge_fill_width(1.0, width);
    bsn! {
        Node {
            position_type: PositionType::Absolute,
            left: {left},
            right: {right},
            top: px(0.0),
            width: {fill},
            height: px(height),
            overflow: {Overflow::clip()},
        }
        Pickable::IGNORE
        Children [
            (
                GaugeArt
                ImageNode { image: {art}, image_mode: NodeImageMode::Stretch }
                Node {
                    position_type: PositionType::Absolute,
                    left: {left},
                    right: {right},
                    top: px(0.0),
                    width: px(width),
                    height: px(height),
                }
                Pickable::IGNORE
            ),
        ]
    }
}

/// `(left, right)` for the anchored edge — one of the two is `Val::Auto`.
fn anchor_edges() -> (Val, Val) {
    match GAUGE_ANCHOR {
        GaugeAnchor::Left => (Val::Px(0.0), Val::Auto),
        GaugeAnchor::Right => (Val::Auto, Val::Px(0.0)),
    }
}

/// Middle node: carries the fill width, clips the art. Sized in px (not
/// percent) because the crop is a texel count, and `gauge_fill_width` is where
/// that count is rounded.
pub fn gauge_crop_node(width: Val, height: f32) -> Node {
    let (left, right) = anchor_edges();
    Node {
        position_type: PositionType::Absolute,
        left,
        right,
        top: Val::Px(0.0),
        width,
        height: Val::Px(height),
        overflow: Overflow::clip(),
        ..default()
    }
}

/// Inner node: the art at its native size, absolutely placed at the crop's
/// anchored edge so cropping moves the *window*, never the bitmap.
pub fn gauge_art_node(width: f32, height: f32) -> Node {
    let (left, right) = anchor_edges();
    Node {
        position_type: PositionType::Absolute,
        left,
        right,
        top: Val::Px(0.0),
        width: Val::Px(width),
        height: Val::Px(height),
        ..default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The invariant this whole module exists for: the art node's size is a
    /// constant of the gauge, and only the crop node's width moves.
    #[test]
    fn only_the_crop_node_carries_the_fill() {
        let (w, h) = (144.0, 8.0);
        let art = gauge_art_node(w, h);
        assert_eq!(art.width, Val::Px(w));
        assert_eq!(art.height, Val::Px(h));

        let half = gauge_crop_node(gauge_fill_width(0.5, w), h);
        let full = gauge_crop_node(gauge_fill_width(1.0, w), h);
        assert_eq!(half.width, Val::Px(72.0));
        assert_eq!(full.width, Val::Px(144.0));
        // ...and the art is unchanged by either.
        assert_eq!(gauge_art_node(w, h).width, art.width);
    }

    /// A crop that does not clip is a no-op: the art is exactly as wide as the
    /// track, so without this the bar would always read full.
    #[test]
    fn the_crop_node_clips() {
        let crop = gauge_crop_node(gauge_fill_width(0.25, 100.0), 8.0);
        assert_eq!(crop.overflow, Overflow::clip());
        assert_eq!(crop.width, Val::Px(25.0));
    }

    /// Rounding is centralised and clamped (see `gauge_fill_width`).
    #[test]
    fn fill_width_rounds_and_clamps() {
        assert_eq!(gauge_fill_width(0.996, 124.0), Val::Px(124.0));
        assert_eq!(gauge_fill_width(0.5, 125.0), Val::Px(63.0)); // 62.5 -> 63
        assert_eq!(gauge_fill_width(0.0, 124.0), Val::Px(0.0));
        assert_eq!(gauge_fill_width(-1.0, 124.0), Val::Px(0.0));
        assert_eq!(gauge_fill_width(2.0, 124.0), Val::Px(124.0));
    }

    /// Left-anchored today; the constant is the single place that flips it.
    #[test]
    fn the_gauge_is_left_anchored() {
        assert_eq!(GAUGE_ANCHOR, GaugeAnchor::Left);
        assert_eq!(gauge_art_node(10.0, 2.0).left, Val::Px(0.0));
        assert_eq!(gauge_crop_node(Val::Px(5.0), 2.0).left, Val::Px(0.0));
    }
}
