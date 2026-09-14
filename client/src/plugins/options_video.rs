//! Content of the options window's Video tab.
//!
//! Idea: the pane is transcribed from the **live** branch of
//! `Media.pk2/resinfo/ifoption_video.txt` — the one guarded by `APPLY_UI_4TH`,
//! which `config/define.txt:15` defines, so the `#else` half (with its quality
//! spinner and a 30px-lower list) is build-excluded and is *not* what the
//! shipped client draws. Geometry is page-local to the pane rect
//! `11,62,364,313` from `ifoption.txt`.
//!
//! Two things are deliberately **not** invented here, because the user's data
//! does not carry them:
//!
//! * **Defaults.** No `Default=`/`Min=`/`Max=` key exists anywhere in
//!   `resinfo/`; the original's shipped defaults live in the exe. What a row
//!   shows is whatever [`GameOptions`] holds, whose own baseline is openroad's
//!   (see `settings/options.rs`), not a recovered constant.
//! * **Which value list belongs to which row.** The value vocabulary is closed
//!   and verified (textuisystem 943-960), but nothing in the data binds a list
//!   to a row. Rows whose list is an inference render their raw stored value
//!   rather than a plausible-looking word.
//!
//! Only controls with a real backing feature are interactive. The rest are
//! rendered — the original has them, so hiding them would be its own drift —
//! but dimmed and marked, per the "no fake controls" rule.

use bevy::camera::{Hdr, RenderTarget};
use bevy::picking::hover::Hovered;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use crate::plugins::config::graphics::FoliageMode;
use crate::plugins::config::ClientConfig;
use crate::plugins::settings::options::GameOptions;
use crate::plugins::textdata::ClientUiStrings;

/// Page-local geometry, all `[V]` from `resinfo/ifoption_video.txt`.
const BOARD_X: f32 = 12.0;
const BOARD_Y: f32 = 12.0;
/// `opt_video_control_03.ddj`, 340x64 (verified from its DDS header).
const BOARD_W: f32 = 340.0;
const BOARD_H: f32 = 64.0;
const LABEL_X: f32 = 15.0;
const LABEL_W: f32 = 156.0;
const LABEL_H: f32 = 12.0;
const VALUE_X: f32 = 171.0;
const VALUE_W: f32 = 176.0;
const VALUE_H: f32 = 20.0;
const RESOLUTION_LABEL_Y: f32 = 23.0;
const RESOLUTION_VALUE_Y: f32 = 18.0;
const BRIGHTNESS_LABEL_Y: f32 = 53.0;
const BRIGHTNESS_VALUE_Y: f32 = 48.0;
/// `GDR_OPT_VIDEO_ST_CTRL_1` / `_2`; `opt_video_tab_back.ddj` is 124x28.
const PROFILE_TAB_Y: f32 = 76.0;
const PROFILE_TAB_W: f32 = 124.0;
const PROFILE_TAB_H: f32 = 28.0;
const PROFILE_TAB_XS: [f32; 2] = [13.0, 137.0];
/// `GDR_OPT_VIDEO_DETAIL_OPT`, the scrolling detail list.
const LIST_X: f32 = 14.0;
const LIST_Y: f32 = 103.0;
const LIST_W: f32 = 336.0;
const LIST_H: f32 = 191.0;
/// Row pitch of the 4th-gen slot list; the classic tree carries no pitch, and
/// the row inventory there is `[U]`.
const ROW_H: f32 = 22.0;

const BOARD_DDJ: &str = "media://interface/option/opt_video_control_03.ddj";
const TAB_DDJ: &str = "media://interface/option/opt_video_tab_back.ddj";
const FONT: &str = crate::assets::BUNDLED_FALLBACK_FACE;

const LABEL_COLOR: Color = Color::srgb(1.0, 0.965, 0.827);
const VALUE_COLOR: Color = Color::WHITE;
/// Rows whose feature does not exist yet render dimmed, so the pane never
/// pretends a control does something.
const UNBACKED_COLOR: Color = Color::srgb(0.45, 0.45, 0.45);
const ACTIVE_PROFILE_COLOR: Color = Color::srgb(1.0, 0.816, 0.318);

/// Which of the two graphics banks the pane is editing. The original keeps two
/// complete profiles — ids `1..=15`/`501..=504` and `101..=115`/`601..=604` —
/// selected by the Graphic 1 / Graphic 2 tabs.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum GraphicProfileTab {
    #[default]
    One,
    Two,
}

/// Root of the Video pane content; carries the selected profile.
#[derive(Component, Default)]
pub(crate) struct VideoPane {
    pub(crate) profile: GraphicProfileTab,
}

/// A profile tab caption, recolored when the selection changes.
#[derive(Component)]
pub(crate) struct ProfileTabLabel(GraphicProfileTab);

/// A detail row's value text, refreshed from [`GameOptions`].
#[derive(Component)]
pub(crate) struct RowValue {
    /// SROptionSet id of the Graphic 1 bank; the Graphic 2 id is `+100`.
    id: u16,
}

/// One detail row of the quality list.
struct DetailRow {
    /// Graphic 1 id (`docs/formats/sroptionset.md`); Graphic 2 is `+100`.
    id: u16,
    /// textuisystem key, `[V]` from the resinfo tree.
    key: &'static str,
    /// English fallback, matching the user's own textuisystem line.
    english: &'static str,
    /// Whether a real feature backs this row today.
    backing: Backing,
}

/// Whether a control can actually change anything in this client.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Backing {
    /// Wired to a live feature and interactive.
    Live,
    /// The original has the control, we have no feature for it yet.
    Missing,
}

/// The 13 named quality rows. Ids and names are `[S]` from the SROptionSet
/// table (`docs/formats/sroptionset.md:62-99`); the label keys are `[V]` from
/// the resinfo tree. Ids 14/15 exist in the id space but their CSV name cells
/// are blank, so they are left out rather than captioned by guess.
const DETAIL_ROWS: [DetailRow; 13] = [
    DetailRow {
        id: 1,
        key: "UIIT_STT_SHADOW_DETAIL",
        english: "Shadow Detail",
        backing: Backing::Missing,
    },
    DetailRow {
        id: 2,
        key: "UIIT_STT_SCENERY_SIGHT_RANGE",
        english: "Background Sight Range",
        backing: Backing::Missing,
    },
    DetailRow {
        id: 3,
        key: "UIIT_STT_CHAR_SIGHT_RANGE",
        english: "Character Sight Range",
        backing: Backing::Missing,
    },
    DetailRow {
        id: 4,
        key: "UIIT_STT_WATER_REFLECTION",
        english: "Water Reflection",
        backing: Backing::Missing,
    },
    DetailRow {
        id: 5,
        key: "UIIT_STT_WATER_DETAIL",
        english: "Water Detail",
        backing: Backing::Missing,
    },
    DetailRow {
        id: 6,
        key: "UIIT_STT_METAL_DETAIL",
        english: "Metallic Sheen",
        backing: Backing::Missing,
    },
    DetailRow {
        id: 7,
        key: "UIIT_STT_LIGHT_EFFECT",
        english: "Light Effect",
        backing: Backing::Missing,
    },
    DetailRow {
        id: 8,
        key: "UIIT_STT_FILTERING",
        english: "Texture Filtering",
        backing: Backing::Missing,
    },
    DetailRow {
        id: 9,
        key: "UIIT_STT_TEXTER_DETAIL",
        english: "Texture Detail",
        backing: Backing::Missing,
    },
    DetailRow {
        id: 10,
        key: "UIIT_STT_LENS_FLAIR",
        english: "Lens Flare",
        backing: Backing::Missing,
    },
    // The one quality row this client can actually honour: `Bloom` is
    // insert/removed on the window cameras (see `apply_bloom_option`).
    DetailRow {
        id: 11,
        key: "UIIT_STT_BLOOM_EFFECT",
        english: "Bloom Effect",
        backing: Backing::Live,
    },
    DetailRow {
        id: 12,
        key: "UIIT_STT_DYNAMIC_ANIMATION",
        english: "Dynamic Animation",
        backing: Backing::Missing,
    },
    DetailRow {
        id: 13,
        key: "UIIT_STT_EFFECT_QUALITY",
        english: "Effect Quality",
        backing: Backing::Missing,
    },
];

/// Id of the bloom row within a profile bank.
const BLOOM_ID: u16 = 11;

/// Spawns the Video pane's content into the pane node.
pub(crate) fn spawn_video_pane(
    pane: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    ui_strings: &ClientUiStrings,
    options: &GameOptions,
) {
    let font = asset_server.load::<Font>(FONT);

    pane.spawn((
        ImageNode::new(asset_server.load(BOARD_DDJ)),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(BOARD_X),
            top: Val::Px(BOARD_Y),
            width: Val::Px(BOARD_W),
            height: Val::Px(BOARD_H),
            ..default()
        },
        Pickable::IGNORE,
    ));

    // Resolution and brightness sit inside the board's extent.
    spawn_static_label(
        pane,
        &font,
        ui_strings.get_or("UIIT_STT_RESOLUTION", "Resolution"),
        RESOLUTION_LABEL_Y,
        LABEL_COLOR,
    );
    let profile = &options.video.graphic1;
    spawn_value_text(
        pane,
        &font,
        &format!("{} x {}", profile.width, profile.height),
        RESOLUTION_VALUE_Y,
        VALUE_COLOR,
    );

    spawn_static_label(
        pane,
        &font,
        ui_strings.get_or("UIIT_STT_LUMINOSITY", "Set Brightness"),
        BRIGHTNESS_LABEL_Y,
        LABEL_COLOR,
    );
    // Brightness is a 5-step enum (textuisystem 946-950), not a 0..255 scalar
    // — but nothing in this client applies it, so the raw value is shown
    // rather than a word implying it took effect.
    spawn_value_text(
        pane,
        &font,
        &format!("{} (not applied)", profile.brightness),
        BRIGHTNESS_VALUE_Y,
        UNBACKED_COLOR,
    );

    // Graphic 1 / Graphic 2: tabs over the detail list (the 1px overlap with
    // the list top is the original's tab-over-panel idiom, not a rounding
    // slip).
    let tab_style = asset_server.load::<Image>(TAB_DDJ);
    for (index, (tab, key, english)) in [
        (
            GraphicProfileTab::One,
            "UIIT_STT_GRAPHIC_QUALITY_CONTROL1",
            "Graphic 1",
        ),
        (
            GraphicProfileTab::Two,
            "UIIT_STT_GRAPHIC_QUALITY_CONTROL2",
            "Graphic 2",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        pane.spawn((
            Button,
            Hovered::default(),
            ImageNode::new(tab_style.clone()),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(PROFILE_TAB_XS[index]),
                top: Val::Px(PROFILE_TAB_Y),
                width: Val::Px(PROFILE_TAB_W),
                height: Val::Px(PROFILE_TAB_H),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
        ))
        .observe(
            move |_activate: On<Activate>, mut panes: Query<&mut VideoPane>| {
                for mut pane in panes.iter_mut() {
                    pane.profile = tab;
                }
            },
        )
        .with_children(|b| {
            b.spawn((
                ProfileTabLabel(tab),
                Text::new(ui_strings.get_or(key, english).to_string()),
                TextFont {
                    font: font.clone().into(),
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(if index == 0 {
                    ACTIVE_PROFILE_COLOR
                } else {
                    VALUE_COLOR
                }),
                Pickable::IGNORE,
            ));
        });
    }

    // The detail list. The original scrolls it; the 12-slot pool in the
    // 4th-gen tree is a viewport height, not the row inventory, so all 13
    // named rows are laid out and the list clips.
    pane.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(LIST_X),
            top: Val::Px(LIST_Y),
            width: Val::Px(LIST_W),
            height: Val::Px(LIST_H),
            flex_direction: FlexDirection::Column,
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.2)),
        Pickable::IGNORE,
    ))
    .with_children(|list| {
        for row in &DETAIL_ROWS {
            spawn_detail_row(list, &font, ui_strings, row, options);
        }
        for row in EXTRA_ROWS {
            spawn_extra_row(list, &font, row);
        }
    });
}

// ---------------------------------------------------------------------------
// openroad extras
//
// #366 checked the user's own PK2 before assuming: there is no grass /
// vegetation / foliage control anywhere in v1.188 — not in
// `resinfo/ifoption_video.txt`, not in `ifvideooptionslot.txt`, not in
// `textuisystem`. Our foliage layer is an openroad addition built on authored-
// but-unused tile data, so per ADR-0009 these rows are presented as a *stated*
// addition (own colour, "openroad" in the caption) rather than dressed up in
// the original's vocabulary. They sit below the 13 original rows so the
// vanilla list is untouched.
//
// They write `ClientConfig` directly and are applied by
// `map::foliage::apply_foliage_settings` on the live-settings mechanism
// (`plugins::settings::live`) — no restart, no scene reload.
// ---------------------------------------------------------------------------

/// openroad-added rows, drawn below the original's detail list.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ExtraRow {
    /// `graphics.foliage.mode`.
    FoliageMode,
    /// `graphics.foliage.density`, the tuft-count multiplier.
    FoliageDensity,
    /// `graphics.foliage.view_distance` — #366's "the single biggest win".
    /// Named after the original's own *sight range* vocabulary, which is the
    /// closest thing it has to this concept.
    FoliageSightRange,
}

const EXTRA_ROWS: [ExtraRow; 3] = [
    ExtraRow::FoliageMode,
    ExtraRow::FoliageDensity,
    ExtraRow::FoliageSightRange,
];

/// Density steps. 1.0 is the authored baseline, so the scale is centred on it
/// rather than starting there.
const DENSITY_STEPS: [f32; 4] = [0.5, 1.0, 1.5, 2.0];
/// Sight-range steps in world units. 300 is the config default and covers the
/// camera's working range (`camera.rs`: 40..400); 0 means "never cull", which
/// is what shipped before #366 and is kept reachable but is not a step anyone
/// lands on by accident.
const SIGHT_RANGE_STEPS: [f32; 4] = [150.0, 300.0, 600.0, 0.0];
/// Openroad additions are drawn in their own colour so a screenshot never
/// suggests the original had this control.
const OPENROAD_COLOR: Color = Color::srgb(0.63, 0.84, 1.0);

impl ExtraRow {
    fn label(self) -> &'static str {
        match self {
            ExtraRow::FoliageMode => "Grass (openroad)",
            ExtraRow::FoliageDensity => "Grass Density (openroad)",
            ExtraRow::FoliageSightRange => "Grass Sight Range (openroad)",
        }
    }

    /// What the row shows for the current config.
    fn value_text(self, config: &ClientConfig) -> String {
        let foliage = &config.graphics.foliage;
        match self {
            ExtraRow::FoliageMode => match foliage.mode {
                FoliageMode::Off => "Off (vanilla)".to_string(),
                FoliageMode::Native => "Authored".to_string(),
                FoliageMode::Pack => "Bundled".to_string(),
                FoliageMode::Both => "Authored + Bundled".to_string(),
            },
            ExtraRow::FoliageDensity => format!("{:.1}x", foliage.density),
            ExtraRow::FoliageSightRange => {
                if foliage.view_distance > 0.0 {
                    format!("{:.0}", foliage.view_distance)
                } else {
                    "Unlimited".to_string()
                }
            }
        }
    }

    /// Advances the row one step, writing `ClientConfig`.
    fn cycle(self, config: &mut ClientConfig) {
        let foliage = &mut config.graphics.foliage;
        match self {
            ExtraRow::FoliageMode => {
                foliage.mode = match foliage.mode {
                    FoliageMode::Off => FoliageMode::Native,
                    FoliageMode::Native => FoliageMode::Pack,
                    FoliageMode::Pack => FoliageMode::Both,
                    FoliageMode::Both => FoliageMode::Off,
                }
            }
            ExtraRow::FoliageDensity => {
                foliage.density = next_step(&DENSITY_STEPS, foliage.density)
            }
            ExtraRow::FoliageSightRange => {
                foliage.view_distance = next_step(&SIGHT_RANGE_STEPS, foliage.view_distance)
            }
        }
    }
}

/// The step after `current`, wrapping. A hand-edited `config.yaml` value that
/// is not on the scale snaps to the first step rather than being preserved —
/// the row is a stepper, and silently keeping an off-scale value would make
/// the next click look like it did nothing.
fn next_step(steps: &[f32], current: f32) -> f32 {
    match steps
        .iter()
        .position(|s| (*s - current).abs() < f32::EPSILON)
    {
        Some(i) => steps[(i + 1) % steps.len()],
        None => steps[0],
    }
}

fn spawn_extra_row(list: &mut ChildSpawnerCommands, font: &Handle<Font>, row: ExtraRow) {
    let mut entity = list.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(ROW_H),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect::horizontal(Val::Px(4.0)),
            ..default()
        },
        Button,
        Hovered::default(),
        Pickable::default(),
    ));
    entity.observe(
        move |_activate: On<Activate>, mut config: ResMut<ClientConfig>| {
            row.cycle(&mut config);
        },
    );
    entity.with_children(|r| {
        r.spawn((
            Text::new(row.label()),
            TextFont {
                font: font.clone().into(),
                font_size: FontSize::Px(11.0),
                ..default()
            },
            TextColor(OPENROAD_COLOR),
            Node {
                width: Val::Px(196.0),
                ..default()
            },
            Pickable::IGNORE,
        ));
        r.spawn((
            row,
            Text::new(String::new()),
            TextFont {
                font: font.clone().into(),
                font_size: FontSize::Px(11.0),
                ..default()
            },
            TextColor(OPENROAD_COLOR),
            Pickable::IGNORE,
        ));
    });
}

/// Keeps the openroad rows showing the live `ClientConfig`.
///
/// Runs every frame like [`refresh_row_values`] rather than on
/// `resource_changed`: the rows are spawned with empty text (the pane builder
/// has no `ClientConfig`), so they need a fill on their first frame too, and a
/// handful of short string formats is not worth a second code path.
pub(crate) fn refresh_extra_rows(
    config: Res<ClientConfig>,
    mut rows: Query<(&ExtraRow, &mut Text)>,
) {
    for (row, mut text) in rows.iter_mut() {
        let wanted = row.value_text(&config);
        if text.0 != wanted {
            *text = Text::new(wanted);
        }
    }
}

fn spawn_detail_row(
    list: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    ui_strings: &ClientUiStrings,
    row: &DetailRow,
    options: &GameOptions,
) {
    let live = row.backing == Backing::Live;
    let id = row.id;

    let mut entity = list.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(ROW_H),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect::horizontal(Val::Px(4.0)),
            ..default()
        },
        Pickable::IGNORE,
    ));
    if live {
        entity.insert((Button, Hovered::default(), Pickable::default()));
        entity.observe(
            move |_activate: On<Activate>,
                  panes: Query<&VideoPane>,
                  mut options: ResMut<GameOptions>| {
                let profile = panes.iter().next().map(|p| p.profile).unwrap_or_default();
                let bank = match profile {
                    GraphicProfileTab::One => &mut options.video.graphic1,
                    GraphicProfileTab::Two => &mut options.video.graphic2,
                };
                let current = bank.quality.get(&id).copied().unwrap_or(1);
                bank.quality.insert(id, u16::from(current == 0));
            },
        );
    }

    entity.with_children(|r| {
        r.spawn((
            Text::new(ui_strings.get_or(row.key, row.english).to_string()),
            TextFont {
                font: font.clone().into(),
                font_size: FontSize::Px(11.0),
                ..default()
            },
            TextColor(if live { LABEL_COLOR } else { UNBACKED_COLOR }),
            Node {
                width: Val::Px(196.0),
                ..default()
            },
            Pickable::IGNORE,
        ));
        r.spawn((
            RowValue { id: row.id },
            Text::new(row_value_text(row, options, GraphicProfileTab::One)),
            TextFont {
                font: font.clone().into(),
                font_size: FontSize::Px(11.0),
                ..default()
            },
            TextColor(if live { VALUE_COLOR } else { UNBACKED_COLOR }),
            Pickable::IGNORE,
        ));
    });
}

/// What a row shows. Backed rows render their state; unbacked ones say so
/// instead of borrowing a value word from the original's vocabulary.
fn row_value_text(row: &DetailRow, options: &GameOptions, profile: GraphicProfileTab) -> String {
    let bank = match profile {
        GraphicProfileTab::One => &options.video.graphic1,
        GraphicProfileTab::Two => &options.video.graphic2,
    };
    let value = bank.quality.get(&row.id).copied().unwrap_or_default();
    match row.backing {
        Backing::Live => if value == 0 { "Off" } else { "On" }.to_string(),
        Backing::Missing => format!("{value} (no effect yet)"),
    }
}

fn spawn_static_label(
    pane: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    text: &str,
    top: f32,
    color: Color,
) {
    pane.spawn((
        Text::new(text.to_string()),
        TextFont {
            font: font.clone().into(),
            font_size: FontSize::Px(11.0),
            ..default()
        },
        TextColor(color),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(LABEL_X),
            top: Val::Px(top),
            width: Val::Px(LABEL_W),
            height: Val::Px(LABEL_H),
            ..default()
        },
        Pickable::IGNORE,
    ));
}

fn spawn_value_text(
    pane: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    text: &str,
    top: f32,
    color: Color,
) {
    pane.spawn((
        Text::new(text.to_string()),
        TextFont {
            font: font.clone().into(),
            font_size: FontSize::Px(11.0),
            ..default()
        },
        TextColor(color),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(VALUE_X),
            top: Val::Px(top),
            width: Val::Px(VALUE_W),
            height: Val::Px(VALUE_H),
            ..default()
        },
        Pickable::IGNORE,
    ));
}

/// Recolors the profile tabs and rewrites every row value when the selected
/// profile changes.
pub(crate) fn apply_profile_tab(
    panes: Query<&VideoPane, Changed<VideoPane>>,
    options: Res<GameOptions>,
    mut labels: Query<(&ProfileTabLabel, &mut TextColor)>,
    mut values: Query<(&RowValue, &mut Text), Without<ProfileTabLabel>>,
) {
    let Some(pane) = panes.iter().next() else {
        return;
    };
    for (label, mut color) in labels.iter_mut() {
        color.0 = if label.0 == pane.profile {
            ACTIVE_PROFILE_COLOR
        } else {
            VALUE_COLOR
        };
    }
    for (value, mut text) in values.iter_mut() {
        let Some(row) = DETAIL_ROWS.iter().find(|r| r.id == value.id) else {
            continue;
        };
        *text = Text::new(row_value_text(row, &options, pane.profile));
    }
}

/// Rewrites row values after an edit, so a click shows its effect.
pub(crate) fn refresh_row_values(
    options: Res<GameOptions>,
    panes: Query<&VideoPane>,
    mut values: Query<(&RowValue, &mut Text)>,
) {
    if !options.is_changed() {
        return;
    }
    let profile = panes.iter().next().map(|p| p.profile).unwrap_or_default();
    for (value, mut text) in values.iter_mut() {
        let Some(row) = DETAIL_ROWS.iter().find(|r| r.id == value.id) else {
            continue;
        };
        *text = Text::new(row_value_text(row, &options, profile));
    }
}

// The window itself is not applied from here any more. Both halves —
// `setup_window_settings` (the boot apply) and `apply_window_mode` (the
// options-driven one) — moved into `plugins::config::window`, which now
// resolves one `WindowIntent` from `config.yaml` plus the session-only
// `video.window_mode_override` and is the single writer. Two modules each
// computing their own answer for `window.mode` is exactly what made
// `config.yaml`'s `mode:` a dead key: whichever ran last won, and that was
// this one.

/// Applies the Bloom quality row to the window cameras.
///
/// Mirrors the dev inspector's toggle (`plugins::dev::render_debug`): `Bloom`
/// is `#[require(Hdr)]`, and a required component is not removed with the one
/// that pulled it in, so `Hdr` has to go explicitly or "bloom off" still
/// renders to a float target. Gated on the config switch for the same reason
/// the dev path is: with bloom disabled in `config.yaml` there is no intensity
/// to apply.
pub(crate) fn apply_bloom_option(
    options: Res<GameOptions>,
    config: Res<ClientConfig>,
    panes: Query<&VideoPane>,
    cameras: Query<(Entity, &RenderTarget), With<Camera3d>>,
    mut commands: Commands,
) {
    if !options.is_changed() || !config.graphics.bloom.enabled {
        return;
    }
    let profile = panes.iter().next().map(|p| p.profile).unwrap_or_default();
    let bank = match profile {
        GraphicProfileTab::One => &options.video.graphic1,
        GraphicProfileTab::Two => &options.video.graphic2,
    };
    let on = bank.quality.get(&BLOOM_ID).copied().unwrap_or(1) != 0;

    for (entity, target) in cameras.iter() {
        if !matches!(target, RenderTarget::Window(_)) {
            continue;
        }
        if on {
            commands
                .entity(entity)
                .insert(config.graphics.bloom.to_bloom());
        } else {
            commands.entity(entity).remove::<(Bloom, Hdr)>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_rows_cover_the_named_sroptionset_ids() {
        // Ids 1..=13 are the named Graphic 1 quality slots; 14/15 exist in the
        // id space but their CSV name cells are blank, so they are left out
        // rather than captioned by guess.
        let ids: Vec<u16> = DETAIL_ROWS.iter().map(|r| r.id).collect();
        assert_eq!(ids, (1..=13).collect::<Vec<u16>>());
    }

    #[test]
    fn only_backed_rows_claim_an_effect() {
        // Every row without a real feature must say so; none may render a
        // value word that implies it took effect.
        let options = GameOptions::default();
        for row in &DETAIL_ROWS {
            let text = row_value_text(row, &options, GraphicProfileTab::One);
            match row.backing {
                Backing::Missing => assert!(
                    text.contains("no effect yet"),
                    "{} must be marked unbacked, got {text:?}",
                    row.english
                ),
                Backing::Live => assert!(
                    text == "On" || text == "Off",
                    "{} is wired and must show its state, got {text:?}",
                    row.english
                ),
            }
        }
    }

    #[test]
    fn bloom_is_the_only_live_row() {
        // If a later change wires another feature, this test should be updated
        // deliberately — it is the guard against quietly marking rows Live.
        let live: Vec<u16> = DETAIL_ROWS
            .iter()
            .filter(|r| r.backing == Backing::Live)
            .map(|r| r.id)
            .collect();
        assert_eq!(live, vec![BLOOM_ID]);
    }

    #[test]
    fn row_value_follows_the_selected_profile() {
        let mut options = GameOptions::default();
        options.video.graphic1.quality.insert(BLOOM_ID, 1);
        options.video.graphic2.quality.insert(BLOOM_ID, 0);
        let bloom = DETAIL_ROWS.iter().find(|r| r.id == BLOOM_ID).unwrap();
        assert_eq!(
            row_value_text(bloom, &options, GraphicProfileTab::One),
            "On"
        );
        assert_eq!(
            row_value_text(bloom, &options, GraphicProfileTab::Two),
            "Off"
        );
    }

    /// The grass rows are openroad additions, and #366 is the reason: the
    /// user's own PK2 has no grass/vegetation/foliage control in
    /// `ifoption_video.txt`, `ifvideooptionslot.txt` or `textuisystem`. A row
    /// that borrowed an original caption would be claiming fidelity we do not
    /// have, so every extra row has to say "openroad" in its label.
    #[test]
    fn every_extra_row_declares_itself_an_openroad_addition() {
        for row in EXTRA_ROWS {
            assert!(
                row.label().contains("openroad"),
                "{row:?} does not mark itself as an addition: {:?}",
                row.label()
            );
        }
        // ...and none of them borrows an original row's caption.
        for row in EXTRA_ROWS {
            assert!(
                !DETAIL_ROWS.iter().any(|d| d.english == row.label()),
                "{row:?} reuses an original caption"
            );
        }
    }

    /// The steppers must return to where they started, or a user who clicks
    /// past the value they wanted can never get back to it.
    #[test]
    fn every_extra_row_cycles_back_to_its_starting_value() {
        for row in EXTRA_ROWS {
            let mut config = test_config();
            let start = row.value_text(&config);
            let steps = match row {
                ExtraRow::FoliageMode => 4,
                ExtraRow::FoliageDensity => DENSITY_STEPS.len(),
                ExtraRow::FoliageSightRange => SIGHT_RANGE_STEPS.len(),
            };
            let mut seen = vec![start.clone()];
            for _ in 0..steps {
                row.cycle(&mut config);
                seen.push(row.value_text(&config));
            }
            assert_eq!(seen.first(), seen.last(), "{row:?} does not wrap: {seen:?}");
            assert_eq!(
                seen[..steps]
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len(),
                steps,
                "{row:?} repeats a value inside one lap: {seen:?}"
            );
        }
    }

    /// `off` is the faithful v1.188 setting (#366, #646): the default must
    /// stay off, and the mode row must start there rather than at a mode that
    /// renders grass the original never had.
    #[test]
    fn the_shipped_default_is_the_vanilla_off_mode() {
        let config = test_config();
        assert_eq!(config.graphics.foliage.mode, FoliageMode::Off);
        assert_eq!(
            ExtraRow::FoliageMode.value_text(&config),
            "Off (vanilla)",
            "the off state has to name itself as the original behaviour"
        );
    }

    /// A hand-edited `config.yaml` value that is not on the stepper's scale
    /// must snap onto it, or the first click looks like it did nothing.
    #[test]
    fn an_off_scale_value_snaps_onto_the_first_step() {
        assert_eq!(next_step(&DENSITY_STEPS, 0.37), DENSITY_STEPS[0]);
        assert_eq!(next_step(&SIGHT_RANGE_STEPS, 1234.0), SIGHT_RANGE_STEPS[0]);
        // and an on-scale value advances rather than snapping
        assert_eq!(next_step(&DENSITY_STEPS, 1.0), 1.5);
    }

    /// `config.example.yaml` through the loader `main()` uses — the same file a
    /// fresh setup copies (#539) — so the rows are exercised against the real
    /// shipped defaults rather than against a struct literal.
    fn test_config() -> ClientConfig {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../config.example.yaml")
            .with_extension("");
        ClientConfig::from_file(path.to_str().expect("the example path is utf-8"))
            .expect("config.example.yaml matches ClientConfig")
    }
}
