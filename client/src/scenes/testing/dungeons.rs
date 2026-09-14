//! Dungeon test scene: spawn a playable character inside a dungeon interior
//! and exercise the whole offline dungeon pipeline — DOF loading, block/prop
//! rendering, per-block lights/fog, portal culling and (with the dungeon nav
//! runtime) click-to-move on the room nav meshes.
//!
//! Idea: the skills scene's character recipe joined with the dungeon
//! plugin's [`EnterDungeon`] flow. The scene itself owns almost nothing: it
//! spawns the player + camera and sends one `EnterDungeon` for the default
//! dungeon (Donwhang cave at its real teleportdata arrival point); the
//! `plugins/dungeon` runtime does the rest, so entering from the world scene
//! behaves identically. The egui window teleports across every
//! `dungeoninfo.txt` dungeon. Run with `SCENE=dungeons` or
//! `scenes.startup: dungeons`.

use bevy::prelude::*;
use bevy_inspector_egui::bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use bevy_inspector_egui::egui;

use crate::plugins::camera::spawn_player_camera;
use crate::plugins::cursor::interactions::GameCursorTarget;
use crate::plugins::dungeon::{ActiveDungeon, EnterDungeon};
use crate::plugins::dynamic_resource_loader::{
    MirroredResource, PendingItemAttachment, PreferredAnimationGroup, UnloadedResource,
};
use crate::plugins::map::objects::{SroBindPoses, SroMeshes};
use crate::plugins::net::entities::DisplayName;
use crate::plugins::player::Player;
use crate::plugins::textdata::ClientDungeonInfo;
use crate::scenes::testing::equipments::{ARMOR_PIECES, RACES};
use crate::scenes::SceneState;
use crate::util::mesh::needs_winding_reversal;
use crate::GameState;

pub struct DungeonsScenePlugin;

impl Plugin for DungeonsScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DungeonsSceneState>()
            // PlayMode so the PlayerCamera is the active mesh-picking camera
            // (click-to-move needs it).
            .add_systems(OnEnter(SceneState::Dungeons), enter_play_mode)
            .add_systems(
                OnEnter(GameState::Game),
                (
                    spawn_player_camera,
                    spawn_dungeons_scene,
                    crate::plugins::hud::minimap::spawn_minimap,
                )
                    .chain()
                    .run_if(in_state(SceneState::Dungeons)),
            )
            .add_systems(
                EguiPrimaryContextPass,
                dungeons_scene_window
                    .run_if(in_state(SceneState::Dungeons))
                    .run_if(crate::plugins::dev::dev_windows_visible),
            );
    }
}

/// Default dungeon: Donwhang Stone Cave (`dungeoninfo.txt` id 1).
const DEFAULT_REGION: u16 = 0x8001;
/// Its in-dungeon exit-gate spot — teleportdata id 10 `GATE_DUNGEON_DH_OUT`,
/// region -32767, raw dungeon-local (1011, 0, -862): a guaranteed-walkable
/// arrival, the same place the real teleport drops you.
const DEFAULT_ARRIVAL: Vec3 = Vec3::new(1011.0, 0.0, -862.0);

#[derive(Resource, Default)]
struct DungeonsSceneState {
    spawned_character: Option<Entity>,
}

fn enter_play_mode(mut next_mode: ResMut<NextState<crate::AppMode>>) {
    next_mode.set(crate::AppMode::PlayMode);
}

fn spawn_dungeons_scene(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut state: ResMut<DungeonsSceneState>,
    mut enter: MessageWriter<EnterDungeon>,
) {
    commands.init_resource::<SroMeshes>();
    commands.init_resource::<SroBindPoses>();
    commands.insert_resource(GlobalAmbientLight {
        brightness: 250.0,
        color: Color::WHITE,
        ..default()
    });

    spawn_test_character(&mut commands, &asset_server, &mut state);
    enter.write(EnterDungeon {
        region_id: DEFAULT_REGION,
        arrival: Some(DEFAULT_ARRIVAL),
    });
}

/// The skills-scene character recipe, fixed to the first race/armor/weapon —
/// this scene tests the dungeon, not the wardrobe.
fn spawn_test_character(
    commands: &mut Commands,
    asset_server: &AssetServer,
    state: &mut DungeonsSceneState,
) {
    let race = &RACES[0];
    let armor = &race.armor[0];
    let weapon = &race.weapons[0];

    let transform = Transform::default()
        .with_scale(Vec3::new(-1.0, 1.0, 1.0))
        .with_rotation(Quat::from_rotation_y(std::f32::consts::PI));
    let mut char_commands = commands.spawn((
        transform,
        Visibility::default(),
        Player::new(),
        GameCursorTarget::default(),
        DisplayName(String::from("Delver")),
        Name::from("dungeon test character"),
        UnloadedResource(asset_server.load(race.male_body)),
        PreferredAnimationGroup(weapon.anim_group.to_string()),
    ));
    if needs_winding_reversal(&transform.to_matrix()) {
        char_commands.insert(MirroredResource);
    }
    let char_entity = char_commands.id();

    for piece in ARMOR_PIECES {
        commands.spawn((
            PendingItemAttachment(asset_server.load(format!(
                "data://res/item/{}/man_item/{}_01_{piece}.bsr",
                race.race_dir, armor.category,
            ))),
            ChildOf(char_entity),
            Name::from(format!("armor {piece}")),
        ));
    }
    for file in weapon.files {
        commands.spawn((
            PendingItemAttachment(asset_server.load(format!(
                "data://res/item/{}/weapon/{file}.bsr",
                race.race_dir
            ))),
            ChildOf(char_entity),
            Name::from("weapon"),
        ));
    }
    state.spawned_character = Some(char_entity);
}

/// The scene's egui window: current-dungeon readout and a teleport list over
/// every `dungeoninfo.txt` entry.
fn dungeons_scene_window(
    mut contexts: EguiContexts,
    dungeon_info: Res<ClientDungeonInfo>,
    active: Option<Res<ActiveDungeon>>,
    mut enter: MessageWriter<EnterDungeon>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    egui::Window::new("Dungeons").show(ctx, |ui| {
        match &active {
            Some(active) => {
                let name = dungeon_info
                    .by_region(active.region_id)
                    .map(|e| e.name().to_string())
                    .unwrap_or_default();
                ui.label(format!(
                    "current: {name} (region {:#06x}) — block {:?}",
                    active.region_id, active.current_block
                ));
            }
            None => {
                ui.label("no active dungeon (loading?)");
            }
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                for entry in dungeon_info.entries() {
                    let is_current = active
                        .as_ref()
                        .is_some_and(|a| a.region_id == entry.region_id);
                    let label = format!("{:2}  {}", entry.id, entry.name());
                    if ui.selectable_label(is_current, label).clicked() && !is_current {
                        // Donwhang keeps its capture-verified gate arrival; other
                        // dungeons land at their entrance block.
                        let arrival =
                            (entry.region_id == DEFAULT_REGION).then_some(DEFAULT_ARRIVAL);
                        enter.write(EnterDungeon {
                            region_id: entry.region_id,
                            arrival,
                        });
                    }
                }
            });
    });
}
