//! Teleport destination board (dialog "Select teleport area" option).
//!
//! Idea: the board is a **paginated card page**, not a line list. Vanilla's
//! `ifteleportboard.txt` declares five `CIFTeleportAreaSlot` cards at a fixed
//! pitch of 80 (`47,105`, `47,185`, `47,265`, `47,345`, `47,425`, each
//! `357x76`), a `CIFSpinButtonCtrl` pager under them at `181,520,80,24`, the
//! title `UIIT_PAG_TELEPORT` at `13,17,393,21` and the subtitle
//! `UIIT_CTL_TELEPORT_TARGET` at `8,65,432,21`. Each card's inside comes from
//! `ifteleportareaslot.txt`: a `64x64` icon at `6,6`, the destination name at
//! `70,14,182,21`, the price at `70,44,182,21` and a `92x36` `UIIT_CTL_MOVE`
//! button at `252,20`. All of those rects are transcribed verbatim.
//!
//! Two things the earlier line-list version called blockers:
//!
//! * **The price is not unknown.** `teleportlink.txt` col 3 *is* the gold fee
//!   (45 of 231 rows priced; see `assets/textdata/teleport.rs`). The board
//!   renders it through `UIIT_CTL_TELEPORT_RESULT` ("Teleport to [%s]area.
//!   [%d]Gold") and switches to `UIIT_CTL_TELEPORT_FREE_RESULT` ("Teleport to
//!   [%s]area.") when the fee is 0 — which is exactly why the data ships that
//!   pair.
//! * **The icon still is.** `GDR_TAS_STATIC_ICON` carries `DDJ=""`, so the
//!   image is chosen in code, and nothing in `teleportdata.txt` /
//!   `teleportlink.txt` names one; `interface/teleport/` holds only two
//!   392x268 window plates, no per-destination 64x64 art. So the icon rect is
//!   laid out and left **empty** rather than filled with an invented picture —
//!   `[U]`, see #601.
//!
//! The board's own outer frame family is likewise unnamed (no `ginterface.txt`
//! row declares `GDR_TB_*`), so the shared `game_window` mframe shell draws the
//! chrome and the authored rects are placed with the title band removed:
//! content origin = board `(0,48)`, the same 48 px top inset the NPC window's
//! authored inner frame uses. That single offset is the only `[S]` number here.
//!
//! Clicking a card's Move button sends the 0x705A request (u32 destination —
//! the u16 guess made the server reset the connection); the window closes with
//! the talk session like the store.
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use packets::agent::prelude::TeleportRequest;
use packets::Packet;

use crate::assets::FontAssets;
use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::game_window::{self, abs_node};
use crate::plugins::hud::npc_dialog::model::{NpcDialogState, OpenTeleport};
use crate::plugins::hud::npc_dialog::ui::DialogClosing;
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::hud::system_message::model::format_template;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::entities::{CharacterRef, NetworkId};
use crate::plugins::textdata::{ClientTeleport, ClientTextNames, ClientUiStrings};
use crate::plugins::ui_v2::style::ImageButtonStyle;

/// `ifteleportboard.txt` ids 1 and 3: the board's title is
/// `UIIT_PAG_TELEPORT` ("Dimensional door"); `UIIT_CTL_TELEPORT_TARGET`
/// ("Select teleport area") is the *subtitle* over the destination list. We
/// had the subtitle in the title band and no subtitle at all.
const TITLE_KEY: &str = "UIIT_PAG_TELEPORT";
const SUBTITLE_KEY: &str = "UIIT_CTL_TELEPORT_TARGET";
/// `UIIT_CTL_TELEPORT_RESULT` / `_FREE_RESULT`, the paid/free price line.
const PRICE_KEY: &str = "UIIT_CTL_TELEPORT_RESULT";
const PRICE_FREE_KEY: &str = "UIIT_CTL_TELEPORT_FREE_RESULT";
const MOVE_KEY: &str = "UIIT_CTL_MOVE";

/// The board's title band is drawn by the shared shell, so the authored rects
/// are laid out from board `y = 48` (see the module doc — the only `[S]`
/// number on this window).
const BOARD_ORIGIN_Y: f32 = 48.0;
/// Content width = the subtitle's extent, `8 + 432`.
const CONTENT_W: f32 = 440.0;
/// `GDR_TB_STATIC_NAME` `8,65,432,21`.
const SUBTITLE_RECT: (f32, f32, f32, f32) = (8.0, 65.0 - BOARD_ORIGIN_Y, 432.0, 21.0);
/// `GDR_TB_TASLOT1` `47,105,357,76`; slots 2-5 follow at +80 each.
const SLOT_RECT: (f32, f32, f32, f32) = (47.0, 105.0 - BOARD_ORIGIN_Y, 357.0, 76.0);
pub const SLOT_PITCH: f32 = 80.0;
pub const SLOTS_PER_PAGE: usize = 5;
/// `GDR_TB_SPIN_PAGE` `181,520,80,24`.
const PAGER_RECT: (f32, f32, f32, f32) = (181.0, 520.0 - BOARD_ORIGIN_Y, 80.0, 24.0);
/// The page ends under the pager.
const CONTENT_H: f32 = PAGER_RECT.1 + PAGER_RECT.3 + 8.0;

/// Card internals, local to the `357x76` slot (`ifteleportareaslot.txt`).
const CARD_ICON_RECT: (f32, f32, f32, f32) = (6.0, 6.0, 64.0, 64.0);
const CARD_PLACE_RECT: (f32, f32, f32, f32) = (70.0, 14.0, 182.0, 21.0);
const CARD_PRICE_RECT: (f32, f32, f32, f32) = (70.0, 44.0, 182.0, 21.0);
const CARD_MOVE_RECT: (f32, f32, f32, f32) = (252.0, 20.0, 92.0, 36.0);

const LINE_COLOR: Color = Color::srgb_u8(150, 220, 120);
const LINE_HOVER_COLOR: Color = Color::srgb_u8(255, 240, 160);
const PRICE_COLOR: Color = Color::srgb(0.85, 0.85, 0.8);
/// The empty icon slot: an inset well, so the authored 64x64 reads as a
/// deliberately unfilled frame rather than as a missing texture.
const ICON_WELL_BG: Color = Color::srgba(0.0, 0.0, 0.0, 0.45);
const ICON_WELL_BORDER: Color = Color::srgba(1.0, 1.0, 1.0, 0.12);

const BUTTON_DDJ: &str = "media://interface/ifcommon/com_button.ddj";
const BUTTON_FOCUS_DDJ: &str = "media://interface/ifcommon/com_button_focus.ddj";
const BUTTON_PRESS_DDJ: &str = "media://interface/ifcommon/com_button_press.ddj";
const ARROW_L_DDJ: &str = "media://interface/ifcommon/com_left_arrow.ddj";
const ARROW_L_FOCUS_DDJ: &str = "media://interface/ifcommon/com_left_arrow_focus.ddj";
const ARROW_L_PRESS_DDJ: &str = "media://interface/ifcommon/com_left_arrow_press.ddj";
const ARROW_R_DDJ: &str = "media://interface/ifcommon/com_right_arrow.ddj";
const ARROW_R_FOCUS_DDJ: &str = "media://interface/ifcommon/com_right_arrow_focus.ddj";
const ARROW_R_PRESS_DDJ: &str = "media://interface/ifcommon/com_right_arrow_press.ddj";

/// Page count for `destinations` links at [`SLOTS_PER_PAGE`] per page.
pub fn page_count(destinations: usize) -> usize {
    destinations.div_ceil(SLOTS_PER_PAGE).max(1)
}

/// The slice of `links` shown on `page` (0-based).
pub fn page_slice<T>(links: &[T], page: usize) -> &[T] {
    let start = (page * SLOTS_PER_PAGE).min(links.len());
    let end = (start + SLOTS_PER_PAGE).min(links.len());
    &links[start..end]
}

/// The open teleport session.
#[derive(Resource, Default)]
pub struct TeleportWindowState {
    pub open_for: Option<Entity>,
    /// Current 0-based page of the destination board.
    pub page: usize,
}

#[derive(Component)]
pub struct TeleportWindowRoot;

/// A destination card's Move button.
#[derive(Component)]
pub struct TeleportLine {
    destination_id: u32,
    npc_id: u32,
}

/// A pager arrow: `-1` back, `+1` forward.
#[derive(Component)]
pub struct TeleportPager(i32);

/// Open on the dialog's teleport option.
pub fn on_open_teleport(
    mut requests: MessageReader<OpenTeleport>,
    mut state: ResMut<TeleportWindowState>,
) {
    for OpenTeleport { npc } in requests.read() {
        state.open_for = Some(*npc);
        state.page = 0;
    }
}

/// The teleport window lives inside the talk session, like the store.
pub fn close_teleport_with_dialog(
    dialog: Res<NpcDialogState>,
    mut state: ResMut<TeleportWindowState>,
) {
    if state.open_for.is_some() && matches!(*dialog, NpcDialogState::Closed) {
        state.open_for = None;
        state.page = 0;
    }
}

/// Rebuild the window on state changes.
#[allow(clippy::too_many_arguments)]
pub fn sync_teleport_window(
    state: Res<TeleportWindowState>,
    existing: Query<Entity, With<TeleportWindowRoot>>,
    npcs: Query<(&CharacterRef, &NetworkId)>,
    teleport: Res<ClientTeleport>,
    names: Res<ClientTextNames>,
    ui_strings: Res<ClientUiStrings>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    if !state.is_changed() {
        return;
    }
    for entity in existing.iter() {
        commands.entity(entity).insert(DialogClosing);
    }
    let Some(npc) = state.open_for else {
        return;
    };
    let Ok((char_ref, network_id)) = npcs.get(npc) else {
        return;
    };
    let Ok(camera) = cam_query.single() else {
        return;
    };
    let Some((_, links)) = teleport.destinations(char_ref.0 as i32) else {
        warn!("teleport: no teleporter for ref {}", char_ref.0);
        return;
    };
    let s = hud_scale();
    let pages = page_count(links.len());
    let page = state.page.min(pages - 1);

    let window = game_window::spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        ui_strings.get_or(TITLE_KEY, "Dimensional door"),
        (CONTENT_W, CONTENT_H),
        (620.0, 120.0),
        s,
    );
    commands
        .entity(window.root)
        .insert((TeleportWindowRoot, GlobalZIndex(57)));
    commands
        .entity(window.expect_close_button())
        .observe(on_close_button);

    let text_font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    };

    // The price line is the data's own paid/free pair, filled positionally by
    // the shared printf-dialect formatter.
    let price_line = |name: &str, fee: u64| {
        let (template, args): (&str, Vec<String>) = if fee > 0 {
            (
                ui_strings.get_or(PRICE_KEY, "Teleport to [%s]area. [%d]Gold"),
                vec![name.to_string(), fee.to_string()],
            )
        } else {
            (
                ui_strings.get_or(PRICE_FREE_KEY, "Teleport to [%s]area."),
                vec![name.to_string()],
            )
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        format_template(template, &args)
    };

    commands.entity(window.content).with_children(|content| {
        // GDR_TB_STATIC_NAME, the board's subtitle
        content.spawn((
            Text::new(ui_strings.get_or(SUBTITLE_KEY, "Select teleport area")),
            text_font(8.5),
            TextColor(PRICE_COLOR),
            TextLayout::justify(Justify::Center),
            abs_node(SUBTITLE_RECT, s),
            Pickable::IGNORE,
        ));

        for (slot, link) in page_slice(links, page).iter().enumerate() {
            let name = teleport
                .info(link.destination)
                .map(|info| {
                    names
                        .name(&info.name_key)
                        .unwrap_or(info.codename.as_str())
                        .to_string()
                })
                .unwrap_or_else(|| format!("Teleporter {}", link.destination));
            let card = (
                SLOT_RECT.0,
                SLOT_RECT.1 + slot as f32 * SLOT_PITCH,
                SLOT_RECT.2,
                SLOT_RECT.3,
            );
            content
                .spawn((abs_node(card, s), Pickable::IGNORE))
                .with_children(|card| {
                    // GDR_TAS_STATIC_ICON: authored 64x64, source [U] — laid
                    // out empty rather than filled with an invented picture
                    card.spawn((
                        abs_node(CARD_ICON_RECT, s),
                        BackgroundColor(ICON_WELL_BG),
                        Outline {
                            width: Val::Px(1.0),
                            color: ICON_WELL_BORDER,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                    // GDR_TAS_STATIC_PLACE: the destination, plus any level gate
                    let mut place = name.clone();
                    if let Some(min) = link.min_level {
                        place.push_str(&format!("  [Lv {min}+]"));
                    }
                    card.spawn((
                        Text::new(place),
                        text_font(8.5),
                        TextColor(LINE_COLOR),
                        abs_node(CARD_PLACE_RECT, s),
                        Pickable::IGNORE,
                    ));
                    // GDR_TAS_STATIC_PRICE
                    card.spawn((
                        Text::new(price_line(&name, link.fee)),
                        text_font(8.5),
                        TextColor(PRICE_COLOR),
                        abs_node(CARD_PRICE_RECT, s),
                        Pickable::IGNORE,
                    ));
                    // GDR_TAS_BTN_MOVE
                    let style = ImageButtonStyle {
                        normal: asset_server.load(BUTTON_DDJ),
                        hover: asset_server.load(BUTTON_FOCUS_DDJ),
                        press: asset_server.load(BUTTON_PRESS_DDJ),
                        ..Default::default()
                    };
                    card.spawn((
                        TeleportLine {
                            destination_id: link.destination,
                            npc_id: network_id.0,
                        },
                        Button,
                        Hovered::default(),
                        abs_node(CARD_MOVE_RECT, s),
                        ImageNode {
                            image: style.normal.clone(),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                        style,
                    ))
                    .with_children(|button| {
                        button.spawn((
                            Text::new(ui_strings.get_or(MOVE_KEY, "Move").to_string()),
                            text_font(8.5),
                            TextColor(LINE_COLOR),
                            TextLayout::justify(Justify::Center),
                            Node {
                                position_type: PositionType::Absolute,
                                top: Val::Px(9.0 * s),
                                width: Val::Percent(100.0),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ));
                    })
                    .observe(on_teleport_line);
                });
        }

        // GDR_TB_SPIN_PAGE — only drawn when there is more than one page; the
        // geometry above never moves, so the board keeps its authored size.
        if pages > 1 {
            content
                .spawn((abs_node(PAGER_RECT, s), Pickable::IGNORE))
                .with_children(|pager| {
                    // `art` must be `'static`: `AssetServer::load` keeps the
                    // path, so a borrowed one cannot escape the closure.
                    let mut arrow = |step: i32, art: [&'static str; 3], left: f32| {
                        let style = ImageButtonStyle {
                            normal: asset_server.load(art[0]),
                            hover: asset_server.load(art[1]),
                            press: asset_server.load(art[2]),
                            ..Default::default()
                        };
                        pager
                            .spawn((
                                TeleportPager(step),
                                Button,
                                Hovered::default(),
                                abs_node((left, 4.0, 16.0, 16.0), s),
                                ImageNode {
                                    image: style.normal.clone(),
                                    image_mode: NodeImageMode::Stretch,
                                    ..default()
                                },
                                style,
                            ))
                            .observe(on_pager_click);
                    };
                    let right_x = PAGER_RECT.2 - 16.0;
                    arrow(-1, [ARROW_L_DDJ, ARROW_L_FOCUS_DDJ, ARROW_L_PRESS_DDJ], 0.0);
                    arrow(
                        1,
                        [ARROW_R_DDJ, ARROW_R_FOCUS_DDJ, ARROW_R_PRESS_DDJ],
                        right_x,
                    );
                    pager.spawn((
                        Text::new(format!("{} / {}", page + 1, pages)),
                        text_font(8.5),
                        TextColor(PRICE_COLOR),
                        TextLayout::justify(Justify::Center),
                        abs_node((16.0, 4.0, PAGER_RECT.2 - 32.0, 16.0), s),
                        Pickable::IGNORE,
                    ));
                });
        }
    });
}

/// Hover tint for the destination lines.
pub fn tint_teleport_lines(
    mut lines: Query<(&Hovered, &mut TextColor), (With<TeleportLine>, Changed<Hovered>)>,
) {
    for (hovered, mut color) in lines.iter_mut() {
        color.0 = if hovered.get() {
            LINE_HOVER_COLOR
        } else {
            LINE_COLOR
        };
    }
}

fn on_close_button(_: On<Activate>, mut state: ResMut<TeleportWindowState>) {
    state.open_for = None;
    state.page = 0;
}

/// Page the board. The rebuild clamps an over-run page, so the arrows only
/// have to stay non-negative.
fn on_pager_click(
    activate: On<Activate>,
    pagers: Query<&TeleportPager>,
    mut state: ResMut<TeleportWindowState>,
) {
    let Ok(TeleportPager(step)) = pagers.get(activate.entity) else {
        return;
    };
    state.page = state.page.saturating_add_signed(*step as isize);
}

fn on_teleport_line(
    activate: On<Activate>,
    lines: Query<&TeleportLine>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut state: ResMut<TeleportWindowState>,
) {
    let Ok(line) = lines.get(activate.entity) else {
        return;
    };
    let request = TeleportRequest {
        npc_unique_id: line.npc_id,
        kind: 2,
        destination_id: line.destination_id,
    };
    state.open_for = None;
    let Ok(conn) = conn.single() else {
        warn!("teleport: no agent connection, dropping teleport request");
        return;
    };
    info!("teleport: requesting teleport (0x705A) {:?}", request);
    if let Err(e) = conn.get_sender().send(Packet::from(request).into()) {
        error!("network: failed to send TeleportRequest: {}", e.0);
    }
}

/// OnExit cleanup.
pub fn cleanup_teleport(
    mut commands: Commands,
    windows: Query<Entity, With<TeleportWindowRoot>>,
    mut state: ResMut<TeleportWindowState>,
) {
    for entity in windows.iter() {
        commands.entity(entity).despawn();
    }
    state.open_for = None;
}

#[cfg(test)]
mod test {
    use super::*;

    /// `ifteleportboard.txt` id 1 is the title (`UIIT_PAG_TELEPORT`,
    /// "Dimensional door"); id 3 is the subtitle over the destination list
    /// (`UIIT_CTL_TELEPORT_TARGET`, "Select teleport area"). We used to put
    /// the subtitle in the title band and drop the title entirely.
    #[test]
    fn teleport_board_uses_the_title_key_not_the_subtitle_key() {
        assert_eq!(TITLE_KEY, "UIIT_PAG_TELEPORT");
        assert_eq!(SUBTITLE_KEY, "UIIT_CTL_TELEPORT_TARGET");
        assert_ne!(TITLE_KEY, SUBTITLE_KEY);
    }

    /// The five `GDR_TB_TASLOT*` rects are `47,105,357,76` on a pitch of 80,
    /// not a 20px line list. Checked against the authored absolute rects
    /// (`ifteleportboard.txt` ids 10-14), re-added to the board origin.
    #[test]
    fn the_board_lays_out_five_cards_on_the_authored_pitch() {
        assert_eq!(SLOTS_PER_PAGE, 5);
        assert_eq!(SLOT_PITCH, 80.0);
        let authored = [105.0, 185.0, 265.0, 345.0, 425.0];
        for (slot, top) in authored.iter().enumerate() {
            let y = SLOT_RECT.1 + slot as f32 * SLOT_PITCH + BOARD_ORIGIN_Y;
            assert_eq!(y, *top, "slot {slot}");
        }
        assert_eq!((SLOT_RECT.2, SLOT_RECT.3), (357.0, 76.0));
        // the last card ends above the pager, which ends inside the content
        let last_bottom = SLOT_RECT.1 + 4.0 * SLOT_PITCH + SLOT_RECT.3;
        assert!(last_bottom <= PAGER_RECT.1);
        assert!(PAGER_RECT.1 + PAGER_RECT.3 <= CONTENT_H);
    }

    /// `ifteleportareaslot.txt`: icon `6,6,64,64`, name `70,14,182,21`, price
    /// `70,44,182,21`, Move `252,20,92,36` — and all four inside the card.
    #[test]
    fn the_card_internals_are_the_authored_rects() {
        assert_eq!(CARD_ICON_RECT, (6.0, 6.0, 64.0, 64.0));
        assert_eq!(CARD_PLACE_RECT, (70.0, 14.0, 182.0, 21.0));
        assert_eq!(CARD_PRICE_RECT, (70.0, 44.0, 182.0, 21.0));
        assert_eq!(CARD_MOVE_RECT, (252.0, 20.0, 92.0, 36.0));
        for rect in [
            CARD_ICON_RECT,
            CARD_PLACE_RECT,
            CARD_PRICE_RECT,
            CARD_MOVE_RECT,
        ] {
            assert!(rect.0 + rect.2 <= SLOT_RECT.2);
            assert!(rect.1 + rect.3 <= SLOT_RECT.3);
        }
        // name above price, both left of the Move button
        assert!(CARD_PLACE_RECT.1 < CARD_PRICE_RECT.1);
        assert!(CARD_PLACE_RECT.0 + CARD_PLACE_RECT.2 <= CARD_MOVE_RECT.0);
    }

    /// A destination list longer than five paginates instead of growing the
    /// window — the acceptance criterion of #601 part 1.
    #[test]
    fn more_than_five_destinations_paginate() {
        let links: Vec<u32> = (0..13).collect();
        assert_eq!(page_count(links.len()), 3);
        assert_eq!(page_slice(&links, 0), &links[0..5]);
        assert_eq!(page_slice(&links, 1), &links[5..10]);
        assert_eq!(page_slice(&links, 2), &links[10..13]);
        // no page ever exceeds one board page
        for page in 0..page_count(links.len()) {
            assert!(page_slice(&links, page).len() <= SLOTS_PER_PAGE);
        }
        // an out-of-range page yields nothing rather than panicking
        assert!(page_slice(&links, 9).is_empty());
        // the empty and the exactly-full cases
        assert_eq!(page_count(0), 1);
        assert_eq!(page_count(5), 1);
        assert_eq!(page_count(6), 2);
    }

    /// The price line is the data's paid/free pair, not a hand-written string:
    /// `UIIT_CTL_TELEPORT_RESULT` when a fee applies, `_FREE_RESULT` when not.
    #[test]
    fn the_price_line_uses_the_paid_and_free_templates() {
        let paid = "Teleport to [%s]area. [%d]Gold";
        let free = "Teleport to [%s]area.";
        assert_eq!(
            format_template(paid, &["Jangan", "5000"]),
            "Teleport to [Jangan]area. [5000]Gold"
        );
        assert_eq!(
            format_template(free, &["Jangan"]),
            "Teleport to [Jangan]area."
        );
        assert_eq!(PRICE_KEY, "UIIT_CTL_TELEPORT_RESULT");
        assert_eq!(PRICE_FREE_KEY, "UIIT_CTL_TELEPORT_FREE_RESULT");
    }
}
