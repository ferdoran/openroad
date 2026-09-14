//! The `ub_new_wnd_` popup menu — one widget, several call sites.
//!
//! Idea: the target right-click menu (`res_ui/targetmenu.2dt`) and the
//! under-bar's Menu flyout (`res_ui/nifundermenubar.2dt`) are the *same*
//! construction at two sizes — a `CNIFrame` on `interface\frame\ub_new_wnd_`, a
//! `CNIFNormaltile` of `com_bg_tile_u.ddj` at inset 20, and rows of
//! `ub_new_menu_button.ddj`. The recovered client module map homes the
//! construction to `NIFUnderMenuBar.cpp` and contains no `NIFTargetMenu.cpp`,
//! so the target menu is an instance of that popup, not a class of its own
//! (`docs/re/ui/hud-target-menu.md` §3.3, `[S]`).
//!
//! The geometry law that makes one widget serve both: **every one of the eight
//! `ub_new_wnd_` pieces is exactly 20x20 and there is no centre piece**, so the
//! client area is the frame rect deflated by 20 on all four sides. That
//! reproduces both authored fills byte-for-byte — `638,321,102,142` →
//! `658,341,62,102` (target menu) and `758,44,138,403` → `778,64,98,363`
//! (under-bar) — which is why the panel here is *sized from its content*
//! instead of carrying a hardcoded 142.
//!
//! Deviation, stated: the original's row pitch alternates 20/19/20/19 (mean
//! 19.5, not expressible as an integer step) and its rows overhang the fill by
//! 12px on each side. This widget uses a uniform 20px pitch and keeps the rows
//! inside the client area. The overhang appears in *both* authored instances, so
//! it is the family's idiom rather than a defect — revisit it on a screenshot
//! (`docs/re/ui/hud-target-menu.md` §9-U2).

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::Button;

use crate::assets::FontAssets;
use crate::plugins::hud::game_window::abs_node;
use crate::plugins::ui_v2::style::ImageButtonStyle;

/// Every `ub_new_wnd_` piece is 20x20, so the border is one piece wide.
pub const PIECE: f32 = 20.0;
/// `ub_new_menu_button.ddj` is 124x20 art; both authored instances use it at
/// 20px high (86 wide here, 97 in the under-bar).
pub const ROW_H: f32 = 20.0;
/// Row box, frame-local: `646,353` against a root at `638,321`.
const ROW_X: f32 = 8.0;
const ROW_W: f32 = 86.0;
/// First row top, frame-local (`353 - 321`) — the border plus the header plate.
const ROWS_TOP: f32 = 32.0;
/// Frame-local header plate (`643,327,93,20`), `exc_box.ddj` borrowed from
/// `interface/exchange/` — there is no header art under `interface/underbar/`.
const HEADER: (f32, f32, f32, f32) = (5.0, 6.0, 93.0, 20.0);
/// What is left below the last row in the authored 5-row instance
/// (`142 - 32 - 5*20`). One sample, so it is a measurement, not a law.
const BOTTOM_PAD: f32 = 10.0;
/// Authored width of the target menu (`638,321,102,142`).
pub const MENU_W: f32 = 102.0;

/// `Color = 0xFFE5B861` on all five buttons — the only tinted records in the
/// file; frame, fill and header are default white. Contrast on the fill tile is
/// 11.0:1 (AAA), measured over the tile's 400 opaque pixels.
const ROW_TEXT: Color = Color::srgb_u8(229, 184, 97);
const ROW_FONT: f32 = 9.0;

const FRAME_DIR: &str = "media://interface/frame/ub_new_wnd_";
const BG_TILE: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_u.ddj";
const HEADER_DDJ: &str = "media://interface/exchange/exc_box.ddj";
const ROW_STEM: &str = "media://interface/underbar/ub_new_menu_button";

#[derive(Component)]
pub struct ContextMenuRoot;

/// Which surface owns the one open popup.
///
/// There is a single popup and several surfaces that raise it, and every one of
/// them polls the *same* [`ContextMenuRow`] components on left-release. So a
/// surface that still believed itself open would read a click meant for
/// another's rows — the quick-party board firing a Banish against the target
/// menu's list, say.
///
/// This is the arbiter. An opener writes its own variant, which revokes every
/// other surface by construction, and a poller reads only when the variant is
/// its own. That replaces the earlier arrangement where each opener had to
/// remember to clear each *other* surface's claim by hand — which already had a
/// hole in it ([`super::target_menu::open_target_menu`] never cleared the quick
/// board's) and would have grown one hole per surface added.
///
/// Each surface keeps its own payload (a target entity, a member JID): this
/// says *who* owns the popup, not *what* they will do with it. A stale payload
/// under a revoked claim is harmless, because that surface's poller no longer
/// runs.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ContextMenuOwner {
    #[default]
    None,
    Target,
    QuickParty,
    PartyRoster,
}

/// The row's index in the list the menu was built from — the caller maps it
/// back to its own action. The descriptor's ids are deliberately not carried:
/// in `targetmenu.2dt` they are neither in visual nor in record order (id 17
/// renders above id 16), so an id-keyed row would render the menu wrong.
#[derive(Component, Clone, Copy)]
pub struct ContextMenuRow(pub usize);

/// One row: its already-localized label and whether it can be picked.
pub struct ContextMenuItem {
    pub label: String,
    pub enabled: bool,
}

/// Height of a menu with `rows` rows — the authored `142` for five rows.
pub fn menu_height(rows: usize) -> f32 {
    ROWS_TOP + rows as f32 * ROW_H + BOTTOM_PAD
}

/// Spawn the popup at `left`/`top` (with `margin_left` for centre-relative
/// anchoring) and return its root. Rows carry [`ContextMenuRow`]; the caller
/// attaches its own observers or reads the component.
pub fn spawn_context_menu(
    commands: &mut Commands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    camera: Entity,
    left: Val,
    top: Val,
    margin_left: Val,
    items: &[ContextMenuItem],
    s: f32,
) -> Entity {
    let (fw, fh) = (MENU_W, menu_height(items.len()));
    let root = commands
        .spawn((
            ContextMenuRoot,
            Name::from("Context Menu"),
            Node {
                position_type: PositionType::Absolute,
                left,
                top,
                margin: UiRect::left(margin_left),
                width: Val::Px(fw * s),
                height: Val::Px(fh * s),
                ..default()
            },
            GlobalZIndex(80),
            UiTargetCamera(camera),
        ))
        .id();

    commands.entity(root).with_children(|popup| {
        // the 8-piece ring; mids and sides overlap their neighbours by one unit
        // so a scaled position cannot open a seam (same as the under-bar's).
        let p = PIECE;
        for (rect, piece) in [
            ((0.0, 0.0, p, p), "left_up"),
            ((p - 1.0, 0.0, fw - 2.0 * p + 2.0, p), "mid_up"),
            ((fw - p, 0.0, p, p), "right_up"),
            ((0.0, p - 1.0, p, fh - 2.0 * p + 2.0), "left_side"),
            ((fw - p, p - 1.0, p, fh - 2.0 * p + 2.0), "right_side"),
            ((0.0, fh - p, p, p), "left_down"),
            ((p - 1.0, fh - p, fw - 2.0 * p + 2.0, p), "mid_down"),
            ((fw - p, fh - p, p, p), "right_down"),
        ] {
            popup.spawn((
                abs_node(rect, s),
                ImageNode {
                    image: asset_server.load(format!("{FRAME_DIR}{piece}.ddj")),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ));
        }

        // client area = the frame deflated by one piece
        popup.spawn((
            abs_node((p, p, fw - 2.0 * p, fh - 2.0 * p), s),
            ImageNode {
                image: asset_server.load(BG_TILE),
                image_mode: NodeImageMode::Tiled {
                    tile_x: true,
                    tile_y: true,
                    stretch_value: s,
                },
                ..default()
            },
            Pickable::IGNORE,
        ));

        popup.spawn((
            abs_node(HEADER, s),
            ImageNode {
                image: asset_server.load(HEADER_DDJ),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));

        for (index, item) in items.iter().enumerate() {
            let y = ROWS_TOP + index as f32 * ROW_H;
            let style = ImageButtonStyle {
                normal: asset_server.load(format!("{ROW_STEM}.ddj")),
                hover: asset_server.load(format!("{ROW_STEM}_focus.ddj")),
                press: asset_server.load(format!("{ROW_STEM}_press.ddj")),
                ..Default::default()
            };
            let mut row = popup.spawn((
                ContextMenuRow(index),
                abs_node((ROW_X, y, ROW_W, ROW_H), s),
                ImageNode {
                    image: if item.enabled {
                        style.normal.clone()
                    } else {
                        asset_server.load(format!("{ROW_STEM}_disable.ddj"))
                    },
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
            ));
            if item.enabled {
                row.insert((Button, Hovered::default(), style));
            } else {
                row.insert(Pickable::IGNORE);
            }
            popup.spawn((
                Text::new(item.label.clone()),
                TextFont {
                    font: fonts.nine.clone().into(),
                    font_size: FontSize::Px(ROW_FONT * s),
                    ..default()
                },
                TextColor(ROW_TEXT),
                TextLayout::justify(Justify::Center),
                abs_node((ROW_X, y + 5.0, ROW_W, ROW_H - 5.0), s),
                Pickable::IGNORE,
            ));
        }
    });
    root
}

/// Close every open context menu.
/// Despawn the popup and release its ownership.
///
/// The owner is taken by `&mut` rather than left to the caller so that closing
/// and disowning cannot come apart: a menu that is gone from the screen but
/// still owned would keep its poller live against rows that no longer exist.
pub fn close_context_menus(
    commands: &mut Commands,
    roots: &Query<Entity, With<ContextMenuRoot>>,
    owner: &mut ContextMenuOwner,
) {
    for root in roots.iter() {
        commands.entity(root).despawn();
    }
    *owner = ContextMenuOwner::None;
}

#[cfg(test)]
mod test {
    use super::*;

    /// The 20px law: the client area of both authored instances must fall out
    /// of `rect.deflate(20)` — that is what lets one widget serve both sizes.
    #[test]
    fn the_client_area_is_the_frame_deflated_by_one_piece() {
        let deflate = |(x, y, w, h): (f32, f32, f32, f32)| {
            (x + PIECE, y + PIECE, w - 2.0 * PIECE, h - 2.0 * PIECE)
        };
        // targetmenu.2dt: root 638,321,102,142 -> com_bg_tile_u 658,341,62,102
        assert_eq!(
            deflate((638.0, 321.0, 102.0, 142.0)),
            (658.0, 341.0, 62.0, 102.0)
        );
        // nifundermenubar.2dt: 758,44,138,403 -> 778,64,98,363
        assert_eq!(
            deflate((758.0, 44.0, 138.0, 403.0)),
            (778.0, 64.0, 98.0, 363.0)
        );
    }

    /// The authored 5-row target menu is 102x142; sizing from content has to
    /// reproduce that exactly, or the panel is not derivable and the constant
    /// would have to be transcribed.
    #[test]
    fn five_rows_reproduce_the_authored_height() {
        assert_eq!(MENU_W, 102.0);
        assert_eq!(menu_height(5), 142.0);
        // and it grows by exactly one row pitch
        assert_eq!(menu_height(6) - menu_height(5), ROW_H);
        // the under-bar's 14 rows would still fit the same construction
        assert!(menu_height(14) > menu_height(5));
    }
}
#[cfg(test)]
mod tests {
    /// Successor to `the_live_context_menu_stays` (#56-C): this widget moved
    /// out of the deleted `plugins::ui` because it is **live**, unlike the two
    /// subtrees that shared that module with it. The guard therefore follows it
    /// here and points at the consumer — if the target menu stops using it, the
    /// widget is dead and should be deleted deliberately, not left behind.
    #[test]
    fn the_context_menu_still_has_its_consumer() {
        let target_menu = include_str!("target_menu.rs");
        assert!(
            target_menu.contains("context_menu::"),
            "hud::target_menu no longer builds on this widget — decide whether \
             it is dead rather than leaving it in place"
        );
    }
}
