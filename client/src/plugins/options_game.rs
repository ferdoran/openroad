//! Options -> Setting pane (`OptionsTab::Game`): the gameplay/HUD toggle rows.
//!
//! Idea: vanilla's Game pane declares **no rows of its own**. `ifoption_game.txt`
//! is four blocks — two section headers (`UIIT_STT_NAMEVIEW` at `13,19`,
//! `UIIT_STT_GAMESET` at `13,162`) over two `CIFScrollManager` viewports
//! (`14,46,336,101` and `14,189,336,101`) that the original fills at runtime. The
//! row shape comes from `ifgameoptionslot.txt`: label `14,8,70,16`, a second
//! static `93,8,31,16`, and one `CIFCheckBox` `130,6,16,16` on
//! `com_checkbutton_off.ddj`. So the rows below are a table, not a layout.
//!
//! That is the **classic** tree, and it is the one we build — but say so out
//! loud, because the corpus ships a second one. `Media/res_ui/optionwnd.2dt`
//! holds a 4th-generation Options window (20 `CNIFGameOptionSlot` instances
//! under four sub-tabs `UIIT_STT_DISPLAY` / `_COMMUNITY` / `_NAMEVIEW` /
//! `_SILKMALL_ETC`), and `APPLY_UI_4TH` is uncommented in `Media/config/define.txt`
//! (line 15; the file is CP949, so grep it accordingly). Which generation v1.188
//! actually renders is still UNKNOWN
//! (`docs/re/ui/newinterface-window-catalog.md` §9-U3).
//!
//! What tips it here is that the two files above carry **no `#ifdef` at all** —
//! unlike `ifoption.txt`, `ifoption_video.txt` and `ifvideooptionslot.txt`, which
//! do gate blocks on `APPLY_UI_4TH`. The Game tab's own resinfo is unconditional,
//! so this layout is what its data describes either way. If the generation
//! question later resolves toward the 2DT tree, this pane's row set is scoped
//! wrong by construction and the `Setting` ids with no home here (2001-2004,
//! 2008-2009, 2016-2018, 2025-2028) gain one.
//!
//! Each row carries its `SROptionSet` id (`docs/formats/sroptionset.md:106-133`)
//! and its textuisystem key, and writes straight into
//! `GameOptions.gameplay.toggles`, which is already id-keyed and already
//! persisted. Only the five name-indicator ids have a runtime consumer today
//! (`hud/nameplates.rs:45-49`), so only those are `Backing::Live`; every other
//! row renders but is inert and dimmed rather than silently doing nothing.

use bevy::ecs::relationship::RelatedSpawnerCommands;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use crate::plugins::settings::options::GameOptions;
use crate::plugins::textdata::ClientUiStrings;

/// Section header positions, pane-local (`ifoption_game.txt:31,50`).
const NAME_VIEW_HEADER: (f32, f32) = (13.0, 19.0);
const GAME_SET_HEADER: (f32, f32) = (13.0, 162.0);
/// The two scroll viewports (`ifoption_game.txt:69,88`).
const NAME_VIEW_VIEWPORT: (f32, f32, f32, f32) = (14.0, 46.0, 336.0, 101.0);
const GAME_SET_VIEWPORT: (f32, f32, f32, f32) = (14.0, 189.0, 336.0, 101.0);

/// Row metrics from the `ifgameoptionslot.txt` prototype.
const ROW_H: f32 = 24.0;
const ROW_LABEL: (f32, f32, f32, f32) = (14.0, 8.0, 70.0, 16.0);
const ROW_MARK: (f32, f32, f32, f32) = (93.0, 8.0, 31.0, 16.0);
const ROW_CHECKBOX: (f32, f32, f32, f32) = (130.0, 6.0, 16.0, 16.0);

const CHECKBOX_OFF: &str = "media://interface/ifcommon/com_checkbutton_off.ddj";
const CHECKBOX_ON: &str = "media://interface/ifcommon/com_checkbutton_on.ddj";
const CHECKBOX_DISABLED: &str = "media://interface/ifcommon/com_checkbutton_on_disable.ddj";

/// `FontColor="255,255,255,255"` on the slot prototype (`ifgameoptionslot.txt:11`).
const LABEL_COLOR: Color = Color::srgb_u8(255, 255, 255);
/// The two section headers are gold, not white: `FontColor="255,239,218,164"`
/// (AARRGGBB) on both `CIFStatic`s in `ifoption_game.txt`. They also carry
/// `DDJ="interface\option\opt_video_quality_tab.ddj"` with `ClientRect="28,12,0,0"`,
/// i.e. vanilla draws each header on a tab graphic — not transcribed here yet.
const HEADER_COLOR: Color = Color::srgb_u8(239, 218, 164);
/// openroad-only: rows whose toggle nothing reads yet are dimmed and inert, so
/// they read as present-but-unimplemented instead of as silent no-ops.
const LABEL_INERT: Color = Color::srgb_u8(128, 128, 128);

/// Whether flipping a row changes anything in this build.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Backing {
    /// A system reads this id today.
    Live,
    /// Stored and persisted, but nothing consumes it yet.
    Inert,
}

/// One checkbox row.
struct ToggleRow {
    /// `SROptionSet` id, or `None` for a vanilla row whose id we have not
    /// identified (see the UNKNOWNs in `docs/re/ui/options-gameplay.md`).
    id: Option<u16>,
    key: &'static str,
    english: &'static str,
    backing: Backing,
}

/// `Indicate Name Settings`. Ordered by ascending id, which matches the label
/// block's own order (`UIIT_STT_*_SIGN`, textuisystem :854-859). The tooltip
/// block `UIIT_STT_GAME_NAMEVIEW_TTDESC_01..06` lists a *different* order, so
/// the on-screen row order stays UNKNOWN; ids win because they are verifiable.
const NAME_VIEW_ROWS: [ToggleRow; 6] = [
    ToggleRow {
        id: Some(2010),
        key: "UIIT_STT_ONESELF_SIGN",
        english: "Own Name",
        backing: Backing::Live,
    },
    ToggleRow {
        id: Some(2011),
        key: "UIIT_STT_OTHER_CHAR_SIGN",
        english: "Other Name",
        backing: Backing::Live,
    },
    ToggleRow {
        id: Some(2012),
        key: "UIIT_STT_MONSTER_SIGN",
        english: "Monster Name",
        backing: Backing::Live,
    },
    ToggleRow {
        id: Some(2013),
        key: "UIIT_STT_NPC_SIGN",
        english: "NPC Name",
        backing: Backing::Live,
    },
    ToggleRow {
        id: Some(2014),
        key: "UIIT_STT_GUILDVIEW_SIGN",
        english: "Guild Name",
        backing: Backing::Live,
    },
    // The NAMEVIEW section has six rows but only five known ids — the beginner's
    // mark has a label and a tooltip (TTDESC_06) and no id we can place.
    ToggleRow {
        id: None,
        key: "UIIT_STT_FIRSTSTEP_MARK_SIGN",
        english: "Beginner\u{2019}s Mark",
        backing: Backing::Inert,
    },
];

/// `Game Settings`. The seven labels the vanilla block offers (textuisystem
/// :866-872); five of them have tooltips (`UIIT_STT_GAMESET_TTDESC_01..05`) and
/// two do not, which is why the tooltip count is not the row count.
const GAME_SET_ROWS: [ToggleRow; 7] = [
    ToggleRow {
        id: Some(2021),
        key: "UIIT_STT_QUICKSTATE_OWNER",
        english: "Self Condition",
        backing: Backing::Inert,
    },
    ToggleRow {
        id: Some(2022),
        key: "UIIT_STT_QUICKSTATE_COS",
        english: "COS Condition",
        backing: Backing::Inert,
    },
    ToggleRow {
        id: Some(2023),
        key: "UIIT_STT_QUICKSTATE_PARTY",
        english: "Party Member Status",
        backing: Backing::Inert,
    },
    ToggleRow {
        id: Some(2024),
        key: "UIIT_STT_QUICKSTATE_MONSTER",
        english: "Monster Condition",
        backing: Backing::Inert,
    },
    ToggleRow {
        id: Some(2019),
        key: "UIIT_STT_CAUTION_HP",
        english: "HP Warning",
        backing: Backing::Inert,
    },
    ToggleRow {
        id: Some(2020),
        key: "UIIT_STT_CAUTION_MP",
        english: "MP Warning",
        backing: Backing::Inert,
    },
    // The warning *tone* has a label and a tooltip but no id in the documented
    // 2001..=2028 range.
    ToggleRow {
        id: None,
        key: "UIIT_STT_CAUTION_SOUND",
        english: "Warning Sound",
        backing: Backing::Inert,
    },
];

/// A checkbox bound to an `SROptionSet` id.
#[derive(Component, Clone, Copy)]
pub(crate) struct GameToggle {
    id: u16,
    live: bool,
}

/// Vanilla defaults an id it has never seen to enabled, matching
/// `hud/nameplates.rs`'s `toggle_on`.
fn toggle_on(options: &GameOptions, id: u16) -> bool {
    options.gameplay.toggles.get(&id).copied().unwrap_or(true)
}

/// The three checkbox states, resolved once so the row spawner needs no
/// `AssetServer` inside the child-spawner closures.
struct CheckboxArt {
    off: Handle<Image>,
    on: Handle<Image>,
    disabled: Handle<Image>,
}

/// Build the Setting pane's two sections into an already-positioned pane node.
pub(crate) fn spawn_game_pane(
    pane: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    font: &Handle<Font>,
    ui_strings: &ClientUiStrings,
    options: &GameOptions,
) {
    let art = CheckboxArt {
        off: asset_server.load(CHECKBOX_OFF),
        on: asset_server.load(CHECKBOX_ON),
        disabled: asset_server.load(CHECKBOX_DISABLED),
    };

    for (header, pos, viewport, rows) in [
        (
            "UIIT_STT_NAMEVIEW",
            NAME_VIEW_HEADER,
            NAME_VIEW_VIEWPORT,
            &NAME_VIEW_ROWS[..],
        ),
        (
            "UIIT_STT_GAMESET",
            GAME_SET_HEADER,
            GAME_SET_VIEWPORT,
            &GAME_SET_ROWS[..],
        ),
    ] {
        pane.spawn((
            Text::new(
                ui_strings
                    .get_or(header, header_fallback(header))
                    .to_string(),
            ),
            TextFont {
                font: font.clone().into(),
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(HEADER_COLOR),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(pos.0),
                top: Val::Px(pos.1),
                ..default()
            },
            Pickable::IGNORE,
        ));

        // The vanilla control is a CIFScrollManager; the row stack is taller
        // than the viewport, so clip and let it scroll rather than overflow.
        pane.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(viewport.0),
                top: Val::Px(viewport.1),
                width: Val::Px(viewport.2),
                height: Val::Px(viewport.3),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            Pickable::default(),
        ))
        .with_children(|list| {
            for (index, row) in rows.iter().enumerate() {
                spawn_toggle_row(list, font, ui_strings, row, index, options, &art);
            }
        });
    }
}

fn header_fallback(key: &str) -> &'static str {
    if key == "UIIT_STT_NAMEVIEW" {
        "Indicate Name Settings"
    } else {
        "Game Settings"
    }
}

fn spawn_toggle_row(
    list: &mut RelatedSpawnerCommands<ChildOf>,
    font: &Handle<Font>,
    ui_strings: &ClientUiStrings,
    row: &ToggleRow,
    index: usize,
    options: &GameOptions,
    art: &CheckboxArt,
) {
    let live_id = match (row.backing, row.id) {
        (Backing::Live, Some(id)) => Some(id),
        _ => None,
    };
    let live = live_id.is_some();
    let color = if live { LABEL_COLOR } else { LABEL_INERT };
    let top = index as f32 * ROW_H;

    // The hit target is the whole row, not the 16x16 box: vanilla's checkbox is
    // under the WCAG 2.2 AA minimum, and widening only the click area changes no
    // vanilla geometry (docs/re/ui/options-gameplay.md:95). `options_video.rs`
    // rows work the same way. The row carries its own `GameToggle` so the
    // observer can read the id off the entity it fired on; the box keeps a copy
    // because `refresh_game_toggles` repaints by `(&GameToggle, &mut ImageNode)`,
    // which only ever matches the box.
    let mut entity = list.spawn(Node {
        position_type: PositionType::Absolute,
        left: Val::Px(0.0),
        top: Val::Px(top),
        width: Val::Percent(100.0),
        height: Val::Px(ROW_H),
        ..default()
    });
    if let Some(id) = live_id {
        entity.insert((
            GameToggle { id, live: true },
            Button,
            Hovered::default(),
            Pickable::default(),
        ));
        entity.observe(on_toggle_activate);
    } else {
        entity.insert(Pickable::IGNORE);
    }

    entity.with_children(|slot| {
        slot.spawn((
            Text::new(ui_strings.get_or(row.key, row.english).to_string()),
            TextFont {
                font: font.clone().into(),
                font_size: FontSize::Px(11.0),
                ..default()
            },
            TextColor(color),
            abs(ROW_LABEL),
            Pickable::IGNORE,
        ));

        // Rows nothing reads yet get a dash in the prototype's second static
        // slot, so "present but not wired" is visible rather than implied.
        if !live {
            slot.spawn((
                Text::new("-".to_string()),
                TextFont {
                    font: font.clone().into(),
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(LABEL_INERT),
                abs(ROW_MARK),
                Pickable::IGNORE,
            ));
        }

        let Some(id) = row.id else {
            // No id means nothing to store; draw the disabled box and stop.
            slot.spawn((
                ImageNode {
                    image: art.disabled.clone(),
                    ..default()
                },
                abs(ROW_CHECKBOX),
                Pickable::IGNORE,
            ));
            return;
        };

        let image = if !live {
            art.disabled.clone()
        } else if toggle_on(options, id) {
            art.on.clone()
        } else {
            art.off.clone()
        };
        // Always `IGNORE`: the click belongs to the row, and the box must not
        // swallow it.
        slot.spawn((
            GameToggle { id, live },
            ImageNode { image, ..default() },
            abs(ROW_CHECKBOX),
            Pickable::IGNORE,
        ));
    });
}

fn abs(rect: (f32, f32, f32, f32)) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(rect.0),
        top: Val::Px(rect.1),
        width: Val::Px(rect.2),
        height: Val::Px(rect.3),
        ..default()
    }
}

/// Flip the row's id in `GameOptions`; persistence picks the change up on its
/// own (`settings::persistence::save_on_change`).
fn on_toggle_activate(
    activate: On<Activate>,
    toggles: Query<&GameToggle>,
    mut options: ResMut<GameOptions>,
) {
    let Ok(toggle) = toggles.get(activate.entity) else {
        return;
    };
    if !toggle.live {
        return;
    }
    let next = !toggle_on(&options, toggle.id);
    options.gameplay.toggles.insert(toggle.id, next);
}

/// Repaint the boxes when the options change from anywhere (this pane, a load
/// of `user_settings.yaml`, or a future `SROptionSet.dat` import).
pub(crate) fn refresh_game_toggles(
    options: Res<GameOptions>,
    asset_server: Res<AssetServer>,
    mut boxes: Query<(&GameToggle, &mut ImageNode)>,
) {
    if !options.is_changed() {
        return;
    }
    for (toggle, mut image) in &mut boxes {
        if !toggle.live {
            continue;
        }
        let art = if toggle_on(&options, toggle.id) {
            CHECKBOX_ON
        } else {
            CHECKBOX_OFF
        };
        image.image = asset_server.load(art);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Every row that claims a backing must be one an actual system reads.
    /// `hud/nameplates.rs:45-49` reads exactly 2010..=2014 and nothing else, so
    /// this pins the honest set — marking another row `Live` has to be a
    /// deliberate edit here.
    #[test]
    fn only_the_nameplate_ids_are_live() {
        let live: Vec<Option<u16>> = NAME_VIEW_ROWS
            .iter()
            .chain(GAME_SET_ROWS.iter())
            .filter(|r| r.backing == Backing::Live)
            .map(|r| r.id)
            .collect();
        assert_eq!(
            live,
            vec![Some(2010), Some(2011), Some(2012), Some(2013), Some(2014)]
        );
    }

    /// The ids are the documented `Setting`-tab ones
    /// (`docs/formats/sroptionset.md:106-133`): all in 2001..=2028, never 2015
    /// (which is Video/Window Mode), and never repeated.
    #[test]
    fn ids_come_from_the_documented_setting_range() {
        let mut seen = Vec::new();
        for row in NAME_VIEW_ROWS.iter().chain(GAME_SET_ROWS.iter()) {
            if let Some(id) = row.id {
                assert!(
                    (2001..=2028).contains(&id),
                    "{id} outside the Setting range"
                );
                assert_ne!(id, 2015, "2015 is the Video tab's window-mode id");
                assert!(!seen.contains(&id), "duplicate id {id}");
                seen.push(id);
            }
        }
        assert_eq!(seen.len(), 11);
    }

    /// Vanilla's classic pane is two sections of six and seven rows — the row
    /// count is the label block's, not the tooltip block's (five tooltips
    /// against seven labels, per `docs/re/ui/options-gameplay.md`).
    #[test]
    fn section_row_counts_match_the_label_blocks() {
        assert_eq!(NAME_VIEW_ROWS.len(), 6);
        assert_eq!(GAME_SET_ROWS.len(), 7);
    }

    /// A row stack taller than its 101px viewport is why vanilla uses a
    /// scroll manager; keep that true so the clip/scroll stays justified.
    #[test]
    fn game_set_rows_overflow_the_vanilla_viewport() {
        assert!(GAME_SET_ROWS.len() as f32 * ROW_H > GAME_SET_VIEWPORT.3);
    }
}
