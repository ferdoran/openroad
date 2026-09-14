//! Community window shell (`GDR_COMMUNITY`, id 23) + its six-page host.
//!
//! Idea: the vanilla shell is `ginterface.txt:541` `GDR_COMMUNITY:CIFCommunity`
//! — `Rect="0,0,477,393"`, `DDJ="interface\frame\mframe_wnd_"`,
//! `Text="UIIT_STT_COMMUNITY"` — so it rides on the shared `game_window`
//! chrome, and its content box is derived by subtracting that chrome's margins
//! from 477x393 rather than chosen. The six page controls of
//! `resinfo/ifcommunity.txt` all share the rect `13,61,451,320`, rebased here
//! into the shell's content space by subtracting the chrome's content origin
//! `(FRAME_VIS_SIDE + CHROME_PAD, CONTENT_TOP)` = `(12, 36)`, as
//! `character_info` does (#310: subtract the shell's own constants, never
//! re-derive the origin from vanilla's interior art).
//!
//! This is the owning shell mail needs (`docs/re/ui/mail-letter-window.md` §8
//! step 1) and the one `docs/re/ui/hud-guild-window.md:382` prescribes; it
//! replaces the underbar's "community window not implemented yet" stub. Only
//! the Letter page has a body — the other five stay empty containers for the
//! guild / friend / war-state / blocking rows.
//!
//! The data declares **no tab strip** for the pages (the page controls carry
//! no tab art and `ifcommunity.txt` has no button block), so page selection is
//! code-side state rather than chrome we invented.

use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use crate::assets::FontAssets;
use crate::plugins::config::ClientConfig;
use crate::plugins::hud::community::guild::spawn_guild_page;
use crate::plugins::hud::community::letter::spawn_letter_page;
use crate::plugins::hud::community::model::{CommunityPage, CommunityState};
use crate::plugins::hud::game_window::{self, abs_node};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::textdata::ClientUiStrings;

// --- Layout constants (resinfo, window units) -------------------------------

/// Vanilla shell rect (`ginterface.txt:541` `Rect="0,0,477,393"`).
const WINDOW_SIZE: (f32, f32) = (477.0, 393.0);
const CONTENT_W: f32 =
    WINDOW_SIZE.0 - 2.0 * (game_window::FRAME_VIS_SIDE + game_window::CHROME_PAD);
const CONTENT_H: f32 = WINDOW_SIZE.1
    - game_window::CONTENT_TOP
    - game_window::CHROME_PAD
    - game_window::FRAME_VIS_BOTTOM;

/// The rect all six page controls share (`ifcommunity.txt`), window space.
const PAGE_RECT_WINDOW: (f32, f32, f32, f32) = (13.0, 61.0, 451.0, 320.0);

/// Spawn anchor (right/top, physical px). **Ours**: `GDR_COMMUNITY` is one of
/// the framed windows `wndpos.dat` does not persist
/// (`docs/re/ui/wndpos-persistence.md:79`), so vanilla's own default position
/// is not in the data.
const WINDOW_RIGHT: f32 = 320.0;
const WINDOW_TOP: f32 = 70.0;

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

fn page_display(selected: CommunityPage, page: CommunityPage) -> Display {
    if selected == page {
        Display::Flex
    } else {
        Display::None
    }
}

// --- Markers ----------------------------------------------------------------

#[derive(Component)]
pub struct CommunityWindowRoot;

#[derive(Component)]
pub struct CommunityPageRoot(pub CommunityPage);

// --- Spawning ---------------------------------------------------------------

/// Spawn the (initially hidden) community window.
pub fn spawn_community_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    state: Res<CommunityState>,
    config: Res<ClientConfig>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Ok(camera) = cam_query.single() else {
        warn!("community window: no 2d camera to attach to");
        return;
    };
    let s = hud_scale();

    let window = game_window::spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        ui_strings.get_or("UIIT_STT_COMMUNITY", "Community"),
        (CONTENT_W, CONTENT_H),
        (WINDOW_RIGHT, WINDOW_TOP),
        s,
    );
    commands
        .entity(window.root)
        .insert((CommunityWindowRoot, GlobalZIndex(55)))
        .entry::<Node>()
        .and_modify(|mut node| node.display = Display::None);
    commands
        .entity(window.expect_close_button())
        .observe(on_close_button);

    let (px, py, pw, ph) = page_rect();
    commands.entity(window.content).with_children(|content| {
        for page in CommunityPage::ALL {
            let mut node = abs_node((px, py, pw, ph), s);
            node.display = page_display(state.page, page);
            let mut entity = content.spawn((CommunityPageRoot(page), node, Pickable::IGNORE));
            if page == CommunityPage::Letter {
                entity.with_children(|letter| {
                    spawn_letter_page(letter, &asset_server, &fonts, &ui_strings, s);
                });
            }
            if page == CommunityPage::Guild {
                entity.with_children(|guild| {
                    spawn_guild_page(
                        guild,
                        &asset_server,
                        &fonts,
                        &ui_strings,
                        config.guild.position_grant,
                        s,
                    );
                });
            }
        }
    });
}

pub fn cleanup_community_window(
    mut commands: Commands,
    windows: Query<Entity, With<CommunityWindowRoot>>,
) {
    for entity in windows.iter() {
        commands.entity(entity).despawn();
    }
}

// --- Behavior ---------------------------------------------------------------

fn on_close_button(_: On<Activate>, mut state: ResMut<CommunityState>) {
    state.open = false;
}

/// Mirror `CommunityState` onto the shell and its pages.
pub fn apply_community_visibility(
    state: Res<CommunityState>,
    mut roots: Query<&mut Node, (With<CommunityWindowRoot>, Without<CommunityPageRoot>)>,
    mut pages: Query<(&CommunityPageRoot, &mut Node), Without<CommunityWindowRoot>>,
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

    /// The chrome's outer box must come out as vanilla's shell rect
    /// (`ginterface.txt:541` `0,0,477,393`).
    #[test]
    fn shell_outer_box_is_the_vanilla_community_rect() {
        assert_eq!(game_window::outer_size((CONTENT_W, CONTENT_H)), WINDOW_SIZE);
    }

    /// Rebasing the shared page rect is a pure translation by the chrome's
    /// content origin (12, 36), and the page still fits the content width.
    #[test]
    fn page_rect_rebases_onto_the_shared_vanilla_rect() {
        assert_eq!(page_rect(), (1.0, 25.0, 451.0, 320.0));
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
        assert!(x + w <= CONTENT_W);
    }

    /// Six pages share one rect and the grammar has no `Visible` key, so
    /// exactly one may be displayed at a time.
    #[test]
    fn exactly_one_community_page_is_displayed() {
        for selected in CommunityPage::ALL {
            let shown = CommunityPage::ALL
                .into_iter()
                .filter(|p| page_display(selected, *p) == Display::Flex)
                .count();
            assert_eq!(shown, 1, "page {selected:?}");
        }
    }
}
