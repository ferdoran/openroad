//! The under-bar MENU popup: one row per major game window, opening upward
//! from the bar's MENU button.
//!
//! Idea: this widget only exists in the **4th-gen** descriptor generation —
//! there is no `resinfo/` counterpart — so unlike the rest of `underbar/` its
//! numbers come from `res_ui/nifundermenubar.2dt` (111 entries,
//! `4 + 111*976 = 108340` B). Three things about that file drive the code
//! below (`docs/re/ui/menu-toggle-bar.md`):
//!
//! 1. **Labels are CHILDREN of their buttons, bound by `ParentId`**, not
//!    siblings ordered by y. The id ladder is deliberately not monotonic in y
//!    (ids 104, 117, 114, 115 are authored after 126), so a y-order binding
//!    would mislabel five rows. [`ROWS`] therefore carries the id with the row.
//! 2. **15 buttons, 14 labelled.** Button id 120 (`ub_new_icon_stallnet`)
//!    duplicates id 117's y=353 and its label child carries an empty `Text`;
//!    it is authoring residue and is not rendered — 14 rows, not 15.
//! 3. **The 20px frame inset is derived, not assumed.** `interface\frame\
//!    ub_new_wnd_` is a prefix, not a file: 8 pieces, no centre, every one
//!    20x20. `rect.deflate(20)` reproduces the authored client rect
//!    `778,64,98,363` byte-for-byte from the frame's `758,44,138,403`.
//!
//! 2DT rects are absolute in one flat design space (children are not re-based
//! on their parent), so everything here is stated frame-local as
//! `authored - (758,44)`, and the frame itself is placed bar-local. Rows
//! overhang the client area by 11px on the left and 10 on the right, and the
//! icons sit further left still, outside the rows: that overhang is the
//! family's idiom, reproduced in `targetmenu.2dt` too, not a defect to fix.
//!
//! Nine rows have a target that exists in openroad today (System, Action,
//! Alchemy, Collection, Party, Party Matching, Community, Guild and Auto
//! Potion). The rest stay visible but inert on purpose — the popup is the
//! discoverability surface for the unbuilt-window backlog, so hiding them
//! would hide the backlog.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::hud::action::ActionWindowState;
use crate::plugins::hud::alchemy::model::AlchemyState;
use crate::plugins::hud::autopotion::model::AutoPotionState;
use crate::plugins::hud::collection::CollectionWindowState;
use crate::plugins::hud::community::model::{CommunityPage, CommunityState};
use crate::plugins::hud::game_window::abs_node;
use crate::plugins::hud::party::model::PartyWindowState;
use crate::plugins::hud::party_matching::model::PartyMatchState;
use crate::plugins::system_window::{spawn_system_window, SystemWindow};
use crate::plugins::textdata::ClientUiStrings;
use crate::plugins::ui_v2::style::ImageButtonStyle;

/// The popup frame, bar-local. Authored at `758,44,138,403` in the 2dt's flat
/// design space, whose under-bar root is `81,430,800,68`; our classic bar is
/// the same 800 wide, so `x - 81` and `y - 447` carry a 2dt rect into
/// bar-local space. The 447 is the 2dt bar's own body top: its four cluster
/// buttons sit at 430/412/432/459 against our `-17/-34/-15/+12`, which give
/// 447 three times and 446 once — a 1px authoring jitter between the two
/// generations, also visible in x (`770-81 = 689` vs our menu button's 688).
/// The popup's bottom therefore lands exactly on the bar's top edge:
/// `-403 + 403 = 0`.
pub const FRAME_RECT: (f32, f32, f32, f32) = (677.0, -403.0, 138.0, 403.0);
/// Every `ub_new_wnd_` piece is 20x20, so the client area is the frame
/// deflated by 20 (checked against the authored `778,64,98,363` in the tests).
const PIECE: f32 = 20.0;

const FRAME_DIR: &str = "media://interface/frame/ub_new_wnd_";
/// `com_bg_tile_u.ddj` lives in `ifcommon/bg_tile/`, NOT in `interface/
/// underbar/` — the doc's asset list had all three arts under `underbar/` and
/// two of them are elsewhere (issue #346 comment).
const BG_TILE: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_u.ddj";
const SEPARATOR_DDJ: &str = "media://interface/underbar/ub_new_line.ddj";
const ROW_STEM: &str = "ub_new_menu_button";
const UB_DIR: &str = "media://interface/underbar/";

/// Row geometry, frame-local, from the authored entries: buttons `789,y,97,20`
/// (x 789-758 = 31), icons `766,y,20,20` (x 8), labels `793,y+7,82,10` (x 35).
const ROW_X: f32 = 31.0;
const ROW_W: f32 = 97.0;
const ROW_H: f32 = 20.0;
const ICON_X: f32 = 8.0;
const ICON_SIZE: f32 = 20.0;
const LABEL_X: f32 = 35.0;
const LABEL_DY: f32 = 7.0;
const LABEL_W: f32 = 82.0;
const LABEL_H: f32 = 10.0;
/// `ub_new_line.ddj` is 120x4, authored at x 767 (frame-local 9).
const SEP_X: f32 = 9.0;
const SEP_W: f32 = 120.0;
const SEP_H: f32 = 4.0;
/// Separators at authored y 186 / 345 / 404.
const SEPARATOR_YS: [f32; 3] = [142.0, 301.0, 360.0];

/// Label colour and size are not in the 2dt's usable fields; the row labels
/// follow the bar's own text convention (`underbar/ui.rs`).
const LABEL_COLOR: Color = Color::WHITE;
const LABEL_FONT_SIZE: f32 = 9.0;

/// One authored row: `(button id, frame-local y, icon stem, text key, English
/// fallback)`. Order is the authored y order; the ids are the `ParentId`s the
/// labels bind through, which is why they are not monotonic here.
/// Fallbacks are the real `textuisystem.txt` English column, used only when the
/// table has not loaded (offline preview scenes).
pub const ROWS: [(u32, f32, &str, &str, &str); 14] = [
    (
        83,
        16.0,
        "ub_new_icon_pt",
        "UIIT_STT_TOGGLE_PARTY",
        "Party ( P )",
    ),
    (
        93,
        41.0,
        "ub_new_icon_ptm",
        "UIIT_STT_TOGGLE_PARTYMATCH",
        "Party Matching(E)",
    ),
    (
        96,
        66.0,
        "ub_new_icon_guild",
        "UIIT_STT_TOGGLE_GUILD",
        "Guild ( U )",
    ),
    (
        99,
        91.0,
        "ub_new_icon_apprenticeship",
        "UIIT_CTL_TC_SHORTKEY_L",
        "Academy ( L )",
    ),
    (
        102,
        116.0,
        "ub_new_icon_apprenticeship_m",
        "UIIT_STT_TC_MACHING_TITLE",
        "Guardian Matching",
    ),
    (
        105,
        151.0,
        "ub_new_icon_action",
        "UIIT_STT_TOGGLE_ACTION",
        "Action ( A )",
    ),
    (
        108,
        176.0,
        "ub_new_icon_commu",
        "UIIT_STT_TOGGLE_COMMUNITY",
        "Community ( U )",
    ),
    (
        111,
        201.0,
        "ub_new_icon_quest",
        "UIIT_STT_TOGGLE_QUEST",
        "Quest ( Q )",
    ),
    (
        114,
        226.0,
        "ub_new_icon_alchemy",
        "UIIT_STT_TOGGLE_ENCHANT",
        "Alchemy ( Y )",
    ),
    (115, 251.0, "ub_new_icon_making", "UIIT_STT_MK", "Craft"),
    (
        104,
        276.0,
        "ub_new_icon_collection",
        "UIIT_PAG_COLLECTION_WINDOW",
        "Collection book",
    ),
    (
        117,
        309.0,
        "ub_new_icon_stall",
        "UIIT_CTL_OPEN_STORE",
        "Stall",
    ),
    (
        123,
        334.0,
        "ub_new_icon_recovery",
        "UIIT_STT_TOGGLE_AUTOPOTION",
        "Auto Potion (T)",
    ),
    (
        126,
        369.0,
        "ub_new_icon_system",
        "UIIT_STT_TOGGLE_SYSTEM",
        "System ( Esc )",
    ),
];

/// The rows whose target window exists in our tree today.
const SYSTEM_ROW_ID: u32 = 126;
/// `UIIT_STT_TOGGLE_ENCHANT` — "Alchemy ( Y )", the alchemy box (#333).
const ALCHEMY_ROW_ID: u32 = 114;
/// The Action row (`ub_new_icon_action`) — a 4th-gen menu entry pointing at the
/// classic-generation `ifaction.txt` panel (#475).
const ACTION_ROW_ID: u32 = 105;
/// The Collection book row (`ub_new_icon_collection`, #537).
const COLLECTION_ROW_ID: u32 = 104;
/// "Party ( P )" — the roster page. Its caption already names the key, so the
/// row and `KeyParty` toggle the same state.
const PARTY_ROW_ID: u32 = 83;
/// "Party Matching(E)" — the LFG board.
const PARTY_MATCH_ROW_ID: u32 = 93;
/// "Community ( U )" — the shell the bar's own Community button also opens.
const COMMUNITY_ROW_ID: u32 = 108;
/// "Guild ( U )" — the same shell forced onto its guild page: vanilla's guild
/// entry point is a page of Community, not a window of its own, which is why
/// two rows point at one resource.
const GUILD_ROW_ID: u32 = 96;
/// "Auto Potion (T)" — the panel already bound to `KeyAutoPotion` (3024), whose
/// own doc names this row as vanilla's opener for it.
const AUTO_POTION_ROW_ID: u32 = 123;

#[derive(Component)]
pub struct MenuPopupRoot;

/// The button id this row was built from, so a row's action is keyed on the
/// authored id rather than on its position.
#[derive(Component)]
pub struct MenuRow(pub u32);

/// Toggle the popup: it is a child of the bar body, so it dies with the bar.
pub fn toggle_menu_popup(
    bar: Entity,
    open: Option<Entity>,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
    scale: f32,
    commands: &mut Commands,
) {
    if let Some(open) = open {
        commands.entity(open).despawn();
        return;
    }
    commands.entity(bar).with_children(|bar| {
        spawn_menu_popup(bar, asset_server, fonts, ui_strings, scale);
    });
}

fn spawn_menu_popup(
    bar: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
    s: f32,
) {
    let (_, _, fw, fh) = FRAME_RECT;
    bar.spawn((
        MenuPopupRoot,
        Name::from("Under-bar Menu Popup"),
        abs_node(FRAME_RECT, s),
    ))
    .with_children(|popup| {
        // the 8-piece ring: all pieces 20x20, no centre piece exists. Mids and
        // sides stretch and overlap their neighbours by 1 unit, like the
        // mframe_wnd_ shell, so scaled positions cannot open a seam.
        let p = PIECE;
        let edges = [
            ((0.0, 0.0, p, p), "left_up"),
            ((p - 1.0, 0.0, fw - 2.0 * p + 2.0, p), "mid_up"),
            ((fw - p, 0.0, p, p), "right_up"),
            ((0.0, p - 1.0, p, fh - 2.0 * p + 2.0), "left_side"),
            ((fw - p, p - 1.0, p, fh - 2.0 * p + 2.0), "right_side"),
            ((0.0, fh - p, p, p), "left_down"),
            ((p - 1.0, fh - p, fw - 2.0 * p + 2.0, p), "mid_down"),
            ((fw - p, fh - p, p, p), "right_down"),
        ];
        for (rect, piece) in edges {
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

        // client area = frame deflated by one piece: the authored 778,64,98,363
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

        for y in SEPARATOR_YS {
            popup.spawn((
                abs_node((SEP_X, y, SEP_W, SEP_H), s),
                ImageNode {
                    image: asset_server.load(SEPARATOR_DDJ),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ));
        }

        for (id, y, icon, key, fallback) in ROWS {
            popup.spawn((
                abs_node((ICON_X, y, ICON_SIZE, ICON_SIZE), s),
                ImageNode {
                    image: asset_server.load(format!("{UB_DIR}{icon}.ddj")),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ));
            // the whole row is the hit area (WCAG: the 20px rows are under the
            // 24px target minimum, so at least make all 97px of them clickable)
            let style = ImageButtonStyle {
                normal: asset_server.load(format!("{UB_DIR}{ROW_STEM}.ddj")),
                hover: asset_server.load(format!("{UB_DIR}{ROW_STEM}_focus.ddj")),
                press: asset_server.load(format!("{UB_DIR}{ROW_STEM}_press.ddj")),
                ..Default::default()
            };
            popup
                .spawn((
                    MenuRow(id),
                    Button,
                    Hovered::default(),
                    abs_node((ROW_X, y, ROW_W, ROW_H), s),
                    ImageNode {
                        image: style.normal.clone(),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    style,
                ))
                .observe(on_row_activate);
            popup.spawn((
                Text::new(ui_strings.get_or(key, fallback).to_string()),
                TextFont {
                    font: fonts.nine.clone().into(),
                    font_size: FontSize::Px(LABEL_FONT_SIZE * s),
                    ..default()
                },
                TextColor(LABEL_COLOR),
                TextLayout::justify(Justify::Left),
                abs_node((LABEL_X, y + LABEL_DY, LABEL_W, LABEL_H), s),
                Pickable::IGNORE,
            ));
        }
    });
}

/// Activate a row.
///
/// Every wired row does the same two things — flip a window's `open` flag, then
/// close the popup behind it — so the dispatch is one `match` on the authored
/// id rather than one `if` per row, and the despawn is written once. An arm
/// returns whether the row had a target at all: the rows whose window does not
/// exist yet fall to `_`, log, and leave the popup up, which is the honest
/// rendering of the backlog this popup indexes.
#[allow(clippy::too_many_arguments)]
fn on_row_activate(
    activate: On<Activate>,
    rows: Query<&MenuRow>,
    popups: Query<Entity, With<MenuPopupRoot>>,
    window: Query<Entity, With<SystemWindow>>,
    cameras: Query<Entity, With<Camera2d>>,
    asset_server: Res<AssetServer>,
    ui_strings: Res<ClientUiStrings>,
    mut alchemy: ResMut<AlchemyState>,
    mut action_state: ResMut<ActionWindowState>,
    mut collection_state: ResMut<CollectionWindowState>,
    mut party_state: ResMut<PartyWindowState>,
    mut party_match_state: ResMut<PartyMatchState>,
    mut community: ResMut<CommunityState>,
    mut autopotion: ResMut<AutoPotionState>,
    mut commands: Commands,
) {
    let Ok(MenuRow(id)) = rows.get(activate.entity) else {
        return;
    };
    let handled = match *id {
        PARTY_ROW_ID => toggle(&mut party_state.open),
        PARTY_MATCH_ROW_ID => toggle(&mut party_match_state.open),
        COLLECTION_ROW_ID => toggle(&mut collection_state.open),
        ACTION_ROW_ID => toggle(&mut action_state.open),
        COMMUNITY_ROW_ID => toggle(&mut community.open),
        GUILD_ROW_ID => {
            // vanilla's Guild button is the Community shell on its guild page,
            // so the row picks the page before flipping the shell
            community.page = CommunityPage::Guild;
            toggle(&mut community.open)
        }
        AUTO_POTION_ROW_ID => toggle(&mut autopotion.open),
        ALCHEMY_ROW_ID => {
            // the only toggle with a teardown: closing the box drops what was
            // staged in it
            alchemy.open = !alchemy.open;
            if !alchemy.open {
                alchemy.clear();
            }
            true
        }
        SYSTEM_ROW_ID => {
            // the one row whose target is a spawned window rather than a state
            // flag, so its "close" is a despawn
            if let Ok(open) = window.single() {
                commands.entity(open).despawn();
            } else if let Some(camera) = cameras.iter().next() {
                spawn_system_window(&mut commands, &asset_server, &ui_strings, camera);
            }
            true
        }
        _ => false,
    };
    if !handled {
        debug!("underbar menu: row id {id} has no window in openroad yet");
        return;
    }
    // the popup closes behind the row it opened
    for popup in popups.iter() {
        commands.entity(popup).despawn();
    }
}

/// Flip a window's open flag. Returns `true` so a row arm is one line: every
/// simple row does exactly this and nothing else.
fn toggle(open: &mut bool) -> bool {
    *open = !*open;
    true
}

#[cfg(test)]
mod test {
    use super::*;

    /// The client rect is DERIVED from the piece size, not transcribed: all
    /// eight `ub_new_wnd_` pieces are 20x20, and deflating the authored frame
    /// by 20 must reproduce the authored `778,64,98,363` exactly. If it ever
    /// stops doing so, one of the two numbers was copied wrong.
    #[test]
    fn the_client_area_is_the_frame_deflated_by_one_piece() {
        // authored, 2dt design space
        let frame = (758.0, 44.0, 138.0, 403.0);
        let client = (
            frame.0 + PIECE,
            frame.1 + PIECE,
            frame.2 - 2.0 * PIECE,
            frame.3 - 2.0 * PIECE,
        );
        assert_eq!(client, (778.0, 64.0, 98.0, 363.0));
        // and the frame we place is that same frame carried into bar-local
        // space by (x-81, y-447)
        assert_eq!(
            FRAME_RECT,
            (frame.0 - 81.0, frame.1 - 447.0, frame.2, frame.3)
        );
        // the popup's bottom edge sits exactly on the bar's top edge
        assert_eq!(FRAME_RECT.1 + FRAME_RECT.3, 0.0);
    }

    /// 15 buttons, 14 labelled: id 120 duplicates id 117's y and has an empty
    /// label, so it is not a row. Rendering 15 would stack two rows at y 353.
    #[test]
    fn the_unlabelled_leftover_is_not_a_row() {
        assert_eq!(ROWS.len(), 14);
        assert!(!ROWS.iter().any(|(id, ..)| *id == 120));
        // id 117 owns the authored y=353 slot alone (frame-local 309)
        let stall = ROWS.iter().find(|(id, ..)| *id == 117).expect("stall row");
        assert_eq!(stall.1, 309.0);
        assert_eq!(ROWS.iter().filter(|(_, y, ..)| *y == 309.0).count(), 1);
    }

    /// The id ladder is NOT monotonic in y — ids 104, 117, 114, 115 are
    /// authored after 126 — which is exactly why labels bind by `ParentId`
    /// and this table carries the id explicitly. A y-ordered id list would
    /// mislabel five rows.
    #[test]
    fn rows_are_y_ordered_but_ids_are_not() {
        let ys: Vec<f32> = ROWS.iter().map(|(_, y, ..)| *y).collect();
        assert!(ys.windows(2).all(|w| w[0] < w[1]), "rows ascend in y");
        let ids: Vec<u32> = ROWS.iter().map(|(id, ..)| *id).collect();
        assert!(
            !ids.windows(2).all(|w| w[0] < w[1]),
            "the authored id ladder is deliberately out of order"
        );
        // every id is distinct, and every one is an authored CNIFMenuButton id
        let authored = [
            83, 93, 96, 99, 102, 105, 108, 111, 120, 123, 126, 104, 117, 114, 115,
        ];
        assert!(ids.iter().all(|id| authored.contains(id)));
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ROWS.len());
    }

    /// Row/icon/label geometry, frame-local, against the authored absolutes.
    #[test]
    fn row_geometry_matches_the_authored_entries() {
        // buttons 789,y,97,20 · icons 766,y,20,20 · labels 793,y+7,82,10
        assert_eq!((ROW_X + 758.0, ROW_W, ROW_H), (789.0, 97.0, 20.0));
        assert_eq!((ICON_X + 758.0, ICON_SIZE), (766.0, 20.0));
        assert_eq!(
            (LABEL_X + 758.0, LABEL_DY, LABEL_W, LABEL_H),
            (793.0, 7.0, 82.0, 10.0)
        );
        // separators ub_new_line.ddj 120x4 at 767, y 186/345/404
        assert_eq!((SEP_X + 758.0, SEP_W, SEP_H), (767.0, 120.0, 4.0));
        assert_eq!(
            SEPARATOR_YS.map(|y| y + 44.0),
            [186.0, 345.0, 404.0],
            "authored separator ys"
        );
        // the icons sit left of the rows and both overhang the client area —
        // the family's idiom (same in targetmenu.2dt), not a defect
        assert!(ICON_X < PIECE, "icons overhang the 20px client inset");
        assert!(ROW_X + ROW_W > FRAME_RECT.2 - PIECE);
    }

    /// The nine wired rows against their authored string keys, and the five
    /// that are still inert. The ids are the authored ones, so a row moving in
    /// the list cannot silently repoint a target — and a future wiring cannot
    /// land without this test noticing, because the inert set is asserted too.
    ///
    /// Row 117 (Stall) counts as inert on purpose rather than for lack of a
    /// `StallState.open` to flip: opening your own stall is a protocol act
    /// (`0x70B1` create), not a window toggle, so a menu row that only flipped
    /// the flag would show an un-created shell
    /// (`docs/re/ui/hud-stall-window.md` §4).
    #[test]
    fn only_the_wired_rows_have_targets() {
        let wired = [
            (SYSTEM_ROW_ID, 126, "UIIT_STT_TOGGLE_SYSTEM"),
            (ACTION_ROW_ID, 105, "UIIT_STT_TOGGLE_ACTION"),
            (ALCHEMY_ROW_ID, 114, "UIIT_STT_TOGGLE_ENCHANT"),
            (COLLECTION_ROW_ID, 104, "UIIT_PAG_COLLECTION_WINDOW"),
            (PARTY_ROW_ID, 83, "UIIT_STT_TOGGLE_PARTY"),
            (PARTY_MATCH_ROW_ID, 93, "UIIT_STT_TOGGLE_PARTYMATCH"),
            (COMMUNITY_ROW_ID, 108, "UIIT_STT_TOGGLE_COMMUNITY"),
            (GUILD_ROW_ID, 96, "UIIT_STT_TOGGLE_GUILD"),
            (AUTO_POTION_ROW_ID, 123, "UIIT_STT_TOGGLE_AUTOPOTION"),
        ];
        for (id, authored, key) in wired {
            assert_eq!(id, authored, "wired row id");
            let row = ROWS.iter().find(|(row, ..)| *row == id).expect("wired row");
            assert_eq!(row.3, key);
        }
        // Community and Guild are two rows onto one resource, which is correct:
        // vanilla's Guild button opens the Community window on its guild page.
        assert_ne!(COMMUNITY_ROW_ID, GUILD_ROW_ID);
        // Academy, Guardian Matching, Quest, Craft and Stall have no window.
        for id in [99, 102, 111, 115, 117] {
            assert!(
                !wired.iter().any(|(wired, ..)| *wired == id),
                "row {id} has no window in openroad yet"
            );
            assert!(ROWS.iter().any(|(row, ..)| *row == id), "row {id} is drawn");
        }
        assert_eq!(wired.len() + 5, ROWS.len());
    }
}
