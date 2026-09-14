//! Minimap (top-right): a 3x3 grid of per-region terrain tiles centered on
//! the player, entity dots, an area-name/coordinate readout and zoom buttons.
//!
//! Idea: the layout is hand-transcribed from the vanilla
//! `Media.pk2/resinfo/ifminimap.txt` (like the mini-info panel transcribes
//! ifplayerminiinfo.txt), uniformly scaled. The map viewport is a square
//! `overflow: clip` node; the `mm_window.ddj` frame drawn on top is opaque
//! everywhere except its circular hole, which turns the square map into the
//! vanilla round minimap for free. Terrain tiles are `minimap/{x}x{z}.ddj`,
//! one 256px image per 1920-unit region, re-pointed when the player crosses a
//! region border; dots and tile offsets are recomputed per frame from the SRO
//! world positions (mind the mirrored render X, see `world_origin`).
//!
//! The tile/dot/arrow children carry per-frame-written absolute positions, so
//! they are spawned programmatically ([`populate_minimap_viewport`]) instead
//! of via bsn; sibling paint order inside the viewport is fixed with `ZIndex`
//! (tiles 0, dots 1, player arrow 2).

use bevy::asset::LoadState;
use bevy::log::warn_once;
use bevy::prelude::*;
use bevy::ui::{Overflow, UiTargetCamera, UiTransform};
use bevy::ui_widgets::Activate;

use crate::assets::dof::JMXVDOF;
use crate::assets::textdata::worldmap::DUNGEON_CENTER_REGION;
use crate::assets::FontAssets;
use crate::plugins::assets::sro::MediaArchive;
use crate::plugins::dungeon::ActiveDungeon;
use crate::plugins::hud::game_window::scaled;
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::hud::world_map::model::{MapMarkers, MarkerKind};
use crate::plugins::map::terrain::REGION_SIZE;
use crate::plugins::net::entities::{DisplayName, RemoteEntity, UniqueMonster};
use crate::plugins::player::Player;
use crate::plugins::textdata::{ClientWorldMap, ClientZoneNames};
use crate::plugins::ui_v2::style::ImageButtonStyle;
use crate::plugins::ui_v2::widgets::{image_button, label};
use crate::plugins::world_origin::WorldOrigin;

// --- Layout constants (resinfo/ifminimap.txt, window space) -----------------

/// Window placement, verbatim from `resinfo/ginterface.txt:807`
/// (`GDR_MINIMAP:CIFMinimap`, ID 10): `Rect=RECT,"892,6,140,184"`, authored
/// against the vanilla 1024x768 canvas. 892 + 140 = 1032, so the window
/// overhangs the right screen edge by 8px — harmless, because mm_window.ddj's
/// rightmost 11 columns are fully transparent, so visible art still ends at
/// x=1020. Anchored from the right to preserve that overhang at any
/// resolution.
const WINDOW_RECT: (f32, f32, f32, f32) = (892.0, 6.0, 140.0, 184.0);
/// Width of the design canvas the resinfo rects are authored against.
const DESIGN_SCREEN_W: f32 = 1024.0;

/// mm_window.ddj frame art size (= the window rect's extent).
const FRAME_W: f32 = WINDOW_RECT.2;
const FRAME_H: f32 = WINDOW_RECT.3;
/// Top and right screen margins in unscaled px; the right one is negative,
/// which is the 8px overhang described above.
const WINDOW_TOP: f32 = WINDOW_RECT.1;
const WINDOW_RIGHT: f32 = DESIGN_SCREEN_W - WINDOW_RECT.0 - WINDOW_RECT.2;

// Element rects (x, y, w, h) in window space, verbatim from the resinfo.
const VIEWPORT_RECT: (f32, f32, f32, f32) = (14.0, 57.0, 105.0, 105.0);
const AREA_NAME_RECT: (f32, f32, f32, f32) = (12.0, 9.0, 104.0, 12.0);
// `GDR_MINIMAP_TEXT_POS_X` (ifminimap.txt:63) and `..._TEXT_POS_Y` (:44): the
// original labels the second readout **Y**, though it carries world Z.
const POS_X_RECT: (f32, f32, f32, f32) = (8.0, 32.0, 56.0, 11.0);
const POS_Z_RECT: (f32, f32, f32, f32) = (67.0, 32.0, 56.0, 11.0);
const ZOOM_IN_RECT: (f32, f32, f32, f32) = (107.0, 136.0, 20.0, 20.0);
const ZOOM_OUT_RECT: (f32, f32, f32, f32) = (90.0, 152.0, 20.0, 20.0);
const MAP_BUTTON_RECT: (f32, f32, f32, f32) = (99.0, 47.0, 24.0, 24.0);
/// `GDR_MINIMAP_DUNGEON_FLOOR_INFO` — the floor badge shown inside dungeons.
const FLOOR_BADGE_RECT: (f32, f32, f32, f32) = (1.0, 42.0, 32.0, 32.0);

const FRAME_TEXTURE: &str = "media://interface/minimap/mm_window.ddj";
const SIGN_CHARACTER: &str = "media://interface/minimap/mm_sign_character.ddj";
const SIGN_MONSTER: &str = "media://interface/minimap/mm_sign_monster.ddj";
const SIGN_NPC: &str = "media://interface/minimap/mm_sign_npc.ddj";
const SIGN_OTHER_PLAYER: &str = "media://interface/minimap/mm_sign_otherplayer.ddj";
const SIGN_UNIQUE: &str = "media://interface/minimap/mm_sign_unique.ddj";
/// The party pair, both previously unused art (`docs/re/ui/hud-minimap.md`
/// §4 lists 13 sign textures, 8 of them undrawn). Sizes are the DDS headers'
/// in the user's own PK2: the dot is 8x8 like every other dot, the arrow 16x16
/// like `mm_sign_character`.
const SIGN_PARTY: &str = "media://interface/minimap/mm_sign_party.ddj";
const SIGN_PARTY_ARROW: &str = "media://interface/minimap/mm_sign_partyarrow.ddj";

/// Sign art sizes (window-space px): 8x8 dots, 12x12 unique, 16x16 arrow.
const DOT_SIZE: f32 = 8.0;
const UNIQUE_DOT_SIZE: f32 = 12.0;
const ARROW_SIZE: f32 = 16.0;

/// AREA_NAME FontColor from the resinfo (ARGB 255,239,218,164).
const AREA_NAME_COLOR: Color = Color::srgb_u8(239, 218, 164);
// UNKNOWN (docs/re/ui/hud-minimap.md §9-U9): the resinfo carries only
// `FontIndex=0` and never a size or a face, so both sizes below are invented.
const AREA_FONT_SIZE: f32 = 10.0;
const POS_FONT_SIZE: f32 = 9.0;

/// Map scale steps in window-space px per 1920-unit region (256 = the tiles'
/// native resolution). The minimum must stay above half the viewport width
/// (52.5) so the 3x3 grid still covers the viewport when the player stands
/// right on a region border.
///
/// UNKNOWN (hud-minimap.md §9-U4): only 256 is data-anchored — it is the
/// tiles' native resolution, so index 2 is 1:1. No PK2 file specifies a zoom
/// step table, so the other four steps and the default index are invented.
const ZOOM_LEVELS: [f32; 5] = [64.0, 128.0, 256.0, 384.0, 512.0];
const DEFAULT_ZOOM_IDX: usize = 2;

const DOT_POOL_SIZE: usize = 64;
/// A separate pool for party signs, sized to the party cap of 8
/// (`PartySetup::capacity()` — 8 when EXP is shared, else 4). They cannot
/// share the entity-dot pool: a member out of view still draws, as a rim
/// arrow, whereas an entity out of view is simply culled.
const PARTY_POOL_SIZE: usize = 8;

/// Displayed-coordinate origin: game coords are world units / 10, offset so
/// that 0 sits at region x 135 / z 92 (the vanilla sector formula).
///
/// UNKNOWN (hud-minimap.md §6): the 135/92 offsets are community-sourced —
/// nothing in the PK2 carries them.
const COORD_X_OFFSET: f32 = 135.0 * 192.0;
const COORD_Z_OFFSET: f32 = 92.0 * 192.0;

// --- State ------------------------------------------------------------------

#[derive(Resource, Clone, Debug)]
pub struct MinimapState {
    pub zoom_idx: usize,
    /// Region sector (x, z) the tile grid is currently centered on; `None`
    /// until the first player position is seen.
    pub center_region: Option<(i32, i32)>,
}

impl Default for MinimapState {
    fn default() -> Self {
        Self {
            zoom_idx: DEFAULT_ZOOM_IDX,
            center_region: None,
        }
    }
}

/// Entity-dot sign textures, loaded once at spawn.
#[derive(Resource)]
pub struct MinimapAssets {
    monster: Handle<Image>,
    npc: Handle<Image>,
    other_player: Handle<Image>,
    unique: Handle<Image>,
    party: Handle<Image>,
    party_arrow: Handle<Image>,
}

// --- Markers ----------------------------------------------------------------

#[derive(Component, Default, Clone)]
pub struct MinimapRoot;
/// The clipped square map area; its tile/dot/arrow children are added by
/// [`populate_minimap_viewport`].
#[derive(Component, Default, Clone)]
pub struct MinimapViewport;
/// Marks a viewport whose children have been populated.
#[derive(Component)]
pub struct MinimapViewportReady;
/// One of the 3x3 terrain tile slots, at grid offset (dx, dz) from the
/// player's region.
#[derive(Component)]
pub struct MinimapTile {
    dx: i8,
    dz: i8,
}
/// One slot of the entity-dot pool.
#[derive(Component)]
pub struct MinimapDot;
/// One slot of the party-sign pool. Separate from [`MinimapDot`] because a
/// party member outside the viewport still draws (clamped to the rim as an
/// arrow) instead of being culled.
#[derive(Component)]
pub struct MinimapPartySign;
#[derive(Component)]
pub struct MinimapArrow;
#[derive(Component, Default, Clone)]
pub struct MinimapAreaText;
#[derive(Component, Default, Clone)]
pub struct MinimapPosXText;
#[derive(Component, Default, Clone)]
pub struct MinimapPosZText;
/// The dungeon floor badge art (`mm_dungeonfloor.ddj`), hidden outside
/// dungeons.
#[derive(Component, Default, Clone)]
pub struct MinimapFloorBadge;
/// The floor-number text on the badge (`1F` / `B3`).
#[derive(Component, Default, Clone)]
pub struct MinimapFloorText;

// --- Dungeon floor context --------------------------------------------------

/// Present while the minimap should draw dungeon floor tiles instead of the
/// overworld grid. Kept in sync by [`sync_minimap_dungeon_context`].
#[derive(Resource, Clone, PartialEq)]
pub struct MinimapDungeonContext {
    pub region_id: u16,
    /// Lowercased floor label of the player's current block — the
    /// `minimap_d` tile filename stem.
    pub floor: String,
    /// `minimap_d/<group>` directory holding the floor's tiles; `None` when
    /// the archive ships none (tiles hide, fail-soft).
    pub group: Option<String>,
    /// `(basement, number)` for the badge, from the Dungeonmap rows.
    pub floor_number: Option<(bool, u32)>,
}

/// Floor-string (lowercased) → `minimap_d` group directory, discovered once
/// from the Media archive listing (`minimap_d/<group>/<floor>_<x>x<z>.ddj`).
/// This resolves the `<group>` UNKNOWN of `docs/formats/minimap.md`
/// operationally — the mapping lives nowhere in the data tables, but the
/// archive layout itself is authoritative.
#[derive(Resource, Default)]
pub struct MinimapDungeonGroups(pub std::collections::HashMap<String, String>);

fn build_dungeon_groups(archive: &bevy_pk2::prelude::Archive) -> MinimapDungeonGroups {
    let mut groups = std::collections::HashMap::new();
    for (path, entry) in archive.root.get_all_entries() {
        if !entry.is_file() {
            continue;
        }
        let lower = path.to_string_lossy().replace('\\', "/").to_lowercase();
        let Some(rest) = lower.strip_prefix("minimap_d/") else {
            continue;
        };
        let (Some((group, file)), true) = (rest.split_once('/'), rest.ends_with(".ddj")) else {
            continue;
        };
        // `dh_a01_floor01_127x126.ddj` → floor stem before the `_<x>x<z>`.
        let stem = file.trim_end_matches(".ddj");
        let Some((floor, _coords)) = stem.rsplit_once('_') else {
            continue;
        };
        groups
            .entry(floor.to_string())
            .or_insert_with(|| group.to_string());
    }
    info!(
        "minimap: indexed {} dungeon floor tile sets under minimap_d/",
        groups.len()
    );
    MinimapDungeonGroups(groups)
}

/// Keep [`MinimapDungeonContext`] in sync with the active dungeon and the
/// player's current floor; entering/leaving/floor changes reset the tile
/// grid's center so the tiles re-point.
#[allow(clippy::too_many_arguments)]
pub fn sync_minimap_dungeon_context(
    active: Option<Res<ActiveDungeon>>,
    dofs: Res<Assets<JMXVDOF>>,
    worldmap: Res<ClientWorldMap>,
    media: Option<Res<MediaArchive>>,
    groups: Option<Res<MinimapDungeonGroups>>,
    context: Option<Res<MinimapDungeonContext>>,
    mut state: ResMut<MinimapState>,
    mut commands: Commands,
) {
    let Some(active) = active else {
        if context.is_some() {
            commands.remove_resource::<MinimapDungeonContext>();
            state.center_region = None;
        }
        return;
    };
    let Some(groups) = groups else {
        // Lazy one-time index; lands as a resource next frame.
        if let Some(media) = media {
            commands.insert_resource(build_dungeon_groups(&media.0));
        }
        return;
    };

    // Current floor: the player's block's floor label; single-floor
    // dungeons without labels fall back to their lone Dungeonmap row.
    let floor = dofs
        .get(&active.dof)
        .and_then(|dof| {
            active
                .current_block
                .and_then(|index| dof.blocks.get(index))
                .and_then(|block| dof.floor_names.get(block.floor_index as usize))
                .map(|name| name.to_lowercase())
        })
        .or_else(|| {
            worldmap.table().and_then(|table| {
                let floors = table.dungeon_floors(active.region_id);
                (floors.len() == 1).then(|| floors[0].floor_string.to_lowercase())
            })
        });
    let Some(floor) = floor else {
        if context.is_some() {
            commands.remove_resource::<MinimapDungeonContext>();
            state.center_region = None;
        }
        return;
    };

    let floor_number = worldmap
        .table()
        .and_then(|table| table.dungeon_floor(active.region_id, &floor))
        .map(|def| (def.basement, def.floor_number));
    let new = MinimapDungeonContext {
        region_id: active.region_id,
        group: groups.0.get(&floor).cloned(),
        floor,
        floor_number,
    };
    if context.as_deref() != Some(&new) {
        state.center_region = None;
        commands.insert_resource(new);
    }
}

/// Show the floor badge + number while a dungeon floor is displayed.
pub fn update_minimap_floor_badge(
    context: Option<Res<MinimapDungeonContext>>,
    mut badges: Query<&mut Visibility, (With<MinimapFloorBadge>, Without<MinimapFloorText>)>,
    mut texts: Query<(&mut Text, &mut Visibility), With<MinimapFloorText>>,
) {
    let target = if context.is_some() {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut visibility in badges.iter_mut() {
        if *visibility != target {
            *visibility = target;
        }
    }
    let label = context
        .as_ref()
        .map(|ctx| match ctx.floor_number {
            Some((true, n)) => format!("B{n}"),
            Some((false, n)) => format!("{n}F"),
            // No Dungeonmap row (e.g. donwhang_event): derive from the
            // floor string's trailing digits, else stay blank.
            None => ctx
                .floor
                .rsplit(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|digits| digits.parse::<u32>().ok())
                .map(|n| format!("{n}F"))
                .unwrap_or_default(),
        })
        .unwrap_or_default();
    for (mut text, mut visibility) in texts.iter_mut() {
        if text.0 != label {
            text.0 = label.clone();
        }
        if *visibility != target {
            *visibility = target;
        }
    }
}

// --- Spawn / cleanup --------------------------------------------------------

pub fn spawn_minimap(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    mut state: ResMut<MinimapState>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found for the minimap");
        return;
    };

    // fresh session state (re-entry must re-point the tile grid)
    *state = MinimapState::default();

    commands.insert_resource(MinimapAssets {
        monster: asset_server.load(SIGN_MONSTER),
        npc: asset_server.load(SIGN_NPC),
        other_player: asset_server.load(SIGN_OTHER_PLAYER),
        unique: asset_server.load(SIGN_UNIQUE),
        party: asset_server.load(SIGN_PARTY),
        party_arrow: asset_server.load(SIGN_PARTY_ARROW),
    });

    commands
        .spawn_scene(minimap(&asset_server, &fonts))
        .insert(UiTargetCamera(camera));
}

pub fn cleanup_minimap(mut commands: Commands, roots: Query<Entity, With<MinimapRoot>>) {
    for entity in roots.iter() {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<MinimapAssets>();
}

fn minimap(asset_server: &AssetServer, fonts: &FontAssets) -> impl Scene {
    let frame: Handle<Image> = asset_server.load(FRAME_TEXTURE);
    let floor_badge: Handle<Image> =
        asset_server.load("media://interface/minimap/mm_dungeonfloor.ddj");
    let zoom_in_style = button_style(asset_server, "mm_zoomin");
    let zoom_out_style = button_style(asset_server, "mm_zoomout");
    let map_button_style = button_style(asset_server, "mm_map_button");

    let area_font = fonts.nine.clone();
    let x_font = fonts.nine.clone();
    let z_font = fonts.nine.clone();
    let floor_font = fonts.nine.clone();

    let s = hud_scale();
    let (vp_l, vp_t, vp_w, vp_h) = scaled(VIEWPORT_RECT, s);
    let (an_l, an_t, an_w, an_h) = scaled(AREA_NAME_RECT, s);
    let (px_l, px_t, px_w, px_h) = scaled(POS_X_RECT, s);
    let (pz_l, pz_t, pz_w, pz_h) = scaled(POS_Z_RECT, s);
    let (zi_l, zi_t, zi_w, zi_h) = scaled(ZOOM_IN_RECT, s);
    let (zo_l, zo_t, zo_w, zo_h) = scaled(ZOOM_OUT_RECT, s);
    let (mb_l, mb_t, mb_w, mb_h) = scaled(MAP_BUTTON_RECT, s);
    let (fb_l, fb_t, fb_w, fb_h) = scaled(FLOOR_BADGE_RECT, s);

    bsn! {
        MinimapRoot
        Name("Minimap")
        Node {
            position_type: PositionType::Absolute,
            top: px(WINDOW_TOP * s),
            right: px(WINDOW_RIGHT * s),
            width: px(FRAME_W * s),
            height: px(FRAME_H * s),
        }
        // above the world, below the loading overlay (200)
        GlobalZIndex(50)
        Pickable::IGNORE
        Children [
            // clipped square map area; the black backdrop shows where tiles
            // are missing (world edge) or still streaming in
            (
                MinimapViewport
                Node {
                    position_type: PositionType::Absolute,
                    left: px(vp_l),
                    top: px(vp_t),
                    width: px(vp_w),
                    height: px(vp_h),
                    overflow: {Overflow::clip()},
                }
                BackgroundColor(Color::BLACK)
                Pickable::IGNORE
            ),
            // the frame on top: opaque ring around a transparent circular
            // hole -> the square map reads as the vanilla round minimap
            (
                ImageNode { image: {frame}, image_mode: NodeImageMode::Stretch }
                Node {
                    position_type: PositionType::Absolute,
                    left: px(0),
                    top: px(0),
                    width: px(FRAME_W * s),
                    height: px(FRAME_H * s),
                }
                Pickable::IGNORE
            ),
            (
                label("", area_font, AREA_FONT_SIZE * s)
                MinimapAreaText
                TextColor({AREA_NAME_COLOR})
                Node { position_type: PositionType::Absolute, left: px(an_l), top: px(an_t), width: px(an_w), height: px(an_h) }
            ),
            (
                label("", x_font, POS_FONT_SIZE * s)
                MinimapPosXText
                TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, left: px(px_l), top: px(px_t), width: px(px_w), height: px(px_h) }
            ),
            (
                label("", z_font, POS_FONT_SIZE * s)
                MinimapPosZText
                TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, left: px(pz_l), top: px(pz_t), width: px(pz_w), height: px(pz_h) }
            ),
            (
                image_button(zoom_in_style, zi_w, zi_h)
                Node { position_type: PositionType::Absolute, left: px(zi_l), top: px(zi_t) }
                on(|_activate: On<Activate>, mut state: ResMut<MinimapState>| {
                    state.zoom_idx = (state.zoom_idx + 1).min(ZOOM_LEVELS.len() - 1);
                })
            ),
            (
                image_button(zoom_out_style, zo_w, zo_h)
                Node { position_type: PositionType::Absolute, left: px(zo_l), top: px(zo_t) }
                on(|_activate: On<Activate>, mut state: ResMut<MinimapState>| {
                    state.zoom_idx = state.zoom_idx.saturating_sub(1);
                })
            ),
            // dungeon floor badge (art + number), shown only inside dungeons
            (
                MinimapFloorBadge
                ImageNode { image: {floor_badge}, image_mode: NodeImageMode::Stretch }
                Node {
                    position_type: PositionType::Absolute,
                    left: px(fb_l),
                    top: px(fb_t),
                    width: px(fb_w),
                    height: px(fb_h),
                }
                Visibility::Hidden
                Pickable::IGNORE
            ),
            (
                label("", floor_font, POS_FONT_SIZE * s)
                MinimapFloorText
                TextColor(Color::WHITE)
                Node {
                    position_type: PositionType::Absolute,
                    left: px(fb_l),
                    top: px(fb_t),
                    width: px(fb_w),
                    height: px(fb_h),
                }
                Visibility::Hidden
            ),
            // world-map toggle (same behavior as the M key)
            (
                image_button(map_button_style, mb_w, mb_h)
                Node { position_type: PositionType::Absolute, left: px(mb_l), top: px(mb_t) }
                on(|_activate: On<Activate>,
                    origin: Res<crate::plugins::world_origin::WorldOrigin>,
                    worldmap: Res<crate::plugins::textdata::ClientWorldMap>,
                    dungeon: Option<Res<MinimapDungeonContext>>,
                    player: Query<&Transform, With<Player>>,
                    mut state: ResMut<crate::plugins::hud::world_map::model::WorldMapState>| {
                    if state.open {
                        state.open = false;
                    } else {
                        crate::plugins::hud::world_map::model::open_for_player_region(
                            &origin, &worldmap, &player, dungeon.as_deref(), &mut state,
                        );
                    }
                })
            ),
        ]
    }
}

fn button_style(asset_server: &AssetServer, stem: &str) -> ImageButtonStyle {
    ImageButtonStyle {
        normal: asset_server.load(format!("media://interface/minimap/{stem}.ddj")),
        hover: asset_server.load(format!("media://interface/minimap/{stem}_focus.ddj")),
        press: asset_server.load(format!("media://interface/minimap/{stem}_press.ddj")),
        ..Default::default()
    }
}

/// Add the per-frame-positioned children to a fresh viewport: 9 terrain tile
/// slots, the dot pool and the player arrow. Polling, because the scene's
/// entities only exist after a command flush.
pub fn populate_minimap_viewport(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    viewports: Query<Entity, (With<MinimapViewport>, Without<MinimapViewportReady>)>,
) {
    let s = hud_scale();
    let h = VIEWPORT_RECT.2 / 2.0;
    for viewport in viewports.iter() {
        let arrow: Handle<Image> = asset_server.load(SIGN_CHARACTER);
        commands
            .entity(viewport)
            .insert(MinimapViewportReady)
            .with_children(|parent| {
                for dz in -1..=1i8 {
                    for dx in -1..=1i8 {
                        parent.spawn((
                            MinimapTile { dx, dz },
                            ImageNode {
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                            Node {
                                position_type: PositionType::Absolute,
                                ..default()
                            },
                            Visibility::Hidden,
                            ZIndex(0),
                            Pickable::IGNORE,
                        ));
                    }
                }
                for _ in 0..DOT_POOL_SIZE {
                    parent.spawn((
                        MinimapDot,
                        ImageNode {
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                        Node {
                            position_type: PositionType::Absolute,
                            ..default()
                        },
                        Visibility::Hidden,
                        ZIndex(1),
                        Pickable::IGNORE,
                    ));
                }
                for _ in 0..PARTY_POOL_SIZE {
                    parent.spawn((
                        MinimapPartySign,
                        ImageNode {
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                        Node {
                            position_type: PositionType::Absolute,
                            ..default()
                        },
                        // above the entity dots: a party member must not be
                        // hidden under a monster dot on the same pixel
                        ZIndex(2),
                        Visibility::Hidden,
                        Pickable::IGNORE,
                    ));
                }
                parent.spawn((
                    MinimapArrow,
                    ImageNode {
                        image: arrow,
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px((h - ARROW_SIZE / 2.0) * s),
                        top: Val::Px((h - ARROW_SIZE / 2.0) * s),
                        width: Val::Px(ARROW_SIZE * s),
                        height: Val::Px(ARROW_SIZE * s),
                        ..default()
                    },
                    UiTransform::default(),
                    ZIndex(2),
                    Pickable::IGNORE,
                ));
            });
    }
}

// --- Per-frame updates ------------------------------------------------------

/// Server-global (gx, gz) of the local player, in world units. The render
/// world mirrors X relative to the region grid (`server_position_to_render`),
/// hence the negation.
fn player_global_xz(origin: &WorldOrigin, transform: &Transform) -> (f32, f32) {
    let sro = origin.to_sro(transform.translation);
    (-sro.x, sro.z)
}

/// Position (and, on a region change, re-point) the 3x3 terrain tiles so the
/// map stays centered on the player. East (+region x) runs right, north
/// (+region z) up.
pub fn update_minimap_tiles(
    mut state: ResMut<MinimapState>,
    origin: Res<WorldOrigin>,
    asset_server: Res<AssetServer>,
    dungeon: Option<Res<MinimapDungeonContext>>,
    player: Query<&Transform, With<Player>>,
    mut tiles: Query<(&MinimapTile, &mut Node, &mut ImageNode, &mut Visibility)>,
) {
    let Ok(player_tf) = player.single() else {
        return;
    };
    if tiles.is_empty() {
        return;
    }

    let (mut gx, mut gz) = player_global_xz(&origin, player_tf);
    if dungeon.is_some() {
        // Inside a dungeon (gx, gz) are raw dungeon-local coordinates; the
        // minimap_d tile grid lives in a sector space centred on (128,128).
        gx += DUNGEON_CENTER_REGION * REGION_SIZE;
        gz += DUNGEON_CENTER_REGION * REGION_SIZE;
    }
    let xsec = (gx / REGION_SIZE).floor() as i32;
    let ysec = (gz / REGION_SIZE).floor() as i32;
    let repoint = state.center_region != Some((xsec, ysec));
    if repoint {
        state.center_region = Some((xsec, ysec));
    }

    let s = hud_scale();
    let h = VIEWPORT_RECT.2 / 2.0;
    let p = ZOOM_LEVELS[state.zoom_idx];
    let k = p / REGION_SIZE;
    // A dungeon floor with no shipped tile set shows the black backdrop.
    let no_dungeon_tiles = dungeon.as_ref().is_some_and(|ctx| ctx.group.is_none());

    for (tile, mut node, mut image, mut visibility) in tiles.iter_mut() {
        let tx = xsec + tile.dx as i32;
        let tz = ysec + tile.dz as i32;
        let in_range = (0..=255).contains(&tx) && (0..=255).contains(&tz) && !no_dungeon_tiles;
        if repoint && in_range {
            let path = match &dungeon {
                Some(ctx) => {
                    let group = ctx.group.as_deref().unwrap_or_default();
                    format!("media://minimap_d/{group}/{}_{tx}x{tz}.ddj", ctx.floor)
                }
                None => format!("media://minimap/{tx}x{tz}.ddj"),
            };
            image.image = asset_server.load(path);
        }

        // tile top edge = its region's north edge
        let left = Val::Px((h + (tx as f32 * REGION_SIZE - gx) * k) * s);
        let top = Val::Px((h - ((tz + 1) as f32 * REGION_SIZE - gz) * k) * s);
        let size = Val::Px(p * s);
        if node.left != left {
            node.left = left;
        }
        if node.top != top {
            node.top = top;
        }
        if node.width != size {
            node.width = size;
            node.height = size;
        }

        // hide world-edge tiles and tiles whose image doesn't exist (the
        // viewport's black backdrop shows through)
        let failed = matches!(
            asset_server.get_load_state(&image.image),
            Some(LoadState::Failed(_))
        );
        let target = if in_range && !failed {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != target {
            *visibility = target;
        }
    }
}

/// The displayed game coordinates: world units / 10, origin at region 135/92
/// (so Jangan reads ~6400 like the vanilla client).
pub fn update_minimap_coords(
    origin: Res<WorldOrigin>,
    dungeon: Option<Res<MinimapDungeonContext>>,
    player: Query<&Transform, With<Player>>,
    mut x_texts: Query<&mut Text, (With<MinimapPosXText>, Without<MinimapPosZText>)>,
    mut z_texts: Query<&mut Text, (With<MinimapPosZText>, Without<MinimapPosXText>)>,
) {
    let Ok(player_tf) = player.single() else {
        return;
    };
    let (gx, gz) = player_global_xz(&origin, player_tf);
    // Dungeon-local coordinates display as-is (no overworld origin offset).
    let (disp_x, disp_z) = if dungeon.is_some() {
        ((gx / 10.0).round() as i32, (gz / 10.0).round() as i32)
    } else {
        (
            (gx / 10.0 - COORD_X_OFFSET).round() as i32,
            (gz / 10.0 - COORD_Z_OFFSET).round() as i32,
        )
    };

    let set_text = |text: &mut Text, value: String| {
        if text.0 != value {
            text.0 = value;
        }
    };
    for mut text in x_texts.iter_mut() {
        set_text(&mut text, coord_text('X', disp_x));
    }
    for mut text in z_texts.iter_mut() {
        set_text(&mut text, coord_text('Y', disp_z));
    }
}

/// One coordinate readout, e.g. `"X: 6400"`. The second axis is labelled **Y**
/// because the original element is `GDR_MINIMAP_TEXT_POS_Y`
/// (`resinfo/ifminimap.txt:44`), even though the value it shows is world Z.
///
/// UNKNOWN (hud-minimap.md §9-U8): the vanilla separator and sign handling are
/// unconfirmed; this is our own rendering of the same two values.
fn coord_text(axis: char, value: i32) -> String {
    format!("{axis}: {value}")
}

/// Area name of the player's region, from textzonename.txt. Inside a
/// dungeon the dungeon's own region id names the whole interior (textzonename
/// stores dungeon ids in the same u16 space, e.g. 32769 = Donwhang Stone
/// Cave); the sector-derived lookup only applies to the overworld.
pub fn update_minimap_area_name(
    state: Res<MinimapState>,
    dungeon: Option<Res<MinimapDungeonContext>>,
    zone_names: Res<ClientZoneNames>,
    mut texts: Query<&mut Text, With<MinimapAreaText>>,
) {
    let region = match &dungeon {
        Some(ctx) => ctx.region_id,
        None => {
            let Some((xsec, ysec)) = state.center_region else {
                return;
            };
            if !(0..=255).contains(&xsec) || !(0..=255).contains(&ysec) {
                return;
            }
            ((ysec as u16) << 8) | (xsec as u16)
        }
    };
    let name = zone_names.name(region).unwrap_or("");
    for mut text in texts.iter_mut() {
        if text.0 != name {
            text.0 = name.to_string();
        }
    }
}

/// Rotate the center arrow to the player's heading. The arrow art points east
/// at zero rotation; `phi` is the server-space heading angle (0 = east,
/// counterclockwise looking down with north up), negated because UI rotation
/// runs clockwise on screen (y down).
pub fn update_minimap_arrow(
    player: Query<&Transform, With<Player>>,
    mut arrows: Query<&mut UiTransform, With<MinimapArrow>>,
) {
    let Ok(player_tf) = player.single() else {
        return;
    };
    // the model faces -Z at identity; un-mirror render X into server space
    let facing = player_tf.rotation * Vec3::NEG_Z;
    let phi = facing.z.atan2(-facing.x);
    let rotation = Rot2::radians(-phi);
    for mut ui_transform in arrows.iter_mut() {
        if ui_transform.rotation != rotation {
            ui_transform.rotation = rotation;
        }
    }
}

/// Project remote entities into the viewport and write them into the dot
/// pool. Decoded from the sign art: monsters red (255,0,0), NPCs blue
/// (65,131,255), other players yellow-green (189,230,0) and uniques purple
/// (156,0,255) at 12x12 — every other dot is 8x8.
pub fn update_minimap_dots(
    state: Res<MinimapState>,
    origin: Res<WorldOrigin>,
    assets: Option<Res<MinimapAssets>>,
    player: Query<&Transform, With<Player>>,
    entities: Query<(&Transform, &RemoteEntity, Has<UniqueMonster>)>,
    mut dots: Query<(&mut Node, &mut ImageNode, &mut Visibility), With<MinimapDot>>,
) {
    let Some(assets) = assets else {
        return;
    };
    let Ok(player_tf) = player.single() else {
        return;
    };
    let (gx, gz) = player_global_xz(&origin, player_tf);

    let s = hud_scale();
    let h = VIEWPORT_RECT.2 / 2.0;
    let k = ZOOM_LEVELS[state.zoom_idx] / REGION_SIZE;

    let mut visible: Vec<(f32, f32, f32, Handle<Image>)> = Vec::new();
    for (transform, kind, unique) in entities.iter() {
        let (image, size) = match kind {
            RemoteEntity::Item => continue,
            RemoteEntity::Monster if unique => (assets.unique.clone(), UNIQUE_DOT_SIZE),
            RemoteEntity::Monster => (assets.monster.clone(), DOT_SIZE),
            RemoteEntity::Npc => (assets.npc.clone(), DOT_SIZE),
            RemoteEntity::Player => (assets.other_player.clone(), DOT_SIZE),
        };
        let sro = origin.to_sro(transform.translation);
        let (ex, ez) = (-sro.x, sro.z);
        let cx = h + (ex - gx) * k;
        let cy = h - (ez - gz) * k;
        // circle cull against the frame's round hole; overflow clip catches
        // the square corners anyway
        if (cx - h).powi(2) + (cy - h).powi(2) > (h + size / 2.0).powi(2) {
            continue;
        }
        visible.push((cx, cy, size, image));
        if visible.len() > DOT_POOL_SIZE {
            warn_once!("minimap: more than {DOT_POOL_SIZE} entities in view, dropping overflow");
            break;
        }
    }

    let mut pending = visible.into_iter();
    for (mut node, mut image_node, mut visibility) in dots.iter_mut() {
        match pending.next() {
            Some((cx, cy, size, image)) => {
                let left = Val::Px((cx - size / 2.0) * s);
                let top = Val::Px((cy - size / 2.0) * s);
                let px_size = Val::Px(size * s);
                if node.left != left {
                    node.left = left;
                }
                if node.top != top {
                    node.top = top;
                }
                if node.width != px_size {
                    node.width = px_size;
                    node.height = px_size;
                }
                if image_node.image != image {
                    image_node.image = image;
                }
                if *visibility != Visibility::Inherited {
                    *visibility = Visibility::Inherited;
                }
            }
            None => {
                if *visibility != Visibility::Hidden {
                    *visibility = Visibility::Hidden;
                }
            }
        }
    }
}

/// Where one party marker draws inside the round viewport.
///
/// Idea: a party member is not an entity dot. An entity outside the hole is
/// simply culled, but a party member out of view is the case the player most
/// wants to see — which is what `mm_sign_partyarrow.ddj` exists for. The RE
/// doc reads the four `*arrow` sign textures as "rim-clamped direction
/// indicators for off-map targets" (`docs/re/ui/hud-minimap.md` §4, `[S]`),
/// and that is what this implements: inside the hole the 8x8 dot at the true
/// position, outside it the 16x16 arrow pushed back onto the rim along the
/// same bearing, rotated to point at the member.
///
/// `radius` is the hole's radius; both returned coordinates are viewport-space
/// centres, like [`update_minimap_dots`] produces.
fn party_sign_placement(dx: f32, dy: f32, radius: f32) -> PartySignPlacement {
    let distance = (dx * dx + dy * dy).sqrt();
    // Half the arrow, so a rim-clamped arrow sits fully inside the hole
    // instead of half-way under the frame ring.
    let rim = (radius - ARROW_SIZE / 2.0).max(0.0);
    if distance <= rim {
        return PartySignPlacement {
            dx,
            dy,
            size: DOT_SIZE,
            arrow: false,
            rotation: 0.0,
        };
    }
    // distance > rim >= 0 here, so it is never zero and the scale is finite.
    let scale = rim / distance;
    PartySignPlacement {
        dx: dx * scale,
        dy: dy * scale,
        size: ARROW_SIZE,
        arrow: true,
        // Screen y grows downward, so the bearing is measured against -dy.
        rotation: dx.atan2(-dy),
    }
}

/// Result of [`party_sign_placement`], in viewport-space offsets from centre.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PartySignPlacement {
    dx: f32,
    dy: f32,
    size: f32,
    arrow: bool,
    rotation: f32,
}

/// Draw the party roster on the minimap (EP-14.4).
///
/// Positions come from `world_map::model::party_markers`, which folds
/// `PartyRoster` into `MapMarkers` in the minimap's own mirrored-X global
/// convention (`(-sro.x, sro.z)`), including the rule that a member inside a
/// dungeon has no overworld position and is skipped.
///
/// **With one deliberate exception**: a member who is spawned near us is drawn
/// from their live `Transform` instead. The roster's position is only as fresh
/// as the last 0x3864 position delta — every few seconds — so a member standing
/// beside you visibly lagged their own character. The world map keeps the
/// throttled fold, so the two maps *can* now disagree by a few metres for
/// in-range members; that is the trade, and it is the right way round, because
/// the minimap is the one you read while moving.
///
/// The local player is not among these: 0x3065 sends the *whole* party, so the
/// player is one of its members, and a party sign under their own centre arrow
/// would be a duplicate. That filtering now happens once at the producer
/// (`world_map::model::party_markers`) rather than here — the world map needed
/// it too, and two copies of the same rule is one to forget.
pub fn update_minimap_party_signs(
    state: Res<MinimapState>,
    origin: Res<WorldOrigin>,
    assets: Option<Res<MinimapAssets>>,
    markers: Option<Res<MapMarkers>>,
    player: Query<&Transform, With<Player>>,
    nearby: Query<(&DisplayName, &Transform), (With<RemoteEntity>, Without<Player>)>,
    mut signs: Query<
        (&mut Node, &mut ImageNode, &mut UiTransform, &mut Visibility),
        With<MinimapPartySign>,
    >,
) {
    let (Some(assets), Some(markers)) = (assets, markers) else {
        return;
    };
    let Ok(player_tf) = player.single() else {
        return;
    };
    let (gx, gz) = player_global_xz(&origin, player_tf);

    let s = hud_scale();
    let h = VIEWPORT_RECT.2 / 2.0;
    let k = ZOOM_LEVELS[state.zoom_idx] / REGION_SIZE;

    let mut placements: Vec<PartySignPlacement> = Vec::new();
    for marker in markers.0.iter() {
        if marker.kind != MarkerKind::Party {
            continue;
        }
        // Prefer a LIVE position when the member is spawned near us. The
        // roster's position only moves when the server pushes a 0x3864
        // type-6 kind-0x20 delta, every few seconds — which is why the dot
        // visibly trailed a member standing right beside you while your own
        // arrow moved smoothly. The join is the name, the same one the
        // producer uses to leave the local player out.
        let (mx, mz) = nearby
            .iter()
            .find(|(name, _)| name.0 == marker.name)
            .map(|(_, transform)| player_global_xz(&origin, transform))
            .unwrap_or((marker.gx, marker.gz));
        let placement = party_sign_placement((mx - gx) * k, -(mz - gz) * k, h);
        placements.push(placement);
        if placements.len() >= PARTY_POOL_SIZE {
            break;
        }
    }

    let mut pending = placements.into_iter();
    for (mut node, mut image_node, mut ui_transform, mut visibility) in signs.iter_mut() {
        let Some(placement) = pending.next() else {
            if *visibility != Visibility::Hidden {
                *visibility = Visibility::Hidden;
            }
            continue;
        };
        let left = Val::Px((h + placement.dx - placement.size / 2.0) * s);
        let top = Val::Px((h + placement.dy - placement.size / 2.0) * s);
        let px_size = Val::Px(placement.size * s);
        if node.left != left {
            node.left = left;
        }
        if node.top != top {
            node.top = top;
        }
        if node.width != px_size {
            node.width = px_size;
            node.height = px_size;
        }
        let image = if placement.arrow {
            assets.party_arrow.clone()
        } else {
            assets.party.clone()
        };
        if image_node.image != image {
            image_node.image = image;
        }
        // The dot is round and must not spin with the bearing.
        let rotation = Rot2::radians(if placement.arrow {
            placement.rotation
        } else {
            0.0
        });
        if ui_transform.rotation != rotation {
            ui_transform.rotation = rotation;
        }
        if *visibility != Visibility::Inherited {
            *visibility = Visibility::Inherited;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EP-14.4: a member inside the hole is a dot at their true offset; one
    /// outside is the arrow, clamped to the rim on the same bearing. The
    /// clamp uses `radius - ARROW_SIZE/2` so the arrow sits fully inside the
    /// hole rather than half under the frame ring.
    #[test]
    fn a_party_member_out_of_view_becomes_a_rim_arrow_on_the_same_bearing() {
        let h = VIEWPORT_RECT.2 / 2.0;
        let rim = h - ARROW_SIZE / 2.0;

        // well inside: the plain dot, untouched position, no rotation
        let inside = party_sign_placement(10.0, -4.0, h);
        assert!(!inside.arrow);
        assert_eq!((inside.dx, inside.dy), (10.0, -4.0));
        assert_eq!(inside.size, DOT_SIZE);
        assert_eq!(inside.rotation, 0.0);

        // far away due north: clamped to the rim, still due north
        let north = party_sign_placement(0.0, -10_000.0, h);
        assert!(north.arrow);
        assert_eq!(north.size, ARROW_SIZE);
        assert!((north.dx.abs()) < 1e-3);
        assert!(
            (north.dy + rim).abs() < 1e-3,
            "dy {} vs -rim {}",
            north.dy,
            -rim
        );
        // and it never overruns the hole
        assert!((north.dx.powi(2) + north.dy.powi(2)).sqrt() <= h - ARROW_SIZE / 2.0 + 1e-3);

        // the bearing survives the clamp: direction is preserved exactly
        let (dx, dy) = (300.0, 400.0);
        let far = party_sign_placement(dx, dy, h);
        assert!(far.arrow);
        assert!((far.dx / far.dy - dx / dy).abs() < 1e-4);
        assert!(((far.dx.powi(2) + far.dy.powi(2)).sqrt() - rim).abs() < 1e-3);
    }

    /// Screen y grows downward, so a member due north must rotate to 0 and one
    /// due east to +90 degrees. Getting the sign wrong points every arrow the
    /// wrong way, which no compile error catches.
    #[test]
    fn the_arrow_rotation_follows_screen_space_not_world_space() {
        let h = VIEWPORT_RECT.2 / 2.0;
        let north = party_sign_placement(0.0, -1000.0, h);
        assert!(north.rotation.abs() < 1e-4, "north {}", north.rotation);
        let east = party_sign_placement(1000.0, 0.0, h);
        assert!(
            (east.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
            "east {}",
            east.rotation
        );
        let south = party_sign_placement(0.0, 1000.0, h);
        assert!((south.rotation.abs() - std::f32::consts::PI).abs() < 1e-4);
    }

    /// The party pool is the party cap, and the party art is the pair the RE
    /// doc lists as unused — both sizes are the DDS headers in the user's PK2.
    #[test]
    fn the_party_pool_and_art_match_the_data() {
        // PartySetup::capacity() is 8 when EXP is shared, else 4
        assert_eq!(PARTY_POOL_SIZE, 8);
        assert_eq!(SIGN_PARTY, "media://interface/minimap/mm_sign_party.ddj");
        assert_eq!(
            SIGN_PARTY_ARROW,
            "media://interface/minimap/mm_sign_partyarrow.ddj"
        );
        // mm_sign_party.ddj is 8x8 like every other dot; partyarrow 16x16
        // like mm_sign_character.
        assert_eq!(DOT_SIZE, 8.0);
        assert_eq!(ARROW_SIZE, 16.0);
    }

    /// The window placement is a transcription of the v1.188 resinfo, so pin it
    /// to the cited bytes: it previously drifted to a hand-picked `(4, 4)`
    /// top-right margin (issue #305).
    #[test]
    fn window_anchor_matches_the_ginterface_rect() {
        // resinfo/ginterface.txt:807 — GDR_MINIMAP:CIFMinimap, ID 10.
        assert_eq!(WINDOW_RECT, (892.0, 6.0, 140.0, 184.0));
        assert_eq!((FRAME_W, FRAME_H), (140.0, 184.0));
        assert_eq!(WINDOW_TOP, 6.0);
        // 1024 - (892 + 140) = -8: the vanilla window overhangs the right edge.
        assert_eq!(WINDOW_RIGHT, -8.0);
    }

    /// Every element rect is byte-exact against `resinfo/ifminimap.txt`; the
    /// cited line is the element's block header.
    #[test]
    fn element_rects_match_ifminimap() {
        assert_eq!(VIEWPORT_RECT, (14.0, 57.0, 105.0, 105.0)); // :139 _ALPHA
        assert_eq!(AREA_NAME_RECT, (12.0, 9.0, 104.0, 12.0)); // :82 _TEXT_AREANAME
        assert_eq!(POS_X_RECT, (8.0, 32.0, 56.0, 11.0)); // :63 _TEXT_POS_X
        assert_eq!(POS_Z_RECT, (67.0, 32.0, 56.0, 11.0)); // :44 _TEXT_POS_Y
        assert_eq!(ZOOM_IN_RECT, (107.0, 136.0, 20.0, 20.0)); // :120 _ZOOMIN
        assert_eq!(ZOOM_OUT_RECT, (90.0, 152.0, 20.0, 20.0)); // :101 _ZOOMOUT
        assert_eq!(MAP_BUTTON_RECT, (99.0, 47.0, 24.0, 24.0)); // :25 _BTN_TOG_MAP
    }

    /// The second readout is `GDR_MINIMAP_TEXT_POS_Y` (ifminimap.txt:44), so it
    /// is labelled "Y" — it used to emit "Z" after the world-space field name.
    #[test]
    fn coordinate_labels_use_the_original_axis_letters() {
        assert_eq!(coord_text('X', 6400), "X: 6400");
        assert_eq!(coord_text('Y', -70), "Y: -70");
    }

    /// Dot sizes are the sign textures' own dimensions: 8x8 for
    /// monster/npc/otherplayer, 12x12 for unique, 16x16 for the player arrow.
    #[test]
    fn dot_sizes_match_the_sign_art() {
        assert_eq!(DOT_SIZE, 8.0);
        assert_eq!(UNIQUE_DOT_SIZE, 12.0);
        assert_eq!(ARROW_SIZE, 16.0);
    }
}

/// Self-registration for the minimap (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct MinimapPlugin;

impl Plugin for MinimapPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<MinimapState>()
            .add_systems(OnEnter(SceneState::GameWorld), spawn_minimap)
            .add_systems(OnExit(SceneState::GameWorld), cleanup_minimap)
            // The Dungeons test scene runs the map widgets without the rest
            // of the HUD.
            .add_systems(OnExit(SceneState::Dungeons), cleanup_minimap)
            .add_systems(
                Update,
                (
                    sync_minimap_dungeon_context,
                    populate_minimap_viewport,
                    update_minimap_tiles,
                    update_minimap_coords,
                    update_minimap_area_name,
                    update_minimap_arrow,
                    update_minimap_dots,
                    update_minimap_party_signs,
                    update_minimap_floor_badge,
                )
                    .run_if(super::map_scenes),
            );
    }
}
