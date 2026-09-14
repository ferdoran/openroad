//! The COS window's grab-settings page — `ifcossetup.txt`, page 3 of the shell.
//!
//! Idea: `docs/re/systems/pet-pick-cos.md` §3 states the verdict this module is
//! built on — **the page IS the flag set, no more and no less**. `ifcossetup.txt`
//! maps 1:1 onto `PickPetSettings`: one radio row per boolean pair, one checkbox
//! per item category, OK/Cancel. So an extra control here would be invented
//! scope and a missing one a dropped feature, and the test at the bottom is what
//! holds that line.
//!
//! The doc's **negative result** is load-bearing and is why this page has three
//! checkboxes rather than six: `UIIT_STT_COSNEWUI_PICKUP_{ASSORTMENT_EXCEPTION,
//! MATERIAL, BULLET}` exist in `textuisystem.txt` and would suggest flag bits
//! 8/16/32 — but they are placed in **zero** resinfo files, so 1.188's page is
//! exactly three checkboxes and two radios. No extra bit is invented here.
//!
//! `AttackPetSettings::OFFENSIVE` gets **no** control: `ifcossetup.txt` declares
//! none, and the attack pet has no settings page of its own in the 1.188 tree.
//! The wire type stays handled on the ack side.
//!
//! Edits are staged, not optimistic. A toggle changes the *edited* value; OK
//! sends `0x7420` and Cancel reverts to the last value the server confirmed —
//! which is what `docs/re/systems/pet-pick-cos.md` §3's OK/Cancel row means.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use packets::agent::pet::{
    CosKind, PetSettingsChangeRequest, PetSettingsChangeResponse, PickPetSettings,
    PET_SETTINGS_TYPE_GOLD,
};

use packets::Packet;

use crate::assets::FontAssets;
use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::cos::state::CosState;
use crate::plugins::hud::game_window::abs_node;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::textdata::ClientUiStrings;

// --- Layout constants (resinfo/ifcossetup.txt, page-local units) ------------

/// `GDR_COS_SETUP_INNER_BOX:CIFFrame` (`:414`, `11,11,308,291`), `opt_inner_box_`.
const INNER_BOX_RECT: (f32, f32, f32, f32) = (11.0, 11.0, 308.0, 291.0);
const INNER_BOX_DIR: &str = "media://interface/option/opt_inner_box_";
const INNER_BOX_PIECE: f32 = 4.0;
/// `GDR_COS_SETUP_BG_TILE:CIFNormalTile` (`:395`, `31,39,268,251`).
const BG_TILE_RECT: (f32, f32, f32, f32) = (31.0, 39.0, 268.0, 251.0);
const BG_TILE_DDJ: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_b.ddj";
/// `GDR_COS_SETUP_TITEL_NAME_STA` (`:376`, `44,16,246,11`, HAlign 1).
const TITLE_RECT: (f32, f32, f32, f32) = (44.0, 16.0, 246.0, 11.0);

/// The three option groups, top to bottom: the `opt_video_tab` header tab
/// (`:357,:319,:281`, art-sized — `opt_video_tab.ddj` measured **124x28**), the
/// `int_window_` frame (`:338,:300,:262`) and the group's label
/// (`:243,:224,:205`).
struct OptionGroup {
    tab_pos: (f32, f32),
    frame_rect: (f32, f32, f32, f32),
    label_rect: (f32, f32, f32, f32),
    label_key: &'static str,
    label_fallback: &'static str,
}
const TAB_ART: (f32, f32) = (124.0, 28.0);
const TAB_DDJ: &str = "media://interface/option/opt_video_tab.ddj";
const GROUP_FRAME_DIR: &str = "media://interface/inventory/int_window_";
const GROUP_FRAME_PIECE: f32 = 16.0;
const OPTION_GROUPS: [OptionGroup; 3] = [
    OptionGroup {
        tab_pos: (21.0, 37.0),
        frame_rect: (22.0, 64.0, 286.0, 42.0),
        label_rect: (32.0, 47.0, 102.0, 11.0),
        label_key: "UIIT_STT_COSNEWUI_PICKUP_SWITCH",
        label_fallback: "Grab function",
    },
    OptionGroup {
        tab_pos: (21.0, 110.0),
        frame_rect: (22.0, 137.0, 286.0, 42.0),
        label_rect: (32.0, 120.0, 102.0, 11.0),
        label_key: "UIIT_STT_COSNEWUI_PICKUPITEM_SET",
        label_fallback: "Target grab authority",
    },
    OptionGroup {
        tab_pos: (21.0, 186.0),
        frame_rect: (22.0, 210.0, 286.0, 42.0),
        label_rect: (32.0, 194.0, 102.0, 11.0),
        label_key: "UIIT_STT_COSNEWUI_PICKUPITEM_ASSORTMENT",
        label_fallback: "Target item",
    },
];

/// `GDR_COS_SETUP_ONOFF_RADIOBTN:CIFRadioButton` (`:186`, `37,78,260,16`) —
/// the grab on/off pair, `PickPetSettings::ENABLED`.
const ONOFF_ROW: (f32, f32, f32, f32) = (37.0, 78.0, 260.0, 16.0);
/// `GDR_COS_SETUP_SCOPE_RADIOBTN:CIFRadioButton` (`:167`, `37,150,260,16`) —
/// own-drops vs all-drops, `PickPetSettings::GRAB_ALL_ITEMS`.
const SCOPE_ROW: (f32, f32, f32, f32) = (37.0, 150.0, 260.0, 16.0);
/// `com_radiobutton_off/_on.ddj`, both measured 16x16 (R5G6B5, alpha-less).
const RADIO_SIZE: (f32, f32) = (16.0, 16.0);
const RADIO_OFF_DDJ: &str = "media://interface/ifcommon/com_radiobutton_off.ddj";
const RADIO_ON_DDJ: &str = "media://interface/ifcommon/com_radiobutton_on.ddj";
/// **Ours, not the data's**: a `CIFRadioButton` is declared as *one* 260x16 row
/// carrying *two* options, and the resinfo says nothing about where inside that
/// row each option sits — the engine lays them out. We split the row in half
/// (option 2 at +130) and put each caption 20 px right of its dot, which keeps
/// both options inside the authored row and is reversible in one constant.
const RADIO_OPTION_PITCH: f32 = 130.0;
const RADIO_LABEL_INSET: f32 = 20.0;

/// `GDR_COS_SETUP_TARGET_CHECKBOX_1/2/3` (`:148,:129,:110`, all art-sized —
/// `com_checkbutton_off.ddj` measured 16x16) with their labels
/// (`:91,:72,:53`). Order is file order 1..3 = Gold / Equipment / Other.
const CHECKBOX_SIZE: (f32, f32) = (16.0, 16.0);
const CHECK_OFF_DDJ: &str = "media://interface/ifcommon/com_checkbutton_off.ddj";
const CHECK_ON_DDJ: &str = "media://interface/ifcommon/com_checkbutton_on.ddj";
const CHECKBOXES: [((f32, f32), (f32, f32, f32, f32), u32, &str, &str); 3] = [
    (
        (37.0, 224.0),
        (58.0, 226.0, 49.0, 11.0),
        PickPetSettings::GOLD,
        "UIIT_STT_GOLD",
        "Gold",
    ),
    (
        (108.0, 224.0),
        (129.0, 226.0, 70.0, 11.0),
        PickPetSettings::EQUIPMENT,
        "UIIT_STT_COSNEWUI_PICKUP_EQUIPITEM",
        "Equipment",
    ),
    (
        (205.0, 224.0),
        (225.0, 226.0, 70.0, 11.0),
        PickPetSettings::OTHER_ITEMS,
        "UIIT_STT_COSNEWUI_PICKUP_ETCITEM",
        "Other items",
    ),
];

/// `GDR_COS_SETUP_OK_BTN` (`:34`, `83,263,0,0`) and `_CANCEL_BTN` (`:15`,
/// `171,263,0,0`), both art-sized `com_button.ddj`, measured **76x24**.
const OK_POS: (f32, f32) = (83.0, 263.0);
const CANCEL_POS: (f32, f32) = (171.0, 263.0);
const BUTTON_SIZE: (f32, f32) = (76.0, 24.0);
const BUTTON_DDJ: &str = "media://interface/ifcommon/com_button.ddj";

/// `FontColor=255,255,255,255` on every control but the three group labels.
const TEXT_COLOR: Color = Color::srgb(1.0, 1.0, 1.0);
/// `FontColor=255,239,218,164` — the three group labels (`:239,:220,:201`).
const GROUP_LABEL_COLOR: Color = Color::srgb(239.0 / 255.0, 218.0 / 255.0, 164.0 / 255.0);

// --- State ------------------------------------------------------------------

/// The page's staged edit of the summoned pick pet's settings.
///
/// `confirmed` is the last value the **server** stated; `edited` is what the
/// controls show. OK sends `edited` and Cancel throws it away — the client
/// never treats its own click as the truth.
#[derive(Resource, Default, Debug, PartialEq, Eq)]
pub struct CosSetupState {
    pub confirmed: PickPetSettings,
    pub edited: PickPetSettings,
}

impl CosSetupState {
    /// Flip one flag in the staged value.
    pub fn toggle(&mut self, bit: u32) {
        self.edited = PickPetSettings(self.edited.0 ^ bit);
    }

    /// Adopt a server-stated value, discarding any staged edit.
    pub fn confirm(&mut self, settings: PickPetSettings) {
        self.confirmed = settings;
        self.edited = settings;
    }

    pub fn is_set(&self, bit: u32) -> bool {
        self.edited.0 & bit != 0
    }
}

/// A checkbox or radio dot bound to one settings bit.
#[derive(Component, Clone, Copy)]
pub struct CosSetupToggle {
    /// The flag the control edits.
    pub bit: u32,
    /// `true` for the control that is lit when the bit is **set**; the other
    /// half of a radio pair carries `false`.
    pub on_state: bool,
}

/// The two page commands.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum CosSetupCommand {
    Ok,
    Cancel,
}

// --- Spawning ---------------------------------------------------------------

/// Fill the Setup page container with `ifcossetup.txt`.
pub fn build_setup_page(
    page: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
    s: f32,
) {
    let text_font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    };
    let image = |rect: (f32, f32, f32, f32), path: String| {
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
    let label = |rect: (f32, f32, f32, f32), text: String, color: Color, justify: Justify| {
        (
            Text::new(text),
            text_font(7.5),
            TextColor(color),
            TextLayout::justify(justify),
            abs_node(rect, s),
            Pickable::IGNORE,
        )
    };

    // background tile, then the inner box ring over it
    page.spawn((
        abs_node(BG_TILE_RECT, s),
        ImageNode {
            image: asset_server.load(BG_TILE_DDJ),
            image_mode: NodeImageMode::Tiled {
                tile_x: true,
                tile_y: true,
                stretch_value: s,
            },
            ..default()
        },
        Pickable::IGNORE,
    ));
    let (bx, by, bw, bh) = INNER_BOX_RECT;
    for ((x, y, w, h), piece) in ring(bw, bh, INNER_BOX_PIECE) {
        page.spawn(image(
            (bx + x, by + y, w, h),
            format!("{INNER_BOX_DIR}{piece}.ddj"),
        ));
    }
    page.spawn(label(
        TITLE_RECT,
        ui_strings
            .get_or(
                "UIIT_STT_COSNEWUI_TECHNOLOGY_ITEMAUTOPICKUP",
                "Item auto-grab",
            )
            .to_string(),
        TEXT_COLOR,
        Justify::Center,
    ));

    for group in OPTION_GROUPS {
        page.spawn(image(
            (group.tab_pos.0, group.tab_pos.1, TAB_ART.0, TAB_ART.1),
            TAB_DDJ.to_string(),
        ));
        let (fx, fy, fw, fh) = group.frame_rect;
        for ((x, y, w, h), piece) in ring(fw, fh, GROUP_FRAME_PIECE) {
            page.spawn(image(
                (fx + x, fy + y, w, h),
                format!("{GROUP_FRAME_DIR}{piece}.ddj"),
            ));
        }
        page.spawn(label(
            group.label_rect,
            ui_strings
                .get_or(group.label_key, group.label_fallback)
                .to_string(),
            GROUP_LABEL_COLOR,
            Justify::Left,
        ));
    }

    // the two radio rows: bit, then the ON option and the OFF option
    for (row, bit, options) in [
        (
            ONOFF_ROW,
            PickPetSettings::ENABLED,
            [
                ("UIIT_STT_COSNEWUI_PICKUP_SWITCH_ON", "ON", true),
                ("UIIT_STT_COSNEWUI_PICKUP_SWITCH_OFF", "OFF", false),
            ],
        ),
        (
            SCOPE_ROW,
            PickPetSettings::GRAB_ALL_ITEMS,
            [
                (
                    "UIIT_STT_COSNEWUI_PICKUPITEM_ASSORTMENT_ALL",
                    "Grab all items",
                    true,
                ),
                (
                    "UIIT_STT_COSNEWUI_PICKUPITEM_ASSORTMENT_SELF",
                    "Grab only my items",
                    false,
                ),
            ],
        ),
    ] {
        for (index, (key, fallback, on_state)) in options.into_iter().enumerate() {
            let x = row.0 + index as f32 * RADIO_OPTION_PITCH;
            page.spawn((
                CosSetupToggle { bit, on_state },
                Button,
                Hovered::default(),
                abs_node((x, row.1, RADIO_SIZE.0, RADIO_SIZE.1), s),
                ImageNode {
                    image: asset_server.load(RADIO_OFF_DDJ),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
            ))
            .observe(on_toggle);
            page.spawn(label(
                (
                    x + RADIO_LABEL_INSET,
                    row.1 + 2.0,
                    RADIO_OPTION_PITCH - RADIO_LABEL_INSET,
                    11.0,
                ),
                ui_strings.get_or(key, fallback).to_string(),
                TEXT_COLOR,
                Justify::Left,
            ));
        }
    }

    // the three category checkboxes + their labels
    for (pos, label_rect, bit, key, fallback) in CHECKBOXES {
        page.spawn((
            CosSetupToggle {
                bit,
                on_state: true,
            },
            Button,
            Hovered::default(),
            abs_node((pos.0, pos.1, CHECKBOX_SIZE.0, CHECKBOX_SIZE.1), s),
            ImageNode {
                image: asset_server.load(CHECK_OFF_DDJ),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
        ))
        .observe(on_toggle);
        page.spawn(label(
            label_rect,
            ui_strings.get_or(key, fallback).to_string(),
            TEXT_COLOR,
            Justify::Left,
        ));
    }

    // OK / Cancel
    for (pos, command, key, fallback) in [
        (OK_POS, CosSetupCommand::Ok, "UIIS_CTL_CONFIRM", "OK"),
        (
            CANCEL_POS,
            CosSetupCommand::Cancel,
            "UIIS_CTL_CANCEL",
            "Cancel",
        ),
    ] {
        page.spawn((
            command,
            Button,
            Hovered::default(),
            abs_node((pos.0, pos.1, BUTTON_SIZE.0, BUTTON_SIZE.1), s),
            ImageNode {
                image: asset_server.load(BUTTON_DDJ),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
        ))
        .observe(on_command)
        .with_children(|button| {
            button.spawn((
                Text::new(ui_strings.get_or(key, fallback).to_string()),
                text_font(7.5),
                TextColor(TEXT_COLOR),
                TextLayout::justify(Justify::Center),
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(6.0 * s),
                    width: Val::Percent(100.0),
                    ..default()
                },
                Pickable::IGNORE,
            ));
        });
    }
}

/// The 8 pieces of a `*_` frame ring over a `w x h` box, at `p` px pieces.
///
/// Shared with [`super::inventory`], whose `opt_inner_box_` panel is the same
/// kit at the same 4 px, rather than letting the bag grow a fourth copy of
/// this arithmetic.
pub(super) fn ring(w: f32, h: f32, p: f32) -> [((f32, f32, f32, f32), &'static str); 8] {
    [
        ((0.0, 0.0, p, p), "left_up"),
        ((p - 1.0, 0.0, w - 2.0 * p + 2.0, p), "mid_up"),
        ((w - p, 0.0, p, p), "right_up"),
        ((0.0, p - 1.0, p, h - 2.0 * p + 2.0), "left_side"),
        ((w - p, p - 1.0, p, h - 2.0 * p + 2.0), "right_side"),
        ((0.0, h - p, p, p), "left_down"),
        ((p - 1.0, h - p, w - 2.0 * p + 2.0, p), "mid_down"),
        ((w - p, h - p, p, p), "right_down"),
    ]
}

// --- Behaviour --------------------------------------------------------------

/// A control was clicked: stage the edit. A radio's OFF half clears the bit and
/// its ON half sets it; a checkbox flips its own.
fn on_toggle(
    activate: On<Activate>,
    toggles: Query<&CosSetupToggle>,
    mut setup: ResMut<CosSetupState>,
) {
    let Ok(toggle) = toggles.get(activate.entity) else {
        return;
    };
    if toggle.on_state == setup.is_set(toggle.bit) {
        return;
    }
    setup.toggle(toggle.bit);
}

/// OK sends `0x7420` with the staged value; Cancel drops it.
fn on_command(
    activate: On<Activate>,
    commands: Query<&CosSetupCommand>,
    mut setup: ResMut<CosSetupState>,
    cos: Res<CosState>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    let Ok(command) = commands.get(activate.entity) else {
        return;
    };
    match command {
        CosSetupCommand::Cancel => setup.edited = setup.confirmed,
        CosSetupCommand::Ok => {
            let Some(pet) = cos.first_of_kind(CosKind::GrabPet) else {
                return;
            };
            let Ok(conn) = conn.single() else {
                return;
            };
            let request = PetSettingsChangeRequest {
                unique_id: pet.unique_id,
                settings_type: PET_SETTINGS_TYPE_GOLD,
                settings: setup.edited.0,
            };
            if let Err(e) = conn.get_sender().send(Packet::from(request).into()) {
                error!("network: failed to send PetSettingsChangeRequest: {}", e.0);
            }
        }
    }
}

/// Seed the page from `0x30C8` and adopt every `0xB420` the server sends —
/// including an unsolicited one, which is how the server reports that it turned
/// grabbing off on a full bag (`pet-pick-cos.md` §3, `COSPETERR_CANT_PICKITEM1`).
pub fn apply_cos_settings(
    mut acks: MessageReader<PetSettingsChangeResponse>,
    cos: Res<CosState>,
    mut setup: ResMut<CosSetupState>,
) {
    for ack in acks.read() {
        if !ack.success {
            // A rejected change leaves the confirmed value standing; the page
            // snaps back to it rather than keeping an edit the server refused.
            setup.edited = setup.confirmed;
            continue;
        }
        if let Some(settings) = ack.settings {
            setup.confirm(PickPetSettings(settings));
        }
    }
    // The summon blob carries the pick pet's current flags in the field
    // `CosBody::unk_f` — [S], from the tail grammar in
    // `docs/re/systems/pet-growth-cos.md` §3 (`unk:u32 x2, SettingsFlags:u32,
    // Name, ...`), which is exactly this field's slot for TypeID4 4. It is the
    // only settings value that exists before the first ack, so the page seeds
    // from it and says so rather than starting at zero.
    if cos.is_changed() {
        if let Some(pet) = cos.first_of_kind(CosKind::GrabPet) {
            if let Some(flags) = pet.body.unk_f {
                setup.confirm(PickPetSettings(flags));
            }
        }
    }
}

/// Mirror the staged value onto the radio dots and checkboxes.
pub fn refresh_cos_setup(
    setup: Res<CosSetupState>,
    asset_server: Res<AssetServer>,
    mut toggles: Query<(&CosSetupToggle, &mut ImageNode)>,
) {
    if !setup.is_changed() {
        return;
    }
    for (toggle, mut image) in toggles.iter_mut() {
        let lit = setup.is_set(toggle.bit) == toggle.on_state;
        let art = match (toggle.bit, lit) {
            // the three category controls are checkboxes; the two pairs are radios
            (
                PickPetSettings::GOLD | PickPetSettings::EQUIPMENT | PickPetSettings::OTHER_ITEMS,
                true,
            ) => CHECK_ON_DDJ,
            (
                PickPetSettings::GOLD | PickPetSettings::EQUIPMENT | PickPetSettings::OTHER_ITEMS,
                false,
            ) => CHECK_OFF_DDJ,
            (_, true) => RADIO_ON_DDJ,
            (_, false) => RADIO_OFF_DDJ,
        };
        let handle = asset_server.load(art);
        if image.image != handle {
            image.image = handle;
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The page is **exactly** the flag set: three checkboxes for the three
    /// category bits and two radio pairs for the two boolean bits — and the
    /// bits are 1/2/4/64/128, with nothing at 8/16/32 (the doc's negative
    /// result: those three textdata keys appear in zero resinfo files).
    #[test]
    fn setup_page_is_exactly_the_pick_pet_flag_set() {
        let checkbox_bits: Vec<u32> = CHECKBOXES.iter().map(|c| c.2).collect();
        assert_eq!(
            checkbox_bits,
            vec![
                PickPetSettings::GOLD,
                PickPetSettings::EQUIPMENT,
                PickPetSettings::OTHER_ITEMS
            ]
        );
        assert_eq!(checkbox_bits, vec![1, 2, 4]);
        let all: u32 = checkbox_bits.iter().fold(0, |a, b| a | b)
            | PickPetSettings::GRAB_ALL_ITEMS
            | PickPetSettings::ENABLED;
        assert_eq!(all, 1 | 2 | 4 | 64 | 128);
        // no control edits a bit at 8, 16 or 32
        assert_eq!(all & 0b0011_1000, 0);
    }

    /// Flipping each control produces exactly its bit in the encoded `u32`,
    /// and nothing else moves.
    #[test]
    fn each_control_flips_exactly_its_own_bit() {
        let mut setup = CosSetupState::default();
        for bit in [
            PickPetSettings::GOLD,
            PickPetSettings::EQUIPMENT,
            PickPetSettings::OTHER_ITEMS,
            PickPetSettings::GRAB_ALL_ITEMS,
            PickPetSettings::ENABLED,
        ] {
            let before = setup.edited.0;
            setup.toggle(bit);
            assert_eq!(setup.edited.0 ^ before, bit, "bit {bit} did not flip alone");
            assert!(setup.is_set(bit));
        }
        assert_eq!(setup.edited.0, 1 | 2 | 4 | 64 | 128);
        // ...and back down again
        for bit in [
            PickPetSettings::GOLD,
            PickPetSettings::EQUIPMENT,
            PickPetSettings::OTHER_ITEMS,
            PickPetSettings::GRAB_ALL_ITEMS,
            PickPetSettings::ENABLED,
        ] {
            setup.toggle(bit);
        }
        assert_eq!(setup.edited.0, 0);
    }

    /// Edits are staged: the confirmed value only moves when the server says
    /// so, and Cancel throws the staged value away.
    #[test]
    fn edits_are_staged_until_the_server_confirms() {
        let mut setup = CosSetupState::default();
        setup.confirm(PickPetSettings(PickPetSettings::ENABLED));
        setup.toggle(PickPetSettings::GOLD);
        assert!(setup.is_set(PickPetSettings::GOLD));
        // the confirmed value is untouched by the edit
        assert_eq!(setup.confirmed.0, PickPetSettings::ENABLED);
        // Cancel
        setup.edited = setup.confirmed;
        assert!(!setup.is_set(PickPetSettings::GOLD));
        // a server ack replaces both halves
        setup.confirm(PickPetSettings(
            PickPetSettings::ENABLED | PickPetSettings::OTHER_ITEMS,
        ));
        assert!(setup.is_set(PickPetSettings::OTHER_ITEMS));
        assert_eq!(setup.confirmed, setup.edited);
    }

    /// Every control sits inside the page rect the shell hosts
    /// (`ifcos.txt`'s `12,66,331,314`), and the radio row split we invented
    /// keeps both options inside the authored 260-wide row.
    #[test]
    fn setup_controls_fit_the_cos_page_rect() {
        let (pw, ph) = (331.0, 314.0);
        for (x, y, w, h) in [
            INNER_BOX_RECT,
            BG_TILE_RECT,
            TITLE_RECT,
            ONOFF_ROW,
            SCOPE_ROW,
            (OK_POS.0, OK_POS.1, BUTTON_SIZE.0, BUTTON_SIZE.1),
            (CANCEL_POS.0, CANCEL_POS.1, BUTTON_SIZE.0, BUTTON_SIZE.1),
        ] {
            assert!(x + w <= pw, "rect {x},{y},{w},{h} overflows the page width");
            assert!(
                y + h <= ph,
                "rect {x},{y},{w},{h} overflows the page height"
            );
        }
        // second radio option + its label stay inside the 260-wide row
        assert!(RADIO_OPTION_PITCH + RADIO_SIZE.0 <= ONOFF_ROW.2);
        assert_eq!(RADIO_OPTION_PITCH * 2.0, ONOFF_ROW.2);
    }
}
