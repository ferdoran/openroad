//! Region-select board — the original step between character select and
//! character creation.
//!
//! Idea: in v1.188 the Create button does not open the creation screen
//! directly; it swaps the select screen into a "Region Select" sub-mode
//! (resinfo/pscharacterselect.txt, section `Select`): the title image changes
//! to `text-region.ddj`, and translucent race plates (`china.ddj` /
//! `europe.ddj`, 440x152, sections `China`/`Europe`) hover over the 3D stage.
//! Picking a plate cuts to a full-screen race-themed loading screen
//! (`loading_charactercustom[_europe].ddj` + `loading_form`/`nowloading`) and
//! then enters the per-race creation screen. All of that is data; only the
//! plate SCREEN POSITIONS and the select→board camera are original client
//! code (UNKNOWN) — the plates here sit as a centered pair (slot order from
//! the baked tab art: europe center, china right) over the held creation
//! camera pose, with no invented camera flight.

use bevy::ecs::relationship::RelatedSpawnerCommands;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::Overflow;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::textdata::{ClientCharacterData, ClientUiStrings};
use crate::plugins::ui_v2::style::ButtonSound;
use crate::plugins::ui_v2::widgets::{image_button, label};
use crate::scenes::intro_v2::character_create::{
    figure_variants, CharCreateSelection, Gender, Race,
};
use crate::scenes::loading_screen::spawn_loading_chrome;

use super::assets::IntroV2Assets;
use super::chrome::InfoTextV2Update;
use super::login_form::main_button_style;
use super::{IntroV2State, IntroV2Ui};

/// Root marker of the region-select UI.
#[derive(Component, Default, Clone)]
pub struct RegionSelectRoot;

/// A clickable race plate.
#[derive(Component, Clone, Copy)]
pub struct RegionPlate(pub Race);

/// The full-screen loading cut shown between the board and the creation
/// screen (original: GDR_LOADING_CHINA/EUROPE). Ticks down inside
/// `CharacterCreate` and despawns; the original gauge is skipped (no real
/// progress source — the creation stage loads asynchronously anyway).
#[derive(Component)]
pub struct CreationLoadingCut(pub Timer);

/// Plate texture paths from pscharacterselect.txt, section `Select`.
const PLATE_CHINA_DDJ: &str = "media://interface/outer/china.ddj";
const PLATE_EUROPE_DDJ: &str = "media://interface/outer/europe.ddj";
const TITLE_REGION_DDJ: &str = "media://interface/outer/text-region.ddj";

/// Every rect this screen takes from the data, in one place so a test can pin
/// them (#644).
///
/// **What the data does and does not author.** All three plates —
/// `GDR_STA_CHINA` (9), `GDR_STA_ISLAM` (10), `GDR_STA_EUROPE` (11) — declare
/// `Rect="0,0,440,152"`: the **size** is authored, the **screen position is
/// not**, exactly like the twelve other `0,0,w,h` controls in section
/// `Select`. So the plate placement below is openroad's (a centered pair), and
/// no amount of re-reading the resinfo will change that; it is `[U]`, not a
/// gap. The title, in contrast, *is* placed by the data at `47,110`.
///
/// Each plate's insides come from its own race section, and the label x
/// genuinely differs per race — China `318`, Europe `250`.
const TITLE_RECT: (f32, f32, f32, f32) = (47.0, 110.0, 292.0, 36.0);
/// The 1600x1200 virtual canvas the `Select` rects are authored on.
const DESIGN: (f32, f32) = (1600.0, 1200.0);
const PLATE_SIZE: (f32, f32) = (440.0, 152.0);
/// `GDR_STATIC1` in sections `China`/`Europe`, plate-local; only x differs.
const PLATE_LABEL_RECT: (f32, f32, f32, f32) = (0.0, 9.0, 92.0, 15.0);
const PLATE_LABEL_X_CHINA: f32 = 318.0;
const PLATE_LABEL_X_EUROPE: f32 = 250.0;
/// `GDR_TEXT_CHINA`/`GDR_TEXT_EUROPE` `CIFTextBox`, plate-local.
const PLATE_BODY_RECT: (f32, f32, f32, f32) = (19.0, 47.0, 401.0, 87.0);
const LOADING_CHINA_DDJ: &str = "media://interface/loading/loading_charactercustom.ddj";
const LOADING_EUROPE_DDJ: &str = "media://interface/loading/loading_charactercustom_europe.ddj";

/// Minimum display time of the loading cut. The original shows it for the
/// duration of the scene load; ours loads fast, so this keeps the cut
/// readable (timing UNKNOWN — not data-derived).
const LOADING_CUT_SECS: f32 = 1.2;

/// Dim tint of a data-blocked plate (no body rows for the race).
const PLATE_DISABLED_TINT: Color = Color::srgba(0.45, 0.45, 0.45, 1.0);

/// Reason shown in the body rect of a data-blocked plate. Deliberate
/// deviation from the original (#643): the v1.188 client always ships both
/// races, so it has no "this race is missing" state and no textuisystem key
/// for one. A corpus without `CHAR_EU_*` figure rows (verified on the
/// maintainer's Media.pk2: `characterdata_5000.txt` holds the 26 `CHAR_CH_*`
/// bodies, ids 1907-1932, and no shard holds a `CHAR_EU_*` row) would
/// otherwise dim the plate with no stated cause, which reads as a client bug.
const PLATE_DISABLED_REASON: &str =
    "Unavailable: this PK2 ships no character bodies for this race\n     (server_dep/silkroad/textdata/characterdata*.txt).";

/// Whether `race` can be created at all on this corpus: the race board and the
/// plate click must agree, so both ask this one predicate. A race is offerable
/// exactly when characterdata holds at least one body row for either gender
/// (`CHAR_{CH,EU}_{MAN,WOMAN}_*`, see `Race::body_prefix`).
pub(crate) fn race_available(char_data: &ClientCharacterData, race: Race) -> bool {
    !figure_variants(char_data, race, Gender::Male).is_empty()
        || !figure_variants(char_data, race, Gender::Female).is_empty()
}

/// `OnEnter(RegionSelect)`: clears the connection keep-alive marker (its job
/// ended with the CharacterList exit) and spawns title + plates + Cancel.
pub fn enter_region_select(
    mut commands: Commands,
    assets: Res<IntroV2Assets>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    char_data: Res<ClientCharacterData>,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    commands.remove_resource::<super::character_create::EnteringCharacterCreate>();

    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found");
        return;
    };

    // Title image at the original anchor (47,110 on the 1600x1200 virtual
    // canvas → percent so it tracks the window).
    commands.spawn((
        RegionSelectRoot,
        IntroV2Ui,
        UiTargetCamera(camera),
        Name::from("Region Select Title"),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(100.0 * TITLE_RECT.0 / DESIGN.0),
            top: Val::Percent(100.0 * TITLE_RECT.1 / DESIGN.1),
            width: Val::Px(TITLE_RECT.2),
            height: Val::Px(TITLE_RECT.3),
            ..default()
        },
        ImageNode::new(asset_server.load(TITLE_REGION_DDJ)),
        Pickable::IGNORE,
    ));

    // The plate pair, centered over the stage. Slot order from the baked tab
    // art (europe center, china right); exact original positions are runtime
    // code (UNKNOWN).
    let eu_available = race_available(&char_data, Race::European);
    let cn_available = race_available(&char_data, Race::Chinese);
    commands
        .spawn((
            RegionSelectRoot,
            IntroV2Ui,
            UiTargetCamera(camera),
            Name::from("Region Select Plates"),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                column_gap: Val::Px(40.0),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|row| {
            spawn_plate(
                row,
                &asset_server,
                &ui_strings,
                &fonts,
                Race::European,
                PLATE_EUROPE_DDJ,
                // label tab rect from section `Europe`: 250,9 (92x15)
                PLATE_LABEL_X_EUROPE,
                "UIO_NEWCHAR_CTL_EUROPEAN",
                "European",
                "UIO_NEWCHAR_CTL_EUROPEAN_TT",
                eu_available,
            );
            spawn_plate(
                row,
                &asset_server,
                &ui_strings,
                &fonts,
                Race::Chinese,
                PLATE_CHINA_DDJ,
                // label tab rect from section `China`: 318,9 (92x15)
                PLATE_LABEL_X_CHINA,
                "UIO_NEWCHAR_CTL_CHINESE",
                "Chinese",
                "UIO_NEWCHAR_CTL_CHINESE_TT",
                cn_available,
            );
        });

    // Cancel back to the list (original: one of the GDR_BTN_CANCEL trio —
    // which of the three belongs to the board sub-mode is UNKNOWN).
    let cancel_font = fonts.nine.clone();
    let cancel_sound = assets.sound_button_sound_a.clone();
    let cancel_text = ui_strings
        .get_or("UIO_COMMON_CTL_CANCEL", "Cancel")
        .to_string();
    commands
        .spawn_scene(bsn! {
            RegionSelectRoot
            Name("Region Select Controls")
            Node {
                position_type: PositionType::Absolute,
                flex_direction: FlexDirection::Row,
                bottom: percent(7.5),
                right: percent(1),
            }
            Children [
                (
                    image_button(main_button_style(&assets), 91.0, 41.0)
                    ButtonSound({cancel_sound})
                    Children [ (label(&cancel_text, cancel_font, 16.0) TextColor(Color::WHITE)) ]
                    on(|_a: On<Activate>, mut next_state: ResMut<NextState<IntroV2State>>| {
                        next_state.set(IntroV2State::CharacterList);
                    })
                ),
            ]
        })
        .insert((UiTargetCamera(camera), IntroV2Ui));
}

/// One 440x152 race plate: baked art + label text on its tab + the vanilla
/// race description in the body rect (19,47,401x87; the original shows it in
/// the plate — hover-only vs always is UNKNOWN, always is chosen here).
#[allow(clippy::too_many_arguments)]
fn spawn_plate(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    ui_strings: &ClientUiStrings,
    fonts: &FontAssets,
    race: Race,
    plate_ddj: &str,
    label_x: f32,
    label_key: &str,
    label_fallback: &str,
    desc_key: &str,
    available: bool,
) {
    let tint = if available {
        Color::WHITE
    } else {
        PLATE_DISABLED_TINT
    };
    let mut plate = parent.spawn((
        RegionPlate(race),
        Button,
        Hovered::default(),
        Name::from(format!("Region Plate {race:?}")),
        Node {
            width: Val::Px(PLATE_SIZE.0),
            height: Val::Px(PLATE_SIZE.1),
            ..default()
        },
        ImageNode {
            image: asset_server.load(plate_ddj.to_string()),
            color: tint,
            ..default()
        },
    ));
    plate.observe(on_plate_clicked);
    plate.with_children(|p| {
        p.spawn((
            Text::new(ui_strings.get_or(label_key, label_fallback).to_string()),
            TextFont {
                font: fonts.nine.clone().into(),
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(if available {
                Color::WHITE
            } else {
                PLATE_DISABLED_TINT
            }),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(label_x),
                top: Val::Px(PLATE_LABEL_RECT.1),
                width: Val::Px(PLATE_LABEL_RECT.2),
                height: Val::Px(PLATE_LABEL_RECT.3),
                justify_content: JustifyContent::Center,
                ..default()
            },
            Pickable::IGNORE,
        ));
        // A blocked plate states its reason in the body rect instead of the
        // race description: dimming alone is indistinguishable from a client
        // bug, and the cause (missing characterdata rows) is not guessable.
        let body_text = if available {
            ui_strings.get_or(desc_key, "").to_string()
        } else {
            PLATE_DISABLED_REASON.to_string()
        };
        p.spawn((
            Text::new(body_text),
            TextFont {
                font: fonts.nine.clone().into(),
                font_size: FontSize::Px(10.0),
                ..default()
            },
            TextColor(Color::srgba(0.9, 0.9, 0.9, 0.9)),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(PLATE_BODY_RECT.0),
                top: Val::Px(PLATE_BODY_RECT.1),
                width: Val::Px(PLATE_BODY_RECT.2),
                height: Val::Px(PLATE_BODY_RECT.3),
                overflow: Overflow::clip(),
                ..default()
            },
            Pickable::IGNORE,
        ));
    });
}

/// Plate click: data-blocked races only report the gap; otherwise seed the
/// creation selection with the race, show the original loading cut and enter
/// the creation screen.
fn on_plate_clicked(
    activate: On<Activate>,
    plates: Query<&RegionPlate>,
    char_data: Res<ClientCharacterData>,
    ui_strings: Res<ClientUiStrings>,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut info_text_writer: MessageWriter<InfoTextV2Update>,
    mut next_state: ResMut<NextState<IntroV2State>>,
    mut commands: Commands,
) {
    let Ok(&RegionPlate(race)) = plates.get(activate.entity) else {
        return;
    };
    if !race_available(&char_data, race) {
        // The label key follows the clicked plate — reporting "European" for a
        // blocked Chinese plate was the original wording bug here (#643).
        let (key, fallback) = match race {
            Race::Chinese => ("UIO_NEWCHAR_CTL_CHINESE", "Chinese"),
            Race::European => ("UIO_NEWCHAR_CTL_EUROPEAN", "European"),
        };
        info_text_writer.write(InfoTextV2Update(format!(
            "No {} character data in this client's PK2.",
            ui_strings.get_or(key, fallback)
        )));
        return;
    }

    commands.insert_resource(CharCreateSelection {
        race,
        ..Default::default()
    });

    if let Some(camera) = cam_query.iter().next() {
        spawn_loading_cut(&mut commands, &asset_server, race, camera);
    }
    next_state.set(IntroV2State::CharacterCreate);
}

/// The original board→creation cut: full-screen race art + the loading-bar
/// chrome (gauge skipped — no real progress source).
fn spawn_loading_cut(
    commands: &mut Commands,
    asset_server: &AssetServer,
    race: Race,
    camera: Entity,
) {
    let art = match race {
        Race::Chinese => LOADING_CHINA_DDJ,
        Race::European => LOADING_EUROPE_DDJ,
    };
    commands
        .spawn((
            CreationLoadingCut(Timer::from_seconds(LOADING_CUT_SECS, TimerMode::Once)),
            UiTargetCamera(camera),
            Name::from("Creation Loading Cut"),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            ImageNode::new(asset_server.load(art)),
            GlobalZIndex(500),
        ))
        .with_children(|cut| {
            // the authored chrome, from the module that owns the rects: the
            // hand-copied version here mixed design spaces (percentage frame,
            // Val::Px caption), so the caption slid off the frame at any size
            // but exactly design scale
            spawn_loading_chrome(cut, asset_server, false);
        });
}

/// Ticks the loading cut down and removes it (runs in `CharacterCreate`).
pub fn tick_loading_cut(
    time: Res<Time>,
    mut cuts: Query<(Entity, &mut CreationLoadingCut)>,
    mut commands: Commands,
) {
    for (entity, mut cut) in cuts.iter_mut() {
        if cut.0.tick(time.delta()).just_finished() {
            commands.entity(entity).despawn();
        }
    }
}

/// `OnExit(RegionSelect)` / `OnExit(CharacterCreate)` cleanup.
pub fn despawn_region_select(roots: Query<Entity, With<RegionSelectRoot>>, mut commands: Commands) {
    for entity in roots.iter() {
        commands.entity(entity).despawn();
    }
}

/// Safety net: a lingering cut is removed when creation is left.
pub fn despawn_loading_cut(cuts: Query<Entity, With<CreationLoadingCut>>, mut commands: Commands) {
    for entity in cuts.iter() {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::textdata::characterdata::{CharacterData, CharacterDataRow};
    use std::collections::HashMap;

    /// Every rect this screen takes from `resinfo/pscharacterselect.txt`
    /// section `Select` and its `China`/`Europe` sections, pinned so a later
    /// edit cannot drift them (#644). Values re-read from the maintainer's own
    /// Media.pk2 on 2026-08-15, cross-checked against the DDS headers of the
    /// art they place.
    #[test]
    fn the_board_rects_are_the_authored_ones() {
        // GDR_STA_REGIONTITLE "47,110,292,36" — text-region.ddj measures 292x36
        assert_eq!(TITLE_RECT, (47.0, 110.0, 292.0, 36.0));
        // GDR_STA_CHINA / _ISLAM / _EUROPE all "0,0,440,152"; all three arts
        // measure 440x152
        assert_eq!(PLATE_SIZE, (440.0, 152.0));
        // GDR_STATIC1 in each race section: 92x15 at y=9, x per race
        assert_eq!(
            (PLATE_LABEL_RECT.1, PLATE_LABEL_RECT.2, PLATE_LABEL_RECT.3),
            (9.0, 92.0, 15.0)
        );
        assert_eq!(PLATE_LABEL_X_CHINA, 318.0);
        assert_eq!(PLATE_LABEL_X_EUROPE, 250.0);
        assert_ne!(
            PLATE_LABEL_X_CHINA, PLATE_LABEL_X_EUROPE,
            "the label x really does differ per race section"
        );
        // GDR_TEXT_CHINA / GDR_TEXT_EUROPE "19,47,401,87"
        assert_eq!(PLATE_BODY_RECT, (19.0, 47.0, 401.0, 87.0));
    }

    /// The plate-local rects have to fit the plate, and the title has to fit
    /// the 1600x1200 canvas its percentage placement is derived from.
    #[test]
    fn the_authored_rects_fit_their_parents() {
        assert_eq!(DESIGN, (1600.0, 1200.0));
        assert!(TITLE_RECT.0 + TITLE_RECT.2 <= DESIGN.0);
        assert!(TITLE_RECT.1 + TITLE_RECT.3 <= DESIGN.1);
        for x in [PLATE_LABEL_X_CHINA, PLATE_LABEL_X_EUROPE] {
            assert!(x + PLATE_LABEL_RECT.2 <= PLATE_SIZE.0);
        }
        assert!(PLATE_LABEL_RECT.1 + PLATE_LABEL_RECT.3 <= PLATE_SIZE.1);
        assert!(PLATE_BODY_RECT.0 + PLATE_BODY_RECT.2 <= PLATE_SIZE.0);
        assert!(PLATE_BODY_RECT.1 + PLATE_BODY_RECT.3 <= PLATE_SIZE.1);
        // label above body, both inside the plate
        assert!(PLATE_LABEL_RECT.1 + PLATE_LABEL_RECT.3 <= PLATE_BODY_RECT.1);
    }

    /// Minimal characterdata row: the parser keeps any tab row with >10
    /// columns and reads the code name from column 2, so only the id and the
    /// code name matter for the race predicate.
    fn row(id: i32, code_name: &str) -> (i32, CharacterDataRow) {
        let mut cols = vec![String::new(); 60];
        cols[1] = id.to_string();
        cols[2] = code_name.to_string();
        (id, CharacterDataRow(cols))
    }

    fn char_data(rows: Vec<(i32, CharacterDataRow)>) -> ClientCharacterData {
        ClientCharacterData::from_table(CharacterData(rows.into_iter().collect::<HashMap<_, _>>()))
    }

    /// The maintainer's Media.pk2 as read through our own textdata parser
    /// (#643): `characterdata_5000.txt` carries the 26 `CHAR_CH_*` player
    /// bodies at ids 1907-1932 and **no** shard carries a `CHAR_EU_*` row —
    /// the `MOB_EU_*`/`NPC_EU_*` rows in the other shards prove the `EU`
    /// token itself reads fine, so the absence is data, not a decode bug.
    /// Consequence: Chinese is offerable, European is not.
    #[test]
    fn a_corpus_without_european_bodies_blocks_only_the_europe_plate() {
        let data = char_data(vec![
            row(1907, "CHAR_CH_MAN_ADVENTURER"),
            row(1919, "CHAR_CH_MAN_WARRIOR"),
            row(1920, "CHAR_CH_WOMAN_ADVENTURER"),
            row(1932, "CHAR_CH_WOMAN_WARRIOR"),
            // same corpus, same read path: EU exists as monsters/NPCs only
            row(5851, "MOB_EU_MOVOI"),
            row(41769, "NPC_EU_EVENT_CARNIVAL_OBJECT_2011"),
        ]);
        assert!(race_available(&data, Race::Chinese));
        assert!(!race_available(&data, Race::European));
    }

    /// The regression guard proper: when a corpus *does* ship European bodies,
    /// the predicate must offer Europe. This is what would have caught a
    /// broken race predicate, which is indistinguishable in the UI from a
    /// corpus that simply has no European rows.
    #[test]
    fn european_bodies_make_the_europe_plate_offerable() {
        let male_only = char_data(vec![row(14000, "CHAR_EU_MAN_FIGHTER")]);
        assert!(race_available(&male_only, Race::European));

        let female_only = char_data(vec![row(14100, "CHAR_EU_WOMAN_FIGHTER")]);
        assert!(race_available(&female_only, Race::European));
    }

    /// A blocked plate must name its cause; dimming alone is what made #643
    /// look like a client defect rather than a data gap.
    #[test]
    fn a_blocked_plate_names_the_file_that_would_unblock_it() {
        assert!(PLATE_DISABLED_REASON.contains("characterdata"));
    }
}
