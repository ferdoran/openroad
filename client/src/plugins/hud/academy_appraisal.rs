//! Guardian appraisal dialog — `ginterface.txt:1883` `GDR_APPRENTICESHIP_JUDGE`
//! id 155, tree `ifapprenticeshipjudge.txt` (6 blocks).
//!
//! Idea: this is the smallest of the Academy's four surfaces
//! (`docs/re/ui/hud-apprenticeship-window.md` §3.4, build-plan step 5) and it
//! needs no new machinery — it is the shared `msgbox2_window_` shell
//! (`hud/modal_dialog.rs`) with three stacked backgrounds, one
//! `CIFRadioButton` **group container** and the two 76x24 `com_button.ddj`
//! footer buttons. Everything here is one of the tree's six authored rects;
//! nothing is derived and nothing is invented.
//!
//! What the dialog is *for* is the guardian's appraisal of an apprentice, and
//! the radio group is where the verdict is picked. The data does not say what
//! the options are: the tree carries **one** block for the whole group and no
//! per-option data (doc §9 U4, consistent with `shared-input-widgets.md:87`,
//! which reads `CIFRadioButton` as a group container whose N options are
//! runtime-supplied). So the options are **state**, not layout — they arrive
//! in [`AcademyAppraisalState::options`] and the group renders however many it
//! is given. An empty list draws an empty group rather than a guessed one; the
//! wire that fills it is `docs/re/systems/academy.md`'s (doc §9 U6), not this
//! module's.
//!
//! Deviation (stated, per §3.5 of the work-loop runbook): the authored radio
//! rect is `34,59,406,40`, which overruns the 420-wide hull by 20 px
//! (`34 + 406 = 440`). We keep the authored origin and clamp the width to the
//! plate's own right inset — an off-canvas 20 px is hand-authoring overflow,
//! and reproducing it would push the last option under the plate border.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::hud::modal_dialog::{MODAL_BOTTOM, MODAL_SCRIM, MODAL_SIDE, MODAL_TOP};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::textdata::ClientUiStrings;
use crate::scenes::SceneState;

/// `ginterface.txt:1883` — `GDR_APPRENTICESHIP_JUDGE` is `0,0,420,174`. Only
/// the size is used: the dialog is centred on the scrim, because the host
/// rect is authored against the original's canvas, not ours.
const PLATE: (f32, f32) = (420.0, 174.0);

const PLATE_ART: &str = "media://interface/messagebox/msgbox2_window_";
const ART: &str = "media://interface/";
/// The shared 16x16 radio glyph (`options-controls.md` §5), already used by
/// the Options input pane.
const RADIO_ON: &str = "media://interface/ifcommon/com_nbutton_on.ddj";
const RADIO_OFF: &str = "media://interface/ifcommon/com_nbutton_off.ddj";

/// `com_bg_tile_b` `10,40,394,126` — the outer fill over the plate interior.
const BG_B_RECT: (f32, f32, f32, f32) = (10.0, 40.0, 394.0, 126.0);
/// `com_bg_tile_e` `15,44,391,72` and a `com_blacksquare_` stretch on the
/// **byte-identical** rect — the inner pane the radio group sits in.
const BG_E_RECT: (f32, f32, f32, f32) = (15.0, 44.0, 391.0, 72.0);
/// The `CIFRadioButton` group container, `34,59,406,40` as authored; see the
/// module's deviation note for the width.
const RADIO_GROUP_RECT: (f32, f32, f32, f32) = (34.0, 59.0, 406.0, 40.0);
/// Confirm `128,132` / Cancel `216,132`, both `com_button.ddj` at its own art
/// size 76x24 (the tree authors no size — it comes from the texture, #597).
const CONFIRM_RECT: (f32, f32, f32, f32) = (128.0, 132.0, 76.0, 24.0);
const CANCEL_RECT: (f32, f32, f32, f32) = (216.0, 132.0, 76.0, 24.0);
/// The radio glyph is 16x16 and sits centred in the group's 40 px band.
const RADIO_GLYPH: f32 = 16.0;

/// The group's drawn width: the authored 406 clamped to the plate's right
/// inset (see the module deviation).
fn radio_group_width() -> f32 {
    let (x, _, w, _) = RADIO_GROUP_RECT;
    w.min(PLATE.0 - MODAL_SIDE - x)
}

/// An absolutely-positioned node from a plate-local rect.
fn plate_node((x, y, w, h): (f32, f32, f32, f32)) -> Node {
    let s = hud_scale();
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(x * s),
        top: Val::Px(y * s),
        width: Val::Px(w * s),
        height: Val::Px(h * s),
        ..default()
    }
}

/// Whether the appraisal dialog is up, and what it is appraising.
///
/// `options` is server state (doc §9 U6): the tree carries no per-option data,
/// so the client cannot know the verdicts until something tells it. Empty is
/// the honest default.
#[derive(Resource, Default)]
pub struct AcademyAppraisalState {
    pub open: bool,
    /// The verdict options, in the order the server sent them.
    pub options: Vec<String>,
    /// Index into [`Self::options`], or `None` while nothing is picked.
    pub selected: Option<usize>,
}

/// Root marker of the spawned dialog.
#[derive(Component)]
pub struct AcademyAppraisalDialog;

/// One option row inside the radio group.
#[derive(Component)]
pub struct AppraisalOption(pub usize);

/// Spawn/despawn the dialog to match [`AcademyAppraisalState`].
pub fn sync_academy_appraisal(
    state: Res<AcademyAppraisalState>,
    ui_strings: Res<ClientUiStrings>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    cameras: Query<Entity, With<Camera2d>>,
    open: Query<Entity, With<AcademyAppraisalDialog>>,
    mut commands: Commands,
) {
    if !state.is_changed() {
        return;
    }
    for entity in open.iter() {
        commands.entity(entity).despawn();
    }
    if !state.open {
        return;
    }
    let Some(camera) = cameras.iter().next() else {
        return;
    };

    let s = hud_scale();
    let text_font = TextFont {
        font: fonts.nine.clone().into(),
        font_size: FontSize::Px(9.0 * s),
        ..default()
    };
    let img = |rect: (f32, f32, f32, f32), path: String| {
        (
            plate_node(rect),
            ImageNode {
                image: asset_server.load(path),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        )
    };

    // Captured out of the button loop so the root can point Enter at Confirm
    // (`hud::focus::HudDialog`).
    let mut confirm_button = None;
    let root = commands
        .spawn((
            AcademyAppraisalDialog,
            Name::from("Academy Appraisal"),
            UiTargetCamera(camera),
            GlobalZIndex(90),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(MODAL_SCRIM),
        ))
        .with_children(|scrim| {
            scrim
                .spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Px(PLATE.0 * s),
                        height: Val::Px(PLATE.1 * s),
                        margin: UiRect::all(Val::Auto),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|plate| {
                    // the msgbox2_window_ 8-piece ring: 16 at the sides, 40
                    // top, 16 bottom (`hud/modal_dialog.rs`)
                    let (w, h) = PLATE;
                    let side = MODAL_SIDE;
                    let mid_w = w - 2.0 * side;
                    let side_h = h - MODAL_TOP - MODAL_BOTTOM;
                    for ((x, y, pw, ph), piece) in [
                        ((0.0, 0.0, side, MODAL_TOP), "left_up"),
                        ((side, 0.0, mid_w, MODAL_TOP), "mid_up"),
                        ((w - side, 0.0, side, MODAL_TOP), "right_up"),
                        ((0.0, MODAL_TOP, side, side_h), "left_side"),
                        ((w - side, MODAL_TOP, side, side_h), "right_side"),
                        ((0.0, h - MODAL_BOTTOM, side, MODAL_BOTTOM), "left_down"),
                        ((side, h - MODAL_BOTTOM, mid_w, MODAL_BOTTOM), "mid_down"),
                        (
                            (w - side, h - MODAL_BOTTOM, side, MODAL_BOTTOM),
                            "right_down",
                        ),
                    ] {
                        plate.spawn(img((x, y, pw, ph), format!("{PLATE_ART}{piece}.ddj")));
                    }

                    plate.spawn(img(
                        BG_B_RECT,
                        format!("{ART}ifcommon/bg_tile/com_bg_tile_b.ddj"),
                    ));
                    // the black square and the tile share the same rect in the
                    // data — the square is the well, the tile its fill
                    plate.spawn((
                        plate_node(BG_E_RECT),
                        BackgroundColor(Color::BLACK),
                        Pickable::IGNORE,
                    ));
                    plate.spawn(img(
                        BG_E_RECT,
                        format!("{ART}ifcommon/bg_tile/com_bg_tile_e.ddj"),
                    ));

                    // The radio group. One authored block, N runtime options
                    // (doc §9 U4) — so the rows are laid out from the option
                    // list, evenly across the group's band.
                    let (gx, gy, _, gh) = RADIO_GROUP_RECT;
                    let group_w = radio_group_width();
                    let count = state.options.len();
                    for (index, option) in state.options.iter().enumerate() {
                        let cell_w = group_w / count as f32;
                        let x = gx + cell_w * index as f32;
                        let glyph = if state.selected == Some(index) {
                            RADIO_ON
                        } else {
                            RADIO_OFF
                        };
                        let mut row = plate.spawn((
                            AppraisalOption(index),
                            Button,
                            Hovered::default(),
                            plate_node((x, gy, cell_w, gh)),
                        ));
                        row.observe(on_option_pick);
                        row.with_children(|row| {
                            row.spawn(img(
                                (0.0, (gh - RADIO_GLYPH) / 2.0, RADIO_GLYPH, RADIO_GLYPH),
                                glyph.to_string(),
                            ));
                            row.spawn((
                                Text::new(option.clone()),
                                text_font.clone(),
                                TextColor(Color::WHITE),
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px((RADIO_GLYPH + 4.0) * s),
                                    top: Val::Px((gh - RADIO_GLYPH) / 2.0 * s),
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ));
                        });
                    }

                    for (rect, key, fallback, is_confirm) in [
                        (
                            CONFIRM_RECT,
                            "UIIT_CTL_TC_APPRAISAL_BUTTON",
                            "Appraise",
                            true,
                        ),
                        (CANCEL_RECT, "UIIT_CTL_CANCEL", "Cancel", false),
                    ] {
                        let mut button = plate.spawn((
                            Button,
                            Hovered::default(),
                            plate_node(rect),
                            ImageNode {
                                image: asset_server.load(format!("{ART}ifcommon/com_button.ddj")),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                        ));
                        if is_confirm {
                            button.observe(on_confirm);
                            confirm_button = Some(button.id());
                        } else {
                            button.observe(on_cancel);
                        }
                        button.with_children(|button| {
                            button.spawn((
                                Text::new(ui_strings.get_or(key, fallback).to_string()),
                                text_font.clone(),
                                TextColor(Color::WHITE),
                                TextLayout::justify(Justify::Center),
                                Node {
                                    width: Val::Percent(100.0),
                                    align_self: AlignSelf::Center,
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ));
                        });
                    }
                });
        })
        .id();
    if let Some(confirm_button) = confirm_button {
        commands
            .entity(root)
            .insert(crate::plugins::hud::focus::HudDialog { confirm_button });
    }
}

/// Pick an option — the radio semantics: exactly one selected at a time.
fn on_option_pick(
    activate: On<Activate>,
    options: Query<&AppraisalOption>,
    mut state: ResMut<AcademyAppraisalState>,
) {
    if let Ok(option) = options.get(activate.entity) {
        state.selected = Some(option.0);
    }
}

/// Confirm. **No packet**: which opcode carries a guardian's appraisal is
/// UNKNOWN (doc §9 U6, owned by `docs/re/systems/academy.md`), and inventing
/// one would be worse than logging the decision and closing.
fn on_confirm(_activate: On<Activate>, mut state: ResMut<AcademyAppraisalState>) {
    match state.selected {
        Some(index) => info!(
            "academy appraisal confirmed: option {index} ({:?}) — no opcode wired yet",
            state.options.get(index)
        ),
        // Vanilla's own assertion strings show the client validates before it
        // sends; with nothing picked there is nothing to send.
        None => info!("academy appraisal confirmed with no option picked — ignored"),
    }
    state.open = false;
}

fn on_cancel(_activate: On<Activate>, mut state: ResMut<AcademyAppraisalState>) {
    state.open = false;
    state.selected = None;
}

/// Close the dialog when the world scene ends.
pub fn cleanup_academy_appraisal(
    open: Query<Entity, With<AcademyAppraisalDialog>>,
    mut state: ResMut<AcademyAppraisalState>,
    mut commands: Commands,
) {
    for entity in open.iter() {
        commands.entity(entity).despawn();
    }
    state.open = false;
    state.selected = None;
}

/// Self-registration (#558): one line in the HUD registry, all wiring here.
pub struct AcademyAppraisalPlugin;

impl Plugin for AcademyAppraisalPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AcademyAppraisalState>()
            .add_systems(OnExit(SceneState::GameWorld), cleanup_academy_appraisal)
            .add_systems(
                Update,
                sync_academy_appraisal.run_if(
                    in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting)),
                ),
            );
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// #548-1. Every rect is `ifapprenticeshipjudge.txt` / `ginterface.txt`
    /// as authored — the whole point of the unit doc is that none of these is
    /// derived. The two backgrounds really do share one rect in the data.
    #[test]
    fn the_appraisal_rects_are_the_authored_ones() {
        assert_eq!(PLATE, (420.0, 174.0));
        assert_eq!(BG_B_RECT, (10.0, 40.0, 394.0, 126.0));
        assert_eq!(BG_E_RECT, (15.0, 44.0, 391.0, 72.0));
        assert_eq!(RADIO_GROUP_RECT, (34.0, 59.0, 406.0, 40.0));
        assert_eq!(CONFIRM_RECT, (128.0, 132.0, 76.0, 24.0));
        assert_eq!(CANCEL_RECT, (216.0, 132.0, 76.0, 24.0));
    }

    /// #548-2. The two footer buttons are the 76x24 `com_button.ddj` extent
    /// (#597 measured it) and they sit side by side inside the plate.
    #[test]
    fn the_footer_buttons_fit_inside_the_plate() {
        for (x, y, w, h) in [CONFIRM_RECT, CANCEL_RECT] {
            assert_eq!((w, h), (76.0, 24.0));
            assert!(x + w <= PLATE.0 - MODAL_SIDE, "button overruns the plate");
            assert!(y + h <= PLATE.1 - MODAL_BOTTOM, "button overruns the plate");
        }
        assert!(CONFIRM_RECT.0 + CONFIRM_RECT.2 <= CANCEL_RECT.0, "overlap");
    }

    /// #548-3. The authored radio rect overruns the hull by 20px
    /// (`34 + 406 = 440` against 420), which is exactly why the drawn width is
    /// clamped rather than transcribed — the stated deviation. If someone
    /// "fixes" the constant to the clamped value, the data citation is lost,
    /// so both halves are pinned.
    #[test]
    fn the_radio_group_is_clamped_because_the_authored_rect_overruns() {
        let (x, _, w, _) = RADIO_GROUP_RECT;
        assert!(x + w > PLATE.0, "the authored rect is the overrunning one");
        let drawn = radio_group_width();
        assert!(drawn < w);
        assert_eq!(x + drawn, PLATE.0 - MODAL_SIDE);
    }

    /// #548-4. The verdict options are runtime state, never layout: the tree
    /// has one group block and no per-option data (doc §9 U4). A default
    /// state therefore draws an *empty* group instead of guessed options.
    #[test]
    fn the_options_are_state_and_default_to_empty() {
        let state = AcademyAppraisalState::default();
        assert!(!state.open);
        assert!(state.options.is_empty());
        assert_eq!(state.selected, None);
    }
}
