//! The in-game scene entered from character selection.
//!
//! Idea: this is the real playable scene, as opposed to the `world_scene` dev
//! sandbox (fly cam + hardcoded China-man). It reuses the world's terrain
//! streaming, floating origin and the shared player/camera systems, but:
//!   * assembles the *actual* selected character (model + equipment, named
//!     after it) from the `JoiningCharacter` resource carried over from
//!     char-select — the same data-driven assembly the char-select previews use;
//!   * spawns only the third-person follow camera (no fly camera), forced
//!     active regardless of `AppMode`;
//!   * shows a loading screen that persists until terrain + player are ready;
//!   * owns the post-join in-game networking (self-spawn foundation): it places
//!     the player at the server position with its server unique id, drives the
//!     in-game clock from the server, and completes the load handshake.
//!
//! The gameplay systems themselves (movement, animation, follow camera) live in
//! `PlayerPlugin`/`CameraPlugin` and are gated on `in_playable_world`, so they
//! run here and in the World sandbox alike.

use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bytes::Bytes;

use packets::agent::prelude::{
    CelestialPosition, CelestialUpdate, CharacterDataBody, CharacterDataEnd, EntitySpeedUpdate,
    GameReady, GameReset, GameResetComplete, MovementAngleResponse, MovementPositionUpdate,
    MovementRequest, MovementResponse, SingleEntityDespawn, SingleEntitySpawn, TeleportResponse,
    GROUP_SPAWN, MOTION_STATE_WALK,
};
use packets::Packet;

use crate::commands::SpawnedFromResource;
use packets::agent::character_data::{parse_character_info, ItemClass};
use packets::hexdump;

use crate::assets::bmt::sheen::ShineColor;
use crate::net::connection::SilkroadConnection;
use crate::net::entity_spawn::{parse_group_spawn, SpawnKind, SpawnedEntity, TextdataResolver};
use crate::plugins::camera::{
    despawn_cinematic_camera, spawn_game_camera, CinematicCamera, CinematicCamera2, PlayerCamera,
};
use crate::plugins::combat::{ActiveAttack, Dying, EntityDied, Slain};
use crate::plugins::cursor::interactions::GameCursorTarget;
use crate::plugins::dynamic_resource_loader::{
    AttachmentRareAura, AttachmentShine, MirroredResource, PendingItemAttachment,
    PreferredAnimationGroup, UnloadedResource,
};
use crate::plugins::effects::{rare, EffectCommandsExt};
use crate::plugins::environment::TimeOfDay;
use crate::plugins::map::terrain::{Terrain, TerrainLoadState};
use crate::plugins::nav::NavLocation;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::character_info::{CharacterInfo, MovementSpeed};
use crate::plugins::net::entities::{
    CharacterRef, DisplayName, DropRarity, EntityVitals, MonsterRarity, NeedsGroundSnap,
    NetworkEntities, NetworkId, RemoteEntity, RemoteMovement, SealDrop, UniqueMonster,
};
use crate::plugins::net::inventory::Inventory;
use crate::plugins::player::{
    spawn_player_character, Player, PlayerCommands, PlayerConfig, PlayerMoveOrder,
};
use crate::plugins::skills::EquippedWeapon;
use crate::plugins::textdata::{
    ClientCharacterData, ClientItemData, ClientRareEffects, ClientTextNames,
};
use crate::plugins::world_origin::{set_dungeon_origin, set_world_origin, WorldOrigin};
use crate::scenes::intro_v2::character_select::{JoiningCharacter, PendingWorldJoin};
use crate::scenes::loading_screen::{spawn_loading_surface, LoadingProgress};
use crate::scenes::world_scene::{preload_starting_area, set_origin_to_spawn_point, SpawnPoints};
use crate::scenes::SceneState;
use crate::util::mesh::{mirrored, needs_winding_reversal};
use crate::util::region::RegionIdExt;

/// How far past the engagement's own reach a server destination may land and
/// still count as an attack approach — one that gets projected onto our direct
/// line to the target. Anything farther is ordinary travel that happens to
/// occur mid-engagement.
///
/// Additive, and sized so the unarmed/melee case reproduces the flat 80 this
/// replaces (`ATTACK_GAP_STOP` 16 + 64). The server's stop point comes from
/// *its* belief of both positions, so the tolerance covers a disagreement
/// between the two views, which does not scale with the weapon: a multiplier
/// would hand a bow an 800-unit radius and start re-projecting genuine long
/// travel that merely passed near the engaged monster.
const ATTACK_APPROACH_SLACK: f32 = 64.0;

/// Whether a server destination `radius` away from the engaged target is an
/// attack approach, for an engagement whose reach is `reach` world units.
fn is_attack_approach(radius: f32, reach: f32) -> bool {
    radius < reach + ATTACK_APPROACH_SLACK
}

pub struct GameScenePlugin;

impl Plugin for GameScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalPlayer>()
            .add_systems(
                OnEnter(SceneState::GameWorld),
                (
                    reset_local_player,
                    set_origin_to_spawn_point,
                    spawn_game_camera,
                    despawn_cinematic_camera::<CinematicCamera>,
                    despawn_cinematic_camera::<CinematicCamera2>,
                    spawn_selected_player,
                    setup_fog,
                    preload_starting_area,
                    spawn_game_loading_overlay,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    on_celestial_position,
                    on_celestial_update,
                    on_character_data,
                    // before on_speed_update: the CHARACTER_DATA state block
                    // carries BASE speeds — if it and a buffed 0x30D0 land in
                    // the same frame, the base insert must not clobber the
                    // buffed write
                    apply_pending_character_data.before(on_speed_update),
                    on_character_data_end,
                    on_teleport_response,
                    on_game_reset,
                    on_group_spawn,
                    on_single_spawn,
                    mark_item_drops_no_shadow,
                    send_movement_request,
                    on_movement_response,
                    on_movement_position_update,
                    on_movement_angle,
                    on_speed_update,
                    dismiss_loading_overlay_when_ready,
                )
                    .run_if(in_state(SceneState::GameWorld)),
            )
            .add_systems(OnExit(SceneState::GameWorld), cleanup_game_scene);
    }
}

/// In-game session state for the local player, populated from the server's
/// post-join stream (celestial position + character data). Reset on each entry
/// to `GameWorld`.
#[derive(Resource, Default)]
struct LocalPlayer {
    /// Server-assigned in-world unique id. Set by whichever of `CharacterData`
    /// (0x3013, id embedded before the spawn position) or `CelestialPosition`
    /// (0x3020) resolves it first.
    unique_id: Option<u32>,
    /// Raw CHARACTER_DATA body kept until it has been applied. Buffered because
    /// the id needed to pin the spawn position may arrive in `CelestialPosition`
    /// after this packet (see [`apply_pending_character_data`]).
    pending_character_data: Option<Bytes>,
    /// `CharacterDataEnd` seen: the self-spawn stream is complete.
    stream_complete: bool,
    /// `GameReady` (0x3012) already sent — guards against re-sending.
    game_ready_sent: bool,
}

fn reset_local_player(mut local: ResMut<LocalPlayer>) {
    *local = LocalPlayer::default();
}

/// The fullscreen loading overlay shown until the world is ready, carrying its
/// own per-entry state.
///
/// Idea: the safety-net timer and the gauge's high-water mark live on the
/// *entity* rather than in `Local`s, because the entity is spawned in
/// `OnEnter(GameWorld)` and despawned by `cleanup_game_scene` — so a second
/// world entry starts at zero by construction instead of by remembering to
/// reset it. As a `Local`, `elapsed` survived the scene exit sitting at the
/// 30 s cap and dismissed the *next* entry's overlay on its first frame,
/// dropping the player into an unbuilt world. Same trap `dev_fast_login`
/// documents for its once-per-visit guards.
#[derive(Component, Default)]
struct GameLoadingOverlay {
    /// Seconds this overlay has been up, for [`MAX_WAIT_SECS`].
    elapsed: f32,
    /// Highest readiness fraction reported so far (0..=1). Monotonic: the
    /// terrain count dips when `rearm_object_passes_on_region_unload` re-arms
    /// completed regions, and a gauge that walks backwards reads as broken.
    progress: f32,
}

/// Spawn the playable character from the character the user picked in
/// char-select (`JoiningCharacter`): the body from its `ref_obj_id` plus one
/// attachment per equipped/avatar item, exactly like the char-select previews
/// (`on_char_selection_action_response`) but with the gameplay component set
/// (`Player` + `GameCursorTarget`). Falls back to the default `PlayerConfig`
/// look when no character was carried over (e.g. launched standalone via
/// `SCENE=game`).
///
/// This is **the** local-player spawn for a real session:
/// `spawn_player_character` is only reached from the two fallback branches
/// below and from the sandbox/dev scenes. It used to say "non-mirrored" here
/// and mean it, which is why the character you actually play kept rendering
/// mirror-imaged (weapon in the wrong hand) long after every other SRO
/// resource had been put in the mirrored frame.
fn spawn_selected_player(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    joining: Option<Res<JoiningCharacter>>,
    char_data: Res<ClientCharacterData>,
    item_data: Res<ClientItemData>,
    rare_effects: Res<ClientRareEffects>,
    config: Res<PlayerConfig>,
    origin: Res<WorldOrigin>,
    mut tree_race: ResMut<crate::plugins::hud::skill_window::model::SkillTreeRace>,
) {
    // Spawn on the world's start point (jangan), where the terrain and the
    // follow camera live. This is the fallback position;
    // `apply_pending_character_data` repositions the player to the server-given
    // spawn once the CHARACTER_DATA packet is resolved (it writes `translation`
    // only, so the mirror below survives).
    //
    // Mirrored like every other SRO resource — see `util::mesh`. Applied to the
    // shared transform so both the join path and the two fallbacks get it;
    // `spawn_player_character` mirrors again on those, which is a no-op because
    // `mirrored` is idempotent.
    let transform = mirrored(Transform::from_translation(
        origin.to_render(SpawnPoints::jangan()),
    ));

    let Some(joining) = joining else {
        // No character carried over: dev fallback so the scene is runnable
        // on its own without a server.
        spawn_player_character(&mut commands, &asset_server, &config, transform);
        return;
    };
    let char = &joining.0;

    let Some(char_row) = char_data.get(&(char.ref_obj_id as i32)) else {
        warn!(
            "no characterdata entry for ref {}; using fallback look",
            char.ref_obj_id
        );
        spawn_player_character(&mut commands, &asset_server, &config, transform);
        return;
    };

    // the skill window shows the tree of the character's race
    // (CHAR_EU_* vs CHAR_CH_* code names)
    *tree_race = if char_row.code_name().starts_with("CHAR_EU") {
        crate::plugins::hud::skill_window::model::SkillTreeRace::European
    } else {
        crate::plugins::hud::skill_window::model::SkillTreeRace::Chinese
    };

    let mut player = commands.spawn((
        Name::from(char.name.as_str()),
        DisplayName(char.name.clone()),
        Player::new(),
        GameCursorTarget::default(),
        // Default stance in case the character carries no weapon; the
        // equipped weapon overrides it below.
        PreferredAnimationGroup("sword".to_string()),
        transform,
        Visibility::default(),
    ));
    if needs_winding_reversal(&transform.to_matrix()) {
        player.insert(MirroredResource);
    }
    if let Some(path) = char_row.resource_path() {
        player.insert(UnloadedResource(asset_server.load(path)));
    }
    let player_entity = player.id();

    attach_character_equipment(
        &mut commands,
        &asset_server,
        &item_data,
        &rare_effects,
        player_entity,
        char.char_items
            .iter()
            .chain(char.avatar_items.iter())
            .map(|i| (i.id, i.plus)),
    );

    // consumed — a re-entry would spawn a fresh player from the join flow again
    commands.remove_resource::<JoiningCharacter>();
}

/// Attach a character's equipped/avatar items to its body: for each item ref id,
/// let a weapon override the parent's [`PreferredAnimationGroup`] (weapons decide
/// the animation stance) and spawn a [`PendingItemAttachment`] child for anything
/// with a 3d resource. Shared by the local player ([`spawn_selected_player`]) and
/// remote players ([`on_group_spawn_data`]).
fn attach_character_equipment(
    commands: &mut Commands,
    asset_server: &AssetServer,
    item_data: &ClientItemData,
    rare_effects: &ClientRareEffects,
    parent: Entity,
    items: impl Iterator<Item = (u32, u8)>,
) {
    for (id, opt_level) in items {
        let Some(item_row) = item_data.get(&(id as i32)) else {
            warn!("no itemdata entry for equipped item: {}", id);
            continue;
        };
        // the equipped weapon decides which animation set the character plays
        if let Some(group) = item_row.animation_group() {
            commands
                .entity(parent)
                .insert(PreferredAnimationGroup(group.to_string()));
        }
        // items without a 3d resource (e.g. pure stat items) cannot be rendered
        let Some(item_path) = item_row.resource_path() else {
            continue;
        };
        let mut item = commands.spawn((
            PendingItemAttachment(asset_server.load(item_path)),
            ChildOf(parent),
            Name::from(format!("item {}", item_row.code_name())),
        ));
        // +N enhancement glow (only weapons carry a skeleton and get it)
        if let Some(color) = ShineColor::for_opt_level(opt_level) {
            item.insert(AttachmentShine::Tier(color));
        }
        // Rare ("Seal of …") items carry looping auras on a weapon bone, per
        // the ItemRare.txt (raretype) + ItemOptionEfp.txt (enchant) tables.
        // Gate on rarity: those tables also key plain non-rare item codes (the
        // enchant table is really the +N glow), so without this a normal shield
        // would wrongly get an aura.
        // Two independent, additive aura systems share the tables:
        // ItemRare.txt rows (no min_opt) are the Seal-of-X aura — rare
        // items only, regardless of enhancement; ItemOptionEfp.txt rows
        // (min_opt, uniformly 8 in the 1.188 data) are the +8 enchant
        // flare — ANY item at that enhancement, plain or rare.
        let auras: Vec<_> = rare_effects
            .get(item_row.code_name())
            .iter()
            .filter(|a| match a.min_opt {
                Some(min) => opt_level >= min,
                None => item_row.is_rare(),
            })
            .cloned()
            .collect();
        if !auras.is_empty() {
            // gate: None — the min_opt rows were validated against the
            // spawn record's opt level right above
            item.insert(AttachmentRareAura { auras, gate: None });
        }
    }
}

/// Insert distance fog on the follow camera, mirroring the world scene's
/// `setup` (ambient light + clear color are set once in `map::setup_lighting`).
fn setup_fog(
    mut commands: Commands,
    config: Res<crate::plugins::config::ClientConfig>,
    camera_query: Query<Entity, With<PlayerCamera>>,
) {
    for cam in camera_query.iter() {
        commands
            .entity(cam)
            .insert(crate::plugins::map::terrain::rendering::fog(
                &config.graphics.fog,
            ));
    }
}

/// Fullscreen loading overlay drawn on top of the 3d scene by the persistent 2d
/// UI camera (see `ui::setup_cameras`), reusing the vanilla loading artwork.
/// Torn down by `dismiss_loading_overlay_when_ready` once the world is ready.
fn spawn_game_loading_overlay(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found for the game loading overlay");
        return;
    };

    let background: Handle<Image> =
        asset_server.load("media://interface/loading/loading_default.ddj");

    commands
        .spawn((
            GameLoadingOverlay::default(),
            Name::from("Game Loading Overlay"),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                // the 4:3 cover box overflows on the axis that does not fit
                overflow: bevy::ui::Overflow::clip(),
                ..default()
            },
            BackgroundColor(Color::BLACK),
            // above everything else the scene might draw
            GlobalZIndex(200),
            Pickable::IGNORE,
            UiTargetCamera(camera),
        ))
        .with_children(|parent| {
            // Background and chrome both keep the authored 4:3: the art fills the
            // window cropped, the chrome sits in the centred 4:3 box — stretching
            // both to the window distorts them (`loading_screen::DESIGN_ASPECT`).
            // With the gauge: `dismiss_loading_overlay_when_ready` reports the
            // world-entry readiness into it.
            spawn_loading_surface(parent, &asset_server, background, true);
        });
}

/// How long the world-entry overlay may stay up before it is lifted regardless.
/// A safety net, not a schedule: it keeps a stalled asset or an absent server
/// (the standalone `SCENE=game` dev path) from trapping the player behind the
/// overlay forever. Reaching it is a defect, so the dismissal says so.
const MAX_WAIT_SECS: f32 = 30.0;

/// Drive the loading gauge and remove the overlay once the world is actually
/// ready to be shown: the server's self-spawn stream has completed (so the
/// player is at its final position), the player's body wrapper has streamed in
/// (a child carrying [`SpawnedFromResource`]) and no terrain region is still
/// loading.
///
/// Idea: readiness is asked of the terrain plugin in the terms *it* maintains.
/// A region that is still loading is a region that still carries a
/// [`TerrainLoadState`] — `load_terrain_objects_system` removes the component
/// one poll after `Completed` precisely to disarm the per-frame polling — so
/// "absent" and `Completed` both mean done. This check used to filter on
/// `With<PreloadedTerrain>` *and* read `&TerrainLoadState`, and both of those
/// components are retired while the world loads: `load_terrain_dynamically`
/// drops the preload marker as soon as the camera is in range (it is a head
/// start, not a pin), and the object pass drops the load state on completion.
/// The query therefore emptied a frame or two after scene entry, `terrain_ready`
/// could never become true again, and every world entry sat behind the overlay
/// for the full [`MAX_WAIT_SECS`] with all its packets long since arrived.
///
/// Reading whatever terrain is currently resident, rather than a set captured at
/// scene entry, also survives the world-origin re-anchor:
/// `preload_starting_area` preloads around Jangan, but
/// [`apply_pending_character_data`] moves the origin onto the server's actual
/// spawn, after which those regions stream out and the ones around the real
/// spawn stream in. A captured set would be measuring the wrong area.
#[allow(clippy::too_many_arguments)]
fn dismiss_loading_overlay_when_ready(
    mut commands: Commands,
    time: Res<Time>,
    local: Res<LocalPlayer>,
    agent: Query<(), With<AgentConnection>>,
    mut overlay: Query<(Entity, &mut GameLoadingOverlay)>,
    player: Query<&Children, With<Player>>,
    body_wrappers: Query<(), With<SpawnedFromResource>>,
    terrain: Query<Option<&TerrainLoadState>, With<Terrain>>,
    // The overlay owns the only gauge in this scene: the boot screen's is
    // despawned by `teardown_loading_screen` on `OnExit(SceneState::Loading)`,
    // and the board->creation cut spawns its chrome gaugeless.
    mut gauge: Query<&mut Node, With<LoadingProgress>>,
) {
    if overlay.is_empty() {
        return;
    }

    let player_ready = player
        .single()
        .map(|children| children.iter().any(|child| body_wrappers.contains(child)))
        .unwrap_or(false);

    let (mut total, mut done) = (0usize, 0usize);
    for load_state in terrain.iter() {
        total += 1;
        if matches!(load_state, None | Some(TerrainLoadState::Completed)) {
            done += 1;
        }
    }
    // `total == 0` is not readiness but its opposite: no region has been spawned
    // yet (or `preload_starting_area` bailed on an unresolved `.mfo`).
    let terrain_ready = total > 0 && done == total;

    // Only wait for the server's self-spawn stream when actually connected;
    // the standalone `SCENE=game` dev path (no agent) lifts without it.
    let stream_ready = agent.is_empty() || local.stream_complete;

    // The three gates the dismissal waits on, equally weighted; terrain reports
    // its own completion fraction because it is the one that takes real time.
    let progress =
        (stream_ready as u8 as f32 + player_ready as u8 as f32 + done as f32 / total.max(1) as f32)
            / 3.0;
    let ready = stream_ready && player_ready && terrain_ready;

    for (entity, mut state) in overlay.iter_mut() {
        state.elapsed += time.delta_secs();
        state.progress = state.progress.max(progress);
        for mut node in gauge.iter_mut() {
            // a fraction of the authored gauge rect, like the boot screen's fill
            node.width = Val::Percent(100.0 * state.progress);
        }
        if ready {
            commands.entity(entity).despawn();
        } else if state.elapsed > MAX_WAIT_SECS {
            warn!(
                "world-entry overlay dismissed by the {MAX_WAIT_SECS}s safety net: \
                 stream={stream_ready} player={player_ready} terrain={done}/{total}"
            );
            commands.entity(entity).despawn();
        }
    }
}

/// Day-cycle fraction (0.0 = midnight, 0.5 = noon) for a server clock time.
fn time_of_day_fraction(hour: u8, minute: u8) -> f32 {
    ((hour as f32 + minute as f32 / 60.0) / 24.0).clamp(0.0, 1.0)
}

/// Convert a server spawn position (region + region-local x/y/z) to render
/// space. SRO mirrors world X (region x -> `-x * 1920`), see `world_origin` /
/// `util::region`. Note: the world origin stays anchored at the jangan start,
/// so spawns more than a few regions away would want an origin re-anchor
/// (follow-up); starting-town spawns land in the streamed area fine.
fn server_position_to_render(region: u16, x: f32, y: f32, z: f32, origin: &WorldOrigin) -> Vec3 {
    origin.to_render(server_position_to_sro(region, x, y, z))
}

/// A server spawn position (region + region-local x/y/z) as an SRO-space point,
/// before the floating-origin shift. Split out from [`server_position_to_render`]
/// so callers that must re-anchor the world origin on the target (a long-range
/// teleport) have the un-shifted SRO position to snap the origin to.
///
/// Dungeon regions (bit 15) have no 1920-unit sector tiling: their
/// coordinates are already dungeon-local (offsets in the DOF's own frame, up
/// to ±20k and negative), so only the X mirror applies.
pub(crate) fn server_position_to_sro(region: u16, x: f32, y: f32, z: f32) -> Vec3 {
    if region.is_dungeon() {
        return Vec3::new(-x, y, z);
    }
    let (rx, rz) = region.to_x_z();
    Vec3::new(-(rx as f32 * 1920.0 + x), y, rz as f32 * 1920.0 + z)
}

/// Convert an SRO heading to a render-space Y rotation. The heading is a `u16`
/// mapping `0..65536` onto `0..2π`, measured as `atan2(Z, X)` in region-offset
/// space (see go-sro). Render space mirrors world X, and the SRO body faces `-Z`
/// — the same facing convention `PlayerPlugin` uses to turn the local player
/// toward its move direction, so networked entities line up with it.
fn heading_to_render_rotation(heading: u16) -> Quat {
    let theta = heading as f32 / 65536.0 * std::f32::consts::TAU;
    // Offset-space facing (cos θ, sin θ) over (X, Z); render mirrors the X axis.
    let render_dir_x = -theta.cos();
    let render_dir_z = theta.sin();
    let yaw = render_dir_x.atan2(render_dir_z) + std::f32::consts::PI;
    Quat::from_rotation_y(yaw)
}

/// Inverse of [`server_position_to_render`]: a render-space point to a region id
/// + region-local x/y/z. Used to build a movement request from a click target.
///
/// Overworld only: this can never produce a dungeon region id (the sector
/// derivation assumes 1920-unit tiling). Movement requests from inside a
/// dungeon must instead pass the dungeon's own region id with dungeon-local
/// coordinates (`-sro.x, y, z` — the inverse of the dungeon branch in
/// [`server_position_to_sro`]); wired up with the dungeon nav runtime.
pub(crate) fn render_to_server_position(
    render: Vec3,
    origin: &WorldOrigin,
) -> (u16, f32, f32, f32) {
    let sro = origin.to_sro(render);
    // World X is mirrored, so the region-x axis runs along `-x`.
    let rx = (-sro.x / 1920.0).floor();
    let rz = (sro.z / 1920.0).floor();
    let region = (((rz as i32) << 8) | (rx as i32 & 0xFF)) as u16;
    let local_x = -sro.x - rx * 1920.0;
    let local_z = sro.z - rz * 1920.0;
    (region, local_x, sro.y, local_z)
}

/// Send a movement request for a click-to-move order (authoritative: the player
/// only walks when the server answers with a `MovementResponse`). Coordinates go
/// out as raw region-local units (×10 scaling made the server wrap the
/// destination several regions over — verified against a capture).
fn send_movement_request(
    mut orders: MessageReader<PlayerMoveOrder>,
    origin: Res<WorldOrigin>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    rider: Res<crate::plugins::cos::RiderState>,
    cos_list: Res<crate::plugins::cos::ActiveCosList>,
) {
    for order in orders.read() {
        let (region, x, y, z) = render_to_server_position(order.0, &origin);
        // Coordinates are raw region-local units (verified against a capture:
        // ×10 scaling made the server wrap the destination several regions over).
        let (x, y, z) = (x.round() as i32, y.round() as i32, z.round() as i32);
        // Mounted on a server-side COS: the order addresses the MOUNT via the
        // pet-action envelope; the server answers with a MovementResponse for
        // the COS uid and the rider is transform-slaved (mount.md §3). A
        // local-only dev mount is driven by cos::riding instead — send nothing.
        let request = match rider.0 {
            Some(cos_uid) => {
                if cos_list.get(cos_uid).is_none_or(|s| s.local_only) {
                    continue;
                }
                debug!(
                    "network: PetActionRequest::Movement cos={} region={:#06X} local=({},{},{})",
                    cos_uid, region, x, y, z
                );
                Packet::from(packets::agent::pet::PetActionRequest::Movement {
                    pet_unique_id: cos_uid,
                    region,
                    x,
                    y,
                    z,
                })
            }
            None => {
                debug!(
                    "network: MovementRequest click_render=({:.1},{:.1},{:.1}) region={:#06X} local=({},{},{})",
                    order.0.x, order.0.y, order.0.z, region, x, y, z
                );
                Packet::from(MovementRequest { region, x, y, z })
            }
        };
        let Ok(conn) = conn.single() else {
            warn!("network: no agent connection to send MovementRequest");
            return;
        };
        if let Err(e) = conn.get_sender().send(request.into()) {
            error!("network: failed to send movement request: {}", e.0);
        }
    }
}

/// Apply a `MovementResponse` (0xB021): walk the addressed entity toward the
/// server's destination. Only the local player is spawned today; other unique
/// ids resolve via [`NetworkEntities`] once the entity-spawn phase lands.
#[allow(clippy::too_many_arguments)]
fn on_movement_response(
    mut reader: MessageReader<MovementResponse>,
    origin: Res<WorldOrigin>,
    local: Res<LocalPlayer>,
    entities: Res<NetworkEntities>,
    active_attack: Res<ActiveAttack>,
    weapon: Res<EquippedWeapon>,
    transforms: Query<&Transform>,
    players: Query<&Transform, With<Player>>,
    mut player_commands: ResMut<PlayerCommands>,
    mut remote_movement: Query<&mut RemoteMovement>,
) {
    for res in reader.read() {
        let is_local = Some(res.unique_id) == local.unique_id;
        if res.has_destination {
            // Coordinates are raw region-local units.
            let mut render = server_position_to_render(
                res.region,
                res.x as f32,
                res.y as f32,
                res.z as f32,
                &origin,
            );
            debug!(
                "network: MovementResponse uid={} region={:#06X} local=({},{},{}) -> render=({:.1},{:.1},{:.1})",
                res.unique_id, res.region, res.x, res.y, res.z, render.x, render.y, render.z
            );
            if is_local {
                // Attack-approach destinations come from the *server's*
                // belief of both positions, which puts the stop point a
                // little beside the monster as the client sees it — the
                // character runs past it and snaps around. Keep the server's
                // stop *radius* but project the point onto our own line to
                // the target, so the approach runs straight at the monster.
                if let Some(target_tf) = active_attack
                    .target
                    .and_then(|target| transforms.get(target).ok())
                {
                    let target_xz = target_tf.translation.xz();
                    let radius = target_xz.distance(render.xz());
                    if is_attack_approach(radius, weapon.engagement_reach(active_attack.stop_range))
                    {
                        if let Ok(player) = players.single() {
                            let from_target = player.translation.xz() - target_xz;
                            let distance = from_target.length();
                            if distance <= radius {
                                // Already inside the stop ring (the target ran
                                // toward/past us mid-chase): the ring point
                                // would be BEHIND the player — never step
                                // back, just hold and keep swinging. The
                                // server keeps re-pathing while the target
                                // flees, so skipped moves self-correct.
                                player_commands.stop();
                                continue;
                            }
                            let projected = target_xz + from_target / distance * radius;
                            render = Vec3::new(projected.x, render.y, projected.y);
                        }
                    }
                }
                // Reconcile the optimistic local move against the server's
                // confirmed/corrected destination.
                player_commands.move_to(render);
            } else if let Some(entity) = entities.get(res.unique_id) {
                // Walk the remote entity toward the server destination.
                if let Ok(mut movement) = remote_movement.get_mut(entity) {
                    movement.target = Some(render);
                }
            }
        } else if is_local {
            // No destination = the server rejected/stopped the move (a "nack").
            debug!("network: MovementResponse uid={} stop", res.unique_id);
            player_commands.stop();
        } else if let Some(entity) = entities.get(res.unique_id) {
            // A remote entity stopped: clear its movement target.
            if let Ok(mut movement) = remote_movement.get_mut(entity) {
                movement.target = None;
            }
        }
    }
}

/// Apply a `MovementPositionUpdate` (0xB023): snap the addressed entity to the
/// server's exact position (a teleport/knockback/correction, not a walk order).
/// The local player and remote entities both resolve through [`NetworkEntities`]
/// (the player carries a `NetworkId` too), so both are handled here; any in-flight
/// walk toward an old target is cancelled.
/// 0xB024 — an entity turned in place. Rotation only: no position, no movement
/// target and no ground snap, so it must not disturb an in-flight walk. The
/// local player turns itself from its own input, so the server's angle for our
/// own uid is ignored rather than fighting it.
fn on_movement_angle(
    mut reader: MessageReader<MovementAngleResponse>,
    local: Res<LocalPlayer>,
    entities: Res<NetworkEntities>,
    mut transforms: Query<&mut Transform>,
) {
    for msg in reader.read() {
        if Some(msg.unique_id) == local.unique_id {
            continue;
        }
        let Some(entity) = entities.get(msg.unique_id) else {
            continue;
        };
        if let Ok(mut transform) = transforms.get_mut(entity) {
            transform.rotation = heading_to_render_rotation(msg.angle);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn on_movement_position_update(
    mut reader: MessageReader<MovementPositionUpdate>,
    origin: Res<WorldOrigin>,
    local: Res<LocalPlayer>,
    entities: Res<NetworkEntities>,
    mut player_commands: ResMut<PlayerCommands>,
    mut transforms: Query<&mut Transform>,
    mut remote_movement: Query<&mut RemoteMovement>,
    mut commands: Commands,
) {
    for msg in reader.read() {
        let Some(entity) = entities.get(msg.unique_id) else {
            continue;
        };
        let render = server_position_to_render(msg.region, msg.x, msg.y, msg.z, &origin);
        debug!(
            "network: MovementPositionUpdate uid={} region={:#06X} pos=({:.1},{:.1},{:.1}) -> render=({:.1},{:.1},{:.1})",
            msg.unique_id, msg.region, msg.x, msg.y, msg.z, render.x, render.y, render.z
        );
        if let Ok(mut transform) = transforms.get_mut(entity) {
            transform.translation = render;
            transform.rotation = heading_to_render_rotation(msg.heading);
        }
        if Some(msg.unique_id) == local.unique_id {
            // Cancel the optimistic local move so it doesn't walk back off the snap.
            // The player's own systems own its Y, so no ground-snap marker here.
            player_commands.stop();
        } else if let Ok(mut movement) = remote_movement.get_mut(entity) {
            movement.target = None;
            // Reconcile the teleported Y with terrain (the update's Y is unreliable).
            commands.entity(entity).insert(NeedsGroundSnap);
        }
    }
}

/// `EntitySpeedUpdate` (0x30D0): the server changed an entity's movement
/// speeds (buffs, GM speed command, mounts). Update its [`MovementSpeed`] and,
/// for remotes, the live [`RemoteMovement`] pace; the local player resolves
/// through the same [`NetworkEntities`] index (it carries a `NetworkId` too),
/// so one path covers both.
fn on_speed_update(
    mut reader: MessageReader<EntitySpeedUpdate>,
    entities: Res<NetworkEntities>,
    mut speeds: Query<Option<&mut MovementSpeed>>,
    mut remote_movement: Query<&mut RemoteMovement>,
    mut commands: Commands,
) {
    for msg in reader.read() {
        let Some(entity) = entities.get(msg.unique_id) else {
            // warn, not debug: a dropped speed update on the LOCAL player is
            // exactly how the post-teleport stale-index bug stayed invisible
            warn!(
                "network: EntitySpeedUpdate for unknown uid {} (walk={:.1} run={:.1}) — dropped",
                msg.unique_id, msg.walk_speed, msg.run_speed
            );
            continue;
        };
        info!(
            "network: EntitySpeedUpdate uid={} walk={:.1} run={:.1}",
            msg.unique_id, msg.walk_speed, msg.run_speed
        );
        let run = match speeds.get_mut(entity) {
            Ok(Some(mut speed)) => {
                speed.apply_update(msg.walk_speed, msg.run_speed);
                speed.run
            }
            Ok(None) => {
                let mut speed = MovementSpeed::DEFAULT;
                speed.apply_update(msg.walk_speed, msg.run_speed);
                commands.entity(entity).insert(speed);
                speed.run
            }
            Err(_) => continue,
        };
        if let Ok(mut movement) = remote_movement.get_mut(entity) {
            movement.speed = run;
        }
    }
}

/// `CelestialPosition` (0x3020): carries the local player's unique id and the
/// server clock. Store the id (mirroring it onto the `Player`) and set the
/// day-cycle time.
fn on_celestial_position(
    mut reader: MessageReader<CelestialPosition>,
    mut local: ResMut<LocalPlayer>,
    tod: Option<ResMut<TimeOfDay>>,
    player: Query<Entity, With<Player>>,
    mut commands: Commands,
) {
    let mut tod = tod;
    for msg in reader.read() {
        info!(
            "network: CelestialPosition unique_id={} time={:02}:{:02}",
            msg.unique_id, msg.hour, msg.minute
        );
        local.unique_id = Some(msg.unique_id);
        if let Ok(entity) = player.single() {
            commands.entity(entity).insert(NetworkId(msg.unique_id));
        }
        if let Some(tod) = tod.as_mut() {
            tod.t = time_of_day_fraction(msg.hour, msg.minute);
        }
    }
}

/// `CelestialUpdate` (0x3027): periodic re-sync of the day-cycle clock.
fn on_celestial_update(mut reader: MessageReader<CelestialUpdate>, tod: Option<ResMut<TimeOfDay>>) {
    let mut tod = tod;
    for msg in reader.read() {
        if let Some(tod) = tod.as_mut() {
            tod.t = time_of_day_fraction(msg.hour, msg.minute);
        }
    }
}

/// `CharacterData` (0x3013): buffer the raw body for
/// [`apply_pending_character_data`]. The spawn position is located by anchoring
/// on the player's unique id, which is embedded in this blob but at a
/// variable offset, so the reliable anchor is the id `CelestialPosition` (0x3020)
/// carries — and that packet can arrive before or after this one.
fn on_character_data(mut reader: MessageReader<CharacterDataBody>, mut local: ResMut<LocalPlayer>) {
    for msg in reader.read() {
        info!("network: CHARACTER_DATA received ({} bytes)", msg.raw.len());
        local.pending_character_data = Some(msg.raw.clone());
    }
}

/// Apply the buffered CHARACTER_DATA once it can be resolved: move the
/// already-spawned player to its server spawn and store the parsed character
/// record ([`CharacterInfo`]) and movement speeds ([`MovementSpeed`]) on it.
/// Preferred anchor is the unique id from `CelestialPosition` (robust); lacking
/// it, an *unambiguous* blob still yields the position and the id embedded next
/// to it. Either packet order works. Fail-safe: with the id known but no
/// position found, the player keeps its jangan fallback and we log a hexdump so
/// the exact layout can be pinned.
#[allow(clippy::too_many_arguments)]
fn apply_pending_character_data(
    mut origin: ResMut<WorldOrigin>,
    mut local: ResMut<LocalPlayer>,
    char_data: Res<ClientCharacterData>,
    item_data: Res<ClientItemData>,
    teleport: Res<crate::plugins::textdata::ClientTeleport>,
    mut player: Query<(Entity, &mut Transform, &mut NavLocation), With<Player>>,
    mut terrain: Query<&mut Transform, (With<Terrain>, Without<Player>)>,
    mut player_commands: ResMut<PlayerCommands>,
    mut commands: Commands,
) {
    let Some(raw) = local.pending_character_data.clone() else {
        return;
    };
    let known_id = local.unique_id;
    let resolver = TextdataResolver {
        char_data: &char_data,
        item_data: &item_data,
        teleport: &teleport,
    };
    let parsed = parse_character_info(&raw, known_id, &resolver);
    let Some(spawn) = parsed.spawn else {
        // With the authoritative id we still can't locate a position: a real
        // layout miss — warn and drop. Without an id the scan was just ambiguous,
        // so keep the buffer and wait for CelestialPosition to pin it.
        if known_id.is_some() {
            warn!(
                "network: could not locate spawn position in CHARACTER_DATA; keeping fallback. head: {}",
                hexdump(&raw, 64),
            );
            local.pending_character_data = None;
        }
        return;
    };
    info!(
        "network: CHARACTER_DATA parsed unique_id={} region={:#06X} pos=({:.1},{:.1},{:.1}) heading={} \
         fully_parsed={} ({}/{} bytes) name={:?} level={:?} hp={:?} gold={:?} items={:?} speeds={:?}",
        spawn.unique_id,
        spawn.region,
        spawn.x,
        spawn.y,
        spawn.z,
        spawn.heading,
        parsed.fully_parsed,
        parsed.forward_parsed_to,
        raw.len(),
        parsed.name,
        parsed.stats.map(|s| s.level),
        parsed.stats.map(|s| s.hp),
        parsed.stats.map(|s| s.gold),
        parsed.inventory.as_ref().map(|i| i.len()),
        parsed
            .state
            .as_ref()
            .map(|s| (s.walk_speed, s.run_speed, s.hwan_speed)),
    );
    if let Some(rec) = parsed.recovered_body {
        // The record's class is unresolvable (the server's item table has rows
        // this client's itemdata lacks), but its width was proven by replaying
        // the whole blob: say so, so the item's missing body stays traceable.
        info!(
            "network: CHARACTER_DATA resynced across unresolvable item ref_id={} \
             (body {} bytes, uninterpreted)",
            rec.ref_id, rec.body_len,
        );
    }
    if let Some(stage) = parsed.failed_stage {
        // A layout drift: log where the forward pass stopped so a live capture
        // can pin it (the anchor fallback already salvaged position/speeds).
        warn!(
            "network: CHARACTER_DATA forward parse stopped at stage '{}' (offset {} of {}). bytes there: {}",
            stage,
            parsed.forward_parsed_to,
            raw.len(),
            hexdump(&raw[parsed.forward_parsed_to.min(raw.len())..], 64),
        );
        if let Some(stop) = parsed.item_stop {
            // A short read, not a corrupt stream: the records before the stop
            // are kept (#455). Say how many survived and what stopped us.
            warn!(
                "network:   item section stopped at record {} (@{}, ref_id={}, {} bytes left); \
                 keeping the {} item(s) read before it",
                stop.index,
                stop.offset,
                stop.ref_id,
                raw.len().saturating_sub(parsed.forward_parsed_to),
                parsed.inventory.as_ref().map(|i| i.len()).unwrap_or(0),
            );
        }
        // Per-item read trace: the first Unknown-class record usually marks
        // where the stream drifted (the item before it read a mis-sized
        // body), so dump the bytes from one record earlier too.
        for (i, t) in parsed.item_trace.iter().enumerate() {
            warn!(
                "network:   item[{}] @{}..{} slot={} rent_type={} ref_id={} class={:?}",
                i, t.start_offset, t.end_offset, t.slot, t.rent_type, t.ref_id, t.class,
            );
        }
        if let Some(first_unknown) = parsed
            .item_trace
            .iter()
            .position(|t| t.class == ItemClass::Unknown)
        {
            let from = parsed.item_trace[first_unknown.saturating_sub(1)].start_offset;
            warn!(
                "network:   bytes from item[{}] (@{}): {}",
                first_unknown.saturating_sub(1),
                from,
                hexdump(&raw[from.min(raw.len())..], 96),
            );
        }
    }
    let speed = parsed
        .state
        .as_ref()
        .map(MovementSpeed::from_state)
        .unwrap_or(MovementSpeed::DEFAULT);
    // Re-anchor the world origin on the spawn BEFORE placing the player (ADR
    // 0006): CHARACTER_DATA also replays after a teleport (0x34B5 GameReset),
    // where the destination can be a whole map away — without the re-anchor
    // the player would land at huge render coordinates. At login the origin
    // already sits on the same region-grid snap, so the shift is a no-op.
    // A dungeon arrival (region bit 15) anchors unsnapped on the arrival
    // point itself: dungeon space has no 1920 grid (ADR-0006 amendment).
    let sro = server_position_to_sro(spawn.region, spawn.x, spawn.y, spawn.z);
    if spawn.region.is_dungeon() {
        set_dungeon_origin(sro, &mut origin, &mut terrain);
    } else {
        set_world_origin(sro, &mut origin, &mut terrain);
    }
    if let Ok((entity, mut transform, mut nav_location)) = player.single_mut() {
        transform.translation = origin.to_render(sro);
        // The server just teleported us: whatever surface we were tracking has
        // nothing to do with the new position.
        *nav_location = NavLocation::Unresolved;
        commands.entity(entity).insert((
            NetworkId(spawn.unique_id),
            CharacterInfo::from(&parsed),
            Inventory::from_character(&parsed),
            speed,
        ));
        // Logged in (or teleported) while mounted: re-attach to the transport
        // once its entity spawns (0x3013's transport pair, parsed but unused
        // before EP-19.1).
        if let Some(transport_id) = parsed.extras.as_ref().and_then(|e| e.transport_id) {
            info!("network: character data says we ride cos uid {transport_id}");
            commands
                .entity(entity)
                .insert(crate::plugins::cos::PendingMount(transport_id));
        }
    }
    // Drop any in-flight click-to-move so the player doesn't walk back
    // toward a pre-teleport target.
    player_commands.stop();
    // Adopt the id the blob carried if CelestialPosition hasn't supplied one, so
    // self-identification (despawn/movement filters) doesn't wait on it.
    if local.unique_id.is_none() {
        local.unique_id = Some(spawn.unique_id);
    }
    local.pending_character_data = None;
}

/// `TeleportResponse` (0xB05A): two-phase teleport ack. The success path
/// needs no action here — the world reset arrives as 0x34B5 and the arrival
/// position inside the CHARACTER_DATA replay (byte-identical for dungeon
/// destinations). Surfacing the phases and any unknown/error shape is the
/// point: an error code would otherwise strand the player in a silent
/// pre-teleport state.
fn on_teleport_response(mut reader: MessageReader<TeleportResponse>) {
    for response in reader.read() {
        match response {
            TeleportResponse::Begin { code: 1 } => {
                info!("network: teleport accepted, zone teardown starting")
            }
            TeleportResponse::Begin { code } => {
                warn!("network: teleport begin with unexpected code {code} — possibly refused")
            }
            TeleportResponse::Committed => info!("network: teleport committed, expecting 0x34B5"),
            TeleportResponse::Unknown { result, tail } => warn!(
                "network: unknown 0xB05A teleport response {result:#04x} tail {:?}",
                tail
            ),
        }
    }
}

/// `CharacterDataEnd` (0x34A6): the self-spawn stream is complete. Answer with
/// `GameReady` (0x3012) so the server proceeds, and unblock the loading overlay.
fn on_character_data_end(
    mut reader: MessageReader<CharacterDataEnd>,
    mut local: ResMut<LocalPlayer>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    let mut ended = false;
    for _ in reader.read() {
        ended = true;
    }
    if !ended {
        return;
    }
    info!("network: CHARACTER_DATA stream complete");
    local.stream_complete = true;

    if local.game_ready_sent {
        return;
    }
    let Ok(conn) = conn.single() else {
        warn!("network: no agent connection to send GameReady");
        return;
    };
    let frame = Packet::from(GameReady).into();
    if let Err(e) = conn.get_sender().send(frame) {
        error!("network: failed to send GameReady: {}", e.0);
        return;
    }
    info!("network: sent GameReady (0x3012)");
    local.game_ready_sent = true;
}

/// 0x34B5 SERVER_AGENT_GAME_RESET — the world reset after a committed
/// teleport. The server tears the zone down and then goes COMPLETELY silent
/// (captured: 41 s without even the 4 s HP tick) until the client answers
/// with 0x34B6 GAME_RESET_COMPLETE, after which it replays the whole
/// CHARACTER_DATA stream (0x34A5/0x3013/0x3020/0x34A6) and waits for a
/// SECOND GameReady. So: ack immediately, clear the character-data stream
/// state (`on_character_data_end`'s `game_ready_sent` guard would otherwise
/// swallow the second 0x3012 — the exact stall the first teleport playtest
/// hit), and sweep the remaining remote entities (the server only despawns a
/// handful explicitly before the reset; the rest are implicitly gone). The
/// replayed CHARACTER_DATA then repositions the player + re-anchors the
/// origin via [`apply_pending_character_data`].
///
/// The unique id MUST be cleared too: the server assigns a NEW id on every
/// teleport (dump-verified: 0x3020 carried 0x018B50 at login, 0x018B84
/// after the first teleport, 0x018BB8 after the next). Kept stale, the
/// position scan anchors on an id that no longer exists in the replayed
/// blob, fails, and discards it; cleared, `apply_pending_character_data`
/// waits for the replayed `CelestialPosition` to pin the fresh id.
fn on_game_reset(
    mut reader: MessageReader<GameReset>,
    mut local: ResMut<LocalPlayer>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    remotes: Query<Entity, With<RemoteEntity>>,
    mut commands: Commands,
) {
    for msg in reader.read() {
        info!(
            "network: GAME_RESET (0x34B5) — teleport arrival in region {:#06x}; resetting world",
            msg.region
        );
        local.unique_id = None;
        local.pending_character_data = None;
        local.stream_complete = false;
        local.game_ready_sent = false;
        let mut swept = 0;
        for entity in remotes.iter() {
            // The teleport's own despawn packets arrive in the same read, so
            // some of these are already queued for despawn (#430).
            commands.entity(entity).try_despawn();
            swept += 1;
        }
        info!("network: swept {swept} remote entities for the world reset");
        let Ok(conn) = conn.single() else {
            warn!("network: no agent connection to ack GAME_RESET");
            continue;
        };
        if let Err(e) = conn
            .get_sender()
            .send(Packet::from(GameResetComplete).into())
        {
            error!("network: failed to send GameResetComplete: {}", e.0);
        } else {
            info!("network: sent GameResetComplete (0x34B6)");
        }
    }
}

/// Decode the group spawn/despawn batches (0x3017 begin / 0x3019 data / 0x3018
/// end). Reads the ordered `Packet` stream rather than the fanned-out per-type
/// messages, because one network read can deliver several begin→data→end triples
/// in a single frame; processing them in wire order keeps each data packet paired
/// with its own begin's kind/count. Parsing is fail-safe (see `net::entity_spawn`),
/// so a malformed record just truncates that batch rather than crashing.
#[allow(clippy::too_many_arguments)]
fn on_group_spawn(
    mut packets: MessageReader<Packet>,
    // The active batch's (spawning, count), set by a begin until its end marker.
    // A `Local` so a batch split across frames stays paired.
    mut batch: Local<Option<(bool, u16)>>,
    origin: Res<WorldOrigin>,
    local: Res<LocalPlayer>,
    entities: Res<NetworkEntities>,
    char_data: Res<ClientCharacterData>,
    item_data: Res<ClientItemData>,
    rare_effects: Res<ClientRareEffects>,
    names: Res<ClientTextNames>,
    ui_strings: Res<crate::plugins::textdata::ClientUiStrings>,
    teleport: Res<crate::plugins::textdata::ClientTeleport>,
    asset_server: Res<AssetServer>,
    death_check: Query<(Has<Slain>, Option<&EntityVitals>, Has<Dying>)>,
    mut died: MessageWriter<EntityDied>,
    mut commands: Commands,
) {
    for packet in packets.read() {
        match packet {
            Packet::GroupEntitySpawnBegin(begin) => {
                *batch = Some((begin.kind == GROUP_SPAWN, begin.count));
            }
            Packet::GroupEntitySpawnEnd(_) => {
                *batch = None;
            }
            Packet::GroupEntitySpawnData(data) => {
                let Some((spawning, count)) = *batch else {
                    warn!("network: GroupEntitySpawnData without a preceding begin; ignoring");
                    continue;
                };
                let resolver = TextdataResolver {
                    char_data: &char_data,
                    item_data: &item_data,
                    teleport: &teleport,
                };
                let parsed = parse_group_spawn(&data.raw, spawning, count, &resolver);
                info!(
                    "network: group {} — {} spawns, {} despawns",
                    if spawning { "spawn" } else { "despawn" },
                    parsed.spawns.len(),
                    parsed.despawns.len(),
                );

                for uid in parsed.despawns {
                    if Some(uid) == local.unique_id {
                        continue; // never despawn the local player
                    }
                    if let Some(entity) = entities.get(uid) {
                        despawn_or_die(entity, &death_check, &mut died, &mut commands);
                    }
                }

                for entity in parsed.spawns {
                    // skip ourselves and anything already spawned
                    if Some(entity.unique_id) == local.unique_id
                        || entities.get(entity.unique_id).is_some()
                    {
                        continue;
                    }
                    spawn_remote_entity(
                        &mut commands,
                        &asset_server,
                        &char_data,
                        &item_data,
                        &rare_effects,
                        &names,
                        &ui_strings,
                        &teleport,
                        &origin,
                        entity,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Single entity spawn/despawn outside a group batch (0x3015 / 0x3016):
/// monster respawns, players walking into or out of range. A 0x3015 body is
/// exactly one group-spawn record, so it reuses [`parse_group_spawn`] with a
/// count of one and the same [`spawn_remote_entity`] path as the batches.
#[allow(clippy::too_many_arguments)]
fn on_single_spawn(
    mut spawns: MessageReader<SingleEntitySpawn>,
    mut despawns: MessageReader<SingleEntityDespawn>,
    origin: Res<WorldOrigin>,
    local: Res<LocalPlayer>,
    entities: Res<NetworkEntities>,
    char_data: Res<ClientCharacterData>,
    item_data: Res<ClientItemData>,
    rare_effects: Res<ClientRareEffects>,
    names: Res<ClientTextNames>,
    ui_strings: Res<crate::plugins::textdata::ClientUiStrings>,
    teleport: Res<crate::plugins::textdata::ClientTeleport>,
    asset_server: Res<AssetServer>,
    death_check: Query<(Has<Slain>, Option<&EntityVitals>, Has<Dying>)>,
    mut died: MessageWriter<EntityDied>,
    mut commands: Commands,
) {
    for msg in despawns.read() {
        if Some(msg.unique_id) == local.unique_id {
            continue; // never despawn the local player
        }
        if let Some(entity) = entities.get(msg.unique_id) {
            debug!("network: single despawn uid={}", msg.unique_id);
            despawn_or_die(entity, &death_check, &mut died, &mut commands);
        }
    }

    for msg in spawns.read() {
        let resolver = TextdataResolver {
            char_data: &char_data,
            item_data: &item_data,
            teleport: &teleport,
        };
        let parsed = parse_group_spawn(&msg.raw, true, 1, &resolver);
        if parsed.spawns.is_empty() {
            // unknown ref id or a record-layout mismatch — keep the evidence
            warn!(
                "network: single spawn parsed to nothing — capture: {}",
                hexdump(&msg.raw, 96)
            );
        }
        for entity in parsed.spawns {
            // skip ourselves and anything already spawned
            if Some(entity.unique_id) == local.unique_id || entities.get(entity.unique_id).is_some()
            {
                continue;
            }
            debug!(
                "network: single spawn uid={} ref={}",
                entity.unique_id, entity.ref_id
            );
            spawn_remote_entity(
                &mut commands,
                &asset_server,
                &char_data,
                &item_data,
                &rare_effects,
                &names,
                &ui_strings,
                &teleport,
                &origin,
                entity,
            );
        }
    }
}

/// Ground item drops sit flush with the terrain, so their meshes cast a hard
/// contact shadow that reads as a black patch under the pile (worse at
/// grazing sun angles). Vanilla drops don't cast shadows — mark item meshes
/// `NotShadowCaster` as they stream in (mirrors `tag_hoverable_entities`).
fn mark_item_drops_no_shadow(
    items: Query<(Entity, &RemoteEntity)>,
    children: Query<&Children>,
    meshes: Query<
        (),
        (
            With<bevy::mesh::Mesh3d>,
            Without<bevy::light::NotShadowCaster>,
        ),
    >,
    mut commands: Commands,
) {
    for (root, kind) in items.iter() {
        if !matches!(kind, RemoteEntity::Item) {
            continue;
        }
        for entity in children.iter_descendants(root) {
            if meshes.contains(entity) {
                commands.entity(entity).insert(bevy::light::NotShadowCaster);
            }
        }
    }
}

/// Route a server despawn: entities a killing blow was seen on (or whose
/// optimistic HP hit zero) linger as corpses — [`EntityDied`] starts the
/// death animation and the `Dying` timer despawns them later; everything
/// else (walked out of range) vanishes immediately as before.
///
/// `try_despawn` rather than `despawn` (#430): a live server sends overlapping
/// despawns — the same unique id in two group batches, or in a group batch and
/// a `0x3016` single despawn — that one network read delivers in a single
/// frame. `NetworkEntities` only drops the id when the despawn *applies* (its
/// `NetworkId` lifecycle hook), and the `Dying` marker of the corpse branch is
/// inserted just as late, so every duplicate in that frame still resolves to
/// the live entity and queues a second despawn. That second command is
/// expected and harmless — the entity is meant to be gone — but the default
/// handler reported each one as an ECS error (59 in one 15-minute session,
/// in bursts of 8 within the same millisecond). Silencing it here keeps the
/// error channel meaningful; it is not a way to hide a wrong despawn, since
/// the first one already did exactly what the server asked.
fn despawn_or_die(
    entity: Entity,
    death_check: &Query<(Has<Slain>, Option<&EntityVitals>, Has<Dying>)>,
    died: &mut MessageWriter<EntityDied>,
    commands: &mut Commands,
) {
    let Ok((slain, vitals, dying)) = death_check.get(entity) else {
        commands.entity(entity).try_despawn();
        return;
    };
    if dying {
        return; // already a lingering corpse
    }
    if slain || vitals.is_some_and(|v| v.hp == 0) {
        died.write(EntityDied(entity));
    } else {
        commands.entity(entity).try_despawn();
    }
}

/// Build a Bevy entity for a parsed remote spawn: the model (from characterdata /
/// itemdata) at the server position, tagged with a [`RemoteEntity`] marker and a
/// [`NetworkId`] so later per-entity packets resolve to it. Players also attach
/// their equipment and get a [`RemoteMovement`] so `MovementResponse` can walk
/// them.
#[allow(clippy::too_many_arguments)]
fn spawn_remote_entity(
    commands: &mut Commands,
    asset_server: &AssetServer,
    char_data: &ClientCharacterData,
    item_data: &ClientItemData,
    rare_effects: &ClientRareEffects,
    names: &ClientTextNames,
    ui_strings: &crate::plugins::textdata::ClientUiStrings,
    teleport: &crate::plugins::textdata::ClientTeleport,
    origin: &WorldOrigin,
    entity: SpawnedEntity,
) {
    let render = server_position_to_render(
        entity.position.region,
        entity.position.x,
        entity.position.y,
        entity.position.z,
        origin,
    );
    // Every SRO resource is placed under the LH -> RH handedness mirror
    // (`util::mesh`): map objects, dungeon props, the char-select previews and
    // every test scene already are. The in-world spawn paths were the ones
    // that migration missed, so remote players, NPCs, monsters and ground
    // items all rendered geometrically mirrored — weapon in the wrong hand,
    // asymmetric armour on the wrong side. One mirror here covers every branch
    // below, since they all place from this transform.
    let transform = mirrored(
        Transform::from_translation(render)
            .with_rotation(heading_to_render_rotation(entity.position.heading)),
    );
    // Constant across the branches: the mirror is what makes the determinant
    // negative, and the monster branch below only scales magnitudes.
    let mirror_winding = needs_winding_reversal(&transform.to_matrix());

    // Movement speeds from the spawn record's state block; items carry none.
    let speed = entity
        .state
        .as_ref()
        .map(MovementSpeed::from_state)
        .unwrap_or(MovementSpeed::DEFAULT);
    let movement = RemoteMovement {
        target: None,
        speed: speed.current(),
        // The spawn record's state block already carries the gait the server
        // has this entity in, so a monster that is strolling when it comes
        // into view starts on its walk clip instead of snapping to run (#275).
        walking: entity
            .state
            .as_ref()
            .is_some_and(|state| state.motion_state == MOTION_STATE_WALK),
    };

    match &entity.kind {
        SpawnKind::Player {
            equipment,
            riding_uid,
        } => {
            let Some(row) = char_data.get(&(entity.ref_id as i32)) else {
                warn!(
                    "network: no characterdata for remote player ref {}",
                    entity.ref_id
                );
                return;
            };
            let name = entity
                .name
                .clone()
                .unwrap_or_else(|| format!("player {}", entity.unique_id));
            let mut parent_cmd = commands.spawn((
                Name::from(name.clone()),
                DisplayName(name),
                RemoteEntity::Player,
                NetworkId(entity.unique_id),
                movement,
                speed,
                // Reconcile Y with the walkable surface once the region's nav
                // mesh has loaded (the server Y floats/sinks the entity).
                NeedsGroundSnap,
                // default stance; a weapon in the equipment overrides it
                PreferredAnimationGroup("sword".to_string()),
                transform,
                Visibility::default(),
            ));
            if mirror_winding {
                parent_cmd.insert(MirroredResource);
            }
            if let Some(path) = row.resource_path() {
                parent_cmd.insert(UnloadedResource(asset_server.load(path)));
            }
            // guilded players only — the nameplate's guild line
            if let Some(guild) = entity.guild.clone() {
                parent_cmd.insert(guild);
            }
            // The body state the character already carries when they come into
            // view. Without this only somebody who toggles invisibility *while
            // on screen* would render right; anyone already invisible when they
            // walked into range would spawn solid. The state block is the same
            // one CHARACTER_DATA uses, and has always been parsed.
            if let Some(body_state) = entity.state.as_ref().map(|state| state.body_state) {
                parent_cmd.insert(crate::plugins::net::entities::RemoteBodyState(body_state));
            }
            // Mounted players attach to their COS once it exists (its record
            // may come later in the same batch).
            if let Some(cos_uid) = riding_uid {
                parent_cmd.insert(crate::plugins::cos::PendingMount(*cos_uid));
            }
            let parent = parent_cmd.id();
            attach_character_equipment(
                commands,
                asset_server,
                item_data,
                rare_effects,
                parent,
                equipment.iter().copied(),
            );
        }
        SpawnKind::Structure => {
            // Teleport gate buildings have no characterdata rows (their
            // visuals are static map objects) but spawn as interactable
            // entities: create an invisible click anchor so the stone opens
            // the teleport dialog like an NPC. The dialog resolves speech via
            // the building codename (`ClientTeleport::owner_codename`).
            //
            // No `MirroredResource` here on purpose: this anchor loads no
            // resource of its own, so there is no mesh whose winding could
            // need reversing — the gate's visible stone is a map object, and
            // `map::objects` already mirrors that.
            let display = teleport
                .owner_name_key(entity.ref_id as i32)
                .and_then(|key| names.name(key))
                .unwrap_or("Teleport Gate")
                .to_string();
            commands.spawn((
                Name::from(format!("gate {}", entity.unique_id)),
                DisplayName(display),
                RemoteEntity::Npc,
                NetworkId(entity.unique_id),
                CharacterRef(entity.ref_id),
                crate::plugins::net::entities::GateVolumeNeeded,
                movement,
                speed,
                NeedsGroundSnap,
                transform,
                Visibility::default(),
            ));
        }
        SpawnKind::Npc | SpawnKind::Monster => {
            let Some(row) = char_data.get(&(entity.ref_id as i32)) else {
                warn!(
                    "network: no characterdata for remote entity ref {}",
                    entity.ref_id
                );
                return;
            };
            let (marker, label) = if matches!(entity.kind, SpawnKind::Monster) {
                (RemoteEntity::Monster, "monster")
            } else {
                (RemoteEntity::Npc, "npc")
            };
            // Localized display name from characterdata (SN_* key → textdataname),
            // falling back to the code name so the nameplate always shows something.
            let display = row
                .name_key()
                .and_then(|key| names.name(key))
                .map(str::to_string)
                .unwrap_or_else(|| row.code_name().clone());
            // Rarity is per-INSTANCE: the spawn packet byte (a normal mob can
            // spawn as a Giant/Champion variant); characterdata's column is
            // only the base-class fallback. Class 3 = unique (world boss,
            // minimap sign); bit 0x10 = party mob. Giants & co. render as the
            // base model scaled up.
            let rarity = MonsterRarity(entity.spawn_rarity.unwrap_or_else(|| row.rarity()));
            let mut transform = transform;
            if matches!(marker, RemoteEntity::Monster) {
                // Multiply rather than assign: the base scale carries the X
                // mirror, and overwriting it with a positive splat would flip
                // giants and champions back to the wrong handedness.
                transform.scale *= rarity.scale();
            }
            let mut cmd = commands.spawn((
                Name::from(format!("{} {}", label, entity.unique_id)),
                DisplayName(display),
                marker,
                NetworkId(entity.unique_id),
                CharacterRef(entity.ref_id),
                movement,
                speed,
                NeedsGroundSnap,
                transform,
                Visibility::default(),
            ));
            // Summon clones reuse their base object's model (their own path is
            // `xxx`), resolved via characterdata's OrgObjCode; truly modelless
            // helpers spawn indexed but invisible rather than loading `res/xxx`.
            if mirror_winding {
                cmd.insert(MirroredResource);
            }
            if let Some(path) = char_data.model_path(row) {
                cmd.insert(UnloadedResource(asset_server.load(path)));
                // A variant reusing a base model wears the base's `_clone`
                // material recolor (falls back to base if the model has none).
                if row.resource_path().is_none() {
                    cmd.insert(crate::commands::MaterialVariant::Clone);
                }
            }
            // Buffs the entity already carries when it comes into view. The
            // spawn record has always parsed them (`net/reader.rs`'s
            // character-state block); nothing kept them until the target
            // window's buff row (#636) needed them. Only inserted when the
            // list is non-empty, so an unbuffed entity carries no component.
            if let Some(buffs) = entity
                .state
                .as_ref()
                .map(|state| &state.buffs)
                .filter(|buffs| !buffs.is_empty())
            {
                cmd.insert(crate::plugins::net::entities::RemoteBuffs(
                    buffs.iter().map(|buff| buff.ref_skill_id).collect(),
                ));
            }
            if matches!(marker, RemoteEntity::Npc) && !entity.talk_options.is_empty() {
                cmd.insert(crate::plugins::net::entities::NpcTalkOptions(
                    entity.talk_options.clone(),
                ));
            }
            if matches!(marker, RemoteEntity::Monster) {
                cmd.insert(rarity);
                if rarity.kind() == 3 {
                    cmd.insert(UniqueMonster);
                }
                // Champions render with the `_champ.bmt` recolor their .bsr
                // ships (mobs without one fall back to the base look).
                if rarity.kind() == 1 {
                    cmd.insert(crate::commands::MaterialVariant::Champion);
                }
                // Live HP bar: full at spawn, updated by EntityBarsUpdate.
                if let Some(max_hp) = row.max_hp() {
                    cmd.insert(EntityVitals::full(max_hp));
                }
            }
        }
        SpawnKind::Cos {
            kind,
            pet_name,
            owner_name,
            owner_uid,
        } => {
            if crate::plugins::cos::spawn::spawn_cos_entity(
                commands,
                asset_server,
                char_data,
                Some(names),
                Some(ui_strings),
                crate::plugins::cos::spawn::CosSpawnParams {
                    ref_id: entity.ref_id,
                    unique_id: entity.unique_id,
                    kind: *kind,
                    pet_name: pet_name.clone(),
                    owner_name: owner_name.clone(),
                    owner_uid: *owner_uid,
                    transform,
                    movement,
                    speed,
                },
            )
            .is_none()
            {
                warn!("network: no characterdata for COS ref {}", entity.ref_id);
            }
        }
        SpawnKind::Item { amount } => {
            // Items don't move, so no RemoteMovement. Ground drops render the
            // itemdata *drop* model (gold piles / category drop bags) rather
            // than the inventory model, which is "xxx" for etc items; when a
            // row has neither, still index it (bare marker) for pickup.
            let row = item_data.get(&(entity.ref_id as i32));
            let resource =
                row.and_then(|row| row.drop_resource_path().or_else(|| row.resource_path()));
            // Hover label: gold piles show their amount, items their name.
            let display = match row {
                Some(row) if row.is_gold() => format!("{} Gold", amount.unwrap_or(0)),
                Some(row) => row
                    .name_key()
                    .and_then(|key| names.name(key))
                    .map(str::to_string)
                    .unwrap_or_else(|| row.code_name().clone()),
                None => format!("item {}", entity.ref_id),
            };
            let mut cmd = commands.spawn((
                Name::from(format!("item {}", entity.unique_id)),
                RemoteEntity::Item,
                NetworkId(entity.unique_id),
                DisplayName(display),
                // The spawn record's rarity byte, the only wire source for the
                // hover label's colour (a drop has no item body — see
                // [`DropRarity`]).
                DropRarity(entity.spawn_rarity.unwrap_or(0)),
                NeedsGroundSnap,
                transform,
                Visibility::default(),
            ));
            if mirror_winding {
                cmd.insert(MirroredResource);
            }
            if let Some(path) = resource {
                cmd.insert(UnloadedResource(asset_server.load(path)));
            }
            let item_entity = cmd.id();
            drop(cmd);
            // Rare ("Seal of …") drops carry a looping sparkle/light pillar,
            // and their hover label reads gold — one decision point for both.
            if let Some(row) = row.filter(|row| row.is_rare()) {
                commands.entity(item_entity).insert(SealDrop);
                commands.attach_effect(
                    asset_server.load(rare::drop_effect_path(row)),
                    item_entity,
                    Transform::IDENTITY,
                );
            }
        }
    }
}

/// Despawn the game scene's camera, player and loading overlay, and drop the
/// join-flow resources, so a later re-entry starts clean.
fn cleanup_game_scene(
    mut commands: Commands,
    cameras: Query<Entity, With<PlayerCamera>>,
    players: Query<Entity, With<Player>>,
    overlay: Query<Entity, With<GameLoadingOverlay>>,
    remotes: Query<Entity, With<RemoteEntity>>,
) {
    for entity in cameras
        .iter()
        .chain(players.iter())
        .chain(overlay.iter())
        .chain(remotes.iter())
    {
        // The chained sets can overlap, and `despawn` is recursive over
        // children, so an entity may already be queued for despawn (#430).
        commands.entity(entity).try_despawn();
    }
    commands.remove_resource::<JoiningCharacter>();
    commands.remove_resource::<PendingWorldJoin>();
}

#[cfg(test)]
mod test {
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::plugins::map::terrain::TerrainId;

    /// #628: the world-entry overlay used to invent an English caption
    /// string exactly where the original bakes the 144x20 `nowloading.ddj`
    /// art, and drew no frame at all. It now goes through
    /// `loading_screen::spawn_loading_chrome`, so pin both halves: the overlay
    /// is art only, and it carries the authored chrome.
    #[test]
    fn the_world_entry_overlay_is_art_only_and_carries_the_shared_chrome() {
        let mut app = App::new();
        // TaskPoolPlugin first: the overlay's `asset_server.load()` calls
        // spawn IO tasks, and without the pools bevy_tasks panics before any
        // assertion here can run.
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<Image>();
        app.world_mut().spawn(Camera2d);
        app.world_mut()
            .run_system_once(spawn_game_loading_overlay)
            .expect("spawn_game_loading_overlay failed");

        let mut overlays = app
            .world_mut()
            .query_filtered::<Entity, With<GameLoadingOverlay>>();
        let root = overlays.iter(app.world()).next().expect("no overlay");

        let mut images = 0usize;
        let mut stack = vec![root];
        while let Some(entity) = stack.pop() {
            assert!(
                app.world().get::<Text>(entity).is_none(),
                "the overlay substitutes text for the nowloading art"
            );
            if app.world().get::<ImageNode>(entity).is_some() {
                images += 1;
            }
            if let Some(children) = app.world().get::<Children>(entity) {
                stack.extend(children.iter());
            }
        }
        // background + loading_form frame + gauge fill + nowloading caption
        assert_eq!(images, 4, "overlay art parts");
    }

    /// Boots just enough of an app to hold a real world-entry overlay: the
    /// asset pools the chrome's `load()` calls need, a `Camera2d` for
    /// `UiTargetCamera`, and the two resources the dismissal reads.
    fn overlay_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<Image>()
        .init_resource::<Time>()
        .init_resource::<LocalPlayer>();
        app.world_mut().spawn(Camera2d);
        app.world_mut()
            .run_system_once(spawn_game_loading_overlay)
            .expect("spawn_game_loading_overlay failed");
        app
    }

    /// The two non-terrain gates, satisfied: a player whose body wrapper has
    /// streamed in, and a completed self-spawn stream (with no
    /// `AgentConnection` spawned, `stream_ready` would hold anyway).
    fn satisfy_player_and_stream(app: &mut App) {
        let player = app.world_mut().spawn(Player::new()).id();
        app.world_mut()
            .spawn((SpawnedFromResource(Handle::default()), ChildOf(player)));
        app.world_mut()
            .resource_mut::<LocalPlayer>()
            .stream_complete = true;
    }

    /// Spawn `n` regions in the shape `load_terrain_objects_system` leaves them
    /// in once they finish: the load state *removed*, not `Completed`.
    fn spawn_finished_regions(app: &mut App, n: u16) {
        for i in 0..n {
            app.world_mut().spawn(Terrain(TerrainId(i)));
        }
    }

    fn run_one_frame(app: &mut App, delta: Duration) {
        app.world_mut().resource_mut::<Time>().advance_by(delta);
        app.world_mut()
            .run_system_once(dismiss_loading_overlay_when_ready)
            .expect("dismiss_loading_overlay_when_ready failed");
    }

    fn overlay_is_up(app: &mut App) -> bool {
        let mut overlays = app
            .world_mut()
            .query_filtered::<Entity, With<GameLoadingOverlay>>();
        overlays.iter(app.world()).next().is_some()
    }

    /// Width of the gauge fill, as a percentage.
    fn gauge_percent(app: &mut App) -> f32 {
        let mut fills = app
            .world_mut()
            .query_filtered::<&Node, With<LoadingProgress>>();
        let widths: Vec<Val> = fills.iter(app.world()).map(|node| node.width).collect();
        assert_eq!(widths.len(), 1, "exactly one gauge fill in the scene");
        match widths[0] {
            Val::Percent(p) => p,
            other => panic!("the gauge fill is not sized in percent: {other:?}"),
        }
    }

    /// The regression. A region that has finished carries **no**
    /// `TerrainLoadState` at all — `load_terrain_objects_system` removes it one
    /// poll after `Completed` to disarm the per-frame polling — and it loses
    /// `PreloadedTerrain` even earlier, on the first `load_terrain_dynamically`
    /// run. The old check filtered on both, so its query emptied within a
    /// couple of frames of scene entry and `terrain_ready` could never be true
    /// again: every world entry sat behind the overlay for the full 30 s
    /// safety net with all its packets long since arrived.
    #[test]
    fn a_finished_region_that_dropped_its_load_state_counts_as_ready() {
        let mut app = overlay_app();
        satisfy_player_and_stream(&mut app);
        spawn_finished_regions(&mut app, 4);

        run_one_frame(&mut app, Duration::from_millis(16));
        assert!(
            !overlay_is_up(&mut app),
            "the overlay is still up after every region finished"
        );

        // control: one region still building holds it up, on a fresh app so the
        // 16 ms above cannot be confused for the safety net
        let mut app = overlay_app();
        satisfy_player_and_stream(&mut app);
        spawn_finished_regions(&mut app, 3);
        app.world_mut().spawn((
            Terrain(TerrainId(9)),
            TerrainLoadState::BuildingMeshes {
                next_group: 0,
                group_size: 6,
            },
        ));
        run_one_frame(&mut app, Duration::from_millis(16));
        assert!(
            overlay_is_up(&mut app),
            "the overlay lifted while a region was still building"
        );
    }

    /// No terrain spawned at all is the *opposite* of ready — it means
    /// `preload_starting_area` has not run yet, or bailed on an unresolved
    /// `.mfo`. Only the safety net may lift the overlay then.
    #[test]
    fn an_empty_world_is_not_ready() {
        let mut app = overlay_app();
        satisfy_player_and_stream(&mut app);

        run_one_frame(&mut app, Duration::from_millis(16));
        assert!(overlay_is_up(&mut app), "the overlay lifted onto no world");

        run_one_frame(&mut app, Duration::from_secs_f32(MAX_WAIT_SECS));
        assert!(!overlay_is_up(&mut app), "the safety net did not fire");
    }

    /// The timer used to be a `Local<f32>`, which survives the scene exit. Once
    /// the first entry had left it at the 30 s cap, the *next* entry's overlay
    /// was dismissed on its very first frame, dropping the player into an
    /// unbuilt world. It now lives on the overlay entity, which is spawned
    /// fresh on entry and despawned by `cleanup_game_scene`.
    #[test]
    fn a_second_world_entry_does_not_inherit_the_first_entrys_timer() {
        let mut app = overlay_app();
        // nothing is ready, so this runs out the whole safety net
        run_one_frame(&mut app, Duration::from_secs_f32(MAX_WAIT_SECS + 1.0));
        assert!(!overlay_is_up(&mut app), "the safety net did not fire");

        // a second entry into the scene: a fresh overlay, same app
        app.world_mut()
            .run_system_once(spawn_game_loading_overlay)
            .expect("spawn_game_loading_overlay failed");
        run_one_frame(&mut app, Duration::from_millis(16));
        assert!(
            overlay_is_up(&mut app),
            "the second entry's overlay inherited the first entry's timer"
        );
    }

    /// The gauge reports the three gates the dismissal waits on: the
    /// self-spawn stream, the player's body, and the terrain's own completion
    /// fraction (the one that takes real time).
    #[test]
    fn the_gauge_fills_from_the_readiness_gates() {
        let mut app = overlay_app();
        assert_eq!(gauge_percent(&mut app), 0.0, "the gauge starts empty");

        // stream ready (no agent connection), no player body, 2 of 4 regions
        // done -> (1 + 0 + 0.5) / 3
        spawn_finished_regions(&mut app, 2);
        for i in 0..2 {
            app.world_mut()
                .spawn((Terrain(TerrainId(100 + i)), TerrainLoadState::LoadedMeshes));
        }
        run_one_frame(&mut app, Duration::from_millis(16));
        let filled = gauge_percent(&mut app);
        assert!(
            (filled - 50.0).abs() < 0.01,
            "gauge at {filled}%, expected 50%"
        );
        assert!(
            overlay_is_up(&mut app),
            "the overlay lifted at half progress"
        );
    }

    /// `rearm_object_passes_on_region_unload` puts completed regions back to
    /// `LoadedMeshes` whenever a neighbour unloads — which happens during the
    /// world-origin re-anchor onto the server's spawn. That is a dip in the
    /// completion count, and a gauge that walks backwards reads as broken, so
    /// the overlay keeps a high-water mark.
    #[test]
    fn the_gauge_does_not_walk_backwards_when_a_region_is_re_armed() {
        let mut app = overlay_app();
        // the player body never arrives, so the overlay stays up throughout
        app.world_mut().spawn(Player::new());
        spawn_finished_regions(&mut app, 2);
        let rearmable = app.world_mut().spawn(Terrain(TerrainId(200))).id();

        run_one_frame(&mut app, Duration::from_millis(16));
        let high_water = gauge_percent(&mut app);
        // stream + terrain, no player body -> 2/3
        assert!(
            (high_water - 200.0 / 3.0).abs() < 0.01,
            "gauge at {high_water}%, expected 66.7%"
        );

        app.world_mut()
            .entity_mut(rearmable)
            .insert(TerrainLoadState::LoadedMeshes);
        run_one_frame(&mut app, Duration::from_millis(16));
        assert_eq!(
            gauge_percent(&mut app),
            high_water,
            "the gauge walked backwards on a re-armed region"
        );
    }

    #[derive(Resource)]
    struct DespawnTarget(Entity);

    /// Bevy reports a failed command through the `log` crate
    /// (`bevy_ecs::error::handler::warn` -> `log::warn!`), NOT through
    /// `tracing` directly — a tracing-only collector sees nothing, which is
    /// what made the first attempt at this test a false green. So capture the
    /// `log` records themselves. No other test in this binary installs a
    /// logger (`LogPlugin` is only wired in `netcheck`/`main`).
    struct CapturedLog;

    static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static LOGGER: OnceLock<()> = OnceLock::new();

    impl log::Log for CapturedLog {
        fn enabled(&self, _metadata: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            let message = record.args().to_string();
            if message.contains("Encountered an error in command") {
                CAPTURED.lock().expect("capture poisoned").push(message);
            }
        }
        fn flush(&self) {}
    }

    fn captured_command_errors(run: impl FnOnce()) -> Vec<String> {
        LOGGER.get_or_init(|| {
            log::set_boxed_logger(Box::new(CapturedLog)).expect("no other logger in this binary");
            log::set_max_level(log::LevelFilter::Trace);
        });
        CAPTURED.lock().expect("capture poisoned").clear();
        run();
        CAPTURED.lock().expect("capture poisoned").clone()
    }

    /// Ask [`despawn_or_die`] to remove the same entity twice in one frame —
    /// exactly what the live server produces when a unique id shows up in two
    /// group despawn batches, or in a batch and a `0x3016`, inside one network
    /// read (#430: 59 such errors in a 15-minute session, in bursts of 8 in
    /// the same millisecond). The first half of the test is the control: the
    /// plain `despawn` reports that duplicate, which is the error the issue
    /// counted. The second half is the fix: through `despawn_or_die` the same
    /// duplicate reports nothing, and the entity is still gone.
    ///
    /// Both halves live in one test because the log capture is process-global.
    /// `run_system_once` applies the commands inline, so the capture is not
    /// racing a frame.
    #[test]
    fn a_repeated_server_despawn_is_not_reported_as_an_error() {
        fn plain_despawn_twice(target: Res<DespawnTarget>, mut commands: Commands) {
            commands.entity(target.0).despawn();
            commands.entity(target.0).despawn();
        }
        fn despawn_twice(
            target: Res<DespawnTarget>,
            death_check: Query<(Has<Slain>, Option<&EntityVitals>, Has<Dying>)>,
            mut died: MessageWriter<EntityDied>,
            mut commands: Commands,
        ) {
            despawn_or_die(target.0, &death_check, &mut died, &mut commands);
            despawn_or_die(target.0, &death_check, &mut died, &mut commands);
        }

        let mut world = World::new();
        let entity = world.spawn_empty().id();
        world.insert_resource(DespawnTarget(entity));
        let control = captured_command_errors(|| {
            world
                .run_system_once(plain_despawn_twice)
                .expect("system runs and its commands are applied");
        });
        assert_eq!(
            control.len(),
            1,
            "control: the plain despawn reports the duplicate — {control:?}"
        );

        let mut world = World::new();
        world.init_resource::<Messages<EntityDied>>();
        let entity = world.spawn_empty().id();
        world.insert_resource(DespawnTarget(entity));
        let reported = captured_command_errors(|| {
            world
                .run_system_once(despawn_twice)
                .expect("system runs and its commands are applied");
        });

        assert!(
            world.get_entity(entity).is_err(),
            "the entity is gone after the first despawn"
        );
        assert!(
            reported.is_empty(),
            "the duplicate despawn was reported: {reported:?}"
        );
    }

    /// The flat 80 this classifier replaced, reproduced exactly for the
    /// unarmed/melee reach — the change is meant to generalise the rule, not
    /// to move the melee case.
    #[test]
    fn the_melee_classification_radius_is_unchanged() {
        use crate::plugins::combat::ATTACK_GAP_STOP;

        assert!(is_attack_approach(79.9, ATTACK_GAP_STOP));
        assert!(!is_attack_approach(80.1, ATTACK_GAP_STOP));
    }

    /// The defect: a bow approach stops ~160 out, so a flat 80 called it
    /// ordinary travel — neither projecting it onto our line to the monster
    /// nor letting the "already inside the ring, hold position" branch run,
    /// which is the branch that actually parks a bow user.
    #[test]
    fn a_bow_approach_is_classified_as_an_approach() {
        use crate::plugins::combat::ATTACK_GAP_STOP;

        assert!(is_attack_approach(170.0, 180.0));
        assert!(
            !is_attack_approach(170.0, ATTACK_GAP_STOP),
            "this is what it used to do"
        );
    }

    #[test]
    fn distant_travel_during_an_engagement_is_still_travel() {
        assert!(!is_attack_approach(400.0, 180.0));
    }
}
