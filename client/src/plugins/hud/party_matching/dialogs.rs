//! The matching board's four dialogs: Form party, Auto match, the leader's
//! applicant prompt, and the applicant's own wait bar.
//!
//! Idea: all four sit on the shared `msgbox2_window_` shell, and all four
//! confirm the same inset from the other side — `314-32 = 282`, `314-32 = 282`,
//! `360-32 = 328`, `365-32 = 333`. Four dialogs, four backgrounds, one constant.
//!
//! **Register and Auto are two dialogs, not one form with a flag.** They are a
//! byte-level fork sharing section 1's geometry, and the temptation to
//! parameterise them is exactly what loses the level range and the title:
//! Register *advertises a party* (purpose, level range, title, with the sharing
//! mode shown read-only because `ifsetpartymode` is where it is chosen), while
//! Auto *asks to be placed in one* (purpose, race, sharing preference, and no
//! title at all).
//!
//! Two honest gaps, both wire-level rather than layout:
//!
//! - **Auto has no opcode.** None of the ten registered matching opcodes
//!   carries a race preference or an auto verb, yet the original ships
//!   `UIIT_MSG_PARTYMATCH_AUTO_PROGRESS` and `_AUTO_FAILURE`, so the flow
//!   exists. Confirm therefore logs its intent and sends nothing; guessing a
//!   packet here is how a server drops the connection.
//! - **ReqJoin's Level / Race / Position / Guild** come from the applicant's
//!   member record, which is presence-masked — so they render only when the
//!   server's mask actually named them, and stay blank otherwise rather than
//!   being invented.

use bevy::ecs::relationship::RelatedSpawnerCommands;
use bevy::input_focus::tab_navigation::TabIndex;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::{EditableText, TextCursorStyle};
use bevy::ui_widgets::{Activate, Button};

use packets::agent::party::{
    PartyMatchCreationRequest, PartyMatchEditedRequest, PartyMatchJoinResponse, PartySetup,
    PARTY_PURPOSE_HUNTING, PARTY_PURPOSE_THIEF, PARTY_PURPOSE_TRADER,
};
use packets::Packet;

use crate::assets::FontAssets;
use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::game_window::abs_node;
use crate::plugins::hud::gauge::{gauge_art_node, gauge_crop_node, gauge_fill_width};
use crate::plugins::hud::modal_dialog::{modal_plate_node, modal_scrim_node, spawn_modal_frame};
use crate::plugins::hud::party::model::effective_setup;
use crate::plugins::hud::party::ui::PartyWindowRequest;
use crate::plugins::hud::party_matching::model::{MatchDialog, PartyMatchState};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::party::{PartyCreateSetup, PartyRoster};
use crate::plugins::textdata::{ClientMasteryData, ClientUiStrings};
use crate::plugins::ui_v2::style::{ImageButtonStyle, TargetColor};

const ART_COMMON: &str = "media://interface/ifcommon/";
const ART_PARTY: &str = "media://interface/party/";
const ART_FRAME: &str = "media://interface/frame/";

// --- Register / Auto shared geometry ----------------------------------------

const FORM_W: f32 = 314.0;
const REGISTER_H: f32 = 373.0;
const AUTO_H: f32 = 337.0;

/// `pt_key.ddj` is 108x24 and art-sized; the three section tabs sit at x 12.
const KEY_W: f32 = 108.0;
const KEY_H: f32 = 24.0;
const SECTION_KEYS: [f32; 3] = [39.0, 112.0, 185.0];
const SECTION_CAPTIONS: [f32; 3] = [50.0, 123.0, 196.0];
const SECTION_FRAMES: [f32; 3] = [66.0, 139.0, 212.0];
const SECTION_FRAME_X: f32 = 13.0;
const SECTION_FRAME_W: f32 = 287.0;
const SECTION_FRAME_H: f32 = 38.0;
/// Auto's third section is taller — it holds two radio groups, not two statics.
const AUTO_SECTION3_H: f32 = 72.0;
const CAPTION_RECT: (f32, f32, f32, f32) = (23.0, 0.0, 87.0, 11.0);
const GROUP_RECT: (f32, f32, f32, f32) = (30.0, 77.0, 272.0, 16.0);

/// Register section 2: the two level fields. `Min.`'s label and edit are
/// authored; `Max.`'s rects are not, so they mirror the pair at the same offset
/// — stated rather than presented as transcribed.
const LEVEL_MIN_LABEL: (f32, f32, f32, f32) = (23.0, 154.0, 32.0, 11.0);
const LEVEL_MIN_FIELD: (f32, f32, f32, f32) = (80.0, 152.0, 66.0, 15.0);
const LEVEL_MAX_LABEL: (f32, f32, f32, f32) = (155.0, 154.0, 32.0, 11.0);
const LEVEL_MAX_FIELD: (f32, f32, f32, f32) = (212.0, 152.0, 66.0, 15.0);

/// Register section 3 — two diamonds and two statics, no input control: the
/// mode is chosen in `ifsetpartymode`, not here. Read-only is not fixed,
/// though; the labels follow the byte (see [`sharing_labels`]).
const EXP_DECO: (f32, f32, f32, f32) = (22.0, 226.0, 12.0, 12.0);
const EXP_TEXT: (f32, f32, f32, f32) = (40.0, 226.0, 120.0, 11.0);
const ITEM_DECO: (f32, f32, f32, f32) = (160.0, 226.0, 12.0, 12.0);
const ITEM_TEXT: (f32, f32, f32, f32) = (178.0, 226.0, 120.0, 11.0);

/// Register section 4 — the title.
const TITLE_CAPTION: (f32, f32, f32, f32) = (22.0, 266.0, 142.0, 11.0);
const TITLE_FRAME: (f32, f32, f32, f32) = (12.0, 283.0, 287.0, 25.0);
const TITLE_FIELD: (f32, f32, f32, f32) = (16.0, 287.0, 279.0, 17.0);

const REGISTER_OK: (f32, f32, f32, f32) = (72.0, 329.0, 76.0, 24.0);
const REGISTER_CANCEL: (f32, f32, f32, f32) = (167.0, 329.0, 76.0, 24.0);
const AUTO_OK: (f32, f32, f32, f32) = (72.0, 298.0, 76.0, 24.0);
const AUTO_CANCEL: (f32, f32, f32, f32) = (167.0, 298.0, 76.0, 24.0);

// --- ReqJoin ----------------------------------------------------------------

const REQJOIN_W: f32 = 360.0;
const REQJOIN_H: f32 = 315.0;
const REQJOIN_DESC: (f32, f32, f32, f32) = (20.0, 59.0, 322.0, 35.0);
const REQJOIN_MASTERY_LABEL: (f32, f32, f32, f32) = (15.0, 123.0, 58.0, 11.0);
const REQJOIN_ICON_FRAMES: [(f32, f32, f32, f32); 2] =
    [(86.0, 119.0, 36.0, 36.0), (128.0, 119.0, 36.0, 36.0)];
const REQJOIN_ICONS: [(f32, f32, f32, f32); 2] =
    [(88.0, 121.0, 32.0, 32.0), (130.0, 121.0, 32.0, 32.0)];
const REQJOIN_ROWS: [((f32, f32, f32, f32), (f32, f32, f32, f32)); 4] = [
    ((184.0, 123.0, 58.0, 11.0), (269.0, 124.0, 58.0, 11.0)),
    ((184.0, 152.0, 58.0, 11.0), (270.0, 152.0, 58.0, 11.0)),
    ((16.0, 191.0, 58.0, 11.0), (98.0, 191.0, 231.0, 11.0)),
    ((15.0, 224.0, 58.0, 11.0), (98.0, 227.0, 229.0, 11.0)),
];
const REQJOIN_GUILD_FRAME: (f32, f32, f32, f32) = (87.0, 220.0, 246.0, 25.0);
/// `com_button` 76x24 with a 12 px gap — NOT the 88-wide mid button. Reading
/// the role instead of the block's own `DDJ=` line is what produced a
/// "zero gap" in an earlier draft of the RE work.
const REQJOIN_OK: (f32, f32, f32, f32) = (103.0, 266.0, 76.0, 24.0);
const REQJOIN_CANCEL: (f32, f32, f32, f32) = (191.0, 266.0, 76.0, 24.0);

// --- JoinProgress -----------------------------------------------------------

const PROGRESS_W: f32 = 365.0;
const PROGRESS_H: f32 = 149.0;
const PROGRESS_BG: (f32, f32, f32, f32) = (16.0, 40.0, 333.0, 93.0);
const PROGRESS_DESC: (f32, f32, f32, f32) = (16.0, 50.0, 335.0, 29.0);
/// Housing and fill nest with a uniform 6 px inset on all four sides.
const PROGRESS_HOUSING: (f32, f32, f32, f32) = (29.0, 98.0, 308.0, 24.0);
const PROGRESS_BAR: (f32, f32, f32, f32) = (35.0, 104.0, 296.0, 12.0);
/// How long the wait bar takes to fill. The original's timeout is code-side
/// (only the "no reply" string is in data), so this is ours and stated.
pub(crate) const PROGRESS_TIMEOUT_SECS: f32 = 30.0;

const LABEL_COLOR: Color = Color::srgb_u8(239, 218, 164);

#[derive(Component)]
pub struct MatchDialogRoot;
#[derive(Component, Clone, Copy)]
enum DialogButton {
    RegisterOk,
    RegisterCancel,
    AutoOk,
    AutoCancel,
    JoinAccept,
    JoinDecline,
    ProgressCancel,
}

impl DialogButton {
    /// The affirmative button of its dialog — what Enter presses
    /// (`hud::focus::HudDialog`). The progress dialog has none: it is a spinner
    /// with nothing to agree to.
    fn is_confirm(self) -> bool {
        matches!(
            self,
            DialogButton::RegisterOk | DialogButton::AutoOk | DialogButton::JoinAccept
        )
    }
}
/// Purpose radio option on the register/auto forms.
#[derive(Component, Clone, Copy)]
struct PurposeOption(u8);

/// The register dialog's party-title input, so Register-OK can read it back.
#[derive(Component)]
struct PartyTitleInput;

/// Vertically centres the 8.5px title text in the 17px field — the same job
/// `ui_v2::widgets::INPUT_PAD_TOP` does for the 12px login inputs, recomputed
/// for this field's own size rather than borrowed.
const TITLE_PAD_TOP: f32 = 2.0;

/// The register dialog's read-only sharing row, made clickable.
///
/// The row itself stays exactly what the data declares — two statics and two
/// diamonds, no input — because the sharing mode is inherited from the party,
/// not chosen when you advertise it. What it gains is a way to reach the place
/// that *does* choose it: the `ifsetpartymode` modal behind the roster page's
/// "Set" button. Without this the row reads as dead text and the setting looks
/// missing, which is exactly how it was reported.
#[derive(Component)]
struct OpenPartyModeRow;

/// The wait bar's crop node — the one thing in a dialog that changes while the
/// dialog stays the same dialog, so it is **repainted** rather than rebuilt.
#[derive(Component)]
pub struct JoinProgressFill;

/// Which dialog is up, without the parts of it that change while it is up.
///
/// This is what decides whether the dialog tree gets rebuilt, and it exists
/// because `is_changed()` is the wrong question. A dialog must be rebuilt when
/// it becomes a *different dialog* — not when some unrelated field of
/// `PartyMatchState` moves, and emphatically not when its own progress bar
/// ticks. `JoinProgress`'s `elapsed` is therefore deliberately absent here.
///
/// Rebuilding on any state change is what killed the buttons: the tree was
/// despawned and respawned every frame, so `Hovered` never survived to be
/// read and `Activate` never saw its `Pressed` marker again at pointer-up.
/// Sorting a column or a page arriving would have done the same thing to an
/// open dialog even after the per-frame tick was fixed.
///
/// The register key carries the **sharing byte** for the opposite reason: its
/// section 3 renders that byte, and the byte changes behind the dialog's back.
/// The mode modal opens *over* the register dialog, so OK leaves the register
/// dialog standing and only `PartyCreateSetup` moves — with the byte out of the
/// key, the labels stayed on whatever they said when the dialog was built and
/// the setting looked as though it had not taken.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DialogKey {
    None,
    Register(Option<u32>, u8),
    Auto,
    ReqJoin(u32),
    JoinProgress(u32),
}

fn dialog_key(dialog: &MatchDialog, setup: u8) -> DialogKey {
    match dialog {
        MatchDialog::None => DialogKey::None,
        MatchDialog::Register { editing } => DialogKey::Register(*editing, setup),
        MatchDialog::Auto => DialogKey::Auto,
        MatchDialog::ReqJoin(notify) => DialogKey::ReqJoin(notify.request_id),
        MatchDialog::JoinProgress { number, .. } => DialogKey::JoinProgress(*number),
    }
}

/// Rebuild whichever dialog is up — only when it becomes a different one.
#[allow(clippy::too_many_arguments)]
pub fn sync_match_dialogs(
    state: Res<PartyMatchState>,
    ui_strings: Res<ClientUiStrings>,
    mastery_data: Res<ClientMasteryData>,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    cameras: Query<Entity, With<Camera2d>>,
    roots: Query<Entity, With<MatchDialogRoot>>,
    // The same pair `on_dialog_button` takes, so what section 3 *shows* and what
    // registration *sends* come out of one function and cannot drift.
    roster: Option<Res<PartyRoster>>,
    pending: Option<Res<PartyCreateSetup>>,
    mut built: Local<Option<DialogKey>>,
    mut commands: Commands,
) {
    let setup = effective_setup(
        roster.as_deref(),
        pending.map(|pending| pending.0).unwrap_or_default(),
    )
    .0;
    let key = dialog_key(&state.dialog, setup);
    if *built == Some(key) {
        return;
    }
    *built = Some(key);
    for root in roots.iter() {
        commands.entity(root).despawn();
    }
    if matches!(state.dialog, MatchDialog::None) {
        return;
    }
    // `.iter().next()` rather than `.single()`, matching the under-bar
    // popup: `single()` is an ERROR when the world holds more than one
    // `Camera2d`, and this path silently returning would look exactly like a
    // dead button — the dialog would never appear and nothing would say why.
    let Some(camera) = cameras.iter().next() else {
        warn!("party match: no 2d camera, dialog not spawned");
        return;
    };
    let s = hud_scale();
    let font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    };

    let (plate_w, plate_h) = match &state.dialog {
        MatchDialog::Register { .. } => (FORM_W, REGISTER_H),
        MatchDialog::Auto => (FORM_W, AUTO_H),
        MatchDialog::ReqJoin(_) => (REQJOIN_W, REQJOIN_H),
        MatchDialog::JoinProgress { .. } => (PROGRESS_W, PROGRESS_H),
        MatchDialog::None => return,
    };

    // Each builder hands back its own affirmative button (the wait dialog has
    // none), so the root can point Enter at it — `hud::focus::HudDialog`.
    let mut confirm_button = None;
    let root = commands
        .spawn((
            MatchDialogRoot,
            Name::from("Party Match Dialog"),
            GlobalZIndex(95),
            modal_scrim_node(),
            UiTargetCamera(camera),
        ))
        .with_children(|scrim| {
            scrim
                .spawn(modal_plate_node(plate_w, plate_h, s))
                .with_children(|plate| {
                    spawn_modal_frame(plate, &asset_server, plate_w, plate_h, s);
                    // Every dialog's background is `host - 32` wide at the
                    // uniform inset; only the height differs.
                    let bg = (16.0, 40.0, plate_w - 32.0, plate_h - 56.0);
                    plate.spawn((
                        abs_node(bg, s),
                        ImageNode {
                            image: asset_server
                                .load(format!("{ART_COMMON}bg_tile/com_bg_tile_b.ddj")),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));

                    confirm_button = match &state.dialog {
                        MatchDialog::Register { editing } => build_register(
                            plate,
                            &asset_server,
                            &fonts,
                            &ui_strings,
                            &state,
                            editing.is_some(),
                            setup,
                            &font,
                            s,
                        ),
                        MatchDialog::Auto => {
                            build_auto(plate, &asset_server, &fonts, &ui_strings, &state, &font, s)
                        }
                        MatchDialog::ReqJoin(notify) => build_reqjoin(
                            plate,
                            &asset_server,
                            &fonts,
                            &ui_strings,
                            &mastery_data,
                            notify,
                            &font,
                            s,
                        ),
                        MatchDialog::JoinProgress { elapsed, .. } => build_progress(
                            plate,
                            &asset_server,
                            &fonts,
                            &ui_strings,
                            *elapsed,
                            &font,
                            s,
                        ),
                        MatchDialog::None => None,
                    };
                });
        })
        .id();
    if let Some(confirm_button) = confirm_button {
        commands
            .entity(root)
            .insert(crate::plugins::hud::focus::HudDialog { confirm_button });
    }
}

/// Repaint the wait bar without rebuilding the dialog around it.
///
/// This is the other half of keying the rebuild on the dialog's *identity*:
/// `elapsed` is excluded from [`DialogKey`], so ticking it no longer respawns
/// the tree — which means the bar needs someone to move it. It is the same
/// "spawn once, repaint" split every other window here uses, and it goes
/// through `gauge_fill_width` so the crop rounds exactly like every other
/// gauge (the art is never resized; only the crop node's width moves).
pub fn refresh_join_progress(
    state: Res<PartyMatchState>,
    mut fills: Query<&mut Node, With<JoinProgressFill>>,
) {
    let MatchDialog::JoinProgress { elapsed, .. } = &state.dialog else {
        return;
    };
    let width = gauge_fill_width(
        (*elapsed / PROGRESS_TIMEOUT_SECS).clamp(0.0, 1.0),
        PROGRESS_BAR.2 * hud_scale(),
    );
    for mut node in fills.iter_mut() {
        node.width = width;
    }
}

/// The three purposes the corpus actually names. `Quest` (wire value 1) is
/// deliberately absent: no "Quest" string exists anywhere in textuisystem while
/// two independent spec sources give the constant, so the conflict is left
/// unresolved rather than papered over with an invented label.
const PURPOSES: [(u8, &str, &str); 3] = [
    (
        PARTY_PURPOSE_HUNTING,
        "UIIT_CTL_PARTYMATCH_PSEARCH_OBJECT_HUNT",
        "Hunting",
    ),
    (
        PARTY_PURPOSE_TRADER,
        "UIIT_CTL_PARTYMATCH_PSEARCH_OBJECT_TRADE",
        "Trade",
    ),
    (
        PARTY_PURPOSE_THIEF,
        "UIIT_CTL_PARTYMATCH_RECORD_OBJECT_THIEF",
        "Thief Union",
    ),
];

#[allow(clippy::too_many_arguments)]
/// The `(key, fallback)` pair each half of the register dialog's section 3
/// renders, chosen by the sharing byte.
///
/// The strings are the mode modal's own — one contiguous run in
/// `textuisystem` (L502-L509) that appears in no resinfo file at all — so the
/// dialog that *shows* the mode and the modal that *sets* it name it the same
/// way. Section 3 stays read-only, which is what the data declares; read-only
/// is not the same as fixed, and hardcoding the `_SHARE` half made it read
/// "Auto Share" whatever had been chosen.
fn sharing_labels(setup: u8) -> [(&'static str, &'static str); 2] {
    let setup = PartySetup(setup);
    [
        if setup.is_exp_shared() {
            ("UIIT_STT_PARTY_EXP_SHARE", "Exp Auto Share")
        } else {
            ("UIIT_STT_PARTY_EXP_SELF", "Exp Free-For-All")
        },
        if setup.is_item_shared() {
            ("UIIT_STT_PARTY_ITEM_SHARE", "Item Auto Share")
        } else {
            ("UIIT_STT_PARTY_ITEM_SELF", "Item Free-For-All")
        },
    ]
}

fn build_register(
    plate: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
    state: &PartyMatchState,
    editing: bool,
    setup: u8,
    font: &impl Fn(f32) -> TextFont,
    s: f32,
) -> Option<Entity> {
    let captions = [
        ("UIIT_CTL_PARTYMATCH_RECORD_OBJECT", "Party Objective"),
        ("UIIT_CTL_PARTYMATCH_RECORD_LIMITLEVEL", "Level Restrict"),
        ("UIIT_CTL_PARTYMATCH_RECORD_PARTYTYPE", "Party type"),
    ];
    for (index, (key, fallback)) in captions.iter().enumerate() {
        spawn_section(
            plate,
            asset_server,
            index,
            SECTION_FRAME_H,
            ui_strings.get_or(key, fallback),
            font(8.5),
            s,
        );
    }

    spawn_purpose_group(plate, asset_server, ui_strings, state.form.purpose, font, s);

    // Section 2 — the level range.
    for (label_rect, field_rect, key, fallback, value) in [
        (
            LEVEL_MIN_LABEL,
            LEVEL_MIN_FIELD,
            "UIIT_CTL_PARTYMATCH_RECORD_LIMITLEVEL_MIN",
            "Min.",
            state.form.level_min,
        ),
        (
            LEVEL_MAX_LABEL,
            LEVEL_MAX_FIELD,
            "UIIT_CTL_PARTYMATCH_RECORD_LIMITLEVEL_MAX",
            "Max.",
            state.form.level_max,
        ),
    ] {
        plate.spawn((
            Text::new(ui_strings.get_or(key, fallback).to_string()),
            font(8.5),
            TextColor(LABEL_COLOR),
            abs_node(label_rect, s),
            Pickable::IGNORE,
        ));
        plate.spawn((
            abs_node(field_rect, s),
            ImageNode {
                image: asset_server.load(format!("{ART_COMMON}bg_tile/com_bg_tile_e.ddj")),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
        plate.spawn((
            Text::new(value.to_string()),
            font(8.5),
            TextColor(Color::WHITE),
            TextLayout::justify(Justify::Center),
            abs_node((field_rect.0, field_rect.1 + 2.0, field_rect.2, 11.0), s),
            Pickable::IGNORE,
        ));
    }

    // Section 3 — read-only sharing mode, two diamonds and two statics.
    //
    // Read-only is the data's own reading (no input control is declared here),
    // but it is not *static*: each flag picks one of the string pair the mode
    // modal owns, `_SHARE` when the bit is set and `_SELF` when it is not.
    // Hardcoding the `_SHARE` half made the row read "Auto Share" whatever the
    // mode was, which is what made the setting look as though it never took.
    let [exp, item] = sharing_labels(setup);
    for (deco, text_rect, key, fallback) in [
        (EXP_DECO, EXP_TEXT, exp.0, exp.1),
        (ITEM_DECO, ITEM_TEXT, item.0, item.1),
    ] {
        plate.spawn((
            abs_node(deco, s),
            ImageNode {
                image: asset_server.load(format!("{ART_COMMON}com_diamond.ddj")),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
        plate.spawn((
            Text::new(ui_strings.get_or(key, fallback).to_string()),
            font(8.5),
            TextColor(LABEL_COLOR),
            abs_node(text_rect, s),
            Pickable::IGNORE,
        ));
    }

    // ...and the whole row opens the modal that owns the setting.
    plate
        .spawn((
            OpenPartyModeRow,
            Button,
            Hovered::default(),
            abs_node(
                (
                    SECTION_FRAME_X,
                    SECTION_FRAMES[2],
                    SECTION_FRAME_W,
                    SECTION_FRAME_H,
                ),
                s,
            ),
        ))
        .observe(on_open_party_mode)
        .with_children(|row| {
            row.spawn((
                Text::new(
                    ui_strings
                        .get_or("UIIT_CTL_PARTY_SETTING", "Set")
                        .to_string(),
                ),
                font(8.0),
                TextColor(LABEL_COLOR),
                TextLayout::justify(Justify::Right),
                Node {
                    position_type: PositionType::Absolute,
                    right: Val::Px(6.0 * s),
                    top: Val::Px(4.0 * s),
                    width: Val::Px(40.0 * s),
                    ..default()
                },
                Pickable::IGNORE,
            ));
        });

    // Section 4 — the title.
    plate.spawn((
        Text::new(
            ui_strings
                .get_or("UIIT_STT_LETTER_TITLE", "Title")
                .to_string(),
        ),
        font(8.5),
        TextColor(LABEL_COLOR),
        abs_node(TITLE_CAPTION, s),
        Pickable::IGNORE,
    ));
    spawn_nine_slice(
        plate,
        asset_server,
        &format!("{ART_FRAME}frame_msg_"),
        TITLE_FRAME,
        8.0,
        s,
    );
    // A real input, not a readout. This was a plain `Text`, so the party title
    // could never be typed — which also made the Change-entry flow untestable,
    // since there was nothing to edit. The component set is the one
    // `ui_v2::widgets::text_input` defines; that helper is a BSN scene and this
    // dialog is built with `commands.spawn`, so the components go on directly
    // rather than through it, and the *set* is what must not diverge.
    let mut field = abs_node(TITLE_FIELD, s);
    field.padding = UiRect::top(Val::Px(TITLE_PAD_TOP * s));
    plate.spawn((
        PartyTitleInput,
        // `EditableText` owns its own buffer, so the seed goes through `new`
        // — a `Text` component alongside it would render nothing and the field
        // would open blank on Change. This is what pre-fills the entry being
        // edited (`MatchAction::Modify` has always seeded `state.form.title`).
        EditableText {
            visible_lines: Some(1.0),
            allow_newlines: false,
            ..EditableText::new(state.form.title.clone())
        },
        font(8.5),
        TextColor(Color::WHITE),
        TargetColor(Srgba::WHITE),
        TextCursorStyle {
            color: Color::WHITE,
            ..default()
        },
        TabIndex(0),
        field,
    ));

    let ok_key = if editing {
        ("UIIS_CTL_CONFIRM", "Change")
    } else {
        ("UIIS_CTL_CONFIRM", "OK")
    };
    spawn_footer(
        plate,
        asset_server,
        fonts,
        ui_strings,
        [
            (DialogButton::RegisterOk, REGISTER_OK, ok_key.0, ok_key.1),
            (
                DialogButton::RegisterCancel,
                REGISTER_CANCEL,
                "UIIS_CTL_CANCEL",
                "Cancel",
            ),
        ],
        s,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_auto(
    plate: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
    state: &PartyMatchState,
    font: &impl Fn(f32) -> TextFont,
    s: f32,
) -> Option<Entity> {
    let captions = [
        ("UIIT_CTL_PARTYMATCH_RECORD_OBJECT", "Party Objective"),
        ("UIIT_CTL_PARTYMATCH_PSEARCH_LIST_RACE", "Race"),
        ("UIIT_CTL_PARTYMATCH_RECORD_PARTYTYPE", "Party type"),
    ];
    for (index, (key, fallback)) in captions.iter().enumerate() {
        let height = if index == 2 {
            AUTO_SECTION3_H
        } else {
            SECTION_FRAME_H
        };
        spawn_section(
            plate,
            asset_server,
            index,
            height,
            ui_strings.get_or(key, fallback),
            font(8.5),
            s,
        );
    }

    spawn_purpose_group(plate, asset_server, ui_strings, state.form.purpose, font, s);

    // Section 2 — the race preference, three options in one group container.
    let races = [
        ("UIIT_CTL_PARTYMATCH_RECORD_RACE_CHN", "China", Some(0u8)),
        ("UIIT_CTL_PARTYMATCH_RECORD_RACE_EUR", "Europe", Some(1)),
        ("UIIT_CTL_PARTYMATCH_RECORD_RACE_OPEN", "Open", None),
    ];
    for (index, (key, fallback, value)) in races.iter().enumerate() {
        let rect = (
            GROUP_RECT.0 + index as f32 * 90.0,
            150.0,
            88.0,
            GROUP_RECT.3,
        );
        spawn_radio_row(
            plate,
            asset_server,
            rect,
            ui_strings.get_or(key, fallback),
            state.form.race == *value,
            font(8.5),
            s,
        );
    }

    spawn_footer(
        plate,
        asset_server,
        fonts,
        ui_strings,
        [
            (DialogButton::AutoOk, AUTO_OK, "UIIS_CTL_CONFIRM", "OK"),
            (
                DialogButton::AutoCancel,
                AUTO_CANCEL,
                "UIIS_CTL_CANCEL",
                "Cancel",
            ),
        ],
        s,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_reqjoin(
    plate: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
    mastery_data: &ClientMasteryData,
    notify: &packets::agent::party::PartyMatchJoinNotify,
    font: &impl Fn(f32) -> TextFont,
    s: f32,
) -> Option<Entity> {
    let name = notify.applicant.name.clone().unwrap_or_default();
    let description = ui_strings
        .get_plain_or(
            "UIIT_STT_PARTYMATCH_JOIN_REQUEST",
            "[%s] has requested to join this party. Confirm?",
        )
        .replace("%s", &name);
    plate.spawn((
        Text::new(description),
        font(9.0),
        TextColor(Color::WHITE),
        abs_node(REQJOIN_DESC, s),
        Pickable::IGNORE,
    ));

    plate.spawn((
        Text::new(
            ui_strings
                .get_or("UIIT_STT_PARTYMATCH_JOINREQUEST_MASTERY", "Mastery")
                .to_string(),
        ),
        font(8.5),
        TextColor(LABEL_COLOR),
        abs_node(REQJOIN_MASTERY_LABEL, s),
        Pickable::IGNORE,
    ));
    let masteries = [notify.mastery_primary, notify.mastery_secondary];
    for index in 0..2 {
        plate.spawn((
            abs_node(REQJOIN_ICON_FRAMES[index], s),
            ImageNode {
                image: asset_server.load(format!("{ART_PARTY}pt_icon_frame02.ddj")),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
        if let Some(icon) = mastery_data
            .get(masteries[index])
            .and_then(|info| info.icon.clone())
        {
            plate.spawn((
                abs_node(REQJOIN_ICONS[index], s),
                ImageNode {
                    image: asset_server.load(icon),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ));
        }
    }

    // Level / Race / Position / Guild. The applicant arrives as a
    // presence-masked record, so a field the mask did not name stays blank
    // rather than being invented.
    let rows = [
        (
            "UIIT_STT_LEVEL",
            "Level",
            notify
                .applicant
                .level
                .map(|level| level.to_string())
                .unwrap_or_default(),
        ),
        (
            "UIIT_CTL_PARTYMATCH_PSEARCH_LIST_RACE",
            "Race",
            String::new(),
        ),
        (
            "UIIT_CTL_PARTYMATCH_JOINREQUEST_PLACE",
            "Position",
            String::new(),
        ),
        (
            "UIIT_CTL_PARTYMATCH_JOINREQUEST_GUILD",
            "Guild",
            notify.applicant.guild_name.clone().unwrap_or_default(),
        ),
    ];
    spawn_nine_slice(
        plate,
        asset_server,
        &format!("{ART_FRAME}frame_msg_"),
        REQJOIN_GUILD_FRAME,
        8.0,
        s,
    );
    for (index, (key, fallback, value)) in rows.iter().enumerate() {
        let (label_rect, value_rect) = REQJOIN_ROWS[index];
        plate.spawn((
            Text::new(ui_strings.get_or(key, fallback).to_string()),
            font(8.5),
            TextColor(LABEL_COLOR),
            abs_node(label_rect, s),
            Pickable::IGNORE,
        ));
        plate.spawn((
            Text::new(value.clone()),
            font(8.5),
            TextColor(Color::WHITE),
            abs_node(value_rect, s),
            Pickable::IGNORE,
        ));
    }

    spawn_footer(
        plate,
        asset_server,
        fonts,
        ui_strings,
        [
            (
                DialogButton::JoinAccept,
                REQJOIN_OK,
                "UIIT_CTL_PARTYMATCH_JOINREQUEST_CONSENT",
                "Confirm",
            ),
            (
                DialogButton::JoinDecline,
                REQJOIN_CANCEL,
                "UIIT_CTL_PARTYMATCH_JOINREQUEST_REFUSE",
                "Cancel",
            ),
        ],
        s,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_progress(
    plate: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
    elapsed: f32,
    font: &impl Fn(f32) -> TextFont,
    s: f32,
) -> Option<Entity> {
    plate.spawn((
        abs_node(PROGRESS_BG, s),
        ImageNode {
            image: asset_server.load(format!("{ART_COMMON}bg_tile/com_bg_tile_b.ddj")),
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Pickable::IGNORE,
    ));
    plate.spawn((
        Text::new(
            ui_strings
                .get_plain_or("UIIT_STT_PARTYMATCH_JOIN_PROGRESS", "Joining...")
                .replace("%s", ""),
        ),
        font(9.0),
        TextColor(Color::WHITE),
        abs_node(PROGRESS_DESC, s),
        Pickable::IGNORE,
    ));
    plate.spawn((
        abs_node(PROGRESS_HOUSING, s),
        ImageNode {
            image: asset_server.load(format!("{ART_PARTY}pt_progress_window.ddj")),
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Pickable::IGNORE,
    ));

    // The bar is a CROP, like every other CIFGauge — the art is never resized.
    let fraction = (elapsed / PROGRESS_TIMEOUT_SECS).clamp(0.0, 1.0);
    let (w, h) = (PROGRESS_BAR.2 * s, PROGRESS_BAR.3 * s);
    let mut track = abs_node(PROGRESS_BAR, s);
    track.overflow = Overflow::clip();
    plate
        .spawn((track, Pickable::IGNORE))
        .with_children(|track| {
            track
                .spawn((
                    JoinProgressFill,
                    gauge_crop_node(gauge_fill_width(fraction, w), h),
                    Pickable::IGNORE,
                ))
                .with_children(|crop| {
                    crop.spawn((
                        ImageNode {
                            image: asset_server.load(format!("{ART_PARTY}pt_progress.ddj")),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                        gauge_art_node(w, h),
                        Pickable::IGNORE,
                    ));
                });
        });

    spawn_footer(
        plate,
        asset_server,
        fonts,
        ui_strings,
        [
            (
                DialogButton::ProgressCancel,
                (145.0, 124.0, 76.0, 24.0),
                "UIIS_CTL_CANCEL",
                "Cancel",
            ),
            // The second slot is unused: the wait dialog has one button.
            (
                DialogButton::ProgressCancel,
                (0.0, -100.0, 0.0, 0.0),
                "",
                "",
            ),
        ],
        s,
    )
}

// --- Shared builders --------------------------------------------------------

/// One numbered section: its `pt_key` tab, its caption and its sub-frame.
fn spawn_section(
    plate: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    index: usize,
    frame_h: f32,
    caption: &str,
    font: TextFont,
    s: f32,
) {
    plate.spawn((
        abs_node((12.0, SECTION_KEYS[index], KEY_W, KEY_H), s),
        ImageNode {
            image: asset_server.load(format!("{ART_PARTY}pt_key.ddj")),
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Pickable::IGNORE,
    ));
    plate.spawn((
        Text::new(caption.to_string()),
        font,
        TextColor(LABEL_COLOR),
        abs_node(
            (
                CAPTION_RECT.0,
                SECTION_CAPTIONS[index],
                CAPTION_RECT.2,
                CAPTION_RECT.3,
            ),
            s,
        ),
        Pickable::IGNORE,
    ));
    spawn_nine_slice(
        plate,
        asset_server,
        "media://interface/inventory/int_window_",
        (
            SECTION_FRAME_X,
            SECTION_FRAMES[index],
            SECTION_FRAME_W,
            frame_h,
        ),
        8.0,
        s,
    );
}

/// The purpose radio group: one container block in the data, three options.
fn spawn_purpose_group(
    plate: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    ui_strings: &ClientUiStrings,
    selected: u8,
    font: &impl Fn(f32) -> TextFont,
    s: f32,
) {
    for (index, (value, key, fallback)) in PURPOSES.iter().enumerate() {
        let rect = (
            GROUP_RECT.0 + index as f32 * 90.0,
            GROUP_RECT.1,
            88.0,
            GROUP_RECT.3,
        );
        let mut entity = plate.spawn((
            PurposeOption(*value),
            Button,
            Hovered::default(),
            abs_node(rect, s),
        ));
        entity.observe(on_purpose_option);
        entity.with_children(|row| {
            row.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Px(16.0 * s),
                    height: Val::Px(16.0 * s),
                    ..default()
                },
                ImageNode {
                    image: asset_server.load(radio_art(selected == *value)),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ));
            row.spawn((
                Text::new(ui_strings.get_or(key, fallback).to_string()),
                font(8.5),
                TextColor(Color::WHITE),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(20.0 * s),
                    top: Val::Px(2.0 * s),
                    width: Val::Px(68.0 * s),
                    ..default()
                },
                Pickable::IGNORE,
            ));
        });
    }
}

/// A read-only radio row (the auto dialog's race options).
fn spawn_radio_row(
    plate: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    rect: (f32, f32, f32, f32),
    label: &str,
    selected: bool,
    font: TextFont,
    s: f32,
) {
    plate.spawn((
        abs_node((rect.0, rect.1, 16.0, 16.0), s),
        ImageNode {
            image: asset_server.load(radio_art(selected)),
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Pickable::IGNORE,
    ));
    plate.spawn((
        Text::new(label.to_string()),
        font,
        TextColor(Color::WHITE),
        abs_node((rect.0 + 20.0, rect.1 + 2.0, rect.2 - 20.0, 11.0), s),
        Pickable::IGNORE,
    ));
}

fn radio_art(on: bool) -> String {
    format!(
        "{ART_COMMON}com_radiobutton_{}.ddj",
        if on { "on" } else { "off" }
    )
}

fn spawn_footer(
    plate: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
    buttons: [(DialogButton, (f32, f32, f32, f32), &str, &str); 2],
    s: f32,
) -> Option<Entity> {
    let mut confirm_button = None;
    for (button, rect, key, fallback) in buttons {
        if rect.2 <= 0.0 {
            continue;
        }
        let is_confirm = button.is_confirm();
        let mut spawned = plate.spawn((
            button,
            Button,
            Hovered::default(),
            ImageButtonStyle {
                normal: asset_server.load(format!("{ART_COMMON}com_button.ddj")),
                hover: asset_server.load(format!("{ART_COMMON}com_button_focus.ddj")),
                press: asset_server.load(format!("{ART_COMMON}com_button_press.ddj")),
                ..Default::default()
            },
            ImageNode {
                image: asset_server.load(format!("{ART_COMMON}com_button.ddj")),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            abs_node(rect, s),
        ));
        spawned.observe(on_dialog_button);
        if is_confirm {
            confirm_button = Some(spawned.id());
        }
        spawned.with_children(|button| {
            button.spawn((
                Text::new(ui_strings.get_or(key, fallback).to_string()),
                TextFont {
                    font: fonts.two.clone().into(),
                    font_size: FontSize::Px(8.5 * s),
                    ..default()
                },
                TextColor(Color::WHITE),
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
    confirm_button
}

fn spawn_nine_slice(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    dir: &str,
    rect: (f32, f32, f32, f32),
    corner: f32,
    s: f32,
) {
    let (x, y, w, h) = rect;
    let (iw, ih) = ((w - 2.0 * corner).max(0.0), (h - 2.0 * corner).max(0.0));
    for (piece, piece_rect) in [
        ("left_up", (x, y, corner, corner)),
        ("mid_up", (x + corner, y, iw, corner)),
        ("right_up", (x + w - corner, y, corner, corner)),
        ("left_side", (x, y + corner, corner, ih)),
        ("right_side", (x + w - corner, y + corner, corner, ih)),
        ("left_down", (x, y + h - corner, corner, corner)),
        ("mid_down", (x + corner, y + h - corner, iw, corner)),
        (
            "right_down",
            (x + w - corner, y + h - corner, corner, corner),
        ),
    ] {
        parent.spawn((
            abs_node(piece_rect, s),
            ImageNode {
                image: asset_server.load(format!("{dir}{piece}.ddj")),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
    }
}

// --- Observers --------------------------------------------------------------

fn on_open_party_mode(_: On<Activate>, mut requests: MessageWriter<PartyWindowRequest>) {
    requests.write(PartyWindowRequest::Setting);
}

fn on_purpose_option(
    activate: On<Activate>,
    options: Query<&PurposeOption>,
    mut state: ResMut<PartyMatchState>,
) {
    if let Ok(option) = options.get(activate.entity) {
        state.form.purpose = option.0;
    }
}

fn on_dialog_button(
    activate: On<Activate>,
    buttons: Query<&DialogButton>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut state: ResMut<PartyMatchState>,
    // Both optional: an observer panics on a missing `Res` exactly as a system
    // does, and the headless netcheck harness builds neither resource.
    roster: Option<Res<PartyRoster>>,
    pending: Option<Res<PartyCreateSetup>>,
    title: Query<&EditableText, With<PartyTitleInput>>,
) {
    let Ok(button) = buttons.get(activate.entity) else {
        return;
    };
    match button {
        DialogButton::RegisterOk => {
            let editing = match &state.dialog {
                MatchDialog::Register { editing } => *editing,
                _ => None,
            };
            // Take the title from the field the player typed into, not from
            // the form snapshot: the field owns the live value and the form
            // only ever held the seed. Persisted back so a re-opened dialog
            // (or a Change on the same entry) shows what was entered.
            if let Ok(input) = title.single() {
                let typed = input.value().to_string().trim().to_string();
                if state.form.title != typed {
                    state.form.title = typed;
                }
            }
            let form = state.form.clone();
            let setup = effective_setup(
                roster.as_deref(),
                pending.map(|pending| pending.0).unwrap_or_default(),
            )
            .0;
            let packet = match editing {
                Some(party_number) => Packet::from(PartyMatchEditedRequest {
                    party_number,
                    unknown: 0,
                    setup,
                    purpose: form.purpose,
                    level_min: form.level_min,
                    level_max: form.level_max,
                    title: form.title.clone(),
                }),
                None => Packet::from(PartyMatchCreationRequest {
                    party_number: 0,
                    unknown: 0,
                    setup,
                    purpose: form.purpose,
                    level_min: form.level_min,
                    level_max: form.level_max,
                    title: form.title.clone(),
                }),
            };
            send(&conn, packet);
            // The dialog stays up until the ack: 0xB069/0xB06A close it on
            // success and report on failure, so a refused registration does not
            // silently look like it worked.
        }
        DialogButton::AutoOk => {
            // UNKNOWN: no registered opcode carries an auto verb or a race
            // preference. Sending a guess is what makes a vSRO server drop the
            // connection, so this records the intent and stops.
            warn!(
                "party match: auto-match confirm has no wire opcode (purpose={}, race={:?}) — \
                 capture 0x706x while the original client confirms this dialog to resolve it",
                state.form.purpose, state.form.race
            );
            state.dialog = MatchDialog::None;
        }
        DialogButton::JoinAccept | DialogButton::JoinDecline => {
            if let MatchDialog::ReqJoin(notify) = &state.dialog {
                let accept = matches!(button, DialogButton::JoinAccept);
                send(
                    &conn,
                    Packet::from(PartyMatchJoinResponse {
                        request_id: notify.request_id,
                        join_id: notify.join_id,
                        accept: u8::from(accept),
                    }),
                );
            }
            state.dialog = MatchDialog::None;
        }
        DialogButton::RegisterCancel | DialogButton::AutoCancel | DialogButton::ProgressCancel => {
            state.dialog = MatchDialog::None;
        }
    }
}

fn send(conn: &Query<&SilkroadConnection, With<AgentConnection>>, packet: Packet) {
    let Ok(conn) = conn.single() else {
        warn!("party match: no agent connection");
        return;
    };
    if let Err(e) = conn.get_sender().send(packet.into()) {
        error!("network: failed to send party match packet: {}", e.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use packets::agent::party::{PartyData, PartyMemberCore, PartyMemberMask};

    fn roster_with(setup: u8) -> PartyRoster {
        let mut roster = PartyRoster::default();
        roster.apply_data(&PartyData {
            presence: 0x03,
            party_number: 9,
            master_join_id: Some(1),
            setup: Some(setup),
            member_count: Some(1),
            members: vec![PartyMemberCore {
                presence: PartyMemberMask::MEMBER_ID | PartyMemberMask::NAME,
                member_id: Some(1),
                name: Some("Ahri".into()),
                ..Default::default()
            }],
        });
        roster
    }

    /// The defect: 0x7069 with `party_number = 0` **creates** the party, so its
    /// `setup` byte is the party's sharing mode. It was hardcoded to 0, which
    /// is why nothing the mode modal chose ever survived the trip.
    #[test]
    fn a_new_party_is_advertised_with_the_mode_the_player_chose() {
        let chosen = PartySetup(PartySetup::EXP_SHARED | PartySetup::ITEM_SHARED);
        let sent = effective_setup(None, chosen).0;

        assert_eq!(sent, chosen.0);
        assert!(
            PartySetup(sent).is_exp_shared(),
            "a chosen EXP share must never serialise as 0"
        );
        assert_eq!(PartySetup(sent).capacity(), 8);
    }

    /// Once the party exists its mode is the server's, not the board's: an
    /// advertisement echoes it rather than rewriting it from a pending byte
    /// that may be stale.
    #[test]
    fn an_existing_partys_mode_is_echoed_not_overwritten() {
        let roster = roster_with(PartySetup::ITEM_SHARED);
        // ...even when the pending choice disagrees
        let pending = PartySetup(PartySetup::EXP_SHARED);

        assert_eq!(
            effective_setup(Some(&roster), pending).0,
            PartySetup::ITEM_SHARED
        );
        // an inactive roster is "no party", pending wins again
        assert_eq!(
            effective_setup(Some(&PartyRoster::default()), pending),
            pending
        );
    }

    /// The rebuild key must ignore everything that moves while a dialog stays
    /// the same dialog — `elapsed` above all, since it changes every frame.
    /// If it ever leaks in, the wait dialog respawns per frame and its Cancel
    /// button goes dead exactly the way the Form-party one did.
    #[test]
    fn the_rebuild_key_ignores_the_ticking_bar() {
        let early = MatchDialog::JoinProgress {
            number: 7,
            elapsed: 0.0,
        };
        let late = MatchDialog::JoinProgress {
            number: 7,
            elapsed: 12.5,
        };
        assert_eq!(dialog_key(&early, 0), dialog_key(&late, 0));

        // ...but a different party really is a different dialog
        let other = MatchDialog::JoinProgress {
            number: 8,
            elapsed: 0.0,
        };
        assert_ne!(dialog_key(&early, 0), dialog_key(&other, 0));
    }

    /// Section 3 is read-only, not fixed: each half follows its own bit. This
    /// is the defect's own shape — the row read "Auto Share" whatever had been
    /// chosen, so the setting looked as though it had been thrown away.
    #[test]
    fn the_sharing_row_follows_the_byte_it_is_given() {
        let [exp, item] = sharing_labels(0);
        assert_eq!(exp.0, "UIIT_STT_PARTY_EXP_SELF");
        assert_eq!(item.0, "UIIT_STT_PARTY_ITEM_SELF");

        let [exp, item] = sharing_labels(PartySetup::EXP_SHARED);
        assert_eq!(exp.0, "UIIT_STT_PARTY_EXP_SHARE");
        assert_eq!(
            item.0, "UIIT_STT_PARTY_ITEM_SELF",
            "the bits are independent"
        );

        let [exp, item] = sharing_labels(PartySetup::EXP_SHARED | PartySetup::ITEM_SHARED);
        assert_eq!(exp.0, "UIIT_STT_PARTY_EXP_SHARE");
        assert_eq!(item.0, "UIIT_STT_PARTY_ITEM_SHARE");

        // ANYONE_CAN_INVITE is not drawn here and must not disturb either half
        let [exp, item] = sharing_labels(PartySetup::ANYONE_CAN_INVITE);
        assert_eq!(exp.0, "UIIT_STT_PARTY_EXP_SELF");
        assert_eq!(item.0, "UIIT_STT_PARTY_ITEM_SELF");
    }

    /// What the row *shows* and what registration *sends* come out of one
    /// function, so they cannot drift — the display bug and the wire bug were
    /// the same bug twice.
    #[test]
    fn the_row_shows_exactly_what_the_request_will_carry() {
        let chosen = PartySetup(PartySetup::EXP_SHARED);
        let sent = effective_setup(None, chosen).0;
        assert_eq!(sharing_labels(sent), sharing_labels(chosen.0));
        assert_eq!(sharing_labels(sent)[0].0, "UIIT_STT_PARTY_EXP_SHARE");

        // in a party, both follow the party rather than the pending choice
        let roster = roster_with(PartySetup::ITEM_SHARED);
        let sent = effective_setup(Some(&roster), chosen).0;
        assert_eq!(sharing_labels(sent)[0].0, "UIIT_STT_PARTY_EXP_SELF");
        assert_eq!(sharing_labels(sent)[1].0, "UIIT_STT_PARTY_ITEM_SHARE");
    }

    /// The register dialog DOES rebuild when the sharing mode moves — the mode
    /// modal opens over it, so OK changes the byte behind a dialog that is
    /// still standing. Without this the row kept its old labels and the
    /// setting looked as though it had been ignored.
    #[test]
    fn the_register_key_follows_the_sharing_mode() {
        let register = MatchDialog::Register { editing: None };
        assert_ne!(
            dialog_key(&register, 0),
            dialog_key(&register, PartySetup::EXP_SHARED)
        );
        // ...and nothing else does: the other dialogs do not render it.
        for dialog in [
            MatchDialog::Auto,
            MatchDialog::None,
            MatchDialog::JoinProgress {
                number: 1,
                elapsed: 0.0,
            },
        ] {
            assert_eq!(
                dialog_key(&dialog, 0),
                dialog_key(&dialog, PartySetup::EXP_SHARED),
                "{dialog:?} rebuilt for a byte it does not draw"
            );
        }
    }

    /// Each dialog is its own identity, so switching between them rebuilds.
    #[test]
    fn every_dialog_kind_has_its_own_key() {
        let keys = [
            dialog_key(&MatchDialog::None, 0),
            dialog_key(&MatchDialog::Register { editing: None }, 0),
            dialog_key(&MatchDialog::Register { editing: Some(3) }, 0),
            dialog_key(&MatchDialog::Auto, 0),
            dialog_key(
                &MatchDialog::JoinProgress {
                    number: 1,
                    elapsed: 0.0,
                },
                0,
            ),
        ];
        for (i, a) in keys.iter().enumerate() {
            for b in keys.iter().skip(i + 1) {
                assert_ne!(a, b, "two different dialogs share a key");
            }
        }
    }

    /// All four dialogs confirm the same msgbox2 inset from the other side —
    /// four backgrounds, one constant.
    #[test]
    fn every_dialog_background_is_the_uniform_inset() {
        for (w, _h) in [
            (FORM_W, REGISTER_H),
            (FORM_W, AUTO_H),
            (REQJOIN_W, REQJOIN_H),
            (PROGRESS_W, PROGRESS_H),
        ] {
            assert_eq!(w - 32.0, w - 2.0 * 16.0);
        }
        assert_eq!(PROGRESS_BG.2, PROGRESS_W - 32.0);
        assert_eq!(PROGRESS_BG.0, 16.0);
    }

    /// The wait bar's fill nests inside its housing with a uniform 6 px inset
    /// on all four sides.
    #[test]
    fn the_progress_bar_nests_in_its_housing() {
        assert_eq!(PROGRESS_BAR.0 - PROGRESS_HOUSING.0, 6.0);
        assert_eq!(PROGRESS_BAR.1 - PROGRESS_HOUSING.1, 6.0);
        assert_eq!(
            (PROGRESS_HOUSING.0 + PROGRESS_HOUSING.2) - (PROGRESS_BAR.0 + PROGRESS_BAR.2),
            6.0
        );
        assert_eq!(
            (PROGRESS_HOUSING.1 + PROGRESS_HOUSING.3) - (PROGRESS_BAR.1 + PROGRESS_BAR.3),
            6.0
        );
    }

    /// ReqJoin's footer is the 76-wide button with a 12 px gap — not the
    /// 88-wide mid button, which is what an earlier reading assumed.
    #[test]
    fn the_reqjoin_footer_uses_the_narrow_button() {
        assert_eq!(REQJOIN_OK.2, 76.0);
        assert_eq!(REQJOIN_CANCEL.0 - (REQJOIN_OK.0 + REQJOIN_OK.2), 12.0);
    }

    /// Register and Auto share section 1 byte for byte, and differ from there —
    /// the evidence that they are a fork, not one parameterised form.
    #[test]
    fn register_and_auto_share_only_their_first_section() {
        assert_eq!(SECTION_KEYS[0], 39.0);
        assert_eq!(SECTION_FRAMES[0], 66.0);
        // ...but the third section is taller in auto, and the footers differ
        assert_ne!(AUTO_SECTION3_H, SECTION_FRAME_H);
        assert_ne!(AUTO_OK.1, REGISTER_OK.1);
    }

    /// Only the purposes the corpus names are offered. `Quest` (wire 1) has no
    /// string anywhere, so it is not given an invented label.
    #[test]
    fn only_the_named_purposes_are_offered() {
        assert_eq!(PURPOSES.len(), 3);
        assert!(!PURPOSES
            .iter()
            .any(|(value, ..)| *value == packets::agent::party::PARTY_PURPOSE_QUEST));
    }
}
