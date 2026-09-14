//! Appearance-change window — `ginterface.txt:1031`
//! `GDR_CHANGE_PLAYER_MODEL:CIFChangePlayerModel` id 54, `0,0,372,400`,
//! tree `ifchangeplayermodel.txt` (25 blocks).
//!
//! Idea: this is the in-game **host** for the appearance panel the
//! character-creation scene already builds — the original expresses the reuse
//! by sharing the `UIO_NEWCHAR_*` string keys, because the resinfo grammar has
//! no include or template mechanism
//! (`docs/re/ui/change-player-model-window.md` §7). So the six controls are the
//! same six controls: a sex header, three body sliders, a model preview,
//! Confirm/Cancel.
//!
//! What is *shared in code* here is the value, not the layout, and that is a
//! deliberate reading of the doc's build plan: the two hosts draw different
//! art (creation uses `slider.ddj` on a 120x24 row, this window uses
//! `msgbox_chshape_variety_bar.ddj` on the authored 148x28 band with
//! `com_scroll_button` + `com_*_bigarrow` chrome), so lifting one host's nodes
//! into the other would force one art family onto both. [`Appearance`] is the
//! part that must not drift, and it is one type.
//!
//! The slider **range is not in the data** (doc §4, the same gap
//! `autopotion-window.md` recorded), so a value is a normalised
//! `0.0..=1.0` [`Unit`] rather than an invented 0..100 — the unknown then
//! lives in exactly one place.
//!
//! No wire: which item opens this window and what Confirm sends are UNKNOWN
//! (doc §9), so Confirm logs the chosen appearance and closes.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::hud::modal_dialog::{MODAL_BOTTOM, MODAL_SCRIM, MODAL_SIDE, MODAL_TOP};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::textdata::ClientUiStrings;
use crate::scenes::SceneState;

/// `0,0,372,400` — only the size is used; the window is centred on the scrim
/// because the authored origin is against the original's canvas, not ours.
const PLATE: (f32, f32) = (372.0, 400.0);

const PLATE_ART: &str = "media://interface/messagebox/msgbox2_window_";
const ART: &str = "media://interface/";
/// `msgbox_chshape_variety_bar.ddj`, 148x28, drawn 1:1.
const SLIDER_BAR: &str = "media://interface/messagebox/msgbox_chshape_variety_bar.ddj";
const THUMB_ART: &str = "media://interface/ifcommon/com_scroll_button.ddj";
const PREV_ART: &str = "media://interface/ifcommon/com_left_bigarrow.ddj";
const NEXT_ART: &str = "media://interface/ifcommon/com_right_bigarrow.ddj";
const S_BUTTON: &str = "media://interface/ifcommon/com_s_button.ddj";
/// The same rotate art the equipment window drives
/// (`hud/inventory/ui.rs:162`), so the button extents are corroborated by a
/// second tree rather than assumed here.
const ROTATE_DIR: &str = "media://interface/equipment/equip_rotate_";
/// The `int_window_` nine-slice kit; its pieces are 16x16, as the other HUD
/// windows that draw this ring already state.
const INT_WINDOW_DIR: &str = "media://interface/inventory/int_window_";
const INT_WINDOW_PIECE: f32 = 16.0;
/// The stand-in border for the untranscribed `opt_inner_box_` kit.
const INNER_BOX_BORDER: Color = Color::srgb(0.35, 0.33, 0.28);

/// `:349` `_INFO_FRAME:CIFFrame` id 11 `13,40,344,313` on `int_window_`.
const INFO_FRAME: (f32, f32, f32, f32) = (13.0, 40.0, 344.0, 313.0);
/// `:330` `_INFO_INNERBOX:CIFSubFrame` id 12 `25,53,164,287` on
/// `opt_inner_box_`, over `:406` BG1 and `:387` BG2, both `com_bg_tile_b`.
const INNER_BOX: (f32, f32, f32, f32) = (25.0, 53.0, 164.0, 287.0);
const BG1: (f32, f32, f32, f32) = (45.0, 81.0, 124.0, 247.0);
const BG2: (f32, f32, f32, f32) = (189.0, 56.0, 14.0, 281.0);
/// `:368` BG3, behind the footer.
const BG3: (f32, f32, f32, f32) = (16.0, 353.0, 340.0, 31.0);

/// Labels are `29,y,156,16`; the sex header's y is 92, the three body rows'
/// are 164/218/272 — pitch **54 exactly, zero jitter** (doc §3.2). The stack
/// is machine-regular, so it is generated from the pitch and pinned by a test
/// against the three authored y values.
const LABEL_X: f32 = 29.0;
const LABEL_W: f32 = 156.0;
const LABEL_H: f32 = 16.0;
const SEX_LABEL_Y: f32 = 92.0;
const ROW0_LABEL_Y: f32 = 164.0;
const ROW_PITCH: f32 = 54.0;
/// Constant label -> slider offset across all three rows.
const LABEL_TO_SLIDER: f32 = 18.0;
/// Slider band `33,y,148,28`.
const SLIDER_X: f32 = 33.0;
const SLIDER_W: f32 = 148.0;
const SLIDER_H: f32 = 28.0;

/// `com_s_button.ddj` is 44x20; the two sex buttons are authored at
/// `60,113` / `113,113` with `0,0` extents, i.e. art-sized.
const SEX_BTN: (f32, f32) = (44.0, 20.0);
const MALE_XY: (f32, f32) = (60.0, 113.0);
const FEMALE_XY: (f32, f32) = (113.0, 113.0);

/// The preview column: a `com_blacksquare_` stretch, a `com_bg_tile_e` fill
/// and the `DDJ=""` render target itself.
const VIEW_SQUARE: (f32, f32, f32, f32) = (203.0, 54.0, 142.0, 285.0);
const VIEW_BG: (f32, f32, f32, f32) = (207.0, 58.0, 134.0, 277.0);
const VIEW: (f32, f32, f32, f32) = (220.0, 63.0, 108.0, 245.0);
/// Rotate buttons at `240,314` / `267,314` / `282,314`, art-sized: the
/// equipment window authors the same three arts at 28x16 / 16x16 / 28x16.
const ROTATE_Y: f32 = 314.0;
const ROTATE_LEFT: (f32, f32, f32, f32) = (240.0, ROTATE_Y, 28.0, 16.0);
const ROTATE_RESET: (f32, f32, f32, f32) = (267.0, ROTATE_Y, 16.0, 16.0);
const ROTATE_RIGHT: (f32, f32, f32, f32) = (282.0, ROTATE_Y, 28.0, 16.0);

/// `:272` / `:253`, `com_button.ddj` 76x24, captioned `UIIS_CTL_CONFIRM` /
/// `UIIS_CTL_CANCEL` — the `UIIS_` prefix is what the data uses here, mixed on
/// purpose, and is deliberately not "corrected" to `UIIT_`.
const CONFIRM_RECT: (f32, f32, f32, f32) = (106.0, 361.0, 76.0, 24.0);
const CANCEL_RECT: (f32, f32, f32, f32) = (191.0, 361.0, 76.0, 24.0);

/// The site-local `Section = SliderBtn` template (`:429,448,467`), all rects
/// local to the 148x28 band: thumb `24,4` 16x16, prev `2,2` 24x24, next
/// `125,2` 24x24 (the next arrow overhangs the band by 1 px — authored, kept).
const THUMB: (f32, f32) = (16.0, 16.0);
const THUMB_Y: f32 = 4.0;
const PREV: (f32, f32, f32, f32) = (2.0, 2.0, 24.0, 24.0);
const NEXT: (f32, f32, f32, f32) = (125.0, 2.0, 24.0, 24.0);
/// One arrow click. The data carries no step (doc §4), so the step is ours:
/// 1/16 of the range, which gives the same coarse feel as the original's
/// index-based slider without inventing an index count.
const STEP: f32 = 1.0 / 16.0;

/// A normalised body-slider value. The original's range/step/defaults are
/// **not in the data** (doc §4), so the unknown is confined to this newtype
/// instead of being spread as an invented 0..100.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unit(f32);

impl Unit {
    pub fn new(value: f32) -> Self {
        Self(value.clamp(0.0, 1.0))
    }
    pub fn get(self) -> f32 {
        self.0
    }
    fn stepped(self, direction: f32) -> Self {
        Self::new(self.0 + direction * STEP)
    }
}

impl Default for Unit {
    /// Mid-range: no default is authored anywhere, and the midpoint is the
    /// only choice that does not claim to know one.
    fn default() -> Self {
        Self(0.5)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sex {
    #[default]
    Male,
    Female,
}

/// The value both hosts of this panel edit. Layout is per host; this is not.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Appearance {
    pub sex: Sex,
    pub figure: Unit,
    pub height: Unit,
    pub volume: Unit,
}

/// The three body rows, in authored order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyRow {
    Figure,
    Height,
    Volume,
}

impl BodyRow {
    const ALL: [BodyRow; 3] = [BodyRow::Figure, BodyRow::Height, BodyRow::Volume];

    fn index(self) -> usize {
        match self {
            BodyRow::Figure => 0,
            BodyRow::Height => 1,
            BodyRow::Volume => 2,
        }
    }

    /// `UIO_NEWCHAR_STT_*` — the same keys character creation resolves.
    fn key(self) -> (&'static str, &'static str) {
        match self {
            BodyRow::Figure => ("UIO_NEWCHAR_STT_FIGURE", "Figure"),
            BodyRow::Height => ("UIO_NEWCHAR_STT_HEIGHT", "Height"),
            BodyRow::Volume => ("UIO_NEWCHAR_STT_VOLUME", "Volume"),
        }
    }

    fn label_y(self) -> f32 {
        ROW0_LABEL_Y + ROW_PITCH * self.index() as f32
    }

    fn slider_y(self) -> f32 {
        self.label_y() + LABEL_TO_SLIDER
    }

    fn value(self, appearance: &Appearance) -> Unit {
        match self {
            BodyRow::Figure => appearance.figure,
            BodyRow::Height => appearance.height,
            BodyRow::Volume => appearance.volume,
        }
    }

    fn set(self, appearance: &mut Appearance, value: Unit) {
        match self {
            BodyRow::Figure => appearance.figure = value,
            BodyRow::Height => appearance.height = value,
            BodyRow::Volume => appearance.volume = value,
        }
    }
}

/// Thumb travel inside the band.
///
/// Deviation (stated): the data authors the thumb at `24,4` and nothing else —
/// no range, no step, no travel. The thumb therefore slides between the two
/// arrows' inner edges (`PREV` right edge to `NEXT` left edge), which is the
/// only travel the authored chrome actually leaves free.
fn thumb_left(value: Unit) -> f32 {
    let start = PREV.0 + PREV.2;
    let end = NEXT.0;
    start + value.get() * (end - start - THUMB.0)
}

/// Whether the window is up, and the appearance being edited.
#[derive(Resource, Default)]
pub struct AppearanceChangeState {
    pub open: bool,
    pub appearance: Appearance,
}

#[derive(Component)]
pub struct AppearanceChangeWindow;

/// A slider arrow: which row it drives, in which direction.
#[derive(Component)]
struct SliderStep(BodyRow, f32);

/// A sex button.
#[derive(Component)]
struct SexButton(Sex);

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

/// A nine-slice ring from a `*_left_up`/`_mid_up`/... kit, laid out as a 3x3
/// grid with an open centre — the same construction `hud/autopotion/ui.rs`
/// uses for this kit, kept local because it takes this module's plate-local
/// rects.
fn spawn_ring(
    parent: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    dir: &str,
    rect: (f32, f32, f32, f32),
    s: f32,
) {
    let piece = INT_WINDOW_PIECE * s;
    parent
        .spawn((
            Node {
                display: Display::Grid,
                grid_template_columns: vec![
                    RepeatedGridTrack::px(1, piece),
                    RepeatedGridTrack::flex(1, 1.0),
                    RepeatedGridTrack::px(1, piece),
                ],
                grid_template_rows: vec![
                    RepeatedGridTrack::px(1, piece),
                    RepeatedGridTrack::flex(1, 1.0),
                    RepeatedGridTrack::px(1, piece),
                ],
                ..plate_node(rect)
            },
            Pickable::IGNORE,
        ))
        .with_children(|grid| {
            for name in [
                Some("left_up"),
                Some("mid_up"),
                Some("right_up"),
                Some("left_side"),
                None,
                Some("right_side"),
                Some("left_down"),
                Some("mid_down"),
                Some("right_down"),
            ] {
                let mut cell = grid.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                if let Some(name) = name {
                    cell.insert(ImageNode {
                        image: asset_server.load::<Image>(format!("{dir}{name}.ddj")),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    });
                }
            }
        });
}

/// Spawn/despawn the window to match [`AppearanceChangeState`].
pub fn sync_appearance_change(
    state: Res<AppearanceChangeState>,
    ui_strings: Res<ClientUiStrings>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    cameras: Query<Entity, With<Camera2d>>,
    open: Query<Entity, With<AppearanceChangeWindow>>,
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
            AppearanceChangeWindow,
            Name::from("Appearance Change"),
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
                    // the msgbox2_window_ 8-piece ring (16 / 40 / 16)
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

                    // the inner frame, the control pane and its backgrounds
                    spawn_ring(plate, &asset_server, INT_WINDOW_DIR, INFO_FRAME, s);
                    // Deviation: the `opt_inner_box_` kit is drawn as a plain
                    // bordered panel, not transcribed — the same call
                    // `options_audio.rs` already made for the same kit, whose
                    // eight piece extents nothing in our tree has measured.
                    plate.spawn((
                        Node {
                            border: UiRect::all(Val::Px(1.0 * s)),
                            ..plate_node(INNER_BOX)
                        },
                        BorderColor::all(INNER_BOX_BORDER),
                        Pickable::IGNORE,
                    ));
                    for rect in [BG1, BG2, BG3] {
                        plate.spawn(img(
                            rect,
                            format!("{ART}ifcommon/bg_tile/com_bg_tile_b.ddj"),
                        ));
                    }

                    // the preview column
                    plate.spawn((
                        plate_node(VIEW_SQUARE),
                        BackgroundColor(Color::BLACK),
                        Pickable::IGNORE,
                    ));
                    plate.spawn(img(
                        VIEW_BG,
                        format!("{ART}ifcommon/bg_tile/com_bg_tile_e.ddj"),
                    ));
                    // `DDJ=""`: a live render target in the original ([S], doc
                    // §3.2). We have no model preview to hand here, so the
                    // rect is reserved and left empty rather than filled with
                    // art the data does not name.
                    plate.spawn((plate_node(VIEW), Pickable::IGNORE));
                    for (rect, stem) in [
                        (ROTATE_LEFT, "left"),
                        (ROTATE_RESET, "reset"),
                        (ROTATE_RIGHT, "right"),
                    ] {
                        plate.spawn((
                            Button,
                            Hovered::default(),
                            plate_node(rect),
                            ImageNode {
                                image: asset_server.load(format!("{ROTATE_DIR}{stem}_button.ddj")),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                        ));
                    }

                    // every label in the pane: the sex header plus the three
                    // body rows, all `29,y,156,16`
                    let mut labels = vec![(SEX_LABEL_Y, "UIO_NEWCHAR_STT_SEX", "Sex")];
                    for row in BodyRow::ALL {
                        let (key, fallback) = row.key();
                        labels.push((row.label_y(), key, fallback));
                    }
                    for (y, key, fallback) in labels {
                        plate.spawn((
                            Text::new(ui_strings.get_or(key, fallback).to_string()),
                            text_font.clone(),
                            TextColor(Color::WHITE),
                            plate_node((LABEL_X, y, LABEL_W, LABEL_H)),
                            Pickable::IGNORE,
                        ));
                    }
                    for (xy, sex, key, fallback) in [
                        (MALE_XY, Sex::Male, "UIO_NEWCHAR_CTL_MALE", "Male"),
                        (FEMALE_XY, Sex::Female, "UIO_NEWCHAR_CTL_FEMALE", "Female"),
                    ] {
                        let mut button = plate.spawn((
                            SexButton(sex),
                            Button,
                            Hovered::default(),
                            plate_node((xy.0, xy.1, SEX_BTN.0, SEX_BTN.1)),
                            ImageNode {
                                image: asset_server.load(S_BUTTON),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                        ));
                        button.observe(on_sex_pick);
                        button.with_children(|button| {
                            button.spawn((
                                Text::new(ui_strings.get_or(key, fallback).to_string()),
                                text_font.clone(),
                                TextColor(if state.appearance.sex == sex {
                                    Color::WHITE
                                } else {
                                    Color::srgb(0.6, 0.6, 0.6)
                                }),
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

                    // the three body sliders
                    for row in BodyRow::ALL {
                        let y = row.slider_y();
                        plate.spawn(img(
                            (SLIDER_X, y, SLIDER_W, SLIDER_H),
                            SLIDER_BAR.to_string(),
                        ));
                        for (rect, direction, art) in
                            [(PREV, -1.0f32, PREV_ART), (NEXT, 1.0, NEXT_ART)]
                        {
                            plate
                                .spawn((
                                    SliderStep(row, direction),
                                    Button,
                                    Hovered::default(),
                                    plate_node((SLIDER_X + rect.0, y + rect.1, rect.2, rect.3)),
                                    ImageNode {
                                        image: asset_server.load(art),
                                        image_mode: NodeImageMode::Stretch,
                                        ..default()
                                    },
                                ))
                                .observe(on_slider_step);
                        }
                        plate.spawn(img(
                            (
                                SLIDER_X + thumb_left(row.value(&state.appearance)),
                                y + THUMB_Y,
                                THUMB.0,
                                THUMB.1,
                            ),
                            THUMB_ART.to_string(),
                        ));
                    }

                    // the footer
                    for (rect, key, fallback, is_confirm) in [
                        (CONFIRM_RECT, "UIIS_CTL_CONFIRM", "Confirm", true),
                        (CANCEL_RECT, "UIIS_CTL_CANCEL", "Cancel", false),
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

fn on_slider_step(
    activate: On<Activate>,
    steps: Query<&SliderStep>,
    mut state: ResMut<AppearanceChangeState>,
) {
    let Ok(step) = steps.get(activate.entity) else {
        return;
    };
    let value = step.0.value(&state.appearance).stepped(step.1);
    step.0.set(&mut state.appearance, value);
}

fn on_sex_pick(
    activate: On<Activate>,
    buttons: Query<&SexButton>,
    mut state: ResMut<AppearanceChangeState>,
) {
    if let Ok(button) = buttons.get(activate.entity) {
        state.appearance.sex = button.0;
    }
}

/// Confirm. **No packet**: which item opens this window and what it sends are
/// UNKNOWN (doc §9), so the decision is logged and the window closes.
fn on_confirm(_activate: On<Activate>, mut state: ResMut<AppearanceChangeState>) {
    info!(
        "appearance change confirmed: {:?} — no opcode wired yet",
        state.appearance
    );
    state.open = false;
}

fn on_cancel(_activate: On<Activate>, mut state: ResMut<AppearanceChangeState>) {
    state.open = false;
}

/// Close the window when the world scene ends.
pub fn cleanup_appearance_change(
    open: Query<Entity, With<AppearanceChangeWindow>>,
    mut state: ResMut<AppearanceChangeState>,
    mut commands: Commands,
) {
    for entity in open.iter() {
        commands.entity(entity).despawn();
    }
    state.open = false;
}

/// Self-registration (#558).
pub struct AppearanceChangePlugin;

impl Plugin for AppearanceChangePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AppearanceChangeState>()
            .add_systems(OnExit(SceneState::GameWorld), cleanup_appearance_change)
            .add_systems(
                Update,
                sync_appearance_change.run_if(
                    in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting)),
                ),
            );
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// #551-1. The label/slider stack is machine-regular in the data — labels
    /// 164/218/272, sliders 182/236/290, pitch 54 with a constant 18 px
    /// offset, zero jitter. It is generated from the pitch, so the generator
    /// is pinned against the authored values (doc §3.2: inventing a "cleanup"
    /// here would be the error, and so would generating the wrong stack).
    #[test]
    fn the_body_rows_reproduce_the_authored_stack() {
        let labels: Vec<f32> = BodyRow::ALL.iter().map(|row| row.label_y()).collect();
        let sliders: Vec<f32> = BodyRow::ALL.iter().map(|row| row.slider_y()).collect();
        assert_eq!(labels, vec![164.0, 218.0, 272.0]);
        assert_eq!(sliders, vec![182.0, 236.0, 290.0]);
        // and the sex row is a header, not a fourth row (21 px, not 54)
        assert_ne!(SEX_LABEL_Y + ROW_PITCH, ROW0_LABEL_Y);
    }

    /// #551-2. The shell and footer are the authored rects: `0,0,372,400` on
    /// `msgbox2_window_`, Confirm `106,361` / Cancel `191,361` at
    /// `com_button.ddj`'s 76x24, with the authored 9 px gap between them.
    #[test]
    fn the_shell_and_footer_are_the_authored_rects() {
        assert_eq!(PLATE, (372.0, 400.0));
        assert_eq!(CONFIRM_RECT, (106.0, 361.0, 76.0, 24.0));
        assert_eq!(CANCEL_RECT, (191.0, 361.0, 76.0, 24.0));
        assert_eq!(CANCEL_RECT.0 - (CONFIRM_RECT.0 + CONFIRM_RECT.2), 9.0);
        // The footer sits 1 px INTO the plate's bottom inset — `361 + 24 =
        // 385` against the art's client bottom edge `400 - 16 = 384`. That is
        // authored, and it is the same shape as the slider's 1 px next-arrow
        // overhang (§3.3): the classic generation does not inset content to
        // its frame art (doc §3.1). Asserted as measured rather than
        // "corrected" to 360.
        for (_, y, _, h) in [CONFIRM_RECT, CANCEL_RECT] {
            assert_eq!(y + h - (PLATE.1 - MODAL_BOTTOM), 1.0);
            assert!(y + h <= PLATE.1, "but it stays inside the plate");
        }
    }

    /// #551-3. The value is normalised because the range is NOT in the data
    /// (doc §4). Clamping is the whole contract of the newtype: an out-of-range
    /// value can never reach a host, and stepping saturates instead of wrapping.
    #[test]
    fn the_unit_clamps_and_saturates() {
        assert_eq!(Unit::new(-1.0).get(), 0.0);
        assert_eq!(Unit::new(2.0).get(), 1.0);
        assert_eq!(Unit::default().get(), 0.5);
        assert_eq!(Unit::new(1.0).stepped(1.0).get(), 1.0);
        assert_eq!(Unit::new(0.0).stepped(-1.0).get(), 0.0);
        assert_eq!(Unit::new(0.5).stepped(1.0).get(), 0.5 + STEP);
    }

    /// #551-4. The thumb stays inside the band the authored chrome leaves
    /// free — between the two arrows, never under one of them. That is the
    /// stated deviation (the data authors one thumb position and no travel),
    /// so both ends are asserted rather than eyeballed.
    #[test]
    fn the_thumb_travels_between_the_authored_arrows() {
        let min = thumb_left(Unit::new(0.0));
        let max = thumb_left(Unit::new(1.0));
        assert_eq!(min, PREV.0 + PREV.2);
        assert_eq!(max + THUMB.0, NEXT.0);
        assert!(min < max, "the band has to leave the thumb room to move");
        // the next arrow really does overhang the 148-wide band by 1 px
        assert_eq!(NEXT.0 + NEXT.2 - SLIDER_W, 1.0);
    }

    /// #551-5. This window binds the character-creation keys, which is the
    /// finding the unit doc is built on: the panel is shared, only the host is
    /// new. The footer, by contrast, uses the `UIIS_` prefix — mixed on
    /// purpose in the data and not to be "corrected".
    #[test]
    fn the_captions_are_the_shared_newchar_keys() {
        let keys: Vec<&str> = BodyRow::ALL.iter().map(|row| row.key().0).collect();
        assert_eq!(
            keys,
            vec![
                "UIO_NEWCHAR_STT_FIGURE",
                "UIO_NEWCHAR_STT_HEIGHT",
                "UIO_NEWCHAR_STT_VOLUME"
            ]
        );
    }
}
