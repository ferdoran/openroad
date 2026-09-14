//! World map window layout + rendering.
//!
//! Idea: vanilla's GDR_WORLDMAP is a 652x424 mframe window (ginterface.txt);
//! the content is a clipped, drag-pannable viewport over a map canvas drawn
//! at the map's native pixel size (worldmap.rs projection). The world map
//! canvas holds one 128px `map_world_<x>x<z>.ddj` image per 4x4-region block
//! (grid gaps simply fail to load and are hidden); a city map is a single
//! image scaled so its used sub-rect spans the logical size. On top of the
//! canvas: the localinfo POIs (SN_ZONE text labels, `xy_*.ddj` icons, and
//! clickable `city_*.ddj` buttons that switch to the city map), the
//! [`MapMarkers`] layer, and the minimap-sign player arrow (same heading
//! math). The window rebuilds on any state change (open/close/map switch);
//! the arrow, auto-follow centering and the follow-label repaint run per
//! frame. Corner controls: back-to-world (city maps) and the follow toggle.

use bevy::asset::LoadState;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::UiTransform;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::textdata::worldmap::{
    dungeon_du, DungeonMapDef, MapDef, PoiKind, DU_PER_REGION,
};
use crate::assets::FontAssets;
use crate::plugins::hud::game_window::{self, abs_node};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::hud::window_positions::PersistedWindow;
use crate::plugins::hud::world_map::model::{
    player_global_xz, MapMarkers, MarkerKind, WorldMapFollow, WorldMapState,
};
use crate::plugins::player::Player;
use crate::plugins::settings::window_positions::WndPosSlot;
use crate::plugins::textdata::{ClientTextNames, ClientUiStrings, ClientWorldMap, ClientZoneNames};
use crate::plugins::ui_v2::style::ImageButtonStyle;
use crate::plugins::world_origin::WorldOrigin;

/// Vanilla window 652x424 minus the mframe chrome.
const CONTENT_W: f32 = 628.0;
const CONTENT_H: f32 = 372.0;
/// Spawn anchor derived from vanilla's own rect instead of hand-placed (#318):
/// `GDR_WORLDMAP` sits at `Rect="100,100,652,424"` in the classic tree's
/// 1024x768 design space (`resinfo/ginterface.txt:240`), and the chrome anchors
/// windows by their right/top corner, so right = `1024 - 100 - 652` = 272.
/// Caveat, unchanged from before: like every other HUD window these are passed
/// as physical px, i.e. the design canvas is not rescaled to the real window.
const WINDOW_RIGHT: f32 = 272.0;
const WINDOW_TOP: f32 = 100.0;

/// `ifworldmap.txt` control rects, in vanilla window space.
/// `GDR_WM_BTN_TO_WMAP` id 6 — 16x16 at `590,10`, i.e. inside the chrome's
/// title strip (band y 6..28) beside the caption, not over the map.
const TO_WMAP_BTN_RECT: (f32, f32, f32, f32) = (590.0, 10.0, 16.0, 16.0);
/// `GDR_WM_BTN_AUTO_MOVE` id 20 — 96x28 at `543,43`; content-local after
/// subtracting the chrome's content origin (12, 36).
const AUTO_MOVE_BTN_RECT: (f32, f32, f32, f32) = (531.0, 7.0, 96.0, 28.0);

/// Same sign as the minimap's player arrow (map_arrow.ddj is a tiny plain
/// triangle; the minimap sign reads much better at this size).
const ARROW_DDJ: &str = "media://interface/minimap/mm_sign_character.ddj";
const ARROW_SIZE: f32 = 16.0;

const WORLD_BTN_DDJ: &str = "media://interface/worldmap/wmap_button_world";
const FOLLOW_BTN_DDJ: &str = "media://interface/ifcommon/com_mid_button";

const LABEL_COLOR: Color = Color::srgb_u8(235, 225, 190);

#[derive(Component)]
pub struct WmWindowRoot;

/// The pannable map canvas (child of the clipped viewport).
#[derive(Component)]
pub struct WmCanvas {
    /// Canvas size in UI px (map px * scale).
    pub size: Vec2,
}

/// The clipped viewport the canvas pans inside.
#[derive(Component)]
pub struct WmViewport;

/// A party/academy marker on the canvas — hoverable, so its name can be read.
#[derive(Component)]
pub struct WmMarkerSign;

/// The marker's name label, hidden until its marker is hovered.
#[derive(Component)]
pub struct WmMarkerLabel;

/// Canvas offset at drag start.
#[derive(Component)]
struct WmPanStart(Vec2);

#[derive(Component)]
pub struct WmTile;

#[derive(Component)]
pub struct WmArrow;

#[derive(Component)]
struct WmCityButton {
    link_map: u32,
}

/// The "back to the world map" corner button (city maps only).
#[derive(Component)]
pub struct WmWorldButton;

/// One dungeon floor-selector button.
#[derive(Component)]
struct WmFloorButton {
    region: u16,
    map_id: u32,
}

/// `1F` / `B3` label of a dungeon floor.
fn floor_label(def: &DungeonMapDef) -> String {
    if def.basement {
        format!("B{}", def.floor_number)
    } else {
        format!("{}F", def.floor_number)
    }
}

/// The auto-follow toggle's label (text switches with [`WorldMapFollow`]).
#[derive(Component)]
pub struct WmFollowLabel;

/// Rebuild the window on any state change.
#[allow(clippy::too_many_arguments)]
pub fn sync_world_map_window(
    state: Res<WorldMapState>,
    existing: Query<Entity, With<WmWindowRoot>>,
    worldmap: Res<ClientWorldMap>,
    names: Res<ClientTextNames>,
    ui_strings: Res<ClientUiStrings>,
    zone_names: Res<ClientZoneNames>,
    markers: Res<MapMarkers>,
    follow: Res<WorldMapFollow>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    if !state.is_changed() && !markers.is_changed() {
        return;
    }
    for entity in existing.iter() {
        commands.entity(entity).despawn();
    }
    if !state.open {
        return;
    }
    let Some(table) = worldmap.table() else {
        warn!("world map: worldmap_mapinfo not loaded");
        return;
    };
    // Dungeon mode: a Dungeonmap floor def replaces the WLocalmap pick.
    let dungeon = state.dungeon_map.and_then(|(region, map_id)| {
        table
            .dungeon_floors(region)
            .iter()
            .find(|def| def.map_id == map_id)
            .map(|def| (region, def))
    });
    let map = match dungeon {
        Some(_) => None,
        None => match table.map(state.current_map) {
            Some(map) => Some(map),
            None => {
                warn!("world map: unknown map id {}", state.current_map);
                return;
            }
        },
    };
    let Ok(camera) = cam_query.single() else {
        return;
    };
    let s = hud_scale();

    let title = match (dungeon, map) {
        (Some((region, def)), _) => format!(
            "{} {}",
            zone_names.name(region).unwrap_or("Dungeon"),
            floor_label(def)
        ),
        (None, Some(map)) => {
            let title_key = &map.name_key;
            if title_key.starts_with("SN_") {
                names.name(title_key).unwrap_or("World map").to_string()
            } else {
                ui_strings.get_or(title_key, "World map").to_string()
            }
        }
        (None, None) => unreachable!("either a dungeon def or a map def is set"),
    };

    let window = game_window::spawn_game_window(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        &title,
        (CONTENT_W, CONTENT_H),
        (WINDOW_RIGHT, WINDOW_TOP),
        s,
    );
    commands.entity(window.root).insert((
        WmWindowRoot,
        GlobalZIndex(56),
        PersistedWindow(WndPosSlot::WorldMap),
    ));
    commands
        .entity(window.expect_close_button())
        .observe(on_close_button);

    let text_font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    };
    let canvas_size = match (dungeon, map) {
        (Some((_, def)), _) => Vec2::new(def.logical_w, def.logical_h) * s,
        (None, Some(map)) => Vec2::new(map.logical_w, map.logical_h) * s,
        (None, None) => unreachable!(),
    };

    commands.entity(window.content).with_children(|content| {
        let mut viewport_node = abs_node((0.0, 0.0, CONTENT_W, CONTENT_H), s);
        viewport_node.overflow = Overflow::clip();
        content
            .spawn((WmViewport, viewport_node, Hovered::default()))
            .observe(on_pan_start)
            .observe(on_pan)
            .with_children(|viewport| {
                viewport
                    .spawn((
                        WmCanvas { size: canvas_size },
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(0.0),
                            top: Val::Px(0.0),
                            width: Val::Px(canvas_size.x),
                            height: Val::Px(canvas_size.y),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .with_children(|canvas| {
                        match (dungeon, map) {
                            (Some((_, def)), _) => {
                                spawn_dungeon_map_images(canvas, def, s, &asset_server);
                            }
                            (None, Some(map)) => {
                                let k = map.px_per_du();
                                spawn_map_images(canvas, map, k, s, &asset_server);
                                spawn_pois(
                                    canvas,
                                    table,
                                    map,
                                    s,
                                    &names,
                                    &ui_strings,
                                    &asset_server,
                                    &text_font,
                                );

                                // marker layer (party/academy plug in later)
                                for marker in &markers.0 {
                                    let px = map.project(marker.gx / 10.0, marker.gz / 10.0) * s;
                                    let sign = match marker.kind {
                                        MarkerKind::Party => "wmap_sign_party",
                                        MarkerKind::UnionParty => "wmap_sign_unionparty",
                                        MarkerKind::Academy => "wmap_sign_apprenticeship",
                                        MarkerKind::Generic => "wmap_sign_location",
                                    };
                                    // Hoverable, unlike the decorations around
                                    // it: the marker carries a name and the
                                    // only way to read it is to point at one.
                                    canvas
                                        .spawn((
                                            WmMarkerSign,
                                            Button,
                                            Hovered::default(),
                                            Node {
                                                position_type: PositionType::Absolute,
                                                left: Val::Px(px.x - 8.0 * s),
                                                top: Val::Px(px.y - 8.0 * s),
                                                width: Val::Px(16.0 * s),
                                                height: Val::Px(16.0 * s),
                                                ..default()
                                            },
                                            ImageNode {
                                                image: asset_server.load(format!(
                                                    "media://interface/worldmap/{sign}.ddj"
                                                )),
                                                image_mode: NodeImageMode::Stretch,
                                                ..default()
                                            },
                                        ))
                                        .with_children(|sign| {
                                            if marker.name.is_empty() {
                                                return;
                                            }
                                            // Spawned hidden and flipped by
                                            // `update_marker_labels`, so
                                            // hide-on-exit needs no observer
                                            // and the label survives the
                                            // window's periodic rebuild.
                                            sign.spawn((
                                                WmMarkerLabel,
                                                Text::new(marker.name.clone()),
                                                text_font(7.0),
                                                TextColor(LABEL_COLOR),
                                                Node {
                                                    position_type: PositionType::Absolute,
                                                    left: Val::Px(0.0),
                                                    top: Val::Px(-9.0 * s),
                                                    ..default()
                                                },
                                                Visibility::Hidden,
                                                Pickable::IGNORE,
                                            ));
                                        });
                                }
                            }
                            (None, None) => unreachable!(),
                        }

                        // player arrow on top
                        canvas.spawn((
                            WmArrow,
                            Node {
                                position_type: PositionType::Absolute,
                                width: Val::Px(ARROW_SIZE * s),
                                height: Val::Px(ARROW_SIZE * s),
                                ..default()
                            },
                            ImageNode {
                                image: asset_server.load(ARROW_DDJ),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                            UiTransform::default(),
                            Visibility::Hidden,
                            Pickable::IGNORE,
                        ));
                    });
            });

        // dungeon floor selector: one button per floor of the current
        // dungeon region (donwhang: 4 floors share the region; jinsi floors
        // are separate regions, so a single button shows)
        if let Some((region, current_def)) = dungeon {
            let floors = table.dungeon_floors(region);
            if floors.len() > 1 {
                let button_style = ImageButtonStyle {
                    normal: asset_server.load(format!("{FOLLOW_BTN_DDJ}.ddj")),
                    hover: asset_server.load(format!("{FOLLOW_BTN_DDJ}_focus.ddj")),
                    press: asset_server.load(format!("{FOLLOW_BTN_DDJ}_press.ddj")),
                    ..Default::default()
                };
                for (index, def) in floors.iter().enumerate() {
                    let selected = def.map_id == current_def.map_id;
                    content
                        .spawn((
                            WmFloorButton {
                                region,
                                map_id: def.map_id,
                            },
                            Button,
                            Hovered::default(),
                            abs_node((4.0 + index as f32 * 34.0, CONTENT_H - 22.0, 30.0, 20.0), s),
                            ImageNode {
                                image: if selected {
                                    button_style.press.clone()
                                } else {
                                    button_style.normal.clone()
                                },
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                            button_style.clone(),
                        ))
                        .observe(on_floor_button)
                        .with_children(|button| {
                            button.spawn((
                                Text::new(floor_label(def)),
                                text_font(7.0),
                                TextColor(if selected {
                                    Color::WHITE
                                } else {
                                    Color::srgb(0.8, 0.8, 0.8)
                                }),
                                TextLayout::justify(Justify::Center),
                                Node {
                                    position_type: PositionType::Absolute,
                                    top: Val::Px(5.0 * s),
                                    width: Val::Percent(100.0),
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ));
                        });
                }
            }
        }

        // the auto-move toggle (`GDR_WM_BTN_AUTO_MOVE`); the back-to-world
        // button lives in the title strip and is spawned on the root below
        let follow_style = ImageButtonStyle {
            normal: asset_server.load(format!("{FOLLOW_BTN_DDJ}.ddj")),
            hover: asset_server.load(format!("{FOLLOW_BTN_DDJ}_focus.ddj")),
            press: asset_server.load(format!("{FOLLOW_BTN_DDJ}_press.ddj")),
            ..Default::default()
        };
        content
            .spawn((
                Button,
                Hovered::default(),
                abs_node(AUTO_MOVE_BTN_RECT, s),
                ImageNode {
                    image: follow_style.normal.clone(),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                follow_style,
            ))
            .observe(on_follow_button)
            .with_children(|button| {
                button.spawn((
                    WmFollowLabel,
                    Text::new(follow_label(&ui_strings, follow.0)),
                    text_font(7.0),
                    TextColor(Color::srgb(0.92, 0.92, 0.92)),
                    TextLayout::justify(Justify::Center),
                    Node {
                        position_type: PositionType::Absolute,
                        top: Val::Px(5.0 * s),
                        width: Val::Percent(100.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            });
    });

    // `GDR_WM_BTN_TO_WMAP` (back to the world map from a city map): vanilla
    // puts it at `590,10`, inside the title strip beside the close button, so
    // it is a child of the window root rather than of the content area.
    if dungeon.is_none() && state.current_map != 0 {
        let world_style = ImageButtonStyle {
            normal: asset_server.load(format!("{WORLD_BTN_DDJ}.ddj")),
            hover: asset_server.load(format!("{WORLD_BTN_DDJ}_focus.ddj")),
            press: asset_server.load(format!("{WORLD_BTN_DDJ}_press.ddj")),
            ..Default::default()
        };
        commands.entity(window.root).with_children(|root| {
            root.spawn((
                WmWorldButton,
                Button,
                Hovered::default(),
                abs_node(TO_WMAP_BTN_RECT, s),
                ImageNode {
                    image: world_style.normal.clone(),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                world_style,
            ))
            .observe(on_world_button);
        });
    }
}

/// The follow toggle's label: vanilla's move-state strings.
fn follow_label(ui_strings: &ClientUiStrings, follow: bool) -> String {
    if follow {
        ui_strings.get_or("UIIT_STT_WORLDMAP_AUTO_MOVE", "Auto Move State")
    } else {
        ui_strings.get_or("UIIT_STT_WORLDMAP_MANUAL_MOVE", "Manual Move State")
    }
    .to_string()
}

/// The map art: 4x4-region tiles for the world map, one scaled image for a
/// city map.
fn spawn_map_images(
    canvas: &mut ChildSpawnerCommands,
    map: &MapDef,
    k: f32,
    s: f32,
    asset_server: &AssetServer,
) {
    if map.tiled {
        // tile files are named after their top-left region (min x, max z)
        // and cover 4x4 regions; grid positions without a file fail to load
        // and get hidden by `hide_failed_tiles`.
        let tile_px = 4.0 * DU_PER_REGION * k * s;
        let mut tz = map.z2 + 3;
        while tz <= map.z1 {
            let mut tx = map.x1;
            while tx + 3 <= map.x2 {
                let origin =
                    map.project(tx as f32 * DU_PER_REGION, (tz + 1) as f32 * DU_PER_REGION) * s;
                canvas.spawn((
                    WmTile,
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(origin.x),
                        top: Val::Px(origin.y),
                        width: Val::Px(tile_px),
                        height: Val::Px(tile_px),
                        ..default()
                    },
                    ImageNode {
                        image: asset_server
                            .load(format!("media://{}{}x{}.ddj", map.texture, tx, tz)),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                tx += 4;
            }
            tz += 4;
        }
    } else {
        // the art occupies the used_w x used_h sub-rect of the texture; draw
        // the full texture scaled so that sub-rect spans the logical size
        // (the overhang holds padding and is clipped by the viewport)
        let full_w = map.tex_w * map.logical_w / map.used_w.max(1.0);
        let full_h = map.tex_h * map.logical_h / map.used_h.max(1.0);
        canvas.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Px(full_w * s),
                height: Val::Px(full_h * s),
                ..default()
            },
            ImageNode {
                image: asset_server.load(format!("media://{}", map.texture)),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
    }
}

/// A dungeon floor's map art: one 256px `{prefix}{x}x{z}.ddj` tile per
/// region cell over the floor's inclusive rect; missing tiles fail to load
/// and get hidden like the world map's grid gaps.
fn spawn_dungeon_map_images(
    canvas: &mut ChildSpawnerCommands,
    def: &DungeonMapDef,
    s: f32,
    asset_server: &AssetServer,
) {
    let k = def.px_per_du();
    let tile_px = DU_PER_REGION * k * s;
    for tz in def.z2..=def.z1 {
        for tx in def.x1..=def.x2 {
            let origin =
                def.project(tx as f32 * DU_PER_REGION, (tz + 1) as f32 * DU_PER_REGION) * s;
            canvas.spawn((
                WmTile,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(origin.x),
                    top: Val::Px(origin.y),
                    width: Val::Px(tile_px),
                    height: Val::Px(tile_px),
                    ..default()
                },
                ImageNode {
                    image: asset_server
                        .load(format!("media://{}{}x{}.ddj", def.tile_prefix, tx, tz)),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
            ));
        }
    }
}

/// The localinfo POIs of the current map.
#[allow(clippy::too_many_arguments)]
fn spawn_pois(
    canvas: &mut ChildSpawnerCommands,
    table: &crate::assets::textdata::worldmap::WorldMapTable,
    map: &MapDef,
    s: f32,
    names: &ClientTextNames,
    ui_strings: &ClientUiStrings,
    asset_server: &AssetServer,
    text_font: &dyn Fn(f32) -> TextFont,
) {
    for poi in table.pois_for(map.id) {
        let px = poi.map_px(map) * s;
        match &poi.kind {
            PoiKind::Label(key) => {
                let Some(name) = names.name(key) else {
                    continue;
                };
                // anchored at the data point (vanilla draws from it, not
                // centered)
                canvas.spawn((
                    Text::new(name.to_string()),
                    text_font(7.0),
                    TextColor(LABEL_COLOR),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(px.x),
                        top: Val::Px(px.y),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            }
            PoiKind::Icon(path) => {
                let (w, h) = if poi.w > 0.0 {
                    (poi.w, poi.h)
                } else {
                    (16.0, 16.0)
                };
                // top-left anchored at the data point
                let node = Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(px.x),
                    top: Val::Px(px.y),
                    width: Val::Px(w * s),
                    height: Val::Px(h * s),
                    ..default()
                };
                let image = ImageNode {
                    image: asset_server.load(format!("media://{path}")),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                };
                if let Some(link_map) = poi.link_map {
                    canvas
                        .spawn((
                            WmCityButton { link_map },
                            Button,
                            Hovered::default(),
                            node,
                            image,
                        ))
                        .observe(on_city_button);
                    // the city art's lower ~30% is an empty name plate —
                    // the display name renders inside it
                    let city_name = table.map(link_map).and_then(|city| {
                        if city.name_key.starts_with("SN_") {
                            names.name(&city.name_key)
                        } else {
                            ui_strings.get(&city.name_key)
                        }
                    });
                    if let Some(city_name) = city_name {
                        canvas.spawn((
                            Text::new(city_name.to_string()),
                            text_font(7.0),
                            TextColor(Color::srgb(0.2, 0.15, 0.08)),
                            TextLayout::justify(Justify::Center),
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(px.x),
                                top: Val::Px(px.y + h * 0.72 * s),
                                width: Val::Px(w * s),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ));
                    }
                } else {
                    canvas.spawn((node, image, Pickable::IGNORE));
                }
            }
        }
    }
}

fn on_close_button(_: On<Activate>, mut state: ResMut<WorldMapState>) {
    state.open = false;
}

fn on_floor_button(
    activate: On<Activate>,
    buttons: Query<&WmFloorButton>,
    mut state: ResMut<WorldMapState>,
) {
    let Ok(button) = buttons.get(activate.entity) else {
        return;
    };
    if state.dungeon_map != Some((button.region, button.map_id)) {
        state.dungeon_map = Some((button.region, button.map_id));
        state.center_on_player = true;
    }
}

fn on_world_button(_: On<Activate>, mut state: ResMut<WorldMapState>) {
    state.current_map = 0;
    state.center_on_player = true;
}

fn on_follow_button(_: On<Activate>, mut follow: ResMut<WorldMapFollow>) {
    follow.0 = !follow.0;
}

/// Repaint the follow toggle's label when the mode flips.
pub fn update_follow_label(
    follow: Res<WorldMapFollow>,
    ui_strings: Res<ClientUiStrings>,
    mut labels: Query<&mut Text, With<WmFollowLabel>>,
) {
    if !follow.is_changed() {
        return;
    }
    for mut text in labels.iter_mut() {
        text.0 = follow_label(&ui_strings, follow.0);
    }
}

fn on_city_button(
    activate: On<Activate>,
    buttons: Query<&WmCityButton>,
    mut state: ResMut<WorldMapState>,
) {
    if let Ok(button) = buttons.get(activate.entity) {
        state.current_map = button.link_map;
        state.center_on_player = true;
    }
}

/// Drag-to-pan: remember the canvas offset, then move it with the pointer.
fn on_pan_start(
    _: On<Pointer<DragStart>>,
    viewports: Query<Entity, With<WmViewport>>,
    canvases: Query<&Node, With<WmCanvas>>,
    mut commands: Commands,
) {
    let (Ok(viewport), Ok(node)) = (viewports.single(), canvases.single()) else {
        return;
    };
    let (Val::Px(left), Val::Px(top)) = (node.left, node.top) else {
        return;
    };
    commands
        .entity(viewport)
        .insert(WmPanStart(Vec2::new(left, top)));
}

fn on_pan(
    drag: On<Pointer<Drag>>,
    starts: Query<&WmPanStart>,
    mut canvases: Query<(&WmCanvas, &mut Node)>,
    mut follow: ResMut<WorldMapFollow>,
) {
    let Ok(start) = starts.get(drag.entity) else {
        return;
    };
    let Ok((canvas, mut node)) = canvases.single_mut() else {
        return;
    };
    // a manual pan takes over from auto-follow
    if follow.0 {
        follow.0 = false;
    }
    let target = start.0 + drag.event.distance;
    apply_pan(canvas, &mut node, target);
}

/// Clamp the canvas offset so the viewport never shows past the map edge
/// (small maps center instead).
fn apply_pan(canvas: &WmCanvas, node: &mut Node, target: Vec2) {
    let s = hud_scale();
    let view = Vec2::new(CONTENT_W, CONTENT_H) * s;
    let clamp_axis = |value: f32, canvas_extent: f32, view_extent: f32| {
        if canvas_extent <= view_extent {
            (view_extent - canvas_extent) * 0.5
        } else {
            value.clamp(view_extent - canvas_extent, 0.0)
        }
    };
    node.left = Val::Px(clamp_axis(target.x, canvas.size.x, view.x));
    node.top = Val::Px(clamp_axis(target.y, canvas.size.y, view.y));
}

/// Per-frame while open: place/rotate the player arrow and run the one-shot
/// center-on-player.
pub fn update_world_map_arrow(
    mut state: ResMut<WorldMapState>,
    follow: Res<WorldMapFollow>,
    worldmap: Res<ClientWorldMap>,
    origin: Res<WorldOrigin>,
    player: Query<&Transform, With<Player>>,
    mut canvases: Query<(&WmCanvas, &mut Node), Without<WmArrow>>,
    mut arrows: Query<(&mut Node, &mut UiTransform, &mut Visibility), With<WmArrow>>,
) {
    if !state.open {
        return;
    }
    let (Some(table), Ok(player_tf)) = (worldmap.table(), player.single()) else {
        return;
    };
    let Ok((mut arrow_node, mut ui_transform, mut visibility)) = arrows.single_mut() else {
        return;
    };
    let s = hud_scale();

    let (gx, gz) = player_global_xz(&origin, player_tf);
    // Dungeon floor maps project the raw dungeon-local position through the
    // 128-centred sector space; overworld maps keep the du projection.
    let (on_map, px) = if let Some((region, map_id)) = state.dungeon_map {
        let Some(def) = table
            .dungeon_floors(region)
            .iter()
            .find(|def| def.map_id == map_id)
        else {
            return;
        };
        let du = dungeon_du(gx, gz);
        (def.contains_du(du.x, du.y), def.project(du.x, du.y) * s)
    } else {
        let Some(map) = table.map(state.current_map) else {
            return;
        };
        let (gx_du, gz_du) = (gx / 10.0, gz / 10.0);
        let on_map = gx_du >= map.left_du()
            && gx_du <= map.right_du()
            && gz_du >= map.bottom_du()
            && gz_du <= map.top_du();
        (on_map, map.project(gx_du, gz_du) * s)
    };
    *visibility = if on_map {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    arrow_node.left = Val::Px(px.x - ARROW_SIZE * 0.5 * s);
    arrow_node.top = Val::Px(px.y - ARROW_SIZE * 0.5 * s);

    // same heading math as the minimap arrow (art points east at zero)
    let facing = player_tf.rotation * Vec3::NEG_Z;
    let phi = facing.z.atan2(-facing.x);
    let rotation = Rot2::radians(-phi);
    if ui_transform.rotation != rotation {
        ui_transform.rotation = rotation;
    }

    // auto-follow keeps the player centered every frame; the one-shot
    // `center_on_player` covers open/map-switch jumps while in manual mode
    if follow.0 || state.center_on_player {
        if let Ok((canvas, mut canvas_node)) = canvases.single_mut() {
            let view = Vec2::new(CONTENT_W, CONTENT_H) * s;
            apply_pan(canvas, &mut canvas_node, view * 0.5 - px);
            // bypass_change_detection: this is a render-side one-shot, not a
            // state change the rebuild system should react to
            state.bypass_change_detection().center_on_player = false;
        }
    }
}

/// Hide world-map tiles whose file doesn't exist (the 33x11 grid has gaps).
pub fn hide_failed_tiles(
    asset_server: Res<AssetServer>,
    mut tiles: Query<(&ImageNode, &mut Visibility), With<WmTile>>,
) {
    for (image, mut visibility) in tiles.iter_mut() {
        let failed = matches!(
            asset_server.get_load_state(image.image.id()),
            Some(LoadState::Failed(_))
        );
        let wanted = if failed {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
}

/// OnExit cleanup.
pub fn cleanup_world_map(
    mut commands: Commands,
    windows: Query<Entity, With<WmWindowRoot>>,
    mut state: ResMut<WorldMapState>,
) {
    for entity in windows.iter() {
        commands.entity(entity).despawn();
    }
    *state = WorldMapState::default();
}

#[cfg(test)]
mod test {
    use super::*;

    /// `ginterface.txt:240` `GDR_WORLDMAP` `Rect="100,100,652,424"`: the
    /// content box must reconstruct that outer size, and the spawn anchor must
    /// be the rect's own origin in the classic tree's 1024x768 design space —
    /// before #318 both anchor values (200/40) derived from nothing.
    #[test]
    fn window_anchor_comes_from_the_vanilla_rect() {
        assert_eq!(
            game_window::outer_size((CONTENT_W, CONTENT_H)),
            (652.0, 424.0)
        );
        assert_eq!(WINDOW_RIGHT, 1024.0 - 100.0 - 652.0);
        assert_eq!(WINDOW_TOP, 100.0);
    }

    /// `ifworldmap.txt`: id 6 is 16x16 at `590,10` — inside the chrome's title
    /// band (y 6..28), not over the map; id 20 is 96x**28** at `543,43`, which
    /// is content-local `(531,7)` and leaves vanilla's 1px right margin.
    #[test]
    fn button_rects_match_ifworldmap() {
        assert_eq!(TO_WMAP_BTN_RECT, (590.0, 10.0, 16.0, 16.0));
        let (_, y, _, h) = TO_WMAP_BTN_RECT;
        assert!(y >= 6.0 && y + h <= 28.0, "id 6 must sit in the title band");

        let (x, y, w, h) = AUTO_MOVE_BTN_RECT;
        assert_eq!(
            (
                x + game_window::FRAME_VIS_SIDE + game_window::CHROME_PAD,
                y + game_window::CONTENT_TOP,
                w,
                h
            ),
            (543.0, 43.0, 96.0, 28.0)
        );
        assert_eq!(CONTENT_W - x - w, 1.0);
    }
}

/// Show a party marker's name while it is hovered.
///
/// The label is a hidden child spawned with the marker, so this only flips
/// visibility — no tooltip rig, no observers, and nothing to clean up when the
/// world map rebuilds its canvas (which it does on every roster position
/// update, taking the labels with it).
pub fn update_marker_labels(
    markers: Query<(&Hovered, &Children), With<WmMarkerSign>>,
    mut labels: Query<&mut Visibility, With<WmMarkerLabel>>,
) {
    for (hovered, children) in markers.iter() {
        let wanted = if hovered.get() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        for child in children.iter() {
            if let Ok(mut visibility) = labels.get_mut(child) {
                if *visibility != wanted {
                    *visibility = wanted;
                }
            }
        }
    }
}
