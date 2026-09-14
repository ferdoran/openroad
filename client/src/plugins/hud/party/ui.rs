//! Party roster window (P) — layout and refresh.
//!
//! Idea: every rect below is hand-transcribed from vanilla's `resinfo/ifparty.txt`
//! (the page) and `resinfo/ifpartyslot.txt` (the row template), and — unusually
//! for this tree — it is used **verbatim, with no rebasing at all**.
//!
//! That works because of an arithmetic coincidence worth stating rather than
//! rediscovering. Vanilla hosts this page inside `GDR_MAINPOPUP` (388x408) at
//! `13,38,364,337`, i.e. inset by (13,38). Our shared shell insets its content
//! by `(FRAME_VIS_SIDE + CHROME_PAD, CONTENT_TOP)` = **(12,36)** — within two
//! pixels of the same place. So handing the shell a 364-wide content area
//! reproduces MainPopup's outer width **exactly** (364 + 2*12 = 388), and every
//! page-space rect transfers 1:1 with no offset arithmetic to get wrong. The
//! character-info window had to subtract its shell origin because its page is
//! authored against a different host; this one does not.
//!
//! Two deliberate omissions, both the same call `character_info/ui.rs` makes:
//! `GDR_PARTY_FRAMEL` (the page's own `sframe_wnd_` ring at `0,0,364,303`) is
//! skipped because the mframe ring already draws a frame and the two would
//! fight, and the page's buttons at y 339..363 overhang the page rect into the
//! shell's bottom band exactly as they overhang it into MainPopup's.
//!
//! The layout closes arithmetically, which is the strongest fidelity check
//! available without a screenshot: `36 + 8*33 = 300` for the header plus seven
//! slots, `+4` for the divider, `+33` for the message board = **337**, the
//! declared page height. `the_layout_closes` pins it.
//!
//! The header row is transcribed **separately** from the slot template on
//! purpose. It looks like the same row translated, and it is not: the ten
//! shared controls sit at five distinct x-offsets and four distinct y-offsets
//! from their slot counterparts (`docs/re/ui/hud-party-window.md` §3b). A dev
//! who offsets one to get the other is wrong on eight of the ten.

use bevy::ecs::relationship::RelatedSpawnerCommands;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::config::hud::PartyPortraitSource;
use crate::plugins::config::ClientConfig;
use crate::plugins::hud::context_menu::{
    close_context_menus, spawn_context_menu, ContextMenuItem, ContextMenuOwner, ContextMenuRoot,
    ContextMenuRow,
};
use crate::plugins::hud::game_window::{self, abs_node};
use crate::plugins::hud::gauge::{gauge_art_node, gauge_crop_node, gauge_fill_width};
use crate::plugins::hud::party::model::{
    effective_setup, member_vitals, mode_keys, roster_rows, we_lead, PartyWindowState, PARTY_ROWS,
};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::entities::DisplayName;
use crate::plugins::net::party::{PartyAction, PartyCreateSetup, PartyRoster};
use crate::plugins::player::Player;
use crate::plugins::textdata::{ClientCharacterData, ClientMasteryData, ClientUiStrings};
use crate::plugins::ui_v2::style::ImageButtonStyle;

// --- Layout (vanilla page space, used verbatim; see the module doc) ----------

/// `GDR_PARTY` is `13,38,364,337` inside MainPopup.
const CONTENT_W: f32 = 364.0;
/// The page is 337 tall; its three buttons sit at y 339..363 and overhang it,
/// so the content area we hand the shell reaches through them.
const CONTENT_H: f32 = 363.0;
const WINDOW_RIGHT: f32 = 660.0;
const WINDOW_TOP: f32 = 80.0;

const ART_PARTY: &str = "media://interface/party/";
const ART_COMMON: &str = "media://interface/ifcommon/";

/// `GDR_PARTY_BGTILE_0` — the header row's backdrop.
const HEADER_TILE_RECT: (f32, f32, f32, f32) = (16.0, 36.0, 332.0, 33.0);
/// `GDR_PARTY_BGTILE_1` — the divider above the message board.
const DIVIDER_RECT: (f32, f32, f32, f32) = (27.0, 300.0, 308.0, 4.0);
/// `GDR_PTY_PARTYSLOT_MSGBOARD` — `pt_msg.ddj` is 364x36 with its bottom three
/// rows fully transparent, so 33 of it is visible. That transparent tail is
/// also why the slot pitch is 33 while the slot rect is 36.
const MSGBOARD_RECT: (f32, f32, f32, f32) = (0.0, 304.0, 364.0, 36.0);

/// Header row (`ifparty.txt`, page space) — laid out by hand, not derived.
const H_ICON_FRAME: (f32, f32, f32, f32) = (6.0, 30.0, 36.0, 36.0);
const H_PORTRAIT: (f32, f32, f32, f32) = (9.0, 33.0, 28.0, 28.0);
const H_CROWN: (f32, f32, f32, f32) = (33.0, 27.0, 12.0, 12.0);
const H_RACE: (f32, f32, f32, f32) = (53.0, 34.0, 16.0, 16.0);
const H_NAME: (f32, f32, f32, f32) = (71.0, 32.0, 83.0, 17.0);
const H_LEVEL_LABEL: (f32, f32, f32, f32) = (156.0, 32.0, 14.0, 17.0);
const H_LEVEL_VALUE: (f32, f32, f32, f32) = (172.0, 32.0, 19.0, 17.0);
const H_GAUGE_BOX: (f32, f32, f32, f32) = (52.0, 51.0, 140.0, 12.0);
const H_HP: (f32, f32, f32, f32) = (54.0, 53.0, 136.0, 4.0);
const H_MP: (f32, f32, f32, f32) = (54.0, 57.0, 136.0, 4.0);
const H_GUILD_PLATE: (f32, f32, f32, f32) = (230.0, 35.0, 48.0, 8.0);
const H_GUILD: (f32, f32, f32, f32) = (201.0, 46.0, 104.0, 17.0);
const H_BUTTON: (f32, f32, f32, f32) = (316.0, 38.0, 44.0, 20.0);

/// Slot rows: seven `CIFPartySlot` at `2,{69..267},360,36`, pitch 33.
const SLOT_X: f32 = 2.0;
const SLOT_TOP: f32 = 69.0;
const SLOT_PITCH: f32 = 33.0;
const SLOT_W: f32 = 360.0;
const SLOT_H: f32 = 36.0;

/// Slot row children (`ifpartyslot.txt`, row-local space).
const S_PORTRAIT: (f32, f32, f32, f32) = (2.0, 4.0, 28.0, 28.0);
const S_RACE: (f32, f32, f32, f32) = (44.0, 5.0, 16.0, 16.0);
const S_NAME: (f32, f32, f32, f32) = (63.0, 4.0, 80.0, 17.0);
const S_LEVEL_LABEL: (f32, f32, f32, f32) = (145.0, 4.0, 14.0, 17.0);
const S_LEVEL_VALUE: (f32, f32, f32, f32) = (161.0, 4.0, 19.0, 17.0);
const S_GUILD: (f32, f32, f32, f32) = (199.0, 14.0, 104.0, 17.0);
const S_GUILD_PLATE: (f32, f32, f32, f32) = (228.0, 7.0, 48.0, 8.0);
const S_HP: (f32, f32, f32, f32) = (43.0, 22.0, 136.0, 4.0);
const S_MP: (f32, f32, f32, f32) = (43.0, 26.0, 136.0, 4.0);
const S_BUTTON: (f32, f32, f32, f32) = (314.0, 7.0, 44.0, 20.0);

/// Bottom mode readout.
const OPTION_ITEM_DECO: (f32, f32, f32, f32) = (45.0, 312.0, 12.0, 12.0);
const OPTION_ITEM_TEXT: (f32, f32, f32, f32) = (57.0, 312.0, 114.0, 17.0);
const OPTION_EXP_DECO: (f32, f32, f32, f32) = (190.0, 312.0, 12.0, 12.0);
const OPTION_EXP_TEXT: (f32, f32, f32, f32) = (202.0, 312.0, 114.0, 17.0);

/// The three `com_button.ddj` (76x24) actions.
const BTN_INVITE: (f32, f32, f32, f32) = (56.0, 339.0, 76.0, 24.0);
const BTN_SETTING: (f32, f32, f32, f32) = (143.0, 339.0, 76.0, 24.0);
const BTN_MATCH: (f32, f32, f32, f32) = (230.0, 339.0, 76.0, 24.0);

/// Mastery badges. **Not vanilla** — no mastery control exists in any of this
/// unit's 70 blocks. The two 16x16 icons are right-aligned inside the guild
/// field, and the guild text is shortened to make room, so the addition costs
/// the guild name 34 of its 104 px rather than overlapping it. Both halves move
/// together (`guild_field`), so the trade is stated in one place.
const MASTERY_SIZE: f32 = 16.0;
const MASTERY_GAP: f32 = 2.0;

/// `FontColor` in resinfo is **A,R,G,B**, so `255,167,155,122` is an opaque
/// sand and `255,240,217,165` an opaque gold — the two readout lines each match
/// their own diamond.
const ITEM_COLOR: Color = Color::srgb_u8(167, 155, 122);
const EXP_COLOR: Color = Color::srgb_u8(240, 217, 165);
/// `GDR_PTY_STATIC_LEVEL_DATA` declares `Color=255,2255,255,255` — a literal
/// four-digit component, i.e. a typo in the shipped data. White is the only
/// sane reading of it.
const TEXT_COLOR: Color = Color::srgb_u8(255, 255, 255);

// --- Markers ----------------------------------------------------------------

#[derive(Component)]
pub struct PartyWindowRoot;
/// The whole drawable row, hidden when the row has no member. Row 0 is the
/// pinned header (whose controls live directly in the content, so it has no
/// wrapper); rows 1..=7 are the slot containers.
#[derive(Component)]
pub struct PartyRowSlot(pub usize);
#[derive(Component)]
pub struct PartyName(pub usize);
#[derive(Component)]
pub struct PartyLevel(pub usize);
#[derive(Component)]
pub struct PartyGuild(pub usize);
#[derive(Component)]
pub struct PartyHpFill(pub usize);
#[derive(Component)]
pub struct PartyMpFill(pub usize);
#[derive(Component)]
pub struct PartyPortrait(pub usize);
#[derive(Component)]
pub struct PartyRaceMark(pub usize);
#[derive(Component)]
pub struct PartyCrown(pub usize);
#[derive(Component)]
pub struct PartyMasteryIcon(pub usize, pub usize);
/// The row's own `pt_button` — "Leave", and only on the local player's row.
#[derive(Component)]
pub struct PartyLeaveButton(pub usize);
/// `true` = the EXP line, `false` = the item line.
#[derive(Component)]
pub struct PartyModeText(pub bool);
/// Header-row decorations that only make sense when somebody is drawn there.
#[derive(Component)]
pub struct PartyHeaderDeco;

/// Which of the three footer buttons was pressed.
#[derive(Component, Clone, Copy)]
pub enum PartyButton {
    Invite,
    Setting,
    Match,
}

/// The guild text field for a row, narrowed when the mastery badges share it.
fn guild_field(base: (f32, f32, f32, f32), show_masteries: bool) -> (f32, f32, f32, f32) {
    if show_masteries {
        let taken = 2.0 * MASTERY_SIZE + MASTERY_GAP;
        (base.0, base.1, base.2 - taken - MASTERY_GAP, base.3)
    } else {
        base
    }
}

/// Where mastery badge `index` (0 or 1) sits, right-aligned in the guild field.
fn mastery_rect(base: (f32, f32, f32, f32), index: usize) -> (f32, f32, f32, f32) {
    let right = base.0 + base.2;
    let x = right - (2.0 - index as f32) * MASTERY_SIZE - (1.0 - index as f32) * MASTERY_GAP;
    // Centre the 16px badge on the 17px text field.
    (
        x,
        base.1 + (base.3 - MASTERY_SIZE) / 2.0,
        MASTERY_SIZE,
        MASTERY_SIZE,
    )
}

// --- Spawning ---------------------------------------------------------------

/// Spawn the (initially hidden) party window.
pub fn spawn_party_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    config: Res<ClientConfig>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Ok(camera) = cam_query.single() else {
        warn!("party: no 2d camera to attach to");
        return;
    };
    let s = hud_scale();
    let show_masteries = config.hud.party.show_masteries;

    let window = game_window::spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        ui_strings.get_or("UIIT_PAG_PARTY", "Party"),
        (CONTENT_W, CONTENT_H),
        (WINDOW_RIGHT, WINDOW_TOP),
        s,
    );
    commands
        .entity(window.root)
        .insert((
            PartyWindowRoot,
            GlobalZIndex(55),
            // Shares the MainPopup wndpos slot with the other pages of the
            // original's single frame — see `hud::main_popup`.
            crate::plugins::hud::window_positions::PersistedWindow(
                crate::plugins::settings::window_positions::WndPosSlot::MainPopup,
            ),
        ))
        .entry::<Node>()
        .and_modify(|mut node| node.display = Display::None);
    commands
        .entity(window.expect_close_button())
        .observe(on_close_button);

    let text_font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    };
    let art = |path: String| asset_server.load::<Image>(path);

    commands.entity(window.content).with_children(|content| {
        let img = |rect: (f32, f32, f32, f32), path: String| {
            (
                abs_node(rect, s),
                ImageNode {
                    image: asset_server.load(path),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            )
        };

        // --- chrome ---------------------------------------------------------
        content.spawn(img(
            HEADER_TILE_RECT,
            format!("{ART_COMMON}bg_tile/com_bg_tile_b.ddj"),
        ));
        content.spawn(img(
            DIVIDER_RECT,
            format!("{ART_COMMON}bg_tile/com_bg_tile_a.ddj"),
        ));
        content.spawn(img(MSGBOARD_RECT, format!("{ART_PARTY}pt_msg.ddj")));

        // --- header row (row 0) --------------------------------------------
        content.spawn((
            PartyHeaderDeco,
            img(H_ICON_FRAME, format!("{ART_PARTY}pt_icon_frame.ddj")),
        ));
        content.spawn((
            PartyPortrait(0),
            img(H_PORTRAIT, format!("{ART_PARTY}pt_face.ddj")),
        ));
        content.spawn((
            PartyCrown(0),
            img(H_CROWN, format!("{ART_COMMON}com_pt_leader.ddj")),
        ));
        content.spawn((
            PartyRaceMark(0),
            img(H_RACE, format!("{ART_COMMON}com_kindred_china16.ddj")),
        ));
        content.spawn((
            PartyHeaderDeco,
            img(H_GAUGE_BOX, format!("{ART_PARTY}pt_box.ddj")),
        ));
        content.spawn((
            PartyHeaderDeco,
            img(H_GUILD_PLATE, format!("{ART_PARTY}pt_guildname_01.ddj")),
        ));

        let header_guild = guild_field(H_GUILD, show_masteries);
        spawn_text(content, PartyName(0), H_NAME, text_font(9.0), TEXT_COLOR, s);
        spawn_static_label(
            content,
            H_LEVEL_LABEL,
            ui_strings.get_or("UIIT_STT_LEVEL_LV", "Lv"),
            text_font(9.0),
            s,
        );
        spawn_text(
            content,
            PartyLevel(0),
            H_LEVEL_VALUE,
            text_font(9.0),
            TEXT_COLOR,
            s,
        );
        spawn_text(
            content,
            PartyGuild(0),
            header_guild,
            text_font(8.0),
            TEXT_COLOR,
            s,
        );
        spawn_gauge(
            content,
            PartyHpFill(0),
            H_HP,
            art(format!("{ART_PARTY}pt_hp.ddj")),
            s,
        );
        spawn_gauge(
            content,
            PartyMpFill(0),
            H_MP,
            art(format!("{ART_PARTY}pt_mp.ddj")),
            s,
        );
        if show_masteries {
            for index in 0..2 {
                content.spawn((
                    PartyMasteryIcon(0, index),
                    abs_node(mastery_rect(H_GUILD, index), s),
                    ImageNode::default(),
                    Visibility::Hidden,
                    Pickable::IGNORE,
                ));
            }
        }
        spawn_row_button(content, &asset_server, &fonts, 0, H_BUTTON, &ui_strings, s);

        // --- slot rows (1..=7) ---------------------------------------------
        for row in 1..PARTY_ROWS {
            let y = SLOT_TOP + (row as f32 - 1.0) * SLOT_PITCH;
            content
                .spawn((
                    PartyRowSlot(row),
                    abs_node((SLOT_X, y, SLOT_W, SLOT_H), s),
                    ImageNode {
                        image: asset_server.load(format!("{ART_PARTY}pt_slot.ddj")),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    Visibility::Hidden,
                ))
                .observe(on_party_row_click)
                .with_children(|slot| {
                    let simg = |rect: (f32, f32, f32, f32), path: String| {
                        (
                            abs_node(rect, s),
                            ImageNode {
                                image: asset_server.load(path),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                            Pickable::IGNORE,
                        )
                    };
                    slot.spawn((
                        PartyPortrait(row),
                        simg(S_PORTRAIT, format!("{ART_PARTY}pt_face.ddj")),
                    ));
                    slot.spawn((
                        PartyRaceMark(row),
                        simg(S_RACE, format!("{ART_COMMON}com_kindred_china16.ddj")),
                    ));
                    slot.spawn(simg(
                        S_GUILD_PLATE,
                        format!("{ART_PARTY}pt_guildname_02.ddj"),
                    ));

                    let guild = guild_field(S_GUILD, show_masteries);
                    spawn_text(slot, PartyName(row), S_NAME, text_font(9.0), TEXT_COLOR, s);
                    spawn_static_label(
                        slot,
                        S_LEVEL_LABEL,
                        ui_strings.get_or("UIIT_STT_LEVEL_LV", "Lv"),
                        text_font(9.0),
                        s,
                    );
                    spawn_text(
                        slot,
                        PartyLevel(row),
                        S_LEVEL_VALUE,
                        text_font(9.0),
                        TEXT_COLOR,
                        s,
                    );
                    spawn_text(slot, PartyGuild(row), guild, text_font(8.0), TEXT_COLOR, s);
                    spawn_gauge(
                        slot,
                        PartyHpFill(row),
                        S_HP,
                        art(format!("{ART_PARTY}pt_hp.ddj")),
                        s,
                    );
                    spawn_gauge(
                        slot,
                        PartyMpFill(row),
                        S_MP,
                        art(format!("{ART_PARTY}pt_mp.ddj")),
                        s,
                    );
                    if show_masteries {
                        for index in 0..2 {
                            slot.spawn((
                                PartyMasteryIcon(row, index),
                                abs_node(mastery_rect(S_GUILD, index), s),
                                ImageNode::default(),
                                Visibility::Hidden,
                                Pickable::IGNORE,
                            ));
                        }
                    }
                    spawn_row_button(slot, &asset_server, &fonts, row, S_BUTTON, &ui_strings, s);
                });
        }

        // --- bottom mode readout -------------------------------------------
        content.spawn(img(
            OPTION_ITEM_DECO,
            format!("{ART_COMMON}com_diamond.ddj"),
        ));
        content.spawn(img(OPTION_EXP_DECO, format!("{ART_COMMON}com_diamond.ddj")));
        spawn_text(
            content,
            PartyModeText(false),
            OPTION_ITEM_TEXT,
            text_font(9.0),
            ITEM_COLOR,
            s,
        );
        spawn_text(
            content,
            PartyModeText(true),
            OPTION_EXP_TEXT,
            text_font(9.0),
            EXP_COLOR,
            s,
        );

        // --- footer buttons -------------------------------------------------
        for (action, rect, key, fallback) in [
            (
                PartyButton::Invite,
                BTN_INVITE,
                "UIIT_CTL_INVITE_PARTY",
                "Invite",
            ),
            (
                PartyButton::Setting,
                BTN_SETTING,
                "UIIT_CTL_PARTY_SETTING",
                "Set",
            ),
            (
                PartyButton::Match,
                BTN_MATCH,
                "UIIT_CTL_PARTYMATCH_MAINBUTTEN",
                "Party match",
            ),
        ] {
            spawn_footer_button(
                content,
                &asset_server,
                &fonts,
                action,
                rect,
                ui_strings.get_or(key, fallback),
                s,
            );
        }
    });
}

/// A value text field: one marker, one `Text`, no background.
fn spawn_text<M: Component>(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    marker: M,
    rect: (f32, f32, f32, f32),
    font: TextFont,
    color: Color,
    s: f32,
) {
    parent.spawn((
        marker,
        Text::new(""),
        font,
        TextColor(color),
        abs_node(rect, s),
        Pickable::IGNORE,
    ));
}

/// A caption the data supplies and nothing ever repaints (the "Lv" prefix).
fn spawn_static_label(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    rect: (f32, f32, f32, f32),
    text: &str,
    font: TextFont,
    s: f32,
) {
    parent.spawn((
        Text::new(text.to_string()),
        font,
        TextColor(TEXT_COLOR),
        abs_node(rect, s),
        Pickable::IGNORE,
    ));
}

/// The three-node gauge from `hud::gauge`: an authored, clipping track, the
/// crop node that carries the fill (and the marker), and the art at its native
/// size. The art is never resized — that is the whole point of the recipe.
fn spawn_gauge<M: Component>(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    marker: M,
    rect: (f32, f32, f32, f32),
    art: Handle<Image>,
    s: f32,
) {
    let (w, h) = (rect.2 * s, rect.3 * s);
    let mut track = abs_node(rect, s);
    track.overflow = Overflow::clip();
    parent
        .spawn((track, Pickable::IGNORE))
        .with_children(|track| {
            track
                .spawn((
                    marker,
                    gauge_crop_node(gauge_fill_width(1.0, w), h),
                    Pickable::IGNORE,
                ))
                .with_children(|crop| {
                    crop.spawn((
                        ImageNode {
                            image: art,
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                        gauge_art_node(w, h),
                        Pickable::IGNORE,
                    ));
                });
        });
}

/// The row's `pt_button`. It reads "Leave" because that is what all seven slot
/// rows declare in the data, and it is shown only on the local player's own
/// row: leaving is the only thing this button can mean, and vanilla's kick
/// ("Banish") is a runtime context-menu item bound in no resinfo file at all
/// (`docs/re/ui/hud-party-window.md` §3b).
#[allow(clippy::too_many_arguments)]
fn spawn_row_button(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    row: usize,
    rect: (f32, f32, f32, f32),
    ui_strings: &ClientUiStrings,
    s: f32,
) {
    parent
        .spawn((
            PartyLeaveButton(row),
            Button,
            Hovered::default(),
            ImageButtonStyle {
                normal: asset_server.load(format!("{ART_PARTY}pt_button.ddj")),
                hover: asset_server.load(format!("{ART_PARTY}pt_button_focus.ddj")),
                press: asset_server.load(format!("{ART_PARTY}pt_button_press.ddj")),
                disable: asset_server.load(format!("{ART_PARTY}pt_button_disable.ddj")),
            },
            ImageNode {
                image: asset_server.load(format!("{ART_PARTY}pt_button.ddj")),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            abs_node(rect, s),
            Visibility::Hidden,
        ))
        .observe(on_leave_button)
        .with_children(|button| {
            button.spawn((
                Text::new(
                    ui_strings
                        .get_or("UIIT_CTL_LEAVE_PARTY", "Leave")
                        .to_string(),
                ),
                TextFont {
                    font: fonts.two.clone().into(),
                    font_size: FontSize::Px(8.0 * s),
                    ..default()
                },
                TextColor(TEXT_COLOR),
                TextLayout::justify(Justify::Center),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(4.0 * s),
                    width: Val::Px(rect.2 * s),
                    ..default()
                },
                Pickable::IGNORE,
            ));
        });
}

#[allow(clippy::too_many_arguments)]
fn spawn_footer_button(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    action: PartyButton,
    rect: (f32, f32, f32, f32),
    caption: &str,
    s: f32,
) {
    parent
        .spawn((
            action,
            Button,
            Hovered::default(),
            ImageButtonStyle {
                normal: asset_server.load(format!("{ART_COMMON}com_button.ddj")),
                hover: asset_server.load(format!("{ART_COMMON}com_button_focus.ddj")),
                press: asset_server.load(format!("{ART_COMMON}com_button_press.ddj")),
                disable: asset_server.load(format!("{ART_COMMON}com_button_disable.ddj")),
            },
            ImageNode {
                image: asset_server.load(format!("{ART_COMMON}com_button.ddj")),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            abs_node(rect, s),
        ))
        .observe(on_footer_button)
        .with_children(|button| {
            button.spawn((
                Text::new(caption.to_string()),
                TextFont {
                    font: fonts.two.clone().into(),
                    font_size: FontSize::Px(8.5 * s),
                    ..default()
                },
                TextColor(TEXT_COLOR),
                TextLayout::justify(Justify::Center),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(6.0 * s),
                    width: Val::Px(rect.2 * s),
                    ..default()
                },
                Pickable::IGNORE,
            ));
        });
}

pub fn cleanup_party_window(mut commands: Commands, roots: Query<Entity, With<PartyWindowRoot>>) {
    for root in roots.iter() {
        commands.entity(root).despawn();
    }
}

// --- Visibility + refresh ---------------------------------------------------

pub fn apply_party_visibility(
    state: Res<PartyWindowState>,
    mut roots: Query<&mut Node, With<PartyWindowRoot>>,
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
}

/// Repaint only while the window is up and something it draws moved.
pub fn party_needs_refresh(
    state: Res<PartyWindowState>,
    roster: Option<Res<PartyRoster>>,
    // The mode modal writes here and nowhere else, so without it in the gate
    // the readout keeps its old text however the source is computed — the same
    // trap `party_matching::DialogKey` had for the Form-party dialog.
    pending: Option<Res<PartyCreateSetup>>,
) -> bool {
    state.open
        && (state.is_changed()
            || roster.is_some_and(|roster| roster.is_changed())
            || pending.is_some_and(|pending| pending.is_changed()))
}

/// Everything the refresh needs about one drawn member, resolved once.
///
/// The repaint is three systems rather than one because a single one wants
/// nineteen parameters and Bevy caps a system at sixteen. Splitting on the
/// *data* rather than on the parameter count keeps that from being an arbitrary
/// cut: [`compute_party_views`] resolves the roster against the client's data
/// tables exactly once, and the two painters only copy the result onto nodes.
/// It also means the expensive half (the characterdata and masterydata
/// lookups) happens once per refresh instead of once per query.
#[derive(Resource, Default)]
pub struct PartyRowViews {
    rows: Vec<Option<RowView>>,
    /// The two `textuisystem` keys for the bottom readout, resolved alongside
    /// the rows so both painters read one answer.
    mode: (&'static str, &'static str),
}

impl PartyRowViews {
    fn get(&self, row: usize) -> Option<&RowView> {
        self.rows.get(row).and_then(|view| view.as_ref())
    }
}

pub struct RowView {
    /// The member's party JID — what a kick names. `None` when the presence
    /// mask never carried one, which is what disables Banish on that row.
    member_id: Option<u32>,
    name: String,
    level: String,
    guild: String,
    hp: f32,
    mp: f32,
    portrait: Option<String>,
    european: bool,
    crown: bool,
    is_local: bool,
    masteries: [Option<String>; 2],
}

/// Resolve the roster into one drawable view per row, against the client's own
/// data tables. Runs before either painter.
pub fn compute_party_views(
    roster: Option<Res<PartyRoster>>,
    // Optional for the same reason the roster is: the netcheck harness and the
    // `ui_testing` scene build no networking plugin, and Bevy panics the
    // schedule on a missing `Res` rather than skipping the system.
    pending: Option<Res<PartyCreateSetup>>,
    config: Res<ClientConfig>,
    char_data: Res<ClientCharacterData>,
    mastery_data: Res<ClientMasteryData>,
    local: Query<&DisplayName, With<Player>>,
    mut views: ResMut<PartyRowViews>,
) {
    let Some(roster) = roster else {
        views.rows.clear();
        return;
    };
    let settings = &config.hud.party;
    let local_name = local.single().ok().map(|name| name.0.as_str());

    views.rows = roster_rows(&roster)
        .iter()
        .map(|member| {
            member.map(|member| {
                let row_char = member.model_id.and_then(|id| char_data.get(&(id as i32)));
                let mastery_icon = |id: Option<u32>| {
                    id.filter(|id| *id != 0)
                        .and_then(|id| mastery_data.get(id))
                        .and_then(|info| info.icon.clone())
                };
                let name = member.name.clone().unwrap_or_default();
                // Smoothing has no nearby reading to prefer yet — the roster is
                // the only source wired here — so this is the wire's own 10 %
                // steps either way until the entity lookup lands. Going through
                // `member_vitals` regardless means the setting keeps exactly
                // one consumer rather than two that can disagree.
                let (hp, mp) = member_vitals(member, None, settings.smooth_vitals);
                RowView {
                    member_id: member.member_id,
                    is_local: local_name.is_some_and(|local| local == name),
                    crown: member.member_id.is_some_and(|id| roster.is_leader(id)),
                    level: member
                        .level
                        .map(|level| level.to_string())
                        .unwrap_or_default(),
                    guild: member.guild_name.clone().unwrap_or_default(),
                    hp,
                    mp,
                    portrait: match settings.portrait_source {
                        PartyPortraitSource::Icon => row_char.and_then(|row| row.icon_path()),
                        PartyPortraitSource::Race => {
                            race_portrait(row_char.map(|row| row.code_name().as_str()))
                        }
                        PartyPortraitSource::None => None,
                    },
                    european: row_char.is_some_and(|row| row.code_name().starts_with("CHAR_EU")),
                    masteries: [
                        mastery_icon(member.mastery_primary),
                        mastery_icon(member.mastery_secondary),
                    ],
                    name,
                }
            })
        })
        .collect();

    // Not `roster.setup`: with no party that is the default 0, so the readout
    // ignored whatever the mode modal had just committed and "Set" looked like
    // a control that did nothing. `effective_setup` is the same rule the
    // Form-party dialog's section 3 renders and the 0x7069 request carries.
    let (item_key, exp_key) = mode_keys(effective_setup(
        Some(&roster),
        pending.map(|pending| pending.0).unwrap_or_default(),
    ));
    views.mode = (item_key, exp_key);
}

/// Paint the text fields: name, level, guild and the two mode readout lines.
pub fn refresh_party_text(
    views: Res<PartyRowViews>,
    ui_strings: Res<ClientUiStrings>,
    mut names: Query<(&PartyName, &mut Text)>,
    mut levels: Query<(&PartyLevel, &mut Text), Without<PartyName>>,
    mut guilds: Query<(&PartyGuild, &mut Text), (Without<PartyName>, Without<PartyLevel>)>,
    mut mode: Query<
        (&PartyModeText, &mut Text),
        (Without<PartyName>, Without<PartyLevel>, Without<PartyGuild>),
    >,
) {
    for (marker, mut text) in names.iter_mut() {
        text.0 = views
            .get(marker.0)
            .map(|row| row.name.clone())
            .unwrap_or_default();
    }
    for (marker, mut text) in levels.iter_mut() {
        text.0 = views
            .get(marker.0)
            .map(|row| row.level.clone())
            .unwrap_or_default();
    }
    for (marker, mut text) in guilds.iter_mut() {
        text.0 = views
            .get(marker.0)
            .map(|row| row.guild.clone())
            .unwrap_or_default();
    }
    let (item_key, exp_key) = views.mode;
    for (marker, mut text) in mode.iter_mut() {
        let key = if marker.0 { exp_key } else { item_key };
        // Looked up, never hardcoded: this run of keys appears in no resinfo
        // file, so the string table is the only place the wording lives.
        text.0 = ui_strings.get_or(key, "").to_string();
    }
}

/// Paint everything that is art or visibility: the row backgrounds, portraits,
/// race marks, crowns, mastery badges, both gauges and the leave button.
#[allow(clippy::too_many_arguments)]
pub fn refresh_party_visuals(
    views: Res<PartyRowViews>,
    asset_server: Res<AssetServer>,
    mut slots: Query<(&PartyRowSlot, &mut Visibility), Without<PartyLeaveButton>>,
    mut hp: Query<(&PartyHpFill, &mut Node)>,
    mut mp: Query<(&PartyMpFill, &mut Node), Without<PartyHpFill>>,
    mut portraits: Query<(&PartyPortrait, &mut ImageNode, &mut Visibility), Without<PartyRowSlot>>,
    mut races: Query<
        (&PartyRaceMark, &mut ImageNode, &mut Visibility),
        (Without<PartyRowSlot>, Without<PartyPortrait>),
    >,
    mut crowns: Query<
        (&PartyCrown, &mut Visibility),
        (
            Without<PartyRowSlot>,
            Without<PartyPortrait>,
            Without<PartyRaceMark>,
        ),
    >,
    mut masteries: Query<
        (&PartyMasteryIcon, &mut ImageNode, &mut Visibility),
        (
            Without<PartyRowSlot>,
            Without<PartyPortrait>,
            Without<PartyRaceMark>,
            Without<PartyCrown>,
        ),
    >,
    // Every `&mut Visibility` query in this system has to be provably disjoint
    // from every other one, and Bevy proves that from the FILTERS, not from the
    // fact that these markers never co-occur. The leaf markers are mutually
    // exclusive in practice, so one side of each pair carrying the exclusion is
    // enough — the row/deco queries already exclude this one.
    mut buttons: Query<
        (&PartyLeaveButton, &mut Visibility),
        (
            Without<PartyPortrait>,
            Without<PartyRaceMark>,
            Without<PartyCrown>,
            Without<PartyMasteryIcon>,
        ),
    >,
    mut header_deco: Query<
        &mut Visibility,
        (
            With<PartyHeaderDeco>,
            Without<PartyRowSlot>,
            Without<PartyPortrait>,
            Without<PartyRaceMark>,
            Without<PartyCrown>,
            Without<PartyMasteryIcon>,
            Without<PartyLeaveButton>,
        ),
    >,
) {
    let s = hud_scale();
    for (slot, mut visibility) in slots.iter_mut() {
        *visibility = visible(views.get(slot.0).is_some());
    }
    for mut visibility in header_deco.iter_mut() {
        *visibility = visible(views.get(0).is_some());
    }
    for (marker, mut node) in hp.iter_mut() {
        let fill = views.get(marker.0).map(|row| row.hp).unwrap_or(0.0);
        node.width = gauge_fill_width(fill, H_HP.2 * s);
    }
    for (marker, mut node) in mp.iter_mut() {
        let fill = views.get(marker.0).map(|row| row.mp).unwrap_or(0.0);
        node.width = gauge_fill_width(fill, H_MP.2 * s);
    }
    for (marker, mut image, mut visibility) in portraits.iter_mut() {
        match views.get(marker.0) {
            Some(row) => {
                *visibility = Visibility::Inherited;
                // The fallback is the backdrop plate, which is all these files
                // ever were: `pt_face.ddj` is a flat opaque black 28x28.
                let path = row
                    .portrait
                    .clone()
                    .unwrap_or_else(|| format!("{ART_PARTY}pt_face.ddj"));
                image.image = asset_server.load(path);
            }
            None => *visibility = Visibility::Hidden,
        }
    }
    for (marker, mut image, mut visibility) in races.iter_mut() {
        match views.get(marker.0) {
            Some(row) => {
                *visibility = Visibility::Inherited;
                image.image =
                    asset_server.load(format!("{ART_COMMON}{}", kindred_art(row.european)));
            }
            None => *visibility = Visibility::Hidden,
        }
    }
    for (marker, mut visibility) in crowns.iter_mut() {
        *visibility = visible(views.get(marker.0).is_some_and(|row| row.crown));
    }
    for (marker, mut image, mut visibility) in masteries.iter_mut() {
        match views
            .get(marker.0)
            .and_then(|row| row.masteries[marker.1].clone())
        {
            Some(path) => {
                *visibility = Visibility::Inherited;
                image.image = asset_server.load(path);
            }
            None => *visibility = Visibility::Hidden,
        }
    }
    for (marker, mut visibility) in buttons.iter_mut() {
        *visibility = visible(views.get(marker.0).is_some_and(|row| row.is_local));
    }
}

/// The 16x16 kindred mark for a member.
fn kindred_art(european: bool) -> &'static str {
    if european {
        "com_kindred_europe16.ddj"
    } else {
        "com_kindred_china16.ddj"
    }
}

/// The `race` portrait source: the orphaned 28x28 plates, chosen by the
/// member's own characterdata code name.
///
/// This is the inference branch of the setting, and it is deliberately shallow:
/// the corpus has no per-race face art that is actually a face (§ the module
/// doc of `config::hud`), so all this can honestly do is pick the placeholder
/// over the black plate. It exists because the option was asked for; `icon` is
/// the branch with data behind it.
fn race_portrait(code_name: Option<&str>) -> Option<String> {
    code_name?;
    Some(format!("{ART_PARTY}pt_no_face.ddj"))
}

fn visible(show: bool) -> Visibility {
    if show {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    }
}

// --- Observers --------------------------------------------------------------

fn on_close_button(_: On<Activate>, mut state: ResMut<PartyWindowState>) {
    state.open = false;
}

fn on_leave_button(_: On<Activate>, mut actions: MessageWriter<PartyAction>) {
    actions.write(PartyAction::Leave);
}

fn on_footer_button(
    activate: On<Activate>,
    buttons: Query<&PartyButton>,
    mut requests: MessageWriter<PartyWindowRequest>,
) {
    if let Ok(button) = buttons.get(activate.entity) {
        requests.write(match button {
            PartyButton::Invite => PartyWindowRequest::Invite,
            PartyButton::Setting => PartyWindowRequest::Setting,
            PartyButton::Match => PartyWindowRequest::Match,
        });
    }
}

/// Right-clicking a member row opens the Banish / Leave menu.
///
/// This is where vanilla puts the kick, and where it was missing: Banish is a
/// **runtime** menu item — `UIIT_CTL_BAN_PARTY` appears in no resinfo and no
/// 2dt file — so the original assembles this menu in code, exactly as the
/// quick-party board already does. Until now this page attached observers only
/// to its close, leave and footer buttons, so a leader had no way to kick
/// anybody from the window that lists the party.
///
/// Row 0 is the leader/local slot and is not a `PartyRowSlot` at all, so only
/// rows 1..7 carry this.
#[allow(clippy::too_many_arguments)]
fn on_party_row_click(
    click: On<Pointer<Press>>,
    slots: Query<&PartyRowSlot>,
    views: Res<PartyRowViews>,
    roster: Option<Res<PartyRoster>>,
    ui_strings: Res<ClientUiStrings>,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    cameras: Query<Entity, With<Camera2d>>,
    local: Query<&DisplayName, With<Player>>,
    roots: Query<Entity, With<ContextMenuRoot>>,
    mut owner: ResMut<ContextMenuOwner>,
    mut state: ResMut<PartyWindowState>,
    mut commands: Commands,
) {
    if click.button != PointerButton::Secondary {
        return;
    }
    let (Ok(slot), Some(camera), Some(roster)) =
        (slots.get(click.entity), cameras.iter().next(), roster)
    else {
        return;
    };
    let Some(view) = views.get(slot.0) else {
        return;
    };
    close_context_menus(&mut commands, &roots, &mut owner);
    state.context_member = view.member_id;

    let local_name = local.single().ok().map(|name| name.0.as_str());
    let items = [
        ContextMenuItem {
            label: ui_strings
                .get_or("UIIT_CTL_BAN_PARTY", "Banish")
                .to_string(),
            // Kicking is the leader's privilege, it needs a JID to name, and
            // nobody kicks themselves — the row for the local player offers
            // Leave, which is the same verb the row's own button carries.
            enabled: we_lead(&roster, local_name) && view.member_id.is_some() && !view.is_local,
        },
        ContextMenuItem {
            label: ui_strings
                .get_or("UIIT_CTL_LEAVE_PARTY", "Leave")
                .to_string(),
            enabled: true,
        },
    ];
    // Anchored to the row's own left edge inside the page, in window space —
    // the popup is a screen-space child of the camera, not of the row.
    let s = hud_scale();
    let y = SLOT_TOP + (slot.0 as f32 - 1.0) * SLOT_PITCH;
    spawn_context_menu(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        Val::Px((SLOT_X + SLOT_W / 2.0) * s),
        Val::Px(y * s),
        Val::Px(0.0),
        &items,
        s,
    );
    *owner = ContextMenuOwner::PartyRoster;
}

/// Fire the picked row and close the menu.
///
/// Polled rather than observed for the same reason the quick board's twin is:
/// every surface spawns the same `ContextMenuRow` components, and
/// [`ContextMenuOwner`] is what keeps one click from being read by two pollers.
pub fn pick_party_row_menu(
    buttons: Res<ButtonInput<MouseButton>>,
    rows: Query<(&ContextMenuRow, &Hovered)>,
    roots: Query<Entity, With<ContextMenuRoot>>,
    mut owner: ResMut<ContextMenuOwner>,
    mut state: ResMut<PartyWindowState>,
    mut actions: MessageWriter<PartyAction>,
    mut commands: Commands,
) {
    if !buttons.just_released(MouseButton::Left) || *owner != ContextMenuOwner::PartyRoster {
        return;
    }
    let picked = rows
        .iter()
        .find(|(_, hovered)| hovered.get())
        .map(|(row, _)| row.0);
    match picked {
        Some(0) => {
            if let Some(member) = state.context_member {
                actions.write(PartyAction::Kick(member));
            }
        }
        Some(1) => {
            actions.write(PartyAction::Leave);
        }
        _ => {}
    }
    // any left click dismisses the menu, picked or not
    close_context_menus(&mut commands, &roots, &mut owner);
    state.context_member = None;
}

/// The footer's three verbs, as one message so the window does not have to know
/// which other module answers them — the mode modal and the match board both
/// live outside this file, and the invite target is owned by the selection.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartyWindowRequest {
    /// Ask the currently selected player into the party. The target menu
    /// already owns that resolution, so this only states the intent.
    Invite,
    /// Open `ifsetpartymode.txt`.
    Setting,
    /// Open the party-matching board.
    Match,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression the first offline run hit (B0001).
    ///
    /// Bevy proves two `&mut` queries disjoint from their **filters**, not from
    /// the fact that the marker components never co-occur on one entity. This
    /// system holds six `&mut Visibility` queries, so every pair needs one side
    /// to exclude the other, and `buttons` had no filters at all — which panics
    /// the whole schedule the first time the window is spawned.
    ///
    /// Initializing the painters against a bare world reproduces that check
    /// without a window, a GPU, or any of the resources they read: the access
    /// conflict is caught when the system's state is built, not when it runs.
    /// So this is the cheap headless guard for a failure that otherwise only
    /// appears in a live session.
    /// The other half of the "Set" defect, and the one that would have left it
    /// looking broken even with the source fixed: the mode modal writes to
    /// `PartyCreateSetup` and nowhere else, so a gate that does not watch it
    /// never repaints and the readout keeps its old text.
    #[test]
    fn committing_the_mode_modal_repaints_the_window() {
        let mut app = App::new();
        app.init_resource::<PartyWindowState>()
            .init_resource::<PartyRoster>()
            .init_resource::<PartyCreateSetup>();
        app.world_mut().resource_mut::<PartyWindowState>().open = true;

        let fired = |app: &mut App| {
            app.world_mut()
                .run_system_cached(party_needs_refresh)
                .expect("the run condition must be callable")
        };

        // Prime it: a cached system that has never run compares against tick 0,
        // so its FIRST call sees every resource as changed regardless. The
        // question this test asks only becomes meaningful from the second.
        fired(&mut app);
        app.update();
        assert!(!fired(&mut app), "an idle frame must not repaint");

        // ...and committing the modal does
        app.world_mut().resource_mut::<PartyCreateSetup>().0 .0 |=
            packets::agent::party::PartySetup::EXP_SHARED;
        assert!(fired(&mut app), "the committed mode did not repaint");
    }

    #[test]
    fn the_painters_have_disjoint_queries() {
        let mut world = World::new();
        let mut schedule = Schedule::default();
        schedule.add_systems((
            compute_party_views,
            refresh_party_text,
            refresh_party_visuals,
            apply_party_visibility,
        ));
        schedule
            .initialize(&mut world)
            .expect("the party painters must build a valid schedule");
    }

    /// The page's own arithmetic, which is the strongest fidelity check there
    /// is without a screenshot: header + 8 rows of 33 lands exactly on the
    /// divider, and the divider plus the visible message board lands exactly on
    /// the declared page height of 337.
    #[test]
    fn the_layout_closes() {
        let header_top = HEADER_TILE_RECT.1;
        assert_eq!(header_top, 36.0);
        // header row + 7 slots, all at the 33 px pitch
        let last_slot_bottom = SLOT_TOP + 7.0 * SLOT_PITCH;
        assert_eq!(header_top + 8.0 * SLOT_PITCH, last_slot_bottom);
        assert_eq!(last_slot_bottom, DIVIDER_RECT.1);
        // divider (4) + the message board's visible 33 of its 36
        assert_eq!(DIVIDER_RECT.1 + DIVIDER_RECT.3, MSGBOARD_RECT.1);
        assert_eq!(MSGBOARD_RECT.1 + 33.0, 337.0);
    }

    /// The shell reproduces MainPopup's outer width exactly, which is what lets
    /// every page rect be used with no rebasing (see the module doc).
    #[test]
    fn the_shell_matches_the_original_host_width() {
        let (outer_w, _) = game_window::outer_size((CONTENT_W, CONTENT_H));
        assert_eq!(outer_w, 388.0, "GDR_MAINPOPUP is 388 wide");
    }

    /// Seven slots at pitch 33 starting at 69 land on the rects the data
    /// declares.
    #[test]
    fn the_slot_rows_sit_where_the_data_says() {
        let declared = [69.0, 102.0, 135.0, 168.0, 201.0, 234.0, 267.0];
        for (index, expected) in declared.iter().enumerate() {
            assert_eq!(SLOT_TOP + index as f32 * SLOT_PITCH, *expected);
        }
    }

    /// The mastery badges are an addition, so they must not overlap the guild
    /// name: turning them on shortens the field by exactly the space they take.
    #[test]
    fn mastery_badges_take_their_space_from_the_guild_field() {
        let plain = guild_field(S_GUILD, false);
        let shared = guild_field(S_GUILD, true);
        assert_eq!(plain, S_GUILD);
        assert!(shared.2 < plain.2);

        let first = mastery_rect(S_GUILD, 0);
        let second = mastery_rect(S_GUILD, 1);
        // both inside the original field, in order, not overlapping
        assert!(first.0 >= shared.0 + shared.2, "badges clear the text");
        assert!(second.0 >= first.0 + MASTERY_SIZE);
        assert_eq!(second.0 + MASTERY_SIZE, S_GUILD.0 + S_GUILD.2);
    }

    /// Row 0 is the pinned header and is laid out by hand — it is NOT the slot
    /// template offset. This pins the claim the RE doc proves, so a later
    /// "simplification" that derives one from the other fails here first.
    #[test]
    fn the_header_row_is_not_the_slot_template_translated() {
        let offsets = [
            (H_PORTRAIT.0 - S_PORTRAIT.0, H_PORTRAIT.1 - S_PORTRAIT.1),
            (H_RACE.0 - S_RACE.0, H_RACE.1 - S_RACE.1),
            (H_NAME.0 - S_NAME.0, H_NAME.1 - S_NAME.1),
            (H_GUILD.0 - S_GUILD.0, H_GUILD.1 - S_GUILD.1),
            (H_HP.0 - S_HP.0, H_HP.1 - S_HP.1),
        ];
        let first = offsets[0];
        assert!(
            offsets.iter().any(|offset| *offset != first),
            "if these ever agree, the two rows really are one layout"
        );
    }
}
