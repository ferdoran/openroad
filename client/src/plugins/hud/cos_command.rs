//! The COS command bar — the fold-out strip of summon commands.
//!
//! Idea: vanilla assembles this bar in code, and we can prove it rather than
//! assume it. `resinfo/ifcoscommand.txt` declares only two blocks (the tab
//! plate and its toggle button) and no resinfo file anywhere instantiates a
//! `CIFCOSCommand` window — but Media ships an unreferenced
//! `am_ctrl_window_front|middle|end` set (44/32/36 × 36), i.e. a front + repeat
//! + cap strip whose width is chosen at runtime from the number of commands.
//! That is exactly the thing the resinfo grammar cannot express as a `Rect`,
//! which is why the authored file is a stub. See
//! `docs/re/ui/cos-pet-window.md` §9.
//!
//! So the *chrome* here is all vanilla — the stretch plate, the authored tab
//! (`0,11,36,28`) and toggle (`5,16,16,16`) rects, the `cos_cmd_*` icons, the
//! `am_key_0N` hotkey glyphs and the `UIIT_STT_COS_*` captions.
//!
//! **The cells are chosen per COS kind.** That is what makes the six shipped
//! key glyphs a non-constraint: a transport is never aimed at anything and a
//! pet is never boarded, so no single kind needs more than six cells even
//! though the vanilla command inventory is far larger. Which commands each
//! kind shows, and in what order, is **ours** — see `slots_for`.
//!
//! The bar sits directly above the skillbar (screenshot-evidenced 2026-08-19;
//! it used to hang beside the minimap, which was a convenience) and acts on
//! one summon at a time — [`CosBarSubject`], switched by clicking a slot in
//! the status stack.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::cos::pet::PetCommand;
use crate::plugins::cos::{ActiveCosList, CosCommand, CosStatus, RiderState};
use crate::plugins::cursor::interactions::entity_select::SelectedEntity;
use crate::plugins::hud::chat::model::ChatState;
use crate::plugins::hud::cos::model::{CosPage, CosWindowState};
use crate::plugins::hud::cos::state::CosState;
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::entities::NetworkId;
use crate::plugins::player::Player;
use crate::plugins::settings::keymap::{
    KEY_COS_AI_TYPE, KEY_COS_ATTACK, KEY_COS_FOLLOW, KEY_COS_RELEASE, KEY_COS_RIDE,
};
use crate::plugins::settings::options::GameOptions;
use crate::plugins::textdata::ClientUiStrings;
use packets::agent::pet::{AttackPetSettings, CosKind};

// --- Layout -------------------------------------------------------------------
//
// Vanilla art sizes (decoded from Media.pk2) and the two authored rects from
// `resinfo/ifcoscommand.txt`. Everything else in this block is ours.

/// `GDR_COSCMD_BOARD:CIFStatic` — `Rect="0,11,36,28"`, art `am_ctrl_tab.ddj`
/// (36×28, an exact match).
const TAB_RECT: (f32, f32, f32, f32) = (0.0, 11.0, 36.0, 28.0);
/// `GDR_COSCMD_OPENCLOSE:CIFButton` — `Rect="5,16,16,16"`, art
/// `am_ctrl_close.ddj` (16×16, an exact match).
const TOGGLE_RECT: (f32, f32, f32, f32) = (5.0, 16.0, 16.0, 16.0);

// All of the following are MEASURED off the decoded art (uncompressed
// A1R5G5B5, not DXT), not inferred from file sizes — an earlier guess that
// `am_ctrl_window` (132) minus `am_ctrl_window_3` (108) was "one cell" was
// wrong: BOTH plates carry exactly three cells and the 24px difference is a
// decorative tail. See `docs/re/ui/cos-pet-window.md` §9.

/// All three plate pieces are 36 tall.
const PLATE_H: f32 = 36.0;
/// **Advance** widths, i.e. how far the next piece starts — not the file
/// widths (44/32/36). Each piece carries transparent right padding that the
/// neighbour must overlap, or every divider drifts off the cell grid.
///
/// The piece names are inverted from what they suggest: `..._end` is the
/// **left** cap (+1 well), `..._middle` is one well, `..._front` is the
/// **right** piece (1 well + a 12px tail). `am_ctrl_window_3` is a pixel-exact
/// paste-up of `end@0 + middle@33 + front@64`, which is what pins these.
const PLATE_LEFT_ADVANCE: f32 = 33.0;
const PLATE_MIDDLE_ADVANCE: f32 = 31.0;
const PLATE_RIGHT_ADVANCE: f32 = 43.0;
/// File widths, for the blit rects (the padding is simply overlapped).
const PLATE_LEFT_W: f32 = 36.0;
const PLATE_MIDDLE_W: f32 = 32.0;
const PLATE_RIGHT_W: f32 = 44.0;
/// Distance between well centres: the `middle` piece IS one cell.
const CELL_PITCH: f32 = 31.0;
/// The well's bevel-inclusive box. The `cos_cmd_*` icons are full-bleed 32×32
/// with their own 1px black frame, so drawn at 26 that frame reads as the slot
/// rim — the vanilla look.
const CELL_ICON: f32 = 26.0;
/// `am_key_0N.ddj` is 8×12 but only its top 8×9 is opaque (an opaque BLACK
/// field with the gold digit on it — that black badge is the art's own, not a
/// rendering fault).
const KEY_GLYPH_W: f32 = 8.0;
const KEY_GLYPH_H: f32 = 9.0;
/// First well's x, and the well row's y, inside the plate.
const CELLS_LEFT: f32 = 6.0;
const CELL_TOP: f32 = 6.0;

/// Where the bar sits: a horizontal strip **directly above the skillbar**,
/// left of its raised menu cluster.
///
/// This is evidence-backed as of 2026-08-18 — a vanilla screenshot shows the
/// strip there. It previously hung under the COS status stack beside the
/// minimap, which was a convenience and always flagged `[U]`
/// (`docs/re/ui/cos-pet-window.md` §9). What remains ours is the exact x: the
/// screenshot places the bar immediately left of the menu cluster, whose
/// backdrop starts at underbar-space x 677, so the plate's right edge goes
/// there.
const UNDERBAR_W: f32 = crate::plugins::hud::underbar::ui::BAR_W;
const UNDERBAR_H: f32 = crate::plugins::hud::underbar::ui::BAR_H;
/// Clearance between the skillbar's top edge and the plate.
const BAR_GAP: f32 = 4.0;
/// The plate's right edge in underbar space — `MENU_BACKDROP_RECT.x`, the left
/// edge of the raised button cluster, so the two never overlap.
const BAR_RIGHT_IN_UNDERBAR: f32 = 677.0;

const CAPTION_FONT_SIZE: f32 = 10.0;

const TAB_DDJ: &str = "media://interface/animal/am_ctrl_tab.ddj";
const TOGGLE_CLOSE: &str = "media://interface/animal/am_ctrl_close.ddj";
const TOGGLE_CLOSE_FOCUS: &str = "media://interface/animal/am_ctrl_close_focus.ddj";
const TOGGLE_CLOSE_PRESS: &str = "media://interface/animal/am_ctrl_close_press.ddj";
const TOGGLE_OPEN: &str = "media://interface/animal/am_ctrl_open.ddj";
/// `..._end` is the LEFT cap and `..._front` the RIGHT piece — the file names
/// are inverted relative to their role (measured; see the advance consts).
const PLATE_LEFT: &str = "media://interface/animal/am_ctrl_window_end.ddj";
const PLATE_MIDDLE: &str = "media://interface/animal/am_ctrl_window_middle.ddj";
const PLATE_RIGHT: &str = "media://interface/animal/am_ctrl_window_front.ddj";

/// One command cell: its icon stem under `icon/action/`, the `UIIT_STT_COS_*`
/// caption key with a fallback, and what it does.
struct CommandSlot {
    icon: &'static str,
    /// The cell's other face. Two commands are one cell with two states, as in
    /// vanilla: board ⇄ dismount (there is no separate dismount key action)
    /// and offensive ⇄ defensive (both captions name the same key, PgDn).
    icon_alt: Option<&'static str>,
    caption: &'static str,
    caption_alt: Option<&'static str>,
    fallback: &'static str,
    action: CosBarButton,
    /// Whether Media ships a `<stem>_disable.ddj` for this icon. Eight of the
    /// 29 `cos_cmd_*` stems do not — `cos_cmd_ai_attack` among them, and it is
    /// *normally* disabled (no target selected) — so a cell without one is
    /// dimmed by tint instead of by swapping the texture.
    has_disable_art: bool,
}

// The cells, per COS kind. **Which commands a kind shows, and in what order,
// is OURS** — `docs/re/ui/cos-pet-window.md` §9 is explicit that no data
// supplies either. What is vanilla is every icon stem, every caption key, and
// the fact that the set differs by kind at all: Board is meaningless to a pet
// and Attack to a horse, which is also why no kind needs more than the six
// `am_key_01..06` hotkey glyphs the original ships.

const BOARD: CommandSlot = CommandSlot {
    icon: "cos_cmd_embark",
    icon_alt: Some("cos_cmd_disembark"),
    caption: "UIIT_STT_COS_RIDE",
    caption_alt: Some("UIIT_STT_COS_DISEMBARK"),
    fallback: "Board",
    action: CosBarButton::BoardToggle,
    has_disable_art: true,
};

const FOLLOW: CommandSlot = CommandSlot {
    icon: "cos_cmd_follower",
    icon_alt: None,
    // The original ships no `UIIT_STT_COS_FOLLOW`; only the chat command
    // `UIIT_KEY_COS_FOLLOW` = "/follow" exists, so this label is OURS.
    caption: "UIIT_STT_COS_FOLLOW",
    caption_alt: None,
    fallback: "Follow",
    action: CosBarButton::Follow,
    has_disable_art: true,
};

const UNSUMMON: CommandSlot = CommandSlot {
    icon: "cos_cmd_unsummon",
    icon_alt: None,
    caption: "UIIT_STT_COS_CLEAN",
    caption_alt: None,
    fallback: "Terminated",
    action: CosBarButton::Unsummon,
    has_disable_art: true,
};

const INVENTORY: CommandSlot = CommandSlot {
    icon: "cos_cmd_inventory",
    icon_alt: None,
    caption: "UIIT_STT_COS_INVENTORY",
    caption_alt: None,
    fallback: "Inventory",
    action: CosBarButton::Inventory,
    has_disable_art: true,
};

/// Offensive ⇄ defensive is **one** switch: vanilla binds both to PgDn and
/// both captions say so ("Offensive (PgDn)" / "Defensive (PgDn)"). One cell
/// showing the pet's current mode, like board ⇄ dismount.
const AI_MODE: CommandSlot = CommandSlot {
    icon: "cos_cmd_defensive",
    icon_alt: Some("cos_cmd_aggressive"),
    caption: "UIIT_STT_COS_DEFENSIVE",
    caption_alt: Some("UIIT_STT_COS_AGGRESSIVE"),
    fallback: "Defensive",
    action: CosBarButton::AiMode,
    has_disable_art: true,
};

/// Send the pet at the selected target. `UIIT_STT_COS_ATTACK` = "Attack" is a
/// real vanilla caption (unlike Follow's), and `cos_cmd_ai_attack` is the only
/// attack art Media ships — with **no** `_disable` sibling.
const ATTACK: CommandSlot = CommandSlot {
    icon: "cos_cmd_ai_attack",
    icon_alt: None,
    caption: "UIIT_STT_COS_ATTACK",
    caption_alt: None,
    fallback: "Attack",
    action: CosBarButton::Attack,
    has_disable_art: false,
};

/// Opens the COS window — on the grab-filter Setup page for a pick pet (that
/// page *is* its behaviour configuration), on Info for everything else.
const INFO: CommandSlot = CommandSlot {
    icon: "cos_cmd_coswindow",
    icon_alt: None,
    caption: "UIIT_STT_COS_COMMAND",
    caption_alt: None,
    fallback: "Information",
    action: CosBarButton::Info,
    has_disable_art: true,
};

/// A transport is ridden and carries cargo; it has no AI and nothing to attack.
const TRANSPORT_SLOTS: [CommandSlot; 5] = [BOARD, FOLLOW, INVENTORY, INFO, UNSUMMON];
/// An attack pet is aimed and has an AI mode; it is never boarded and has no bag.
const ATTACK_PET_SLOTS: [CommandSlot; 5] = [ATTACK, FOLLOW, AI_MODE, INFO, UNSUMMON];
/// A pick pet has a bag and a grab filter, but does not fight.
const PICK_PET_SLOTS: [CommandSlot; 4] = [FOLLOW, INVENTORY, INFO, UNSUMMON];
/// A mercenary/guild guard: we model nothing of its own behaviour yet.
const FELLOW_SLOTS: [CommandSlot; 2] = [FOLLOW, UNSUMMON];

/// The cells for a COS of `kind`.
fn slots_for(kind: CosKind) -> &'static [CommandSlot] {
    match kind {
        CosKind::Vehicle | CosKind::Transport => &TRANSPORT_SLOTS,
        CosKind::GrowthPet => &ATTACK_PET_SLOTS,
        CosKind::GrabPet => &PICK_PET_SLOTS,
        CosKind::Fellow => &FELLOW_SLOTS,
    }
}

/// Opaque width of the plate for `cells` commands: the left cap and the right
/// piece each carry one well, the rest are `middle` repeats. Verified against
/// the shipped 3-cell plate (`am_ctrl_window_3`, opaque 0..106 = 107).
fn plate_width(cells: usize) -> f32 {
    let middles = cells.saturating_sub(2) as f32;
    PLATE_LEFT_ADVANCE + middles * PLATE_MIDDLE_ADVANCE + PLATE_RIGHT_ADVANCE
}

/// Top-left of command cell `k` inside the plate.
fn cell_rect(k: usize) -> (f32, f32, f32, f32) {
    (
        CELLS_LEFT + k as f32 * CELL_PITCH,
        CELL_TOP,
        CELL_ICON,
        CELL_ICON,
    )
}

/// The art for a cell. `has_disable_art` is load-bearing: eight `cos_cmd_*`
/// stems ship no `_disable` sibling, so asking for one would request a missing
/// asset — for those the caller dims with [`DIMMED`] instead.
fn icon_path(stem: &str, enabled: bool, has_disable_art: bool) -> String {
    if enabled || !has_disable_art {
        format!("media://icon/action/{stem}.ddj")
    } else {
        format!("media://icon/action/{stem}_disable.ddj")
    }
}

/// Tint for a disabled cell whose art has no `_disable` variant.
const DIMMED: Color = Color::srgba(0.45, 0.45, 0.45, 0.7);

// --- Components ---------------------------------------------------------------

/// Which summon the bar is showing commands for.
///
/// Deliberately **not** [`SelectedEntity`]: that is the one global world
/// selection, and the Attack cell needs it to mean *the victim*. If the bar
/// keyed off it, clicking your pet to reach its Attack cell would make the pet
/// its own target. The status stack sets both — this one to switch the bar,
/// `SelectedEntity` because clicking a slot also selects the creature.
#[derive(Resource, Default)]
pub struct CosBarSubject(pub Option<u32>);

/// The COS the bar acts on: the explicit pick, else the ridden mount, else the
/// newest summon. A stale pick (the COS was unsummoned) falls through.
pub fn bar_subject(
    subject: &CosBarSubject,
    list: &ActiveCosList,
    rider: &RiderState,
) -> Option<CosStatus> {
    subject
        .0
        .and_then(|uid| list.get(uid))
        .or_else(|| rider.0.and_then(|uid| list.get(uid)))
        .or_else(|| list.0.last())
        .cloned()
}

#[derive(Component, Clone, Default)]
pub struct CosCommandBar;
/// The kind the bar's cells were built for, so a subject change of a
/// *different* kind rebuilds them and one of the same kind does not.
#[derive(Component, Clone, Copy)]
pub struct CosBarKind(pub CosKind);
/// The strip holding the plate + cells; hidden when the bar is folded away.
#[derive(Component, Clone, Default)]
pub struct CosBarStrip;
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum CosBarButton {
    BoardToggle,
    Follow,
    Unsummon,
    /// Open the COS window on its inventory page (the pick pet's bag).
    Inventory,
    /// Open the COS window: Setup for a pick pet, Info otherwise.
    Info,
    /// The one AI-mode switch (0x7420) — sends whichever mode the pet is not in.
    AiMode,
    /// Send the pet at the selected target (0x70C5 action 2).
    Attack,
    /// Rendered (vanilla has the command) but inert until its feature lands.
    Disabled,
}
/// What flips a two-faced cell to its alternate icon and caption.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum AltDriver {
    /// Board ⇄ dismount, driven by the rider state.
    Mounted,
    /// Defensive ⇄ offensive, driven by the pet's settings word.
    Offensive,
}

/// A cell with two faces. Carries both stems so the slot table stays the only
/// place the art is named, and the driver so the refresh knows which state
/// flips it — the bar has two such cells and they answer to different things.
#[derive(Component, Clone)]
pub struct CosAltFace {
    base: &'static str,
    alt: &'static str,
    driver: AltDriver,
}
/// Hover caption target.
#[derive(Component, Clone, Default)]
pub struct CosBarCaption;
/// Fold state of the bar (the vanilla tab toggle).
#[derive(Component, Clone, Copy)]
pub struct CosBarFolded(pub bool);
/// The toggle button, whose art follows [`CosBarFolded`].
#[derive(Component, Clone, Default)]
pub struct CosBarToggle;
/// A cell's caption text, resolved once the string table is available.
#[derive(Component, Clone)]
pub struct CosCellCaption {
    key: &'static str,
    key_alt: Option<&'static str>,
    fallback: &'static str,
}

// --- Spawn / despawn ----------------------------------------------------------

/// Build the bar when the first summon appears, drop it when the last goes.
pub fn sync_cos_command_bar(
    list: Res<ActiveCosList>,
    rider: Res<RiderState>,
    subject: Res<CosBarSubject>,
    bars: Query<(Entity, &CosBarKind), With<CosCommandBar>>,
    cam_query: Query<Entity, With<Camera2d>>,
    asset_server: Res<AssetServer>,
    fonts: Option<Res<FontAssets>>,
    mut commands: Commands,
) {
    let bar = bars.single();
    let Some(status) = bar_subject(&subject, &list, &rider) else {
        if let Ok((bar, _)) = bar {
            commands.entity(bar).despawn();
        }
        return;
    };
    // The cells are per kind, so the bar is rebuilt when the subject's kind
    // changes — switching between two horses keeps it, switching from a horse
    // to an attack pet does not.
    match bar {
        Ok((_, built_for)) if built_for.0 == status.kind => return,
        Ok((bar, _)) => commands.entity(bar).despawn(),
        Err(_) => {}
    }
    let (Some(camera), Some(fonts)) = (cam_query.iter().next(), fonts) else {
        return;
    };

    let slots = slots_for(status.kind);
    let s = hud_scale();
    let plate_w = plate_width(slots.len());
    let strip_left = TAB_RECT.2; // the strip starts right of the tab plate

    // The underbar is a full-width `bottom: 0` row that centres a fixed
    // 800-wide body, so the bar mirrors that exactly rather than anchoring to
    // the viewport's right edge — only then do the two stay aligned at any
    // window width. Inside the centred body the plate is right-aligned to
    // `BAR_RIGHT_IN_UNDERBAR`, clearing the raised menu cluster.
    let bar_w = (strip_left + plate_w) * s;
    commands
        .spawn((
            CosCommandBar,
            CosBarKind(status.kind),
            CosBarFolded(false),
            Name::from("COS Command Bar"),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                bottom: Val::Px((UNDERBAR_H + BAR_GAP) * s),
                width: Val::Percent(100.0),
                height: Val::Px((TAB_RECT.1 + TAB_RECT.3) * s),
                justify_content: JustifyContent::Center,
                ..default()
            },
            // Above the underbar (50) so the plate is not drawn beneath its
            // decorations; still below the loading overlay (200).
            GlobalZIndex(60),
            Pickable::IGNORE,
            UiTargetCamera(camera),
        ))
        .with_children(|centred| {
            // The underbar-sized body the bar is positioned within.
            centred
                .spawn((
                    Node {
                        width: Val::Px(UNDERBAR_W * s),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|body| {
                    body.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px((BAR_RIGHT_IN_UNDERBAR * s) - bar_w),
                            top: Val::Px(0.0),
                            width: Val::Px(bar_w),
                            height: Val::Percent(100.0),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .with_children(|bar| {
                        // --- the fold-out tab + its toggle (the only authored rects) ---
                        bar.spawn((
                            ImageNode {
                                image: asset_server.load(TAB_DDJ),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                            abs(TAB_RECT, s),
                            Pickable::IGNORE,
                        ));
                        bar.spawn((
                            CosBarToggle,
                            Button,
                            Hovered::default(),
                            ImageNode {
                                image: asset_server.load(TOGGLE_CLOSE),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                            crate::plugins::ui_v2::style::ImageButtonStyle {
                                normal: asset_server.load(TOGGLE_CLOSE),
                                hover: asset_server.load(TOGGLE_CLOSE_FOCUS),
                                press: asset_server.load(TOGGLE_CLOSE_PRESS),
                                // Media ships no `_disable` variant for the bar toggle
                                // (unlike the command icons), and the toggle is never
                                // disabled — it folds the bar, which is always allowed.
                                disable: asset_server.load(TOGGLE_CLOSE),
                            },
                            abs(TOGGLE_RECT, s),
                        ))
                        .observe(on_toggle);

                        // --- the runtime-width plate + the command cells ---
                        bar.spawn((
                            CosBarStrip,
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(strip_left * s),
                                top: Val::Px(0.0),
                                width: Val::Px(plate_w * s),
                                height: Val::Px(PLATE_H * s),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ))
                        .with_children(|strip| {
                            // Left cap, one `middle` per interior cell, then the right
                            // piece — each blitted at its FILE width but advanced by its
                            // OPAQUE width, so the neighbour overlaps the transparent
                            // padding and the dividers stay on the 31px grid.
                            let middles = slots.len().saturating_sub(2);
                            let mut x = 0.0;
                            for (art, width, advance) in
                                std::iter::once((PLATE_LEFT, PLATE_LEFT_W, PLATE_LEFT_ADVANCE))
                                    .chain(std::iter::repeat_n(
                                        (PLATE_MIDDLE, PLATE_MIDDLE_W, PLATE_MIDDLE_ADVANCE),
                                        middles,
                                    ))
                                    .chain(std::iter::once((
                                        PLATE_RIGHT,
                                        PLATE_RIGHT_W,
                                        PLATE_RIGHT_ADVANCE,
                                    )))
                            {
                                strip.spawn((
                                    ImageNode {
                                        image: asset_server.load(art),
                                        image_mode: NodeImageMode::Stretch,
                                        ..default()
                                    },
                                    abs((x, 0.0, width, PLATE_H), s),
                                    Pickable::IGNORE,
                                ));
                                x += advance;
                            }

                            for (index, slot) in slots.iter().enumerate() {
                                let enabled = slot.action != CosBarButton::Disabled;
                                let rect = cell_rect(index);
                                let mut cell = strip.spawn((
                                    slot.action,
                                    CosCellCaption {
                                        key: slot.caption,
                                        key_alt: slot.caption_alt,
                                        fallback: slot.fallback,
                                    },
                                    Button,
                                    Hovered::default(),
                                    ImageNode {
                                        image: asset_server.load(icon_path(
                                            slot.icon,
                                            enabled,
                                            slot.has_disable_art,
                                        )),
                                        // A cell whose art has no `_disable` sibling is
                                        // dimmed by tint instead — see `DIMMED`.
                                        color: if enabled || slot.has_disable_art {
                                            Color::WHITE
                                        } else {
                                            DIMMED
                                        },
                                        image_mode: NodeImageMode::Stretch,
                                        ..default()
                                    },
                                    abs(rect, s),
                                ));
                                if let Some(alt) = slot.icon_alt {
                                    cell.insert(CosAltFace {
                                        base: slot.icon,
                                        alt,
                                        driver: if slot.action == CosBarButton::AiMode {
                                            AltDriver::Offensive
                                        } else {
                                            AltDriver::Mounted
                                        },
                                    });
                                }
                                if enabled {
                                    cell.observe(on_bar_button);
                                }
                                // The 1-based hotkey glyph, tucked into the cell's
                                // bottom-right corner.
                                strip.spawn((
                                    ImageNode {
                                        image: asset_server.load(format!(
                                            "media://interface/animal/am_key_{:02}.ddj",
                                            index + 1
                                        )),
                                        image_mode: NodeImageMode::Stretch,
                                        ..default()
                                    },
                                    abs(
                                        (
                                            rect.0 + CELL_ICON - KEY_GLYPH_W,
                                            rect.1 + CELL_ICON - KEY_GLYPH_H,
                                            KEY_GLYPH_W,
                                            KEY_GLYPH_H,
                                        ),
                                        s,
                                    ),
                                    Pickable::IGNORE,
                                ));
                            }
                        });

                        // --- hover caption ABOVE the plate ---
                        // It used to hang below, which was free space under the minimap;
                        // above the skillbar that space is the skillbar, so the caption
                        // is flipped over the plate instead.
                        bar.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(strip_left * s),
                                bottom: Val::Px((PLATE_H + 2.0) * s),
                                width: Val::Px(plate_w * s),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ))
                        .with_children(|row| {
                            row.spawn((
                                CosBarCaption,
                                Text::default(),
                                TextFont {
                                    font: fonts.nine.clone().into(),
                                    font_size: FontSize::Px(CAPTION_FONT_SIZE * s),
                                    ..default()
                                },
                                TextColor(Color::WHITE),
                                TextLayout::justify(Justify::Center),
                                Pickable::IGNORE,
                            ));
                        });
                    });
                });
        });
}

/// Absolutely positioned node from an `(x, y, w, h)` rect at scale `s`.
fn abs(rect: (f32, f32, f32, f32), s: f32) -> Node {
    let (l, t, w, h) = crate::plugins::hud::game_window::scaled(rect, s);
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(l),
        top: Val::Px(t),
        width: Val::Px(w),
        height: Val::Px(h),
        ..default()
    }
}

// --- Behavior -----------------------------------------------------------------

/// Fold the strip away, leaving the tab — the vanilla open/close toggle.
fn on_toggle(
    _: On<Activate>,
    mut bars: Query<&mut CosBarFolded>,
    mut strips: Query<&mut Visibility, With<CosBarStrip>>,
) {
    let Ok(mut folded) = bars.single_mut() else {
        return;
    };
    folded.0 = !folded.0;
    for mut visibility in strips.iter_mut() {
        *visibility = if folded.0 {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }
}

/// Assemble the context every cell acts through — the bar's subject, the
/// current AI mode, and a legal victim for the Attack cell.
#[allow(clippy::too_many_arguments)]
fn dispatch_ctx<'a>(
    subject: &CosBarSubject,
    list: &ActiveCosList,
    rider: &RiderState,
    state: Option<&CosState>,
    selected: Option<Entity>,
    ids: &Query<&NetworkId>,
    players: &Query<Entity, With<Player>>,
    window: Option<&'a mut CosWindowState>,
) -> DispatchCtx<'a> {
    let subject = bar_subject(subject, list, rider);
    let offensive = subject
        .as_ref()
        .and_then(|status| state.and_then(|state| state.get(status.unique_id)))
        .is_some_and(|cos| AttackPetSettings(cos.settings()).is_offensive());
    DispatchCtx {
        mounted: subject
            .as_ref()
            .is_some_and(|status| rider.0 == Some(status.unique_id)),
        victim: attack_victim(selected, ids, players, list),
        offensive,
        subject,
        window,
    }
}

/// Bar clicks act on the bar's subject — the cells were chosen for its kind.
#[allow(clippy::too_many_arguments)]
fn on_bar_button(
    activate: On<Activate>,
    buttons: Query<&CosBarButton>,
    list: Res<ActiveCosList>,
    rider: Res<RiderState>,
    subject: Res<CosBarSubject>,
    state: Option<Res<CosState>>,
    selected: Option<Res<SelectedEntity>>,
    ids: Query<&NetworkId>,
    players: Query<Entity, With<Player>>,
    mut commands_out: MessageWriter<CosCommand>,
    mut pet_out: MessageWriter<PetCommand>,
    window: Option<ResMut<CosWindowState>>,
) {
    let Ok(action) = buttons.get(activate.entity) else {
        return;
    };
    let mut window = window.map(ResMut::into_inner);
    let mut ctx = dispatch_ctx(
        &subject,
        &list,
        &rider,
        state.as_deref(),
        selected.and_then(|s| s.0),
        &ids,
        &players,
        window.as_deref_mut(),
    );
    dispatch(*action, &mut ctx, &mut commands_out, &mut pet_out);
}

/// Everything a cell needs to act.
///
/// A struct rather than more parameters because the Attack cell broke the old
/// shape: `dispatch` used to resolve a single COS uid and act on it, but an
/// attack carries *two* ids — the pet and its victim.
pub struct DispatchCtx<'a> {
    /// The COS the bar is showing — every cell acts on this one, since the
    /// cells themselves were chosen for its kind.
    pub subject: Option<CosStatus>,
    /// A legal victim for the Attack cell, or `None` when the selection is
    /// not something a pet may be sent at. See [`attack_victim`].
    pub victim: Option<u32>,
    /// Whether the subject is currently in offensive mode — the AI cell sends
    /// the *other* mode. Read from `CosState`'s settings word by the caller,
    /// which is where the authoritative 0xB420 value lands.
    pub offensive: bool,
    pub mounted: bool,
    /// The COS *window* is another plugin's resource — the bar stands alone in
    /// tests and in the headless harness, and a missing window means only that
    /// the Info and bag cells have nothing to open.
    pub window: Option<&'a mut CosWindowState>,
}

/// The selected entity's uid, if a pet may legally be sent at it.
///
/// Guarded because the status stack sets `SelectedEntity` too: clicking your
/// own pet to bring up its Attack cell would otherwise aim the pet at itself.
/// The player is excluded for the same reason.
pub fn attack_victim(
    selected: Option<Entity>,
    ids: &Query<&NetworkId>,
    players: &Query<Entity, With<Player>>,
    list: &ActiveCosList,
) -> Option<u32> {
    let entity = selected?;
    if players.get(entity).is_ok() {
        return None;
    }
    let uid = ids.get(entity).ok()?.0;
    // Our own summons are not targets.
    (list.get(uid).is_none()).then_some(uid)
}

fn dispatch(
    action: CosBarButton,
    ctx: &mut DispatchCtx,
    commands_out: &mut MessageWriter<CosCommand>,
    pet_out: &mut MessageWriter<PetCommand>,
) {
    let Some(status) = ctx.subject.clone() else {
        return;
    };
    let uid = status.unique_id;
    let rider_mounted = ctx.mounted;
    match action {
        CosBarButton::BoardToggle => {
            if rider_mounted {
                commands_out.write(CosCommand::Dismount);
            } else {
                commands_out.write(CosCommand::Board(uid));
            }
        }
        CosBarButton::Follow => {
            commands_out.write(CosCommand::Follow(uid));
        }
        CosBarButton::Unsummon => {
            commands_out.write(CosCommand::Unsummon(uid));
        }
        CosBarButton::AiMode => {
            // One switch: send whichever mode the pet is not already in. The
            // caller supplies the current mode through the cell's icon state,
            // so the toggle reads it back off the status entry.
            pet_out.write(PetCommand::SetAiMode {
                unique_id: uid,
                offensive: !ctx.offensive,
            });
        }
        CosBarButton::Attack => {
            // No legal victim means the cell is dimmed; clicking it anyway
            // must do nothing rather than aim at whatever was last selected.
            if let Some(target_unique_id) = ctx.victim {
                pet_out.write(PetCommand::Attack {
                    unique_id: uid,
                    target_unique_id,
                });
            }
        }
        CosBarButton::Inventory => {
            if let Some(window) = ctx.window.as_deref_mut() {
                window.open = true;
                window.page = CosPage::Inventory;
            }
        }
        CosBarButton::Info => {
            if let Some(window) = ctx.window.as_deref_mut() {
                window.open = true;
                // A pick pet's "behaviour configuration" is the grab-filter
                // page; everything else opens on its stats.
                window.page = if status.kind == CosKind::GrabPet {
                    CosPage::Setup
                } else {
                    CosPage::Info
                };
            }
        }
        CosBarButton::Disabled => {}
    }
}

/// Vanilla's own captions name these keys ("Dismount (Home)", "Terminated
/// (PgUp)"), so they are bound by default in the keymap; PgDn's AI toggle has
/// no behavior yet and is deliberately not dispatched.
pub fn cos_command_hotkeys(
    keys: Res<ButtonInput<KeyCode>>,
    options: Res<GameOptions>,
    chat: Option<Res<ChatState>>,
    list: Res<ActiveCosList>,
    rider: Res<RiderState>,
    subject: Res<CosBarSubject>,
    state: Option<Res<CosState>>,
    selected: Option<Res<SelectedEntity>>,
    ids: Query<&NetworkId>,
    players: Query<Entity, With<Player>>,
    mut commands_out: MessageWriter<CosCommand>,
    mut pet_out: MessageWriter<PetCommand>,
    window: Option<ResMut<CosWindowState>>,
) {
    // Never fire while the player is typing (e.g. the "/board" chat command).
    if chat.is_some_and(|chat| chat.input_open) || list.0.is_empty() {
        return;
    }
    let pressed = |id: u16| {
        options
            .key_for(id)
            .is_some_and(|key| keys.just_pressed(key))
    };
    let mut window = window.map(ResMut::into_inner);
    let mut ctx = dispatch_ctx(
        &subject,
        &list,
        &rider,
        state.as_deref(),
        selected.and_then(|s| s.0),
        &ids,
        &players,
        window.as_deref_mut(),
    );
    // Only fire a key whose command the subject's own bar actually offers —
    // PgDn on a horse must not send a pet AI change.
    // Which commands the subject's own bar offers — PgDn on a horse must not
    // send a pet AI change. Resolved up front so it does not borrow `ctx`
    // across the dispatch calls below.
    let offered = ctx
        .subject
        .as_ref()
        .map(|status| slots_for(status.kind))
        .unwrap_or(&[]);
    let offers = |action: CosBarButton| offered.iter().any(|s| s.action == action);

    for (key, action) in [
        (KEY_COS_RIDE, CosBarButton::BoardToggle),
        (KEY_COS_RELEASE, CosBarButton::Unsummon),
        // One key for both faces of the switch, as in vanilla: `dispatch`
        // sends whichever mode the pet is not already in.
        (KEY_COS_AI_TYPE, CosBarButton::AiMode),
        (KEY_COS_ATTACK, CosBarButton::Attack),
        (KEY_COS_FOLLOW, CosBarButton::Follow),
    ] {
        if pressed(key) && offers(action) {
            dispatch(action, &mut ctx, &mut commands_out, &mut pet_out);
        }
    }
}

// --- Per-frame refresh ---------------------------------------------------------

/// The two-faced cells' art, the Attack cell's enabled tint, the toggle's
/// open/close art, and the hover caption.
#[allow(clippy::type_complexity)]
pub fn update_cos_command_bar(
    rider: Res<RiderState>,
    list: Res<ActiveCosList>,
    subject: Res<CosBarSubject>,
    state: Option<Res<CosState>>,
    selected: Option<Res<SelectedEntity>>,
    ids: Query<&NetworkId>,
    players: Query<Entity, With<Player>>,
    ui_strings: Res<ClientUiStrings>,
    asset_server: Res<AssetServer>,
    bars: Query<&CosBarFolded>,
    hovered: Query<(&Hovered, &CosCellCaption, Option<&CosAltFace>)>,
    mut alt_icons: Query<(&mut ImageNode, &CosAltFace)>,
    // Disjoint from `toggles` by `CosBarToggle` and from `alt_icons` by
    // `CosAltFace` — all three take `&mut ImageNode`, so the exclusions are
    // what make the three queries provably non-overlapping.
    mut attack_cells: Query<
        (&mut ImageNode, &CosBarButton),
        (Without<CosAltFace>, Without<CosBarToggle>),
    >,
    mut toggles: Query<
        (
            &mut ImageNode,
            &mut crate::plugins::ui_v2::style::ImageButtonStyle,
        ),
        (With<CosBarToggle>, Without<CosAltFace>),
    >,
    mut captions: Query<&mut Text, With<CosBarCaption>>,
) {
    let status = bar_subject(&subject, &list, &rider);
    let mounted = status
        .as_ref()
        .is_some_and(|status| rider.0 == Some(status.unique_id));
    let offensive = status
        .as_ref()
        .and_then(|status| state.as_deref().and_then(|s| s.get(status.unique_id)))
        .is_some_and(|cos| AttackPetSettings(cos.settings()).is_offensive());
    let alt_active = |driver: AltDriver| match driver {
        AltDriver::Mounted => mounted,
        AltDriver::Offensive => offensive,
    };

    // The Attack cell is only live while something attackable is selected, and
    // its art has no `_disable` variant — so it is dimmed by tint.
    let victim = attack_victim(selected.and_then(|s| s.0), &ids, &players, &list);
    for (mut image, action) in attack_cells.iter_mut() {
        if *action != CosBarButton::Attack {
            continue;
        }
        let wanted = if victim.is_some() {
            Color::WHITE
        } else {
            DIMMED
        };
        if image.color != wanted {
            image.color = wanted;
        }
    }

    for (mut image, face) in alt_icons.iter_mut() {
        let stem = if alt_active(face.driver) {
            face.alt
        } else {
            face.base
        };
        let wanted: Handle<Image> = asset_server.load(icon_path(stem, true, true));
        if image.image != wanted {
            image.image = wanted;
        }
    }

    if let (Ok(folded), Ok((mut icon, mut style))) = (bars.single(), toggles.single_mut()) {
        let (normal, hover, press) = if folded.0 {
            (TOGGLE_OPEN, TOGGLE_OPEN, TOGGLE_OPEN)
        } else {
            (TOGGLE_CLOSE, TOGGLE_CLOSE_FOCUS, TOGGLE_CLOSE_PRESS)
        };
        let wanted: Handle<Image> = asset_server.load(normal);
        if style.normal != wanted {
            style.normal = wanted.clone();
            style.hover = asset_server.load(hover);
            style.press = asset_server.load(press);
            icon.image = wanted;
        }
    }

    // Caption of whatever cell the cursor is over (blank otherwise).
    if let Ok(mut text) = captions.single_mut() {
        let wanted = hovered
            .iter()
            .find(|(hover, _, _)| hover.get())
            .map(|(_, caption, face)| {
                // The alternate caption follows the same driver as the cell's
                // alternate icon, so the AI cell reads "Offensive"/"Defensive"
                // off the pet's mode rather than off the rider state.
                let show_alt = face.is_some_and(|face| alt_active(face.driver));
                let key = match (show_alt, caption.key_alt) {
                    (true, Some(alt)) => alt,
                    _ => caption.key,
                };
                ui_strings.get_or(key, caption.fallback).to_string()
            })
            .unwrap_or_default();
        if text.0 != wanted {
            text.0 = wanted;
        }
    }
}

/// Self-registration for the COS command bar (#558). The bar is a child of the
/// status stack's anchor but its own widget, so it registers its own systems;
/// it needs no spawn/cleanup pair because [`sync_cos_command_bar`] builds and
/// tears the bar down from the active-summon list.
pub struct CosCommandPlugin;

impl Plugin for CosCommandPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CosBarSubject>().add_systems(
            Update,
            (
                sync_cos_command_bar,
                update_cos_command_bar,
                cos_command_hotkeys,
            )
                .run_if(crate::scenes::in_playable_world),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::cos::CosStatus;
    use packets::agent::pet::CosKind;

    fn horse() -> CosStatus {
        CosStatus {
            unique_id: 7,
            ref_id: 2137,
            kind: CosKind::Vehicle,
            name: Some("Horse".into()),
            hp: 100,
            hp_max: 100,
            local_only: true,
        }
    }

    fn pet_status(kind: CosKind) -> CosStatus {
        CosStatus {
            unique_id: 11,
            ref_id: 4242,
            kind,
            name: Some("Nuri".into()),
            hp: 360,
            hp_max: 360,
            local_only: true,
        }
    }

    /// Pinned against the shipped 3-cell plate: `am_ctrl_window_3` is opaque
    /// over x=0..106, i.e. 107px, and is a pixel-exact paste-up of
    /// `end@0 + middle@33 + front@64`. If this drifts, the assembly no longer
    /// matches the art the pieces were cut from.
    #[test]
    fn the_plate_width_matches_the_shipped_three_cell_plate() {
        assert_eq!(plate_width(3), 107.0);
        // Each further command adds exactly one `middle`.
        assert_eq!(plate_width(4) - plate_width(3), PLATE_MIDDLE_ADVANCE);
        // Our six-command bar.
        assert_eq!(plate_width(6), 200.0);
    }

    /// The measured well grid: 26x26 boxes at 31px pitch, first at x=6, y=6.
    /// The previous 24px pitch / 20px icon guess rendered six icons over a
    /// plate that only exposed four wells.
    #[test]
    fn cell_rects_sit_on_the_measured_well_grid() {
        assert_eq!(cell_rect(0), (6.0, 6.0, 26.0, 26.0));
        assert_eq!(cell_rect(1), (37.0, 6.0, 26.0, 26.0));
        assert_eq!(cell_rect(5), (161.0, 6.0, 26.0, 26.0));
        // Every cell of *every* kind's set has to land inside the plate that
        // set is drawn on — the sets are different lengths now, so checking
        // one of them proves nothing about the others.
        for kind in ALL_KINDS {
            let slots = slots_for(kind);
            let plate = plate_width(slots.len());
            for k in 0..slots.len() {
                let (x, y, w, h) = cell_rect(k);
                assert!(
                    x + w <= plate,
                    "{kind:?} cell {k} ends at {} > {plate}",
                    x + w
                );
                assert!(y + h <= PLATE_H);
            }
        }
    }

    const ALL_KINDS: [CosKind; 5] = [
        CosKind::Vehicle,
        CosKind::Transport,
        CosKind::GrowthPet,
        CosKind::GrabPet,
        CosKind::Fellow,
    ];

    /// The slot set is chosen per COS kind — Board is meaningless to a pet and
    /// Attack to a horse — and no set may exceed the six `am_key_01..06`
    /// hotkey glyphs the original ships.
    #[test]
    fn each_kind_gets_the_commands_that_apply_to_it() {
        let has = |kind: CosKind, action: CosBarButton| {
            slots_for(kind).iter().any(|s| s.action == action)
        };

        // Only a rideable COS is boarded.
        assert!(has(CosKind::Vehicle, CosBarButton::BoardToggle));
        assert!(!has(CosKind::GrowthPet, CosBarButton::BoardToggle));
        assert!(!has(CosKind::GrabPet, CosBarButton::BoardToggle));

        // Only the attack pet is aimed, and only it has an AI mode.
        assert!(has(CosKind::GrowthPet, CosBarButton::Attack));
        assert!(has(CosKind::GrowthPet, CosBarButton::AiMode));
        for kind in [CosKind::Vehicle, CosKind::GrabPet, CosKind::Fellow] {
            assert!(!has(kind, CosBarButton::Attack), "{kind:?}");
            assert!(!has(kind, CosBarButton::AiMode), "{kind:?}");
        }

        // A pick pet has a bag; an attack pet does not (its inventory_size is
        // 0 on the wire).
        assert!(has(CosKind::GrabPet, CosBarButton::Inventory));
        assert!(!has(CosKind::GrowthPet, CosBarButton::Inventory));

        for kind in ALL_KINDS {
            // Every summon can be dismissed and told to follow.
            assert!(has(kind, CosBarButton::Unsummon), "{kind:?}");
            assert!(has(kind, CosBarButton::Follow), "{kind:?}");
            assert!(
                slots_for(kind).len() <= 6,
                "{kind:?} exceeds the six shipped hotkey glyphs"
            );
        }
    }

    /// Fire one cell against a hand-built context and report what came out.
    fn fire(
        subject: Option<CosStatus>,
        victim: Option<u32>,
        action: CosBarButton,
    ) -> (usize, Vec<PetCommand>, CosPage) {
        let mut app = App::new();
        app.add_message::<CosCommand>()
            .add_message::<PetCommand>()
            .init_resource::<CosWindowState>();

        let mut system = IntoSystem::into_system(
            move |mut out: MessageWriter<CosCommand>,
                  mut pet_out: MessageWriter<PetCommand>,
                  mut window: ResMut<CosWindowState>| {
                let mut ctx = DispatchCtx {
                    subject: subject.clone(),
                    victim,
                    offensive: false,
                    mounted: false,
                    window: Some(&mut window),
                };
                dispatch(action, &mut ctx, &mut out, &mut pet_out);
            },
        );
        system.initialize(app.world_mut());
        system.run((), app.world_mut()).unwrap();
        let cos = app
            .world_mut()
            .resource_mut::<Messages<CosCommand>>()
            .drain()
            .count();
        let pet = app
            .world_mut()
            .resource_mut::<Messages<PetCommand>>()
            .drain()
            .collect::<Vec<_>>();
        let page = app.world().resource::<CosWindowState>().page;
        (cos, pet, page)
    }

    /// The Attack cell carries two ids, and the second one is guarded: the
    /// status stack sets `SelectedEntity` when you click a COS slot, so
    /// without the guard, selecting your pet to reach its Attack cell would
    /// aim the pet at itself.
    #[test]
    fn attack_needs_a_victim_and_sends_both_ids() {
        let pet = pet_status(CosKind::GrowthPet);

        let (_, sent, _) = fire(Some(pet.clone()), Some(909), CosBarButton::Attack);
        assert!(matches!(
            sent.as_slice(),
            [PetCommand::Attack {
                unique_id: 11,
                target_unique_id: 909
            }]
        ));

        // No legal victim: the cell is dimmed, and clicking it anyway must not
        // fall back to some previously-selected target.
        let (_, sent, _) = fire(Some(pet), None, CosBarButton::Attack);
        assert!(sent.is_empty());

        // And with no summon at all nothing dispatches.
        let (cos, sent, _) = fire(None, Some(909), CosBarButton::Attack);
        assert_eq!((cos, sent.len()), (0, 0));
    }

    /// `attack_victim` is the guard itself: it refuses the player and refuses
    /// our own summons, and only then yields a uid.
    #[test]
    fn attack_victim_refuses_the_player_and_our_own_summons() {
        let mut app = App::new();
        app.init_resource::<ActiveCosList>();
        app.world_mut().resource_mut::<ActiveCosList>().0 = vec![horse()];
        let monster = app.world_mut().spawn(NetworkId(909)).id();
        let own_cos = app.world_mut().spawn(NetworkId(7)).id();
        let player = app.world_mut().spawn((NetworkId(42), Player)).id();

        let mut system = IntoSystem::into_system(
            move |list: Res<ActiveCosList>,
                  ids: Query<&NetworkId>,
                  players: Query<Entity, With<Player>>| {
                (
                    attack_victim(Some(monster), &ids, &players, &list),
                    attack_victim(Some(own_cos), &ids, &players, &list),
                    attack_victim(Some(player), &ids, &players, &list),
                    attack_victim(None, &ids, &players, &list),
                )
            },
        );
        system.initialize(app.world_mut());
        let (monster, own, player, none) = system.run((), app.world_mut()).unwrap();

        assert_eq!(monster, Some(909), "an ordinary entity is a valid target");
        assert_eq!(own, None, "our own horse is not");
        assert_eq!(player, None, "nor is the player");
        assert_eq!(none, None);
    }

    /// The Info cell opens the page that suits the kind: a pick pet's
    /// behaviour configuration *is* the grab-filter Setup page.
    #[test]
    fn info_opens_setup_for_a_pick_pet_and_info_otherwise() {
        let page = |kind| fire(Some(pet_status(kind)), None, CosBarButton::Info).2;
        assert_eq!(page(CosKind::GrabPet), CosPage::Setup);
        assert_eq!(page(CosKind::GrowthPet), CosPage::Info);
        assert_eq!(
            fire(Some(horse()), None, CosBarButton::Info).2,
            CosPage::Info
        );

        // The bag cell is still the inventory page.
        assert_eq!(
            fire(
                Some(pet_status(CosKind::GrabPet)),
                None,
                CosBarButton::Inventory
            )
            .2,
            CosPage::Inventory
        );
    }

    /// Every cell acts on the bar's own subject, since the cells were chosen
    /// for that subject's kind.
    #[test]
    fn cells_act_on_the_bars_subject() {
        assert_eq!(fire(Some(horse()), None, CosBarButton::Unsummon).0, 1);
        assert_eq!(fire(Some(horse()), None, CosBarButton::BoardToggle).0, 1);
        assert_eq!(fire(Some(horse()), None, CosBarButton::Disabled).0, 0);
        assert_eq!(fire(None, None, CosBarButton::Unsummon).0, 0);

        // The AI switch sends the mode the pet is *not* in.
        let (_, sent, _) = fire(
            Some(pet_status(CosKind::GrowthPet)),
            None,
            CosBarButton::AiMode,
        );
        assert!(matches!(
            sent.as_slice(),
            [PetCommand::SetAiMode {
                unique_id: 11,
                offensive: true
            }]
        ));
    }

    /// The bar's subject: the explicit pick, else the ridden mount, else the
    /// newest summon — and a stale pick falls through rather than blanking it.
    #[test]
    fn the_bar_subject_falls_through_a_stale_pick() {
        let mut list = ActiveCosList::default();
        list.0 = vec![horse(), pet_status(CosKind::GrowthPet)];
        let rider = RiderState::default();

        // Nothing picked: the newest summon.
        let kind = |subject: Option<u32>, rider: &RiderState| {
            bar_subject(&CosBarSubject(subject), &list, rider).map(|s| s.kind)
        };
        assert_eq!(kind(None, &rider), Some(CosKind::GrowthPet));
        // Picked the horse.
        assert_eq!(kind(Some(7), &rider), Some(CosKind::Vehicle));
        // A pick for a COS that is gone falls through to the newest.
        assert_eq!(kind(Some(999), &rider), Some(CosKind::GrowthPet));
        // Riding wins over the newest, but not over an explicit pick.
        let mounted = RiderState(Some(7));
        assert_eq!(kind(None, &mounted), Some(CosKind::Vehicle));
        assert_eq!(kind(Some(11), &mounted), Some(CosKind::GrowthPet));

        // No summons at all: no bar.
        assert_eq!(
            bar_subject(&CosBarSubject(None), &ActiveCosList::default(), &rider).map(|s| s.kind),
            None
        );
    }

    /// Every cell's icon and hotkey glyph must exist in Media; a typo here is
    /// a blank button nobody notices until a playtest.
    #[test]
    fn every_slot_names_shipped_art() {
        // The eight `cos_cmd_*` stems Media ships with NO `_disable` sibling.
        // A slot naming one of these must declare `has_disable_art: false`, or
        // `icon_path` will request an asset that does not exist — which is how
        // the Attack cell (normally disabled) would have shipped broken.
        const NO_DISABLE_ART: [&str; 8] = [
            "cos_cmd_ai_attack",
            "cos_cmd_ai_auto",
            "cos_cmd_ai_hold",
            "cos_cmd_ai_page",
            "cos_cmd_charm",
            "cos_cmd_inventory_close",
            "cos_cmd_inventory_open",
            "cos_cmd_prev",
        ];

        for kind in ALL_KINDS {
            for slot in slots_for(kind) {
                assert!(slot.icon.starts_with("cos_cmd_"), "{}", slot.icon);
                if let Some(alt) = slot.icon_alt {
                    assert!(alt.starts_with("cos_cmd_"), "{alt}");
                    // A two-faced cell swaps between enabled arts, so both
                    // faces must exist as plain stems.
                    assert!(!NO_DISABLE_ART.contains(&alt) || !slot.has_disable_art);
                }
                assert!(
                    slot.caption.starts_with("UIIT_STT_COS_"),
                    "{}",
                    slot.caption
                );
                assert_eq!(
                    slot.has_disable_art,
                    !NO_DISABLE_ART.contains(&slot.icon),
                    "{} declares the wrong _disable availability",
                    slot.icon
                );
            }
        }
    }

    /// The plate is right-aligned to the underbar's raised menu cluster, so
    /// the widest set must still fit left of it and inside the underbar.
    #[test]
    fn the_bar_clears_the_underbar_menu_cluster() {
        let widest = ALL_KINDS
            .iter()
            .map(|kind| plate_width(slots_for(*kind).len()) + TAB_RECT.2)
            .fold(0.0f32, f32::max);
        assert!(
            BAR_RIGHT_IN_UNDERBAR - widest >= 0.0,
            "the bar ({widest}) does not fit left of x{BAR_RIGHT_IN_UNDERBAR}"
        );
        assert!(BAR_RIGHT_IN_UNDERBAR <= UNDERBAR_W);
    }

    /// Query disjointness is only proven at system init, and a miss is a
    /// first-frame panic in GameWorld.
    #[test]
    fn the_bar_systems_have_no_conflicting_queries() {
        bevy::tasks::IoTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Image>()
            .init_resource::<ActiveCosList>()
            .init_resource::<RiderState>()
            .init_resource::<CosBarSubject>()
            .init_resource::<ClientUiStrings>()
            .init_resource::<GameOptions>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_message::<CosCommand>()
            // `PetCommand` is the COS plugin's message; the bar is registered
            // separately, so the harness supplies it the way the app does.
            .add_message::<PetCommand>()
            .add_systems(
                Update,
                (
                    sync_cos_command_bar,
                    update_cos_command_bar,
                    cos_command_hotkeys,
                ),
            );
        app.world_mut()
            .resource_mut::<ActiveCosList>()
            .0
            .push(horse());
        app.update();
    }
}
