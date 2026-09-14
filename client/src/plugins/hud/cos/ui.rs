//! COS (companion) window shell + its three-page host.
//!
//! Idea: this is step 1 of `docs/re/ui/cos-pet-window.md` §8 — the vanilla
//! shell and the frame that hosts its pages, with the pages themselves left as
//! empty containers for the follow-up work. Geometry is transcribed from the
//! user's own resinfo: the shell is `GDR_COS_WND:CIFCOS`
//! (`resinfo/ginterface.txt:1330`, id 120, `Rect="0,0,355,390"`,
//! `DDJ="interface\frame\mframe_wnd_"`, `Text=""` — a blank-caption
//! `mframe_wnd_` window, as `docs/re/ui/hud-game-window-chrome.md:62` lists
//! it), so it rides on the shared `game_window` chrome. Inside it,
//! `GDR_COS_IFRAME:CIFFrame` (`resinfo/ifcos.txt`, `int_window_` art) and all
//! three page controls share the single rect `12,66,331,314`; those window
//! rects are rebased into the shell's content space by subtracting its content
//! origin `(FRAME_VIS_SIDE + CHROME_PAD, CONTENT_TOP)` = `(12, 36)`, the same
//! way `character_info` does (never re-derive that origin from vanilla's
//! interior art, see #310).
//!
//! Two things are deliberately *not* invented here. The pages carry no tab
//! strip: `ifcos.txt` declares no tab control and `ifcoscommand.txt` is a
//! two-block stub whose bar the original assembles in code, so page selection
//! is a code-side `CosPage` (`[U]` in the doc §9) rather than art we made up.
//! And the window's default position is `[U]` — `wndpos.dat` window[5] persists
//! it per user (`docs/formats/wndpos.md:21,44`) and openroad has no wndpos
//! parser yet, so the spawn anchor below is ours, not vanilla's.

use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use crate::assets::FontAssets;
use crate::plugins::hud::cos::info;
use crate::plugins::hud::cos::inventory;
use crate::plugins::hud::cos::model::{CosPage, CosWindowState};
use crate::plugins::hud::cos::setup;
use crate::plugins::hud::game_window::{self, abs_node};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::textdata::ClientUiStrings;

// --- Layout constants (resinfo, window units) -------------------------------

/// Vanilla shell rect (`ginterface.txt:1330` `Rect="0,0,355,390"`).
const WINDOW_SIZE: (f32, f32) = (355.0, 390.0);
/// Content box derived from that rect, so the chrome's outer box *is* the
/// vanilla shell: the frame's visible margins and pad are subtracted rather
/// than a content size being picked (331x338 today).
const CONTENT_W: f32 =
    WINDOW_SIZE.0 - 2.0 * (game_window::FRAME_VIS_SIDE + game_window::CHROME_PAD);
const CONTENT_H: f32 = WINDOW_SIZE.1
    - game_window::CONTENT_TOP
    - game_window::CHROME_PAD
    - game_window::FRAME_VIS_BOTTOM;

/// The rect shared by `GDR_COS_IFRAME` and all three page controls, in vanilla
/// window space (`ifcos.txt`).
const PAGE_RECT_WINDOW: (f32, f32, f32, f32) = (12.0, 66.0, 331.0, 314.0);
/// `int_window_*` frame pieces are 16px (as in inventory/skill window).
const FRAME_PIECE: f32 = 16.0;
const FRAME_DIR: &str = "media://interface/inventory/int_window_";

/// Spawn anchor (right/top, physical px). **Ours**, not vanilla's — see the
/// module note on `wndpos.dat`.
const WINDOW_RIGHT: f32 = 360.0;
const WINDOW_TOP: f32 = 80.0;

/// `PAGE_RECT_WINDOW` rebased into the shell's content space.
fn page_rect() -> (f32, f32, f32, f32) {
    let (x, y, w, h) = PAGE_RECT_WINDOW;
    (
        x - (game_window::FRAME_VIS_SIDE + game_window::CHROME_PAD),
        y - game_window::CONTENT_TOP,
        w,
        h,
    )
}

/// Whether `page` is the one the shell currently shows.
fn page_display(selected: CosPage, page: CosPage) -> Display {
    if selected == page {
        Display::Flex
    } else {
        Display::None
    }
}

// --- Markers ----------------------------------------------------------------

#[derive(Component)]
pub struct CosWindowRoot;

/// One of the three page containers; the follow-up pages fill these.
#[derive(Component)]
pub struct CosPageRoot(pub CosPage);

// --- Spawning ---------------------------------------------------------------

/// Spawn the (initially hidden) COS window.
pub fn spawn_cos_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    state: Res<CosWindowState>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Ok(camera) = cam_query.single() else {
        warn!("cos window: no 2d camera to attach to");
        return;
    };
    let s = hud_scale();

    // vanilla's caption is empty (`Text=STRING,""`), so the title band stays blank
    let window = game_window::spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        "",
        (CONTENT_W, CONTENT_H),
        (WINDOW_RIGHT, WINDOW_TOP),
        s,
    );
    commands
        .entity(window.root)
        .insert((CosWindowRoot, Name::from("COS Window"), GlobalZIndex(55)))
        .entry::<Node>()
        .and_modify(|mut node| node.display = Display::None);
    commands
        .entity(window.expect_close_button())
        .observe(on_close_button);

    let (px, py, pw, ph) = page_rect();
    commands.entity(window.content).with_children(|content| {
        // GDR_COS_IFRAME: the int_window_ ring around the page area
        let pieces = [
            ((0.0, 0.0, FRAME_PIECE, FRAME_PIECE), "left_up"),
            (
                (
                    FRAME_PIECE - 1.0,
                    0.0,
                    pw - 2.0 * FRAME_PIECE + 2.0,
                    FRAME_PIECE,
                ),
                "mid_up",
            ),
            (
                (pw - FRAME_PIECE, 0.0, FRAME_PIECE, FRAME_PIECE),
                "right_up",
            ),
            (
                (
                    0.0,
                    FRAME_PIECE - 1.0,
                    FRAME_PIECE,
                    ph - 2.0 * FRAME_PIECE + 2.0,
                ),
                "left_side",
            ),
            (
                (
                    pw - FRAME_PIECE,
                    FRAME_PIECE - 1.0,
                    FRAME_PIECE,
                    ph - 2.0 * FRAME_PIECE + 2.0,
                ),
                "right_side",
            ),
            (
                (0.0, ph - FRAME_PIECE, FRAME_PIECE, FRAME_PIECE),
                "left_down",
            ),
            (
                (
                    FRAME_PIECE - 1.0,
                    ph - FRAME_PIECE,
                    pw - 2.0 * FRAME_PIECE + 2.0,
                    FRAME_PIECE,
                ),
                "mid_down",
            ),
            (
                (pw - FRAME_PIECE, ph - FRAME_PIECE, FRAME_PIECE, FRAME_PIECE),
                "right_down",
            ),
        ];
        for ((x, y, w, h), piece) in pieces {
            content.spawn((
                abs_node((px + x, py + y, w, h), s),
                ImageNode {
                    image: asset_server.load(format!("{FRAME_DIR}{piece}.ddj")),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ));
        }

        // the three page containers, all on the one shared rect
        for page in [CosPage::Info, CosPage::Inventory, CosPage::Setup] {
            let mut node = abs_node((px, py, pw, ph), s);
            node.display = page_display(state.page, page);
            let mut container = content.spawn((CosPageRoot(page), node, Pickable::IGNORE));
            match page {
                CosPage::Info => {
                    container.with_children(|info_page| {
                        info::build_info_page(info_page, &asset_server, &fonts, &ui_strings, s);
                    });
                }
                CosPage::Inventory => {
                    container.with_children(|bag_page| {
                        inventory::build_inventory_page(bag_page, &asset_server, &fonts, s);
                    });
                }
                CosPage::Setup => {
                    container.with_children(|setup_page| {
                        setup::build_setup_page(setup_page, &asset_server, &fonts, &ui_strings, s);
                    });
                }
            }
        }
    });
}

pub fn cleanup_cos_window(mut commands: Commands, windows: Query<Entity, With<CosWindowRoot>>) {
    for entity in windows.iter() {
        commands.entity(entity).despawn();
    }
}

// --- Behavior ---------------------------------------------------------------

fn on_close_button(_: On<Activate>, mut state: ResMut<CosWindowState>) {
    state.open = false;
}

/// Mirror `CosWindowState` onto the shell and its pages.
pub fn apply_cos_visibility(
    state: Res<CosWindowState>,
    mut roots: Query<&mut Node, (With<CosWindowRoot>, Without<CosPageRoot>)>,
    mut pages: Query<(&CosPageRoot, &mut Node), Without<CosWindowRoot>>,
) {
    if !state.is_changed() {
        return;
    }
    for mut node in roots.iter_mut() {
        node.display = if state.open {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (page, mut node) in pages.iter_mut() {
        node.display = page_display(state.page, page.0);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The chrome's outer box must come out as vanilla's own shell rect
    /// (`ginterface.txt:1330` `Rect="0,0,355,390"`) — the content box is
    /// derived from it, not chosen.
    #[test]
    fn shell_outer_box_is_the_vanilla_cos_rect() {
        assert_eq!(game_window::outer_size((CONTENT_W, CONTENT_H)), WINDOW_SIZE);
    }

    /// The page host keeps vanilla's `12,66,331,314` after the rebase into the
    /// shell's content space, i.e. rebasing is a pure translation by the
    /// chrome's content origin (12, 36).
    #[test]
    fn page_rect_rebases_onto_the_shared_vanilla_rect() {
        assert_eq!(page_rect(), (0.0, 30.0, 331.0, 314.0));
        let (x, y, w, h) = page_rect();
        assert_eq!(
            (
                x + game_window::FRAME_VIS_SIDE + game_window::CHROME_PAD,
                y + game_window::CONTENT_TOP,
                w,
                h
            ),
            PAGE_RECT_WINDOW
        );
    }

    /// `ifcos.txt` gives the three pages one rect and the grammar has no
    /// `Visible` key, so exactly one page may be displayed at a time ([S]).
    #[test]
    fn exactly_one_page_is_displayed() {
        for selected in [CosPage::Info, CosPage::Inventory, CosPage::Setup] {
            let shown = [CosPage::Info, CosPage::Inventory, CosPage::Setup]
                .into_iter()
                .filter(|p| page_display(selected, *p) == Display::Flex)
                .count();
            assert_eq!(shown, 1, "page {selected:?}");
        }
    }
}
