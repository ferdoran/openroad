//! Skill-system test scene: spawn a playable character and a 10k-HP training
//! dummy over a ground plane and exercise the whole offline skill loop —
//! learn/level skills in the S-key skill window, drag them onto the underbar,
//! cast with the slot hotkeys (flat 10 damage per hit), and force
//! stun/freeze to watch casts get interrupted.
//!
//! Idea: this is the particles scene's spawn recipe joined with the game
//! scene's monster assembly, plus [`LocalCombat`] so the skills plugin runs
//! its local simulation instead of sending 0x7074. The character is a real
//! [`Player`] (movement/animation/cast systems all run via
//! `in_playable_world`); the dummy is a `RemoteEntity::Monster` with a
//! synthetic [`NetworkId`], so selection, nameplates, the target window and
//! damage popups behave exactly like in the networked scene. Run with
//! `SCENE=skills` or `scenes.startup: skills`.

use bevy::prelude::*;
use bevy_inspector_egui::bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use bevy_inspector_egui::egui;

use crate::plugins::camera::{spawn_player_camera, DebugCamera, PlayerCamera};
use crate::plugins::cursor::interactions::entity_select::SelectedEntity;
use crate::plugins::cursor::interactions::GameCursorTarget;
use crate::plugins::dynamic_resource_loader::{
    MirroredResource, PendingItemAttachment, PreferredAnimationGroup, UnloadedResource,
};
use crate::plugins::hud::skill_window::model::SkillTreeRace;
use crate::plugins::hud::target_window::spawn_target_window;
use crate::plugins::hud::underbar::model::PlayerProgress;
use crate::plugins::hud::underbar::ui::spawn_underbar;
use crate::plugins::map::objects::{SroBindPoses, SroMeshes};
use crate::plugins::net::entities::{
    CharacterRef, DisplayName, EntityVitals, MonsterRarity, NetworkId, RemoteEntity, RemoteMovement,
};
use crate::plugins::player::Player;
use crate::plugins::skills::book::SkillBook;
use crate::plugins::skills::cast::{SkillCooldowns, SkillSwing};
use crate::plugins::skills::status::{Frozen, Stunned};
use crate::plugins::skills::{EquippedWeapon, LocalCombat};
use crate::plugins::textdata::{ClientCharacterData, ClientMasteryData, ClientTextNames};
use crate::scenes::testing::equipments::{ARMOR_PIECES, RACES};
use crate::scenes::SceneState;
use crate::util::mesh::needs_winding_reversal;
use crate::GameState;

pub struct SkillsScenePlugin;

impl Plugin for SkillsScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SkillsSceneState>()
            // PlayMode so the PlayerCamera (the one mesh picking targets) is
            // the active camera — selection needs it
            .add_systems(OnEnter(SceneState::Skills), enter_play_mode)
            .add_systems(
                OnEnter(GameState::Game),
                (
                    spawn_player_camera,
                    spawn_skills_scene,
                    spawn_underbar,
                    spawn_target_window,
                    frame_camera,
                )
                    .chain()
                    .run_if(in_state(SceneState::Skills)),
            )
            .add_systems(
                EguiPrimaryContextPass,
                skills_scene_window
                    .run_if(in_state(SceneState::Skills))
                    .run_if(crate::plugins::dev::dev_windows_visible),
            )
            .add_systems(
                Update,
                reset_dummy_vitals.run_if(in_state(SceneState::Skills)),
            );
    }
}

/// Where the character stands.
const CHARACTER_POS: Vec3 = Vec3::new(40.0, 0.0, 0.0);
/// Where the dummy stands: beside the character, clearly separated from the
/// framed camera's line of sight.
const DUMMY_POS: Vec3 = Vec3::new(85.0, 0.0, 15.0);
/// The training dummy's fixed HP pool.
const DUMMY_MAX_HP: u32 = 10_000;

const GENDERS: [(&str, &str); 2] = [("Man", "man"), ("Woman", "woman")];

/// Marks the training dummy: its vitals snap back to full at 0.
#[derive(Component)]
pub struct DummyMonster;

/// egui state of the scene's control window.
#[derive(Resource)]
struct SkillsSceneState {
    race_idx: usize,
    gender_idx: usize,
    armor_idx: usize,
    weapon_idx: usize,
    degree: u32,
    /// Simulated character level (caps mastery levels, fills the HUD gauge).
    level: u8,
    spawned_character: Option<Entity>,
    /// Monster picker filter over characterdata code names.
    monster_filter: String,
    /// Picked monster ref id; defaults to the first filter match on spawn.
    monster_ref: Option<i32>,
    spawned_dummy: Option<Entity>,
    next_dummy_uid: u32,
}

impl Default for SkillsSceneState {
    fn default() -> Self {
        Self {
            race_idx: 0,
            gender_idx: 0,
            armor_idx: 0,
            weapon_idx: 0,
            degree: 1,
            level: 20,
            spawned_character: None,
            monster_filter: String::from("ch_mangyang"),
            monster_ref: None,
            spawned_dummy: None,
            // synthetic uid space far away from anything a server would send
            next_dummy_uid: 0x8000_0000,
        }
    }
}

fn enter_play_mode(mut next_mode: ResMut<NextState<crate::AppMode>>) {
    next_mode.set(crate::AppMode::PlayMode);
}

fn spawn_skills_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.init_resource::<SroMeshes>();
    commands.init_resource::<SroBindPoses>();
    commands.insert_resource(GlobalAmbientLight {
        brightness: 100.0,
        color: Color::WHITE,
        ..default()
    });
    // the offline switch: casts simulate locally, 10 damage per hit
    commands.insert_resource(LocalCombat::default());
    // a mana pool for the MP cast gate — no server feeds vitals offline
    commands.insert_resource(crate::plugins::hud::player_mini_info::PlayerVitals {
        name: String::from("Tester"),
        level: 60,
        hp: 5000,
        mp: 5000,
        max_hp: Some(5000),
        max_mp: Some(5000),
        ..default()
    });

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(400.0, 300.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.14, 0.15, 0.13),
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::from_xyz(40.0, 0.0, 40.0),
        Name::new("ground"),
    ));
}

fn frame_camera(
    mut camera_query: Query<&mut Transform, Or<(With<PlayerCamera>, With<DebugCamera>)>>,
) {
    // frames the character spot and the dummy spot
    let look_at = Vec3::new(40.0, 10.0, 20.0);
    let eye = Vec3::new(40.0, 35.0, -70.0);
    for mut transform in camera_query.iter_mut() {
        *transform = Transform::from_translation(eye).looking_at(look_at, Vec3::Y);
    }
}

/// The scene's egui control window: character spawner (with level),
/// dummy-monster spawner, and the simulation panel (SP, masteries, forced
/// status effects, dummy cast).
#[allow(clippy::too_many_arguments)]
fn skills_scene_window(
    mut commands: Commands,
    mut contexts: EguiContexts,
    asset_server: Res<AssetServer>,
    mut state: ResMut<SkillsSceneState>,
    char_data: Res<ClientCharacterData>,
    names: Res<ClientTextNames>,
    masteries: Res<ClientMasteryData>,
    mut progress: ResMut<PlayerProgress>,
    mut book: ResMut<SkillBook>,
    mut cooldowns: ResMut<SkillCooldowns>,
    mut tree_race: ResMut<SkillTreeRace>,
    mut equipped: ResMut<EquippedWeapon>,
    selected: Res<SelectedEntity>,
    mut vitals: ResMut<crate::plugins::hud::player_mini_info::PlayerVitals>,
    mut swings: MessageWriter<SkillSwing>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };

    egui::Window::new("Skills Scene")
        .default_width(280.0)
        .show(ctx, |ui| {
            ui.heading("Character");
            ui.horizontal(|ui| {
                ui.label("Race:");
                for (idx, race) in RACES.iter().enumerate() {
                    if ui
                        .selectable_label(state.race_idx == idx, race.label)
                        .clicked()
                    {
                        state.race_idx = idx;
                    }
                }
            });
            let race = &RACES[state.race_idx];
            state.armor_idx = state.armor_idx.min(race.armor.len() - 1);
            state.weapon_idx = state.weapon_idx.min(race.weapons.len() - 1);
            state.degree = state.degree.min(race.max_degree);

            ui.horizontal(|ui| {
                ui.label("Gender:");
                for (idx, (label, _)) in GENDERS.iter().enumerate() {
                    if ui
                        .selectable_label(state.gender_idx == idx, *label)
                        .clicked()
                    {
                        state.gender_idx = idx;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Armor:");
                egui::ComboBox::from_id_salt("skills_armor")
                    .selected_text(race.armor[state.armor_idx].label)
                    .show_ui(ui, |ui| {
                        for (idx, armor) in race.armor.iter().enumerate() {
                            ui.selectable_value(&mut state.armor_idx, idx, armor.label);
                        }
                    });
                ui.label("Weapon:");
                egui::ComboBox::from_id_salt("skills_weapon")
                    .selected_text(race.weapons[state.weapon_idx].name)
                    .show_ui(ui, |ui| {
                        for (idx, weapon) in race.weapons.iter().enumerate() {
                            ui.selectable_value(&mut state.weapon_idx, idx, weapon.name);
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Level:");
                let mut level = state.level as u32;
                ui.add(egui::Slider::new(&mut level, 1..=110));
                state.level = level as u8;
                if progress.level != state.level {
                    progress.level = state.level;
                }
            });
            if ui.button("Spawn character").clicked() {
                spawn_test_character(
                    &mut commands,
                    &asset_server,
                    &mut state,
                    &mut progress,
                    &mut tree_race,
                    &mut equipped,
                );
            }

            ui.separator();
            ui.heading("Training dummy");
            ui.horizontal(|ui| {
                ui.label("Filter:");
                ui.text_edit_singleline(&mut state.monster_filter);
            });
            let filter = state.monster_filter.to_lowercase();
            if let Some(data) = char_data.data() {
                let mut matches: Vec<(i32, &str)> = data
                    .iter()
                    .filter(|(_, row)| row.is_monster())
                    .filter(|(_, row)| {
                        filter.is_empty() || row.code_name().to_lowercase().contains(&filter)
                    })
                    .map(|(id, row)| (*id, row.code_name().as_str()))
                    .collect();
                matches.sort_unstable_by_key(|(id, _)| *id);
                matches.truncate(50);
                if state.monster_ref.is_none()
                    || !matches.iter().any(|(id, _)| Some(*id) == state.monster_ref)
                {
                    state.monster_ref = matches.first().map(|(id, _)| *id);
                }
                let selected_label = state
                    .monster_ref
                    .and_then(|id| data.get(&id))
                    .map(|row| monster_label(row, &names))
                    .unwrap_or_else(|| "—".to_string());
                let mut picked = state.monster_ref;
                egui::ComboBox::from_id_salt("skills_monster")
                    .width(240.0)
                    .selected_text(selected_label)
                    .show_ui(ui, |ui| {
                        for (id, _) in &matches {
                            let label = data
                                .get(id)
                                .map(|row| monster_label(row, &names))
                                .unwrap_or_default();
                            ui.selectable_value(&mut picked, Some(*id), label);
                        }
                    });
                state.monster_ref = picked;
                if ui.button("Spawn dummy (10k HP)").clicked() {
                    if let Some(ref_id) = state.monster_ref {
                        spawn_dummy(
                            &mut commands,
                            &asset_server,
                            &char_data,
                            &names,
                            &mut state,
                            ref_id,
                        );
                    }
                }
            } else {
                ui.label("characterdata loading…");
            }

            ui.separator();
            ui.heading("Simulation");
            ui.horizontal(|ui| {
                ui.label(format!("SP: {}", progress.skill_points));
                if ui.button("+10").clicked() {
                    progress.skill_points += 10;
                }
                if ui.button("+100").clicked() {
                    progress.skill_points += 100;
                }
                if ui.button("+1000").clicked() {
                    progress.skill_points += 1000;
                }
                if ui.button("+10000").clicked() {
                    progress.skill_points += 10000;
                }
            });
            ui.horizontal(|ui| {
                ui.label(format!("MP: {}", vitals.mp));
                if ui.button("Refill").clicked() {
                    vitals.mp = vitals.max_mp.unwrap_or(5000);
                    vitals.hp = vitals.max_hp.unwrap_or(5000);
                }
            });
            if ui
                .button(format!("Set all masteries to level {}", state.level))
                .clicked()
            {
                for mastery in masteries.iter() {
                    book.masteries.insert(mastery.id, state.level as u32);
                }
            }
            if ui.button("Clear cooldowns").clicked() {
                cooldowns.0.clear();
            }

            ui.add_space(4.0);
            let target = selected.0.or(state.spawned_dummy);
            ui.horizontal(|ui| {
                ui.label("Selected target:");
                if ui.button("Stun 5s").clicked() {
                    if let Some(target) = target {
                        commands
                            .entity(target)
                            .insert(Stunned(Timer::from_seconds(5.0, TimerMode::Once)));
                    }
                }
                if ui.button("Freeze 5s").clicked() {
                    if let Some(target) = target {
                        commands
                            .entity(target)
                            .insert(Frozen(Timer::from_seconds(5.0, TimerMode::Once)));
                    }
                }
            });
            if ui.button("Dummy performs a cast").clicked() {
                if let Some(dummy) = state.spawned_dummy {
                    // basic-attack fallback swing; stun/freeze mid-swing to
                    // watch the interrupt
                    swings.write(SkillSwing {
                        owner: dummy,
                        skill_id: None,
                        codename: String::new(),
                        anim_group: None,
                        ready_anim_type: None,
                        wait_anim_type: None,
                        anim_type: None,
                        charge_secs: 0.0,
                        preparing_secs: 0.0,
                        flying_speed: None,
                        damage: None,
                        target: None,
                        popups: Vec::new(),
                        instance: None,
                    });
                }
            }
        });
}

fn monster_label(
    row: &crate::assets::textdata::characterdata::CharacterDataRow,
    names: &ClientTextNames,
) -> String {
    let display = row
        .name_key()
        .and_then(|key| names.name(key))
        .unwrap_or("?");
    let level = row.level().unwrap_or(0);
    format!("{display} (lv{level}) {}", row.code_name())
}

/// Spawn the playable character: the particles-scene assembly recipe plus
/// [`Player`], so movement/animation/cast systems drive it.
fn spawn_test_character(
    commands: &mut Commands,
    asset_server: &AssetServer,
    state: &mut SkillsSceneState,
    progress: &mut PlayerProgress,
    tree_race: &mut SkillTreeRace,
    equipped: &mut EquippedWeapon,
) {
    if let Some(previous) = state.spawned_character.take() {
        if let Ok(mut entity) = commands.get_entity(previous) {
            entity.despawn();
        }
    }
    let race = &RACES[state.race_idx];
    // the skill window shows the tree of the spawned character's race, and
    // the cast pipeline checks skills against the wielded weapon class
    *tree_race = if race.race_dir == "europe" {
        SkillTreeRace::European
    } else {
        SkillTreeRace::Chinese
    };
    // Class only: this scene's weapon table names `.bsr` stems, not item ref
    // ids, so there is no itemdata row to take a reach from — the approach
    // falls back to the melee default, which is what an offline scene with no
    // server-driven approach wants anyway.
    equipped.class = Some(race.weapons[state.weapon_idx].type_id);
    equipped.reach = None;
    let (_, gender_dir) = GENDERS[state.gender_idx];
    let body_path = if state.gender_idx == 0 {
        race.male_body
    } else {
        race.female_body
    };
    let armor = &race.armor[state.armor_idx];
    let weapon = &race.weapons[state.weapon_idx];
    let degree = state.degree;

    let transform = Transform::from_translation(CHARACTER_POS)
        .with_scale(Vec3::new(-1.0, 1.0, 1.0))
        .with_rotation(Quat::from_rotation_y(std::f32::consts::PI));
    let mut char_commands = commands.spawn((
        transform,
        Visibility::default(),
        Player::new(),
        GameCursorTarget::default(),
        DisplayName(format!("Tester ({})", race.label)),
        Name::from(format!("skills test character {}", race.label)),
        UnloadedResource(asset_server.load(body_path)),
        PreferredAnimationGroup(weapon.anim_group.to_string()),
    ));
    if needs_winding_reversal(&transform.to_matrix()) {
        char_commands.insert(MirroredResource);
    }
    let char_entity = char_commands.id();

    for piece in ARMOR_PIECES {
        let path = format!(
            "data://res/item/{}/{gender_dir}_item/{}_{degree:02}_{piece}.bsr",
            race.race_dir, armor.category,
        );
        commands.spawn((
            PendingItemAttachment(asset_server.load(path)),
            ChildOf(char_entity),
            Name::from(format!("armor {piece}")),
        ));
    }
    for file in weapon.files {
        let file = file.replacen("_01", &format!("_{degree:02}"), 1);
        commands.spawn((
            PendingItemAttachment(asset_server.load(format!(
                "data://res/item/{}/weapon/{file}.bsr",
                race.race_dir
            ))),
            ChildOf(char_entity),
            Name::from("weapon"),
        ));
    }
    if weapon.one_handed {
        commands.spawn((
            PendingItemAttachment(asset_server.load(format!(
                "data://res/item/{}/shield/shield_{degree:02}.bsr",
                race.race_dir
            ))),
            ChildOf(char_entity),
            Name::from("shield"),
        ));
    }

    progress.level = state.level;
    state.spawned_character = Some(char_entity);
}

/// Spawn the training dummy: the game scene's monster assembly with a
/// synthetic network id, a fixed 10k HP pool and no ground snap (flat
/// plane, no nav data).
fn spawn_dummy(
    commands: &mut Commands,
    asset_server: &AssetServer,
    char_data: &ClientCharacterData,
    names: &ClientTextNames,
    state: &mut SkillsSceneState,
    ref_id: i32,
) {
    if let Some(previous) = state.spawned_dummy.take() {
        if let Ok(mut entity) = commands.get_entity(previous) {
            entity.despawn();
        }
    }
    let Some(row) = char_data.get(&ref_id) else {
        warn!("skills scene: no characterdata for ref {ref_id}");
        return;
    };
    let display = row
        .name_key()
        .and_then(|key| names.name(key))
        .map(str::to_string)
        .unwrap_or_else(|| row.code_name().clone());
    let uid = state.next_dummy_uid;
    state.next_dummy_uid += 1;
    // face the character spot
    let transform = Transform::from_translation(DUMMY_POS)
        .with_rotation(Quat::from_rotation_y(std::f32::consts::PI));
    let mut cmd = commands.spawn((
        Name::from(format!("training dummy {uid}")),
        DisplayName(display),
        DummyMonster,
        RemoteEntity::Monster,
        NetworkId(uid),
        CharacterRef(ref_id as u32),
        MonsterRarity(row.rarity()),
        RemoteMovement::default(),
        EntityVitals::full(DUMMY_MAX_HP),
        transform,
        Visibility::default(),
    ));
    if let Some(path) = char_data.model_path(row) {
        cmd.insert(UnloadedResource(asset_server.load(path)));
    }
    state.spawned_dummy = Some(cmd.id());
}

/// The training-dummy rule: at 0 HP the pool snaps back to full (no death,
/// no respawn round-trip — endless target practice).
fn reset_dummy_vitals(mut dummies: Query<&mut EntityVitals, With<DummyMonster>>) {
    for mut vitals in dummies.iter_mut() {
        if vitals.hp == 0 {
            vitals.hp = vitals.max_hp;
        }
    }
}
