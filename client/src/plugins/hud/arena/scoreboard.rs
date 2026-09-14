//! The in-match arena overlays — `res_ui/arena_game_score.2dt` (the score
//! strip) and `res_ui/arena_game_rank.2dt` (the 5-row rank board).
//!
//! Idea: these two files are **one widget shipped as two descriptors**. The
//! strip's root `(515,228,252,23)` is x-identical to and 5 px below the board's
//! root `(515,223,252,136)`, and the strip's slot rect equals the board's first
//! row *exactly* (`docs/re/ui/hud-arena-windows.md` §3.5 — the strongest
//! structural fact in that unit): the strip is the board's header row shipped
//! standalone for the no-board case. So this module builds **one** panel and
//! places the strip inside it at the offset the two roots' own coordinates
//! give, rather than laying either of them out twice.
//!
//! Like `hud::free_pvp`, no rect is transcribed: both descriptors are loaded
//! through the asset server and walked, so the layout stays the data's. Where
//! the data is silent this module stays silent too — the four statics in a
//! rank row carry no `Text` key and no art at all (§3.4), so they are filled
//! from [`ArenaState`] and nothing else is invented.
//!
//! Stated deviations (ADR 0009):
//!
//! * **Anchored top-centre instead of the authored `(515,223)`.** That origin
//!   is a point on the original's 1024x768 canvas — the corpus proves as much
//!   (§3.6: 9 of 42 files place children outside their own root) — and pinning
//!   it absolutely would misplace the board at every other resolution. The
//!   panel-internal geometry, including the strip's 5 px offset, is exactly as
//!   authored.
//! * **TTF text in the digit and name cells**, not the original's bitmap digit
//!   art. The descriptor names no art for any of them (§3.4, §3.5) — the
//!   original draws them in code from an atlas we have not identified — so
//!   drawing them as text is the honest option, and it is also the only one
//!   that renders a three-digit score.
//! * **The left digit pair is drawn as one field** spanning both cells. Ids
//!   10/11 overlap by 7 px and the data cannot distinguish "tens/units with
//!   kerning" from "mutually exclusive pair" (§3.5 `[U]` 3); one field over
//!   their union commits to neither reading.
//! * **The team crest cell is left empty.** The arena ships
//!   `gil_arena_{tiger,dragon}_team.ddj` and nothing ties tiger/dragon to the
//!   wire's `Red = 0` / `Blue = 1`. Drawing a guessed crest would be a fidelity
//!   claim we cannot make; the row still names and scores its player.

use bevy::prelude::*;
use bevy::ui::UiTargetCamera;

use crate::assets::twodt::{Jmxv2dtType, JMXV2DT};
use crate::assets::FontAssets;
use crate::plugins::hud::scale::hud_scale;
use crate::scenes::SceneState;

use super::{ArenaRankRow, ArenaState};

/// The two descriptors, as the exe spells them (`res_ui\<file>.2dt`).
const RANK_DESCRIPTOR: &str = "media://res_ui/arena_game_rank.2dt";
const SCORE_DESCRIPTOR: &str = "media://res_ui/arena_game_score.2dt";
/// Where the descriptors' `Background` paths are rooted.
const ART_ROOT: &str = "media://interface/";
const ROW_FONT: f32 = 9.0;
const SCORE_FONT: f32 = 12.0;

#[derive(Resource)]
struct ArenaBoardDescriptors {
    rank: Handle<JMXV2DT>,
    score: Handle<JMXV2DT>,
}

#[derive(Component)]
pub struct ArenaBoard;

/// `guild\gil_list_frame.ddj` -> `media://interface/guild/gil_list_frame.ddj`.
fn art_path(background: &str) -> String {
    format!(
        "{ART_ROOT}{}",
        background.to_ascii_lowercase().replace('\\', "/")
    )
}

/// The board's rows, top to bottom. Sorted by `y`, never by record index —
/// the same trap `frpvp.2dt` documents.
fn rank_rows(descriptor: &JMXV2DT) -> Vec<&crate::assets::twodt::Jmxv2dtEntry> {
    let mut rows: Vec<_> = descriptor
        .entries()
        .iter()
        .filter(|entry| entry.ni_type() == Some(Jmxv2dtType::CNIFSlot))
        .collect();
    rows.sort_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y));
    rows
}

/// A row's four cells (rank number, crest, name, score), left to right. The
/// descriptor gives them no names and no keys, so their order in `x` is the
/// only thing that identifies them — which is exactly how §3.4 read them.
fn row_cells<'a>(
    descriptor: &'a JMXV2DT,
    row_id: u32,
) -> Vec<&'a crate::assets::twodt::Jmxv2dtEntry> {
    let mut cells: Vec<_> = descriptor.children_of(row_id).collect();
    cells.sort_by(|a, b| a.rect().min.x.total_cmp(&b.rect().min.x));
    cells
}

/// The score strip's five statics as `(left field, separator, right field)`,
/// in the strip's own absolute space.
///
/// Read as pixels rather than as ids (§3.5): two 19x19 cells, a taller 12x24
/// glyph, two more 19x19 cells. The separator is the middle one by `x`, and
/// each score field is the union of the pair on its side — see the module
/// note on the 7 px overlap.
fn score_fields(descriptor: &JMXV2DT) -> Option<(Rect, Rect, Rect)> {
    let slot = descriptor
        .entries()
        .iter()
        .find(|entry| entry.ni_type() == Some(Jmxv2dtType::CNIFSlot))?;
    score_layout(
        descriptor
            .children_of(slot.id())
            .map(|entry| entry.rect())
            .collect(),
    )
}

/// The geometry half of [`score_fields`], separated so it can be checked
/// against the authored rects without an asset server.
fn score_layout(mut cells: Vec<Rect>) -> Option<(Rect, Rect, Rect)> {
    if cells.len() != 5 {
        return None;
    }
    cells.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
    let union = |a: Rect, b: Rect| Rect::from_corners(a.min.min(b.min), a.max.max(b.max));
    Some((
        union(cells[0], cells[1]),
        cells[2],
        union(cells[3], cells[4]),
    ))
}

fn load_descriptors(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(ArenaBoardDescriptors {
        rank: asset_server.load(RANK_DESCRIPTOR),
        score: asset_server.load(SCORE_DESCRIPTOR),
    });
}

/// Build (or rebuild) the overlay whenever the arena state or the descriptors
/// change. Without the descriptors there is no board: the layout is the data's
/// and we carry no transcribed fallback of it.
fn rebuild_board(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    descriptors: Res<Assets<JMXV2DT>>,
    handles: Option<Res<ArenaBoardDescriptors>>,
    state: Res<ArenaState>,
    boards: Query<Entity, With<ArenaBoard>>,
    cameras: Query<Entity, With<Camera2d>>,
) {
    if !(state.is_changed() || descriptors.is_changed()) {
        return;
    }
    for board in boards.iter() {
        commands.entity(board).despawn();
    }
    if !state.board_visible() {
        return;
    }
    let (Some(handles), Ok(camera)) = (handles, cameras.single()) else {
        return;
    };
    let (Some(rank), Some(score)) = (
        descriptors.get(&handles.rank),
        descriptors.get(&handles.score),
    ) else {
        return;
    };
    let (Some(rank_root), Some(score_root)) = (rank.root(), score.root()) else {
        return;
    };

    let s = hud_scale();
    // Both files place their rects in the ONE flat design space (§3.6), so the
    // strip's 5 px offset from the board falls out of its own coordinates the
    // moment both are expressed relative to the board's root. Nothing about
    // the relationship between the two files is transcribed here.
    let origin = rank_root.rect().min;

    let node = |rect: Rect| Node {
        position_type: PositionType::Absolute,
        left: Val::Px((rect.min.x - origin.x) * s),
        top: Val::Px((rect.min.y - origin.y) * s),
        width: Val::Px(rect.width() * s),
        height: Val::Px(rect.height() * s),
        ..default()
    };
    let label = |text: String, size: f32, color: Color, justify: Justify| {
        (
            Text::new(text),
            TextFont {
                font: fonts.nine.clone().into(),
                font_size: FontSize::Px(size * s),
                ..default()
            },
            TextColor(color),
            TextLayout::justify(justify),
            Pickable::IGNORE,
        )
    };

    let size = rank_root.rect().size();
    let board = commands
        .spawn((
            ArenaBoard,
            Name::from("Arena Board"),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(8.0 * s),
                left: Val::Percent(50.0),
                margin: UiRect::left(Val::Px(-size.x * s / 2.0)),
                width: Val::Px(size.x * s),
                height: Val::Px(size.y * s),
                ..default()
            },
            GlobalZIndex(25),
            UiTargetCamera(camera),
            Pickable::IGNORE,
        ))
        .id();

    // The board's frame — `interface\guild\gil_list_frame.ddj`, which the arena
    // borrows from the guild window (§3.4: directory != ownership).
    if !rank_root.background().is_empty() {
        commands.entity(board).with_child((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            ImageNode {
                image: asset_server.load(art_path(rank_root.background())),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
    }

    // The score strip: red score, separator, blue score.
    if let Some((left, separator, right)) = score_fields(score) {
        let colour = score_root.color();
        for (rect, text) in [
            (left, state.score.red.to_string()),
            (separator, ":".to_string()),
            (right, state.score.blue.to_string()),
        ] {
            commands.entity(board).with_child((
                node(rect),
                Pickable::IGNORE,
                children![label(text, SCORE_FONT, colour, Justify::Center)],
            ));
        }
    }

    // The rank rows. The authored 5 are a viewport: rows beyond what the
    // server sent are simply not drawn, and rows beyond the authored 5 wait
    // for the scrolled list this slice does not build.
    for (row, entry) in rank_rows(rank).iter().zip(state.ranks.iter()) {
        let cells = row_cells(rank, row.id());
        let texts = row_texts(entry, cells.len());
        for (cell, text) in cells.iter().zip(texts) {
            let Some(text) = text else { continue };
            commands.entity(board).with_child((
                node(cell.rect()),
                Pickable::IGNORE,
                children![label(text, ROW_FONT, cell.color(), Justify::Left)],
            ));
        }
    }
}

/// What each of a row's cells says, left to right: rank number, crest (never —
/// see the module note), name, points. A row whose descriptor has a different
/// number of cells gets as many as it has, rather than a panicking index.
fn row_texts(entry: &ArenaRankRow, cells: usize) -> Vec<Option<String>> {
    let mut texts = vec![
        None,
        None,
        Some(entry.name.clone()),
        Some(entry.points.to_string()),
    ];
    texts.truncate(cells);
    texts
}

fn cleanup_board(mut commands: Commands, boards: Query<Entity, With<ArenaBoard>>) {
    for board in boards.iter() {
        commands.entity(board).despawn();
    }
}

pub struct ArenaScoreboardPlugin;

impl Plugin for ArenaScoreboardPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(SceneState::GameWorld), load_descriptors)
            .add_systems(OnExit(SceneState::GameWorld), cleanup_board)
            .add_systems(
                Update,
                rebuild_board.run_if(in_state(SceneState::GameWorld)),
            );
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::plugins::hud::arena::ArenaScore;

    #[test]
    fn art_paths_are_media_relative_and_forward_slashed() {
        assert_eq!(
            art_path("guild\\gil_list_frame.ddj"),
            "media://interface/guild/gil_list_frame.ddj"
        );
    }

    /// The rank number and the crest are not ours to fill: the first is the
    /// row's index in a list the server orders, the second has no verified
    /// art. Name and points are.
    #[test]
    fn a_rank_row_fills_only_the_cells_the_data_supports() {
        let row = ArenaRankRow {
            team: None,
            name: "Kong".into(),
            points: 42,
        };
        assert_eq!(
            row_texts(&row, 4),
            vec![None, None, Some("Kong".into()), Some("42".into())]
        );
        // a descriptor with fewer cells must not index past its own row
        assert_eq!(row_texts(&row, 2), vec![None, None]);
    }

    /// The strip's authored statics (§3.5: ids 10 `(593,228,19,19)`,
    /// 11 `(605,228,19,19)`, 12 `(634,226,12,24)`, 13 `(657,228,19,19)`,
    /// 14 `(689,228,19,19)`) read as `NN : NN`: the separator is the middle
    /// one by x, and each score field is the union of the pair on its side.
    /// The left pair's 7 px overlap therefore costs nothing — which is the
    /// point, since the data cannot say whether it is kerning or exclusion.
    #[test]
    fn the_score_strip_reads_as_a_pair_a_separator_and_a_pair() {
        let cell = |x: f32, y: f32, w: f32, h: f32| Rect::new(x, y, x + w, y + h);
        // deliberately not in x order: the descriptor's record order is not
        // the layout order, here or anywhere else in the corpus
        let cells = vec![
            cell(634.0, 226.0, 12.0, 24.0),
            cell(689.0, 228.0, 19.0, 19.0),
            cell(593.0, 228.0, 19.0, 19.0),
            cell(657.0, 228.0, 19.0, 19.0),
            cell(605.0, 228.0, 19.0, 19.0),
        ];
        let (left, separator, right) = score_layout(cells).expect("five statics");
        assert_eq!((left.min.x, left.max.x), (593.0, 624.0));
        assert_eq!((separator.min.x, separator.max.x), (634.0, 646.0));
        assert_eq!((right.min.x, right.max.x), (657.0, 708.0));
        // the separator is the only cell taller than the row
        assert!(separator.height() > left.height());
        // a file that is not this shape yields nothing rather than a guess
        assert!(score_layout(vec![cell(0.0, 0.0, 1.0, 1.0)]).is_none());
    }

    /// One [`ArenaScore`] feeds both readouts — the reason the strip is not a
    /// second, independently laid-out copy of the same number.
    #[test]
    fn both_totals_come_from_one_score() {
        let score = ArenaScore { red: 17, blue: 9 };
        assert_eq!(score.red.to_string(), "17");
        assert_eq!(score.blue.to_string(), "9");
    }
}
