//! The in-game help book (`GDR_GAMEGUIDE:CIFGameGuide`, `ginterface.txt:1514`,
//! `ifgameguide.txt` + `ifggmenu/ifggmainslot/ifggsubslot`) — #575.
//!
//! Idea: the whole book is *data*. `gameguidedata.txt` (1,023 rows) is a
//! two-level tree and `texthelp.txt` (468 markup bodies) is its content; both
//! already load into [`ClientGameGuide`]. So this file is only the surface:
//! a navigation drawer that walks the tree and a page pane that shows the
//! selected leaf's body. The tree is walked from the leaves' parent column —
//! the categories' own child count disagrees with reality six times
//! (`docs/re/ui/game-guide-window.md` §3d), so it is never the source here.
//!
//! Geometry is the descriptor's, expressed **page-local**: the authored window
//! nests an `int_window_` frame `GDR_GUIDE_FRAME_1` at `13,40,395,401` inside
//! the `mframe_wnd_` shell, and every page rect below is `authored - (13, 40)`.
//! Our own framed window supplies the shell, so the nesting is not reproduced.
//!
//! Three stated deviations (all from the unit doc's §5 "Deviation, if any"):
//!
//! * **The drawer is docked, not hung off the window.** The original puts
//!   `GDR_GUIDE_MENU` at `-206,45,213,390`, i.e. its entire navigation lives
//!   *outside* the window's own rect. That only works in a renderer that does
//!   not clip children to the parent, and it is unreachable by keyboard. We
//!   keep the authored 213 px width and dock the drawer at the left edge, so
//!   the content area is `213 + 395` wide and every child is inside its parent.
//! * **The disabled 700 rows are loaded and hidden, not dropped.** Enabling
//!   them stays a data decision (the loader already keeps them).
//! * **The body is drawn as plain text.** `texthelp.txt` is `CIFPML` markup;
//!   the shared markup→Bevy-text renderer is its own piece of work, and until
//!   it exists `plain_text` (used by every other markup-carrying table) shows
//!   the help rather than the tags.
//!
//! Two authored rects are `Rect "…,0,0"` — "size from the art" (`gd_paper`,
//! `gd_guide`). Their pixel size lives in the `.ddj`, not in the descriptor,
//! so those two decorations are not drawn rather than drawn at an invented
//! size.

use bevy::ecs::relationship::RelatedSpawnerCommands;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};
use std::collections::HashSet;

use crate::assets::textdata::gameguide::{GuideKind, GuideNode};
use crate::assets::FontAssets;
use crate::plugins::hud::game_window::{abs_node, spawn_game_window};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::textdata::{ClientGameGuide, ClientUiStrings};
use crate::scenes::SceneState;

/// `GDR_GUIDE_MENU` is `213` wide; docked instead of hung (module note).
const DRAWER_W: f32 = 213.0;
/// `GDR_GUIDE_FRAME_1` `13,40,395,401` — the page half of the content area.
const PAGE_W: f32 = 395.0;
const PAGE_H: f32 = 401.0;
const CONTENT_W: f32 = DRAWER_W + PAGE_W;
const CONTENT_H: f32 = PAGE_H;

/// `GDR_GUIDE_TILE_1` / `_2`, page-local (`29,56` / `264,56` minus `13,40`).
const PAGE_TILE_L: (f32, f32, f32, f32) = (16.0, 16.0, 128.0, 369.0);
const PAGE_TILE_R: (f32, f32, f32, f32) = (251.0, 16.0, 128.0, 369.0);
/// `GDR_GUIDE_QUEST_NAME_STA` `53,90,200,12` — the page heading.
const PAGE_TITLE: (f32, f32, f32, f32) = (40.0, 50.0, 200.0, 12.0);
/// `GDR_GUIDE_DATA_PML` `53,96,290,303` — the body pane.
const PAGE_BODY: (f32, f32, f32, f32) = (40.0, 56.0, 290.0, 303.0);
/// `GDR_GUIDE_DATA_SCL_BG` `356,97,12,292` — the scroll line decoration.
const PAGE_SCROLL_LINE: (f32, f32, f32, f32) = (343.0, 57.0, 12.0, 292.0);

/// `ifggmenu.txt`: the `CIFScrollManager` viewport `10,34,192,346`, drawer-local.
const LIST_VIEWPORT: (f32, f32, f32, f32) = (10.0, 34.0, 192.0, 346.0);
/// `com_bg_tile_d` `22,46,168,322` behind it.
const LIST_FILL: (f32, f32, f32, f32) = (22.0, 46.0, 168.0, 322.0);

/// `ifggmainslot.txt`: drop-down button `4,4,24,24`, title `28,8,131,12`.
const CATEGORY_ARROW: (f32, f32, f32, f32) = (4.0, 4.0, 24.0, 24.0);
const CATEGORY_TITLE: (f32, f32, f32, f32) = (28.0, 8.0, 131.0, 12.0);
/// The prototype states no row height; the button is 24 px at a 4 px inset,
/// so the row is `4 + 24 + 4`.
const CATEGORY_ROW_H: f32 = 32.0;
/// `ifggsubslot.txt`: lamp `33,7,8,16`, title `43,9,107,12` — the 43 vs 28 is
/// the indent that makes a leaf read as a child.
const LEAF_LAMP: (f32, f32, f32, f32) = (33.0, 7.0, 8.0, 16.0);
const LEAF_TITLE: (f32, f32, f32, f32) = (43.0, 9.0, 107.0, 12.0);
/// Likewise unstated: the lamp ends at `7 + 16 = 23`, rounded up to a whole
/// pixel pitch.
const LEAF_ROW_H: f32 = 24.0;

const BG_TILE_B: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_b.ddj";
const BG_TILE_D: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_d.ddj";
const SCROLL_LINE: &str = "media://interface/guide/gd_scroll_line.ddj";
const INDEX_BUTTON: &str = "media://interface/guide/gd_index_button_close.ddj";

const TITLE_FONT: f32 = 10.0;
const ROW_FONT: f32 = 9.0;
const BODY_FONT: f32 = 9.0;

/// Which categories are unfolded and which page is shown.
#[derive(Resource, Default)]
pub struct GameGuideWindowState {
    pub open: bool,
    /// Category ids whose leaves are listed. The original gives every category
    /// its own drop-down button, so more than one may be open at a time.
    pub expanded: HashSet<u32>,
    /// The selected leaf id, `None` on the start page.
    pub page: Option<u32>,
}

#[derive(Component)]
pub struct GameGuideWindowRoot;

/// The rebuilt part of the window (everything but the shell).
#[derive(Component)]
pub struct GameGuideBoard;

/// A drawer row: the node id it was built from, so its action is keyed on the
/// data rather than on its position.
#[derive(Component, Clone, Copy)]
pub struct GuideCategoryRow(pub u32);

#[derive(Component, Clone, Copy)]
pub struct GuideLeafRow(pub u32);

/// One rendered drawer row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrawerRow {
    pub id: u32,
    pub category: bool,
}

/// The drawer's rows in file order: every enabled category, each followed by
/// its own enabled leaves when it is expanded.
///
/// This is the whole navigation model, kept as a pure function so the tree
/// walk is testable without an `App`. Leaves are matched by their parent
/// column (§3d) — the categories' stated child count is a checksum the loader
/// already exposes, never the source of the tree.
pub fn drawer_rows(nodes: &[GuideNode], expanded: &HashSet<u32>) -> Vec<DrawerRow> {
    let mut rows = Vec::new();
    for node in nodes.iter().filter(|n| n.enabled) {
        let GuideKind::Category { .. } = node.kind else {
            continue;
        };
        rows.push(DrawerRow {
            id: node.id,
            category: true,
        });
        if !expanded.contains(&node.id) {
            continue;
        }
        for leaf in nodes
            .iter()
            .filter(|n| n.enabled && n.kind == GuideKind::Leaf { parent: node.id })
        {
            rows.push(DrawerRow {
                id: leaf.id,
                category: false,
            });
        }
    }
    rows
}

/// The row's height by kind (the two prototypes are different sizes).
pub fn row_height(row: &DrawerRow) -> f32 {
    if row.category {
        CATEGORY_ROW_H
    } else {
        LEAF_ROW_H
    }
}

pub fn spawn_game_guide_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    cameras: Query<Entity, With<Camera2d>>,
    mut state: ResMut<GameGuideWindowState>,
) {
    let Ok(camera) = cameras.single() else {
        warn!("game guide: no 2d camera to attach to");
        return;
    };
    *state = GameGuideWindowState::default();

    let window = spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        // `Text "UIIT_STT_HELP"` on the registry entry.
        ui_strings.get_or("UIIT_STT_HELP", "Help"),
        (CONTENT_W, CONTENT_H),
        (160.0, 100.0),
        hud_scale(),
    );
    commands
        .entity(window.root)
        .insert((GameGuideWindowRoot, GlobalZIndex(20)));
    commands.entity(window.expect_close_button()).observe(
        |_: On<Activate>, mut state: ResMut<GameGuideWindowState>| {
            state.open = false;
        },
    );
    commands
        .entity(window.content)
        .insert(GameGuideBoard)
        .despawn_related::<Children>();
}

/// Rebuild the drawer and the page whenever the selection changes (and once
/// when the two tables arrive).
pub fn refresh_game_guide_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    guide: Res<ClientGameGuide>,
    state: Res<GameGuideWindowState>,
    boards: Query<Entity, With<GameGuideBoard>>,
) {
    if !state.is_changed() && !guide.is_changed() {
        return;
    }
    let s = hud_scale();
    let nodes = guide.nodes();
    let rows = drawer_rows(nodes, &state.expanded);
    let font = fonts.nine.clone();

    let text = |content: String, size: f32, rect: (f32, f32, f32, f32)| {
        (
            Text::new(content),
            TextFont {
                font: font.clone().into(),
                font_size: FontSize::Px(size * s),
                ..default()
            },
            TextColor(Color::WHITE),
            abs_node(rect, s),
            Pickable::IGNORE,
        )
    };

    for board in boards.iter() {
        commands.entity(board).despawn_related::<Children>();
        commands.entity(board).with_children(|content| {
            // --- the docked drawer -------------------------------------------
            content
                .spawn((
                    abs_node((0.0, 0.0, DRAWER_W, CONTENT_H), s),
                    Pickable::IGNORE,
                ))
                .with_children(|drawer| {
                    drawer.spawn((
                        abs_node(LIST_FILL, s),
                        tile(&asset_server, BG_TILE_D, s),
                        Pickable::IGNORE,
                    ));
                    // The vanilla control is a CIFScrollManager over 44
                    // categories and up to 979 leaves; the viewport holds a
                    // dozen rows, so clip and scroll rather than overflow.
                    drawer
                        .spawn((
                            Node {
                                overflow: Overflow::scroll_y(),
                                ..abs_node(LIST_VIEWPORT, s)
                            },
                            Pickable::default(),
                        ))
                        .with_children(|list| {
                            let mut y = 0.0;
                            for row in &rows {
                                let h = row_height(row);
                                spawn_row(
                                    list,
                                    &asset_server,
                                    &guide,
                                    nodes,
                                    row,
                                    y,
                                    h,
                                    s,
                                    &font,
                                    state.page == Some(row.id),
                                );
                                y += h;
                            }
                        });
                });

            // --- the page ----------------------------------------------------
            content
                .spawn((
                    abs_node((DRAWER_W, 0.0, PAGE_W, PAGE_H), s),
                    Pickable::IGNORE,
                ))
                .with_children(|page| {
                    for rect in [PAGE_TILE_L, PAGE_TILE_R] {
                        page.spawn((
                            abs_node(rect, s),
                            tile(&asset_server, BG_TILE_B, s),
                            Pickable::IGNORE,
                        ));
                    }
                    page.spawn((
                        abs_node(PAGE_SCROLL_LINE, s),
                        stretch(&asset_server, SCROLL_LINE),
                        Pickable::IGNORE,
                    ));
                    let selected = state.page.and_then(|id| nodes.iter().find(|n| n.id == id));
                    let (heading, body) = match selected {
                        Some(node) => {
                            (guide.menu_label(node), guide.body(node).unwrap_or_default())
                        }
                        // `UIIT_STT_GAMEGUIDE_START_1/_2` fill the two start-page
                        // text boxes (unit doc §7 U3).
                        None => (
                            ui_strings
                                .get_or("UIIT_STT_GAMEGUIDE", "Game Guide")
                                .to_string(),
                            format!(
                                "{}\n\n{}",
                                ui_strings.get_or("UIIT_STT_GAMEGUIDE_START_1", ""),
                                ui_strings.get_or("UIIT_STT_GAMEGUIDE_START_2", ""),
                            ),
                        ),
                    };
                    page.spawn(text(heading, TITLE_FONT, PAGE_TITLE));
                    page.spawn((
                        Node {
                            overflow: Overflow::scroll_y(),
                            ..abs_node(PAGE_BODY, s)
                        },
                        Pickable::default(),
                    ))
                    .with_children(|pane| {
                        pane.spawn((
                            Text::new(body),
                            TextFont {
                                font: font.clone().into(),
                                font_size: FontSize::Px(BODY_FONT * s),
                                ..default()
                            },
                            TextColor(Color::WHITE),
                            Node {
                                width: Val::Px(PAGE_BODY.2 * s),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ));
                    });
                });
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_row(
    list: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    guide: &ClientGameGuide,
    nodes: &[GuideNode],
    row: &DrawerRow,
    y: f32,
    h: f32,
    s: f32,
    font: &Handle<Font>,
    selected: bool,
) {
    let Some(node) = nodes.iter().find(|n| n.id == row.id) else {
        return;
    };
    let label = guide.menu_label(node);
    let rect = if row.category {
        CATEGORY_TITLE
    } else {
        LEAF_TITLE
    };
    let color = if selected {
        Color::srgb(1.0, 0.85, 0.45)
    } else {
        Color::WHITE
    };
    let mut row_entity = list.spawn((
        Button,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(y * s),
            width: Val::Px(LIST_VIEWPORT.2 * s),
            height: Val::Px(h * s),
            ..default()
        },
    ));
    if row.category {
        row_entity.insert(GuideCategoryRow(row.id));
    } else {
        row_entity.insert(GuideLeafRow(row.id));
    }
    row_entity.with_children(|r| {
        if row.category {
            r.spawn((
                abs_node(CATEGORY_ARROW, s),
                stretch(asset_server, INDEX_BUTTON),
                Pickable::IGNORE,
            ));
        } else {
            r.spawn((
                abs_node(LEAF_LAMP, s),
                stretch(asset_server, INDEX_BUTTON),
                Pickable::IGNORE,
            ));
        }
        r.spawn((
            Text::new(label),
            TextFont {
                font: font.clone().into(),
                font_size: FontSize::Px(ROW_FONT * s),
                ..default()
            },
            TextColor(color),
            abs_node(rect, s),
            Pickable::IGNORE,
        ));
    });
    row_entity.observe(on_row_activate);
}

/// A category folds, a leaf opens its page — the drop-down button and the lamp
/// of the two prototypes.
fn on_row_activate(
    activate: On<Activate>,
    categories: Query<&GuideCategoryRow>,
    leaves: Query<&GuideLeafRow>,
    mut state: ResMut<GameGuideWindowState>,
) {
    if let Ok(GuideCategoryRow(id)) = categories.get(activate.entity) {
        if !state.expanded.remove(id) {
            state.expanded.insert(*id);
        }
        return;
    }
    if let Ok(GuideLeafRow(id)) = leaves.get(activate.entity) {
        state.page = Some(*id);
    }
}

fn stretch(asset_server: &AssetServer, path: &str) -> ImageNode {
    ImageNode {
        image: asset_server.load(path.to_string()),
        image_mode: NodeImageMode::Stretch,
        ..default()
    }
}

fn tile(asset_server: &AssetServer, path: &str, s: f32) -> ImageNode {
    ImageNode {
        image: asset_server.load(path.to_string()),
        image_mode: NodeImageMode::Tiled {
            tile_x: true,
            tile_y: true,
            stretch_value: s,
        },
        ..default()
    }
}

pub fn apply_game_guide_window_visibility(
    state: Res<GameGuideWindowState>,
    mut roots: Query<&mut Node, With<GameGuideWindowRoot>>,
) {
    if !state.is_changed() {
        return;
    }
    let display = if state.open {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in roots.iter_mut() {
        if node.display != display {
            node.display = display;
        }
    }
}

pub fn cleanup_game_guide_window(
    mut commands: Commands,
    roots: Query<Entity, With<GameGuideWindowRoot>>,
    mut state: ResMut<GameGuideWindowState>,
) {
    for root in roots.iter() {
        commands.entity(root).despawn();
    }
    *state = GameGuideWindowState::default();
}

/// Self-registration (#558): the HUD registry only names this plugin.
pub struct GameGuidePlugin;

impl Plugin for GameGuidePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameGuideWindowState>()
            .add_systems(OnEnter(SceneState::GameWorld), spawn_game_guide_window)
            .add_systems(OnExit(SceneState::GameWorld), cleanup_game_guide_window)
            .add_systems(
                Update,
                (
                    apply_game_guide_window_visibility,
                    refresh_game_guide_window,
                )
                    .chain()
                    .run_if(in_state(SceneState::GameWorld)),
            );
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::assets::textdata::gameguide::GuideIndex;

    fn index() -> Vec<GuideNode> {
        // enable, id, title, depth, col4, body key, menu key
        let content = "\
1\t1000\tPlay guide\t0\t2\t\tSRO_GGW_MENU_PLAY\n\
1\t1001\tFirst steps\t1\t1000\tSRO_GGW_PG_1\tSRO_GGW_MENU_PG_1\n\
1\t1002\tSecond steps\t1\t1000\tSRO_GGW_PG_2\tSRO_GGW_MENU_PG_2\n\
0\t1003\tCut page\t1\t1000\tSRO_GGW_PG_3\tSRO_GGW_MENU_PG_3\n\
1\t2000\tInterface\t0\t1\t\tSRO_GGW_MENU_IF\n\
1\t2001\tThe HUD\t1\t2000\tSRO_GGW_IF_1\tSRO_GGW_MENU_IF_1\n\
0\t3000\tCut category\t0\t0\t\tSRO_GGW_MENU_CUT\n";
        GuideIndex::parse(content).nodes
    }

    /// Collapsed, the drawer is exactly the enabled categories in file order.
    #[test]
    fn collapsed_drawer_lists_only_the_enabled_categories() {
        let rows = drawer_rows(&index(), &HashSet::new());
        assert_eq!(
            rows,
            vec![
                DrawerRow {
                    id: 1000,
                    category: true
                },
                DrawerRow {
                    id: 2000,
                    category: true
                },
            ]
        );
    }

    /// Expanding inserts that category's leaves directly after it — and only
    /// its own, matched by the leaves' parent column.
    #[test]
    fn expanding_a_category_inserts_its_own_leaves_after_it() {
        let rows = drawer_rows(&index(), &HashSet::from([1000]));
        assert_eq!(
            rows,
            vec![
                DrawerRow {
                    id: 1000,
                    category: true
                },
                DrawerRow {
                    id: 1001,
                    category: false
                },
                DrawerRow {
                    id: 1002,
                    category: false
                },
                DrawerRow {
                    id: 2000,
                    category: true
                },
            ]
        );
    }

    /// The 700 disabled rows are loaded and hidden, not dropped at parse time
    /// (stated deviation ii): the loader still carries them.
    #[test]
    fn disabled_rows_are_loaded_but_never_drawn() {
        let nodes = index();
        assert!(nodes.iter().any(|n| n.id == 1003 && !n.enabled));
        assert!(nodes.iter().any(|n| n.id == 3000 && !n.enabled));
        let rows = drawer_rows(&nodes, &HashSet::from([1000, 3000]));
        assert!(!rows.iter().any(|r| r.id == 1003 || r.id == 3000));
    }

    /// The drawer is docked inside the window instead of hanging off it at
    /// `-206` (stated deviation i): every rect this window draws is inside its
    /// own content area, which is what keeps it safe under child clipping.
    #[test]
    fn every_rect_stays_inside_the_content_area() {
        let drawer = [LIST_VIEWPORT, LIST_FILL];
        for (x, y, w, h) in drawer {
            assert!(
                x >= 0.0 && y >= 0.0,
                "drawer rect starts outside the window"
            );
            assert!(x + w <= DRAWER_W && y + h <= CONTENT_H);
        }
        let page = [
            PAGE_TILE_L,
            PAGE_TILE_R,
            PAGE_TITLE,
            PAGE_BODY,
            PAGE_SCROLL_LINE,
        ];
        for (x, y, w, h) in page {
            assert!(x >= 0.0 && y >= 0.0);
            assert!(DRAWER_W + x + w <= CONTENT_W && y + h <= CONTENT_H);
        }
        // the authored drawer width is kept; only its origin moved
        assert_eq!(DRAWER_W, 213.0);
        assert_eq!(CONTENT_W, DRAWER_W + PAGE_W);
    }

    /// Rows are the two authored prototypes: a leaf is indented past its
    /// category's title and is the shorter of the two.
    #[test]
    fn row_prototypes_keep_the_authored_indent_and_pitch() {
        assert_eq!(CATEGORY_ROW_H, CATEGORY_ARROW.1 * 2.0 + CATEGORY_ARROW.3);
        assert!(LEAF_ROW_H < CATEGORY_ROW_H);
        assert!(LEAF_TITLE.0 > CATEGORY_TITLE.0);
        assert!(LEAF_LAMP.0 + LEAF_LAMP.2 <= LEAF_TITLE.0);
        // both prototypes fit the 192 px viewport they are scrolled in
        assert!(CATEGORY_TITLE.0 + CATEGORY_TITLE.2 <= LIST_VIEWPORT.2);
        assert!(LEAF_TITLE.0 + LEAF_TITLE.2 <= LIST_VIEWPORT.2);
        assert_eq!(
            row_height(&DrawerRow {
                id: 1000,
                category: true
            }),
            CATEGORY_ROW_H
        );
        assert_eq!(
            row_height(&DrawerRow {
                id: 1001,
                category: false
            }),
            LEAF_ROW_H
        );
    }
}
