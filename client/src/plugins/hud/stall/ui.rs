//! The player stall window (`GDR_STALL`, `ginterface.txt:821`, id 33,
//! `467x490` on the shared `mframe_wnd_` chrome) — the shell, its 14
//! `ifstall.txt` blocks and the 2x5 item grid (#779, EP-16.2).
//!
//! Idea, and the one thing that makes this window different from every other
//! HUD transcription in the tree: **`ifstall.txt`'s rects are absolute window
//! space, not content space.** The proof is in the data —
//! `GDR_STALL_INTERNAL_FRAME` is `11,39,447,440`, so its right edge is
//! `11+447 = 458 = 467-9` and its bottom `39+440 = 479 = 490-11`, i.e. the
//! numbers are measured from the *window's* top-left, not from a content
//! origin (`docs/re/ui/hud-stall-window.md` §3d). Applying this lane's usual
//! `CONTENT_TOP` convention would shift the whole window down by 36 px, which
//! the unit doc names as the single most likely silent error here. So the
//! shell is spawned with `content_at: (0, 0)` and every constant below is the
//! resinfo rect verbatim.
//!
//! What the data does *not* say out loud:
//!
//! * **The grid is 2 columns x 5 rows at pitch 204x40.** Nothing declares it;
//!   it is arithmetic on three independent facts (§3f): the divider
//!   `GDR_STALL_BGTILE_4` at `226,115,15,201` splits the `423`-wide scroll
//!   manager as `204 + 15 + 204`; the unreferenced row plates
//!   `stl_slot_02/05.ddj` are `205x41` opaque, i.e. `204+1` by `40+1` in the
//!   shared-border idiom; and the row template `ifstallslot.txt` fits inside
//!   204x40 (max extents 200 and 38). Ten slots is also the wire capacity
//!   (`docs/re/systems/stall.md:78-80`).
//! * **The row plate is drawn code-side.** `ifstallslot.txt` declares no row
//!   background, which is exactly why `stl_slot_02/05.ddj` are referenced by
//!   no resinfo file at all. Drawing them is a sourced-from-art decision, not
//!   an invention (§3f).
//! * **Two authored overflows are reproduced, not clamped** (§3e): the
//!   greeting plate `stl_slot_03` runs to x=448, 6 px past its background
//!   tile's right edge (`27+415 = 442`), and the state badge `stl_slot_04`
//!   reaches y=109, 1 px past `GDR_STALL_BGTILE_1`'s bottom (`55+53 = 108`).
//!   Both stay inside the internal frame. A tidy implementation would "fix"
//!   them and stop matching the original.
//!
//! Deliberately **not** here, per the ticket and §8 step 7: no colour picker
//! (the strings and art ship but no control declares them), and none of the
//! `stl_purchase_tab_*` / `stl_sale_tab_*` family (dead `FleaMarketMode`
//! content). The embedded chat module (`GDR_STALL_CHAT`, chat channel 9) and
//! the wire are their own tickets. The buyer half of the wire lives in
//! `net.rs` (#780); the shell itself still renders purely from
//! [`StallState`], and the only thing it sends is the buy that a click on a
//! listed row asks for.
//!
//! Accessibility (§8 step 8): the state badge is an icon *and* a text plate
//! whose string is runtime-swapped between `UIIT_STT_TRADING_NOW` and
//! `UIIT_STT_STALL_MODIFYING`. Both strings ship, so the state reaches a
//! screen reader as text rather than only as an icon swap.

use bevy::prelude::*;
use bevy::text::FontSize;

use crate::assets::FontAssets;
use crate::plugins::hud::game_window::{self, abs_node, spawn_game_window_with};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::hud::stall::model::{StallState, StallTradingState};
use crate::plugins::textdata::ClientUiStrings;

/// `ginterface.txt:830` `Rect="0,0,467,490"`.
const OUTER: (f32, f32) = (467.0, 490.0);
/// `GDR_STALL_INTERNAL_FRAME` `11,39,447,440` on `int_window_` (id 104).
const INNER_FRAME: (f32, f32, f32, f32) = (11.0, 39.0, 447.0, 440.0);
const INNER_FRAME_DIR: &str = "media://interface/inventory/int_window_";
/// `int_window_` pieces are 16x16, as in the inventory and skill windows.
const INNER_FRAME_CORNER: f32 = 16.0;

/// The three `com_bg_tile_b` fills and the `com_bg_tile_e` grid divider,
/// verbatim (ids 100..103).
const BGTILE_1: (f32, f32, f32, f32) = (27.0, 55.0, 415.0, 53.0);
const BGTILE_2: (f32, f32, f32, f32) = (27.0, 324.0, 415.0, 34.0);
/// The 5 px seam between the chat module's two panes (§3g) — the block that
/// looks like decoration and is not.
const BGTILE_3: (f32, f32, f32, f32) = (326.0, 358.0, 7.0, 105.0);
const BGTILE_4: (f32, f32, f32, f32) = (226.0, 115.0, 15.0, 201.0);
const BG_TILE_B: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_b.ddj";
const BG_TILE_E: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_e.ddj";

/// Header band: the title plate and its edit button, both at their art size
/// (the blocks declare `Rect` with w=h=0, which means "use the DDJ extent").
const TITLE_BUTTON: (f32, f32, f32, f32) = (22.0, 51.0, 28.0, 28.0);
const TITLE_PLATE: (f32, f32, f32, f32) = (48.0, 50.0, 308.0, 28.0);
/// `GDR_BTN_TRADINGSTATE`, `com_button.ddj` 76x24 at `369,51`.
const TRADING_BUTTON: (f32, f32, f32, f32) = (369.0, 51.0, 76.0, 24.0);
/// `GDR_STALL_OWNERSTATE_MSG`, `stl_slot_04.ddj` 96x24 at `350,85` — reaches
/// y=109, 1 px below BGTILE_1 (§3e overflow #2).
const STATE_BADGE: (f32, f32, f32, f32) = (350.0, 85.0, 96.0, 24.0);
/// `GDR_STALL_OWNERSTATE_ICON` `354,88,20,20`.
const STATE_ICON: (f32, f32, f32, f32) = (354.0, 88.0, 20.0, 20.0);
/// Badge text box from the block's own `ClientRect="27,4,2,2"` inset on the
/// 96x24 plate: x 377->444, y 89->107, immediately right of the icon.
const STATE_TEXT: (f32, f32, f32, f32) = (377.0, 89.0, 67.0, 18.0);

/// Greeting band: the edit button and the plate that overhangs its tile by
/// 6 px (§3e overflow #1).
const GREETING_BUTTON: (f32, f32, f32, f32) = (22.0, 328.0, 28.0, 28.0);
const GREETING_PLATE: (f32, f32, f32, f32) = (48.0, 327.0, 400.0, 28.0);

/// `GDR_STALL_DISPLAY:CIFScrollManager` `22,108,423,216` — the grid host.
const DISPLAY: (f32, f32, f32, f32) = (22.0, 108.0, 423.0, 216.0);
/// Column origins and cell pitch, derived in §3f (see the module note).
const GRID_COL_X: [f32; 2] = [22.0, 241.0];
const GRID_Y0: f32 = 115.0;
const CELL_W: f32 = 204.0;
const CELL_H: f32 = 40.0;
pub const GRID_ROWS: usize = 5;
pub const GRID_COLS: usize = 2;
/// Wire capacity, and what the grid arithmetic lands on independently.
pub const STALL_SLOTS: usize = GRID_ROWS * GRID_COLS;
/// The row plate: 205x41 opaque, one pixel of shared border past the cell.
const ROW_PLATE_W: f32 = 205.0;
const ROW_PLATE_H: f32 = 41.0;

/// `GDR_STALL_CHAT` `22,357,423,110` — drawn as its background only; the chat
/// module itself is a separate ticket (channel 9, §3g).
const CHAT_HOST: (f32, f32, f32, f32) = (22.0, 357.0, 423.0, 110.0);

const TITLE_BUTTON_DDJ: &str = "media://interface/stall/stl_titlebutton.ddj";
const WORD_BUTTON_DDJ: &str = "media://interface/stall/stl_wordbutton.ddj";
const TITLE_PLATE_DDJ: &str = "media://interface/stall/stl_slot_01.ddj";
const GREETING_PLATE_DDJ: &str = "media://interface/stall/stl_slot_03.ddj";
const BADGE_DDJ: &str = "media://interface/stall/stl_slot_04.ddj";
const COM_BUTTON_DDJ: &str = "media://interface/ifcommon/com_button.ddj";
/// `ifstall.txt:10` names `stl_condition_icon_1.ddj`, which **is not on disk**
/// (only `_01` and `_02` ship) — a dangling reference in the shipped data
/// (§9-U2). We draw `_01` for open and `_02` for modifying, which is the only
/// reading under which both files have a purpose.
const STATE_ICON_OPEN_DDJ: &str = "media://interface/stall/stl_condition_icon_01.ddj";
const STATE_ICON_MODIFY_DDJ: &str = "media://interface/stall/stl_condition_icon_02.ddj";
/// The two states of the row plate (§9-U1 leaves which is which open; the
/// empty row is the neutral one, so `_02` is drawn for an empty slot).
const ROW_PLATE_DDJ: &str = "media://interface/stall/stl_slot_02.ddj";

/// Font sizes: the tree carries `FontIndex=0` everywhere, and this lane draws
/// that index at 9 px (see the sibling windows).
const FONT: f32 = 9.0;
/// `GDR_BTN_TRADINGSTATE` is the one cream label in the file
/// (`FontColor="255,254,251,216"`, ARGB).
const TRADING_LABEL_COLOR: Color = Color::srgb_u8(254, 251, 216);

/// Expanded hit area for the two 25-px-opaque edit buttons (§8 step 8): the
/// *art* stays 28x28 as authored, the pickable node is padded out to the WCAG
/// 2.2 AA 24x24 minimum measured on the opaque bbox. Nothing visible moves.
const EDIT_HIT_PAD: f32 = 2.0;

#[derive(Component)]
pub struct StallWindowRoot;

/// The rebuilt half of the window (everything but the chrome).
#[derive(Component)]
pub struct StallBoard;

/// One of the ten grid cells, in reading order (row-major).
#[derive(Component, Clone, Copy)]
pub struct StallSlot(pub usize);

/// Window-space rect of grid cell `index`, row-major: `(0,0)` is the top-left
/// cell of the left column.
pub fn slot_rect(index: usize) -> (f32, f32, f32, f32) {
    let col = index % GRID_COLS;
    let row = index / GRID_COLS;
    (
        GRID_COL_X[col],
        GRID_Y0 + row as f32 * CELL_H,
        CELL_W,
        CELL_H,
    )
}

/// Opens the window: the `mframe_wnd_` shell in **window space** plus the
/// `int_window_` internal frame the tree declares.
pub fn spawn_stall_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    cameras: Query<Entity, With<Camera2d>>,
) {
    let Ok(camera) = cameras.single() else {
        warn!("stall window: no 2d camera to attach to");
        return;
    };
    let window = spawn_game_window_with(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        ui_strings.get_or("UIIT_STT_STALL", "Stall"),
        game_window::WindowGeometry {
            outer: OUTER,
            // window space, not content space — see the module note (§3d)
            content_at: (0.0, 0.0),
        },
        Some(game_window::InnerFrame {
            dir: INNER_FRAME_DIR,
            rect: INNER_FRAME,
            corner: INNER_FRAME_CORNER,
        }),
        (180.0, 100.0),
        hud_scale(),
        game_window::GameWindowStyle::default(),
    );
    // No `PersistedWindow`: `wndpos.dat` has exactly ten slots and none of
    // them is the stall (`docs/re/ui/wndpos-persistence.md` §2 — the registry
    // row it *does* carry is `GDR_STALL`'s ginterface id, not a file slot), so
    // opting in would invent an eleventh slot.
    commands
        .entity(window.root)
        .insert((StallWindowRoot, GlobalZIndex(25)));
    commands
        .entity(window.expect_close_button())
        .observe(super::net::on_stall_close_button);
    commands
        .entity(window.content)
        .insert(StallBoard)
        .despawn_related::<Children>();
}

/// Rebuilds the board from [`StallState`]: the four tiles, the header band,
/// the 2x5 grid, the greeting band and the chat host's background.
pub fn refresh_stall_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    state: Res<StallState>,
    boards: Query<Entity, With<StallBoard>>,
) {
    if !state.is_changed() {
        return;
    }
    let s = hud_scale();
    let font = fonts.nine.clone();
    let tile = |path: &'static str| {
        (
            ImageNode {
                image: asset_server.load(path),
                image_mode: NodeImageMode::Tiled {
                    tile_x: true,
                    tile_y: true,
                    stretch_value: 1.0,
                },
                ..default()
            },
            Pickable::IGNORE,
        )
    };
    let art = |path: &str| {
        (
            ImageNode {
                image: asset_server.load(path.to_string()),
                ..default()
            },
            Pickable::IGNORE,
        )
    };
    let label = |content: String, rect: (f32, f32, f32, f32), color: Color| {
        (
            Text::new(content),
            TextFont {
                font: font.clone().into(),
                font_size: FontSize::Px(FONT * s),
                ..default()
            },
            TextColor(color),
            abs_node(rect, s),
            Pickable::IGNORE,
        )
    };

    let (state_key, state_fallback, state_icon) = match state.trading {
        StallTradingState::Open => ("UIIT_STT_TRADING_NOW", "Trading now", STATE_ICON_OPEN_DDJ),
        StallTradingState::Modifying => (
            "UIIT_STT_STALL_MODIFYING",
            "Modifying",
            STATE_ICON_MODIFY_DDJ,
        ),
    };
    let toggle_key = match state.trading {
        StallTradingState::Open => ("UIIT_STT_END_STALL", "Modify stall"),
        StallTradingState::Modifying => ("UIIT_STT_START_STALL", "Open stall"),
    };

    for board in boards.iter() {
        commands.entity(board).despawn_related::<Children>();
        commands.entity(board).with_children(|window| {
            // --- background tiles (ids 100..103) -------------------------
            for rect in [BGTILE_1, BGTILE_2, BGTILE_3] {
                window.spawn((abs_node(rect, s), tile(BG_TILE_B)));
            }
            window.spawn((abs_node(BGTILE_4, s), tile(BG_TILE_E)));
            window.spawn((abs_node(CHAT_HOST, s), tile(BG_TILE_B)));

            // --- header band ---------------------------------------------
            window.spawn((abs_node(TITLE_PLATE, s), art(TITLE_PLATE_DDJ)));
            window.spawn(label(
                state.title.clone(),
                inset(TITLE_PLATE, 6.0),
                Color::WHITE,
            ));
            window.spawn((abs_node(hit_area(TITLE_BUTTON), s), art(TITLE_BUTTON_DDJ)));

            // The on-sale switch is the owner's control; a visitor sees the
            // button (one shell serves both roles) but it is not a target for
            // them (#781).
            let mut trading = window.spawn((
                abs_node(TRADING_BUTTON, s),
                ImageNode {
                    image: asset_server.load(COM_BUTTON_DDJ),
                    ..default()
                },
            ));
            if state.owner {
                trading.observe(super::owner::on_trading_state_button);
            } else {
                trading.insert(Pickable::IGNORE);
            }
            window.spawn(label(
                ui_strings.get_or(toggle_key.0, toggle_key.1).to_string(),
                inset(TRADING_BUTTON, 4.0),
                TRADING_LABEL_COLOR,
            ));

            // The badge is an icon PLUS text, so the state reaches a screen
            // reader as words and not only as a swapped icon (§8 step 8).
            window.spawn((abs_node(STATE_BADGE, s), art(BADGE_DDJ)));
            window.spawn((abs_node(STATE_ICON, s), art(state_icon)));
            window.spawn(label(
                ui_strings.get_or(state_key, state_fallback).to_string(),
                STATE_TEXT,
                Color::WHITE,
            ));

            // --- the 2x5 grid --------------------------------------------
            for index in 0..STALL_SLOTS {
                let (x, y, _, _) = slot_rect(index);
                let occupied = state.slots.get(index).and_then(Option::as_ref).is_some();
                let mut cell = window.spawn((
                    abs_node((x, y, ROW_PLATE_W, ROW_PLATE_H), s),
                    ImageNode {
                        image: asset_server.load(ROW_PLATE_DDJ),
                        ..default()
                    },
                    StallSlot(index),
                ));
                // Only a listed row is a target: an empty plate that swallows
                // clicks would send buys for slots the stall does not sell
                // (#780).
                if occupied {
                    cell.observe(super::net::on_stall_slot_press);
                } else {
                    cell.insert(Pickable::IGNORE);
                }
                if let Some(row) = state.slots.get(index).and_then(Option::as_ref) {
                    window.spawn(label(
                        row.name.clone(),
                        (x + 6.0, y + 6.0, CELL_W - 70.0, 16.0),
                        Color::WHITE,
                    ));
                    window.spawn(label(
                        row.price.to_string(),
                        (x + CELL_W - 64.0, y + 6.0, 58.0, 16.0),
                        Color::WHITE,
                    ));
                }
            }

            // --- greeting band -------------------------------------------
            window.spawn((abs_node(GREETING_PLATE, s), art(GREETING_PLATE_DDJ)));
            window.spawn(label(
                state.greeting.clone(),
                inset(GREETING_PLATE, 6.0),
                Color::WHITE,
            ));
            window.spawn((abs_node(hit_area(GREETING_BUTTON), s), art(WORD_BUTTON_DDJ)));
        });
    }
}

/// Text box inside a plate, `pad` in from every edge.
fn inset(rect: (f32, f32, f32, f32), pad: f32) -> (f32, f32, f32, f32) {
    (
        rect.0 + pad,
        rect.1 + pad,
        rect.2 - 2.0 * pad,
        rect.3 - 2.0 * pad,
    )
}

/// The pickable box of an edit button: the art's rect grown by
/// [`EDIT_HIT_PAD`] on every side. The art is unchanged (§8 step 8 forbids
/// enlarging it), only the target grows.
fn hit_area(rect: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    (
        rect.0 - EDIT_HIT_PAD,
        rect.1 - EDIT_HIT_PAD,
        rect.2 + 2.0 * EDIT_HIT_PAD,
        rect.3 + 2.0 * EDIT_HIT_PAD,
    )
}

/// Despawns the window when the state closes.
pub fn despawn_closing_stall(
    mut commands: Commands,
    state: Res<StallState>,
    roots: Query<Entity, With<StallWindowRoot>>,
) {
    if state.open {
        return;
    }
    for root in roots.iter() {
        commands.entity(root).despawn();
    }
}

/// Opens the window when the state says so and nothing is on screen yet.
pub fn open_pending_stall(state: Res<StallState>, roots: Query<(), With<StallWindowRoot>>) -> bool {
    state.open && roots.is_empty()
}

/// Scene exit tears the window down with everything else.
pub fn cleanup_stall(
    mut commands: Commands,
    mut state: ResMut<StallState>,
    roots: Query<Entity, With<StallWindowRoot>>,
) {
    for root in roots.iter() {
        commands.entity(root).despawn();
    }
    *state = StallState::default();
}

#[cfg(test)]
mod test {
    use super::*;

    /// The grid arithmetic of §3f, re-derived from the transcribed numbers so
    /// a future edit cannot quietly break the three closures that identify it.
    #[test]
    fn the_grid_is_two_columns_of_five_at_pitch_204_by_40() {
        assert_eq!(STALL_SLOTS, 10);
        // column origins + divider + column = the scroll manager's width
        assert_eq!(GRID_COL_X[1] - GRID_COL_X[0], CELL_W + BGTILE_4.2);
        assert_eq!(CELL_W + BGTILE_4.2 + CELL_W, DISPLAY.2);
        // the divider is centred in it
        assert_eq!((DISPLAY.2 - BGTILE_4.2) / 2.0, CELL_W);
        // the row plate is the cell plus the shared 1 px border
        assert_eq!(ROW_PLATE_W, CELL_W + 1.0);
        assert_eq!(ROW_PLATE_H, CELL_H + 1.0);
        // and the last row's plate bottom is the divider's bottom, exactly
        let (_, last_y, _, _) = slot_rect(STALL_SLOTS - 1);
        assert_eq!(last_y + ROW_PLATE_H, BGTILE_4.1 + BGTILE_4.3);
    }

    /// Row-major order, both columns, five rows — an off-by-one here would
    /// silently stack two slots on one plate.
    #[test]
    fn slots_run_row_major_over_both_columns() {
        assert_eq!(slot_rect(0), (22.0, 115.0, 204.0, 40.0));
        assert_eq!(slot_rect(1), (241.0, 115.0, 204.0, 40.0));
        assert_eq!(slot_rect(2), (22.0, 155.0, 204.0, 40.0));
        assert_eq!(slot_rect(9), (241.0, 275.0, 204.0, 40.0));
        let ys: Vec<f32> = (0..STALL_SLOTS)
            .step_by(2)
            .map(|i| slot_rect(i).1)
            .collect();
        assert_eq!(ys, vec![115.0, 155.0, 195.0, 235.0, 275.0]);
    }

    /// Window space, not content space (§3d): the internal frame's closure is
    /// what proves the convention, so it is asserted rather than trusted.
    #[test]
    fn the_internal_frame_closes_against_the_window_not_a_content_box() {
        assert_eq!(INNER_FRAME.0 + INNER_FRAME.2, OUTER.0 - 9.0);
        assert_eq!(INNER_FRAME.1 + INNER_FRAME.3, OUTER.1 - 11.0);
        // every transcribed rect lives inside that frame
        for rect in [
            BGTILE_1,
            BGTILE_2,
            BGTILE_3,
            BGTILE_4,
            TITLE_PLATE,
            TRADING_BUTTON,
            STATE_BADGE,
            GREETING_PLATE,
            DISPLAY,
            CHAT_HOST,
        ] {
            assert!(rect.0 >= INNER_FRAME.0, "{rect:?} left of the frame");
            assert!(
                rect.0 + rect.2 <= INNER_FRAME.0 + INNER_FRAME.2,
                "{rect:?} right of the frame"
            );
        }
    }

    /// Both authored overflows are reproduced, not clamped (§3e). This test
    /// exists to fail if someone "fixes" them.
    #[test]
    fn the_two_authored_overflows_are_reproduced() {
        // greeting plate runs 6 px past its background tile
        assert_eq!(GREETING_PLATE.0 + GREETING_PLATE.2, 448.0);
        assert_eq!(BGTILE_2.0 + BGTILE_2.2, 442.0);
        // state badge reaches 1 px below its tile
        assert_eq!(STATE_BADGE.1 + STATE_BADGE.3, 109.0);
        assert_eq!(BGTILE_1.1 + BGTILE_1.3, 108.0);
    }

    /// The edit buttons keep their authored art and grow only their target
    /// (§8 step 8) — enlarging the art would be the forbidden change.
    #[test]
    fn edit_buttons_grow_their_hit_area_and_not_their_art() {
        assert_eq!(TITLE_BUTTON.2, 28.0);
        let hit = hit_area(TITLE_BUTTON);
        assert_eq!(hit.2, 32.0);
        assert_eq!(hit.3, 32.0);
        // It grows into the title plate by 4 px, which is deliberate and
        // harmless: the plate is a static label with no target of its own, so
        // the pad steals nothing. What it must not do is reach the *next*
        // interactive control, the trading toggle at x=369.
        assert!(hit.0 + hit.2 < TRADING_BUTTON.0);
        assert!(hit.0 < TITLE_PLATE.0);
    }
}
