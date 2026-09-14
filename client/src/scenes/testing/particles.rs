//! Particle effect testing scene: an egui character window that spawns a
//! customizable character (race, gender, armor, weapon) over a ground plane,
//! and an effect picker window that plays any of the .efp effects in
//! Particles.pk2 attached to the character, its weapon, or free-standing in
//! the world.
//!
//! Run with `scenes.startup: particles` in config.yaml or `SCENE=particles`.

use std::env;
use std::path::PathBuf;

use bevy::prelude::*;
use bevy_inspector_egui::bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use bevy_inspector_egui::egui;
use bevy_pk2::prelude::Archive;

use crate::assets::bmt::sheen::{
    set_shine, ShineColor, ShineParams, SroSheenMaterial, SHINE_SPHEREMAP_PATH,
};
use crate::assets::bsr::resource::SroResource;
use crate::commands::{AnimationLibrary, SpawnedFromResource};
use crate::plugins::camera::{spawn_player_camera, DebugCamera, PlayerCamera};
use crate::plugins::dynamic_resource_loader::{
    AttachmentRareAura, MirroredResource, PendingItemAttachment, PreferredAnimationGroup,
    UnloadedResource,
};
use crate::plugins::effects::EffectCommandsExt;
use crate::plugins::map::objects::{SroBindPoses, SroMeshes};
use crate::plugins::textdata::ClientRareEffects;
use crate::scenes::testing::equipments::{ARMOR_PIECES, RACES};
use crate::scenes::SceneState;
use crate::util::mesh::needs_winding_reversal;
use crate::GameState;

pub struct ParticleTestingScenePlugin;

impl Plugin for ParticleTestingScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CharacterSpawner>()
            .add_systems(
                OnEnter(GameState::Game),
                (
                    spawn_player_camera,
                    spawn_particle_scene,
                    setup_effect_picker,
                    frame_camera,
                )
                    .chain()
                    .run_if(in_state(SceneState::ParticleTesting)),
            )
            .add_systems(
                // egui UI must build inside the multipass context pass —
                // in Update it would render but never receive input
                EguiPrimaryContextPass,
                (character_spawner_window, effect_picker_window)
                    .run_if(in_state(SceneState::ParticleTesting))
                    .run_if(crate::plugins::dev::dev_windows_visible),
            );
    }
}

/// Effects are authored in SRO resource units (a character is ~18 units
/// tall).
const EFFECT_HEIGHT: f32 = 5.0;

/// Where the customizable character stands, visible from the framed camera.
const CHARACTER_POS: Vec3 = Vec3::new(40.0, 0.0, 0.0);

fn spawn_particle_scene(
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

    // dark ground plane so additive effects are clearly visible
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(400.0, 200.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.12, 0.12, 0.14),
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::from_xyz(90.0, 0.0, 0.0),
        Name::new("ground"),
    ));
}

/// Marks the character spawned from the customizer window.
#[derive(Component)]
struct SpawnedTestCharacter;

/// State of the character customizer window: the picked race / gender /
/// armor / weapon, the currently spawned character (at most one), and the
/// weapon resource handles used to find its weapon wrappers for effect
/// attachment.
#[derive(Resource)]
struct CharacterSpawner {
    race_idx: usize,
    gender_idx: usize,
    armor_idx: usize,
    weapon_idx: usize,
    /// Equipment degree (1-based), capped by the race's
    /// [`RaceSpec::max_degree`](super::equipments::RaceSpec::max_degree).
    degree: u32,
    spawned: Option<Entity>,
    body_handle: Option<Handle<SroResource>>,
    weapon_handles: Vec<Handle<SroResource>>,
    /// Shield resource of a one-handed loadout; its wrapper gets the same
    /// enchant glow and enhancement shine as the weapon hands.
    shield_handle: Option<Handle<SroResource>>,
    /// Index into the body wrapper's [`AnimationLibrary`] entries; `None`
    /// leaves the default stand animation playing.
    selected_anim: Option<usize>,
    anim_filter: String,
    /// Rare ("Seal of …") tier for the weapon/shield: 0 = none, 1..=3 =
    /// Star/Moon/Sun (`_A/_B/_C_RARE` item codes). Attaches the real
    /// ItemRare.txt auras — server-free aura playtesting.
    seal_tier: usize,
}

impl Default for CharacterSpawner {
    fn default() -> Self {
        Self {
            race_idx: 0,
            gender_idx: 0,
            armor_idx: 0,
            weapon_idx: 0,
            degree: 1,
            spawned: None,
            body_handle: None,
            weapon_handles: Vec::new(),
            shield_handle: None,
            selected_anim: None,
            anim_filter: String::new(),
            seal_tier: 0,
        }
    }
}

const SEAL_TIERS: [&str; 4] = ["None", "Star", "Moon", "Sun"];

const GENDERS: [(&str, &str); 2] = [("Man", "man"), ("Woman", "woman")];

/// The customizer window: race / gender / armor / weapon choices plus a
/// Spawn button. Spawning replaces the previously spawned character (a
/// recursive despawn that also removes its attached items and effects).
fn character_spawner_window(
    mut commands: Commands,
    mut contexts: EguiContexts,
    asset_server: Res<AssetServer>,
    mut spawner: ResMut<CharacterSpawner>,
    libraries: Query<(Entity, &SpawnedFromResource, &AnimationLibrary)>,
    mut players: Query<&mut AnimationPlayer>,
    rare_effects: Res<ClientRareEffects>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };

    // animation library of the spawned character's body wrapper; appears
    // once the resource is instantiated
    let body_library = spawner.body_handle.as_ref().and_then(|body| {
        libraries
            .iter()
            .find(|(_, spawned_from, _)| &spawned_from.0 == body)
            .map(|(entity, _, library)| (entity, library))
    });

    egui::Window::new("Character")
        .default_width(260.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Race:");
                for (idx, race) in RACES.iter().enumerate() {
                    if ui
                        .selectable_label(spawner.race_idx == idx, race.label)
                        .clicked()
                    {
                        spawner.race_idx = idx;
                    }
                }
            });
            let race = &RACES[spawner.race_idx];
            // switching race can shrink the armor/weapon lists and the
            // degree range
            spawner.armor_idx = spawner.armor_idx.min(race.armor.len() - 1);
            spawner.weapon_idx = spawner.weapon_idx.min(race.weapons.len() - 1);
            spawner.degree = spawner.degree.min(race.max_degree);

            ui.horizontal(|ui| {
                ui.label("Gender:");
                for (idx, (label, _)) in GENDERS.iter().enumerate() {
                    if ui
                        .selectable_label(spawner.gender_idx == idx, *label)
                        .clicked()
                    {
                        spawner.gender_idx = idx;
                    }
                }
            });

            ui.horizontal(|ui| {
                ui.label("Armor:");
                egui::ComboBox::from_id_salt("armor_combo")
                    .selected_text(race.armor[spawner.armor_idx].label)
                    .show_ui(ui, |ui| {
                        for (idx, armor) in race.armor.iter().enumerate() {
                            ui.selectable_value(&mut spawner.armor_idx, idx, armor.label);
                        }
                    });
            });

            ui.horizontal(|ui| {
                ui.label("Weapon:");
                egui::ComboBox::from_id_salt("weapon_combo")
                    .selected_text(race.weapons[spawner.weapon_idx].name)
                    .show_ui(ui, |ui| {
                        for (idx, weapon) in race.weapons.iter().enumerate() {
                            ui.selectable_value(&mut spawner.weapon_idx, idx, weapon.name);
                        }
                    });
            });

            ui.horizontal(|ui| {
                ui.label("Degree:");
                egui::ComboBox::from_id_salt("degree_combo")
                    .selected_text(spawner.degree.to_string())
                    .show_ui(ui, |ui| {
                        for degree in 1..=race.max_degree {
                            ui.selectable_value(&mut spawner.degree, degree, degree.to_string());
                        }
                    });
            });

            ui.horizontal(|ui| {
                ui.label("Seal:");
                for (idx, label) in SEAL_TIERS.iter().enumerate() {
                    if ui
                        .selectable_label(spawner.seal_tier == idx, *label)
                        .clicked()
                    {
                        spawner.seal_tier = idx;
                    }
                }
            });

            // animation of the spawned character, across all its groups
            // (stand of the weapon's group plays by default)
            if let Some((wrapper, library)) = body_library {
                ui.horizontal(|ui| {
                    ui.label("Animation:");
                    ui.text_edit_singleline(&mut spawner.anim_filter);
                });
                let filter = spawner.anim_filter.to_lowercase();
                let selected_text = spawner
                    .selected_anim
                    .and_then(|idx| library.entries.get(idx))
                    .map(|entry| entry.label.as_str())
                    .unwrap_or("stand (default)");
                let mut picked = None;
                egui::ComboBox::from_id_salt("anim_combo")
                    .width(340.0)
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        for (idx, entry) in library.entries.iter().enumerate() {
                            if !filter.is_empty() && !entry.label.to_lowercase().contains(&filter) {
                                continue;
                            }
                            if ui
                                .selectable_label(spawner.selected_anim == Some(idx), &entry.label)
                                .clicked()
                            {
                                picked = Some(idx);
                            }
                        }
                    });
                if let Some(idx) = picked {
                    spawner.selected_anim = Some(idx);
                    if let Ok(mut player) = players.get_mut(wrapper) {
                        player.stop_all();
                        player.play(library.entries[idx].node).repeat();
                    }
                }
            } else if spawner.spawned.is_some() {
                ui.label("animations loading…");
            }

            ui.add_space(4.0);
            if ui.button("Spawn").clicked() {
                spawn_selected_character(&mut commands, &asset_server, &mut spawner, &rare_effects);
            }
            if let Some(spawned) = spawner.spawned {
                ui.label(format!("spawned at {CHARACTER_POS:?} (entity {spawned})"));
            }
        });
}

/// Spawns the character described by the current picker state, replacing the
/// previous one. Same assembly recipe as the equipments scene: mirrored
/// body resource plus pending attachments for the full armor set, the
/// weapon file(s), and a shield when the weapon is one-handed.
/// The ItemRare.txt auras for an item file stem (`blade_03`, `shield_02`) at
/// the picked tier: the code is `ITEM_{CH|EU}_{STEM}_{A|B|C}_RARE`, with any
/// dual-wield `_l`/`_r` hand suffix stripped.
fn rare_auras_for(
    rare_effects: &ClientRareEffects,
    race_dir: &str,
    stem: &str,
    seal_tier: usize,
) -> Option<AttachmentRareAura> {
    let letter = ["A", "B", "C"].get(seal_tier.checked_sub(1)?)?;
    let region = if race_dir == "china" { "CH" } else { "EU" };
    let stem = stem
        .trim_end_matches("_l")
        .trim_end_matches("_r")
        .to_uppercase();
    let code = format!("ITEM_{region}_{stem}_{letter}_RARE");
    // All rows attach, including the +N-gated ItemOptionEfp enchant flares —
    // the test spawn acts as a maxed (+8) item so every layer is visible.
    let auras = rare_effects.get(&code);
    if auras.is_empty() {
        warn!("no ItemRare.txt entry for {code}");
        return None;
    }
    Some(AttachmentRareAura {
        auras: auras.to_vec(),
        gate: None,
    })
}

fn spawn_selected_character(
    commands: &mut Commands,
    asset_server: &AssetServer,
    spawner: &mut CharacterSpawner,
    rare_effects: &ClientRareEffects,
) {
    if let Some(previous) = spawner.spawned.take() {
        if let Ok(mut entity) = commands.get_entity(previous) {
            entity.despawn();
        }
    }
    spawner.weapon_handles.clear();
    spawner.shield_handle = None;
    spawner.selected_anim = None;

    let race = &RACES[spawner.race_idx];
    let (_, gender_dir) = GENDERS[spawner.gender_idx];
    let body_path = if spawner.gender_idx == 0 {
        race.male_body
    } else {
        race.female_body
    };
    let armor = &race.armor[spawner.armor_idx];
    let weapon = &race.weapons[spawner.weapon_idx];
    let degree = spawner.degree;

    // Mirror on X (scale.x = -1) like every SRO resource so the character
    // renders with correct handedness; the shared winding rule then reverses
    // its meshes because the placement determinant is negative.
    let transform = Transform::from_translation(CHARACTER_POS)
        .with_scale(Vec3::new(-1.0, 1.0, 1.0))
        .with_rotation(Quat::from_rotation_y(std::f32::consts::PI));
    let body_handle = asset_server.load(body_path);
    spawner.body_handle = Some(body_handle.clone());
    let mut char_commands = commands.spawn((
        transform,
        Visibility::default(),
        SpawnedTestCharacter,
        Name::from(format!(
            "{} {gender_dir} {} {} deg{degree}",
            race.label, armor.label, weapon.name
        )),
        UnloadedResource(body_handle),
        PreferredAnimationGroup(weapon.anim_group.to_string()),
    ));
    if needs_winding_reversal(&transform.to_matrix()) {
        char_commands.insert(MirroredResource);
    }
    let char_entity = char_commands.id();

    // full armor set: all 6 pieces of the category worn together
    for piece in ARMOR_PIECES {
        let path = format!(
            "data://res/item/{}/{gender_dir}_item/{}_{degree:02}_{piece}.bsr",
            race.race_dir, armor.category,
        );
        commands.spawn((
            PendingItemAttachment(asset_server.load(path)),
            ChildOf(char_entity),
            Name::from(format!("armor {} {piece}", armor.label)),
        ));
    }

    // weapon(s): dual-wield weapons bring two models. The Weapon file
    // stems embed degree 01 ("sword_01", "dagger_01_l"); swap in the
    // picked degree.
    for file in weapon.files {
        let file = file.replacen("_01", &format!("_{degree:02}"), 1);
        let path = format!("data://res/item/{}/weapon/{file}.bsr", race.race_dir);
        let handle = asset_server.load(path);
        spawner.weapon_handles.push(handle.clone());
        let mut item = commands.spawn((
            PendingItemAttachment(handle),
            ChildOf(char_entity),
            Name::from(format!("weapon {}", weapon.name)),
        ));
        if let Some(auras) = rare_auras_for(rare_effects, race.race_dir, &file, spawner.seal_tier) {
            item.insert(auras);
        }
    }

    // one-handed weapons keep the left hand free for a shield
    if weapon.one_handed {
        let stem = format!("shield_{degree:02}");
        let path = format!("data://res/item/{}/shield/{stem}.bsr", race.race_dir);
        let handle = asset_server.load(path);
        spawner.shield_handle = Some(handle.clone());
        let mut item = commands.spawn((
            PendingItemAttachment(handle),
            ChildOf(char_entity),
            Name::from("shield"),
        ));
        if let Some(auras) = rare_auras_for(rare_effects, race.race_dir, &stem, spawner.seal_tier) {
            item.insert(auras);
        }
    }

    spawner.spawned = Some(char_entity);
}

/// Where a picked effect targets: attached to the spawned character, to its
/// weapon model(s), or free-standing at a fixed world position.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum EffectTarget {
    #[default]
    Character,
    Weapon,
    World,
}

impl EffectTarget {
    fn label(self) -> &'static str {
        match self {
            EffectTarget::Character => "Character",
            EffectTarget::Weapon => "Weapon",
            EffectTarget::World => "World",
        }
    }
}

/// State of the effect-picker window: every .efp in Particles.pk2, the
/// current filter/selection/target, and the applied effects still alive.
#[derive(Resource)]
struct EffectPicker {
    all: Vec<String>,
    filter: String,
    selected: Option<String>,
    target: EffectTarget,
    spawned: Vec<(String, Entity)>,
    /// Selection of the weapon-glow quick picker (a `battle/hwan_*.efp`).
    glow_selected: Option<String>,
    /// The applied weapon glow, one wrapper entity per weapon hand,
    /// replaced (not accumulated) on re-apply like real enchant glows.
    glow_spawned: Vec<Entity>,
    /// Enhancement-glow tier applied to the weapon's sheen materials
    /// (`None` = no glow).
    shine: Option<ShineColor>,
    /// Calibration sliders for the glow: overall intensity, an optional idle
    /// scroll of the sphere-map highlight, and the two-color pulse rate.
    shine_intensity: f32,
    shine_scroll: f32,
    shine_pulse: f32,
    /// The per-instance material clones currently carrying the shine, so
    /// slider changes can mutate them in place.
    shine_handles: Vec<Handle<SroSheenMaterial>>,
}

impl Default for EffectPicker {
    fn default() -> Self {
        Self {
            all: Vec::new(),
            filter: String::new(),
            selected: None,
            target: EffectTarget::default(),
            spawned: Vec::new(),
            glow_selected: None,
            glow_spawned: Vec::new(),
            shine: None,
            shine_intensity: ShineColor::White.colors().0.w,
            shine_scroll: 0.0,
            shine_pulse: 0.0,
            shine_handles: Vec::new(),
        }
    }
}

impl EffectPicker {
    /// The glow configuration the sliders currently describe: the tier's two
    /// colors (at the current intensity), the highlight scroll, and the
    /// pulse rate (the tier default unless overridden by the slider).
    fn shine_params(&self) -> ShineParams {
        let Some(tier) = self.shine else {
            return ShineParams::default();
        };
        let (a, b) = tier.colors();
        let with_intensity = |c: Vec4| c.truncate().extend(self.shine_intensity);
        ShineParams {
            color: with_intensity(a),
            color2: with_intensity(b),
            // the original scrolls the highlight (u, -v) for weapon types
            // 1/2 (itemtypenumber.txt); the slider scales that direction
            scroll: Vec2::new(self.shine_scroll, -self.shine_scroll),
            pulse: self.shine_pulse,
        }
    }
}

/// Where world-targeted effects play: centered in front of the camera.
const PICKED_EFFECT_POS: Vec3 = Vec3::new(70.0, EFFECT_HEIGHT, -30.0);

/// Directory containing the .pk2 archives (same path resolution as
/// SroAssetPlugin).
fn sro_pk2_dir() -> PathBuf {
    env::var_os("SRO_PK2_PATH")
        .or_else(|| env::var_os("SRO_PATH"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let mut working_dir = env::current_dir().unwrap();
            working_dir.push("assets");
            working_dir
        })
}

/// Enumerates all effect files by opening Particles.pk2 directly (same
/// path resolution as SroAssetPlugin). Done once on scene enter; the
/// archive index parse is cheap.
fn setup_effect_picker(mut commands: Commands) {
    let archive = Archive::configured(sro_pk2_dir().join("Particles.pk2"));
    let mut all: Vec<String> = archive
        .root
        .get_all_entries()
        .into_iter()
        .filter(|(_, entry)| entry.is_file())
        .map(|(path, _)| path.to_string_lossy().replace('\\', "/"))
        .filter(|path| path.to_lowercase().ends_with(".efp"))
        .collect();
    all.sort();
    info!("effect picker: {} effects in Particles.pk2", all.len());
    commands.insert_resource(EffectPicker { all, ..default() });
}

/// The picker window: filter box + dropdown over all effects, a target
/// choice (character / weapon / world), Apply, and a Clear button. Effects
/// accumulate; character/weapon targets parent the effect to the entity so
/// it follows animation through transform propagation.
fn effect_picker_window(
    mut commands: Commands,
    mut contexts: EguiContexts,
    asset_server: Res<AssetServer>,
    picker: Option<ResMut<EffectPicker>>,
    spawner: Res<CharacterSpawner>,
    weapons: Query<(Entity, &SpawnedFromResource)>,
    entities: Query<Entity>,
    children: Query<&Children>,
    sheen_meshes: Query<&MeshMaterial3d<SroSheenMaterial>>,
    mut sheen_materials: ResMut<Assets<SroSheenMaterial>>,
) {
    let Some(mut picker) = picker else { return };
    let Ok(ctx) = contexts.ctx_mut() else { return };

    // drop entries that died with a respawned/despawned character
    picker
        .spawned
        .retain(|(_, entity)| entities.contains(*entity));

    // weapon wrappers of the current character (spawned asynchronously by
    // the attachment pipeline; dual-wield weapons have one per hand). The
    // shield wrapper joins them for the enchant glow and enhancement shine
    // (enhanced shields glow like weapons), but not the "Weapon" effect
    // target.
    let mut weapon_wrappers = Vec::new();
    let mut glow_wrappers = Vec::new();
    for (entity, spawned_from) in &weapons {
        if spawner.weapon_handles.contains(&spawned_from.0) {
            weapon_wrappers.push(entity);
            glow_wrappers.push(entity);
        } else if spawner.shield_handle.as_ref() == Some(&spawned_from.0) {
            glow_wrappers.push(entity);
        }
    }

    egui::Window::new("Effect Picker")
        .default_width(380.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Filter:");
                ui.text_edit_singleline(&mut picker.filter);
            });

            let filter = picker.filter.to_lowercase();
            let filtered: Vec<String> = picker
                .all
                .iter()
                .filter(|path| filter.is_empty() || path.to_lowercase().contains(&filter))
                .cloned()
                .collect();
            ui.label(format!(
                "{} of {} effects",
                filtered.len(),
                picker.all.len()
            ));

            let selected_text = picker
                .selected
                .clone()
                .unwrap_or_else(|| "— select an effect —".into());
            egui::ComboBox::from_id_salt("effect_picker_combo")
                .width(340.0)
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    for path in &filtered {
                        let is_selected = picker.selected.as_deref() == Some(path.as_str());
                        if ui.selectable_label(is_selected, path).clicked() {
                            picker.selected = Some(path.clone());
                        }
                    }
                });

            ui.horizontal(|ui| {
                ui.label("Target:");
                for target in [
                    EffectTarget::Character,
                    EffectTarget::Weapon,
                    EffectTarget::World,
                ] {
                    ui.selectable_value(&mut picker.target, target, target.label());
                }
            });

            let target_ready = match picker.target {
                EffectTarget::World => true,
                EffectTarget::Character => spawner.spawned.is_some(),
                EffectTarget::Weapon => !weapon_wrappers.is_empty(),
            };
            if !target_ready {
                match picker.target {
                    EffectTarget::Character => ui.label("spawn a character first"),
                    EffectTarget::Weapon if spawner.spawned.is_some() => {
                        ui.label("weapon still attaching…")
                    }
                    _ => ui.label("spawn a character first"),
                };
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let apply = ui
                    .add_enabled(
                        picker.selected.is_some() && target_ready,
                        egui::Button::new("Apply"),
                    )
                    .clicked();
                if apply {
                    let path = picker.selected.clone().unwrap();
                    let handle = asset_server.load(format!("particles://{path}"));
                    match picker.target {
                        EffectTarget::World => {
                            let entity = commands.spawn_effect(
                                handle,
                                Transform::from_translation(PICKED_EFFECT_POS),
                                None,
                            );
                            commands
                                .entity(entity)
                                .insert(Name::new(format!("picked: {path}")));
                            picker.spawned.push((format!("{path} → world"), entity));
                        }
                        EffectTarget::Character => {
                            let target = spawner.spawned.unwrap();
                            let entity =
                                commands.attach_effect(handle, target, Transform::IDENTITY);
                            commands
                                .entity(entity)
                                .insert(Name::new(format!("picked: {path}")));
                            picker.spawned.push((format!("{path} → character"), entity));
                        }
                        EffectTarget::Weapon => {
                            for wrapper in &weapon_wrappers {
                                let entity = commands.attach_effect(
                                    handle.clone(),
                                    *wrapper,
                                    Transform::IDENTITY,
                                );
                                commands
                                    .entity(entity)
                                    .insert(Name::new(format!("picked: {path}")));
                                picker.spawned.push((format!("{path} → weapon"), entity));
                            }
                        }
                    }
                }

                let clear = ui
                    .add_enabled(!picker.spawned.is_empty(), egui::Button::new("Clear"))
                    .clicked();
                if clear {
                    for (_, entity) in picker.spawned.drain(..) {
                        if let Ok(mut entity) = commands.get_entity(entity) {
                            entity.despawn();
                        }
                    }
                }
            });

            if !picker.spawned.is_empty() {
                ui.add_space(4.0);
                ui.label(format!("{} active:", picker.spawned.len()));
                for (label, _) in &picker.spawned {
                    ui.label(label);
                }
            }

            // weapon enchant glow quick picker: the battle/hwan_* effects
            // the original client attaches per plus-level. Naming (from the
            // file set, semantics provisional): {b,r} color, {c,s,w} weapon
            // class, {s,m,b} size, plus special hwan_{g,v,y}.
            ui.separator();
            ui.label("Weapon glow (battle/hwan_*):");
            picker
                .glow_spawned
                .retain(|entity| entities.contains(*entity));
            let glows: Vec<String> = picker
                .all
                .iter()
                .filter(|path| path.starts_with("battle/hwan_"))
                .cloned()
                .collect();
            ui.horizontal(|ui| {
                let selected_text = picker
                    .glow_selected
                    .clone()
                    .unwrap_or_else(|| "— select a glow —".into());
                egui::ComboBox::from_id_salt("weapon_glow_combo")
                    .width(200.0)
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        for path in &glows {
                            let is_selected =
                                picker.glow_selected.as_deref() == Some(path.as_str());
                            if ui.selectable_label(is_selected, path).clicked() {
                                picker.glow_selected = Some(path.clone());
                            }
                        }
                    });
                let can_apply = picker.glow_selected.is_some() && !glow_wrappers.is_empty();
                if ui
                    .add_enabled(can_apply, egui::Button::new("Apply"))
                    .clicked()
                {
                    for entity in picker.glow_spawned.drain(..) {
                        if let Ok(mut entity) = commands.get_entity(entity) {
                            entity.despawn();
                        }
                    }
                    let path = picker.glow_selected.clone().unwrap();
                    let handle = asset_server.load(format!("particles://{path}"));
                    for wrapper in &glow_wrappers {
                        let entity =
                            commands.attach_effect(handle.clone(), *wrapper, Transform::IDENTITY);
                        commands
                            .entity(entity)
                            .insert(Name::new(format!("weapon glow: {path}")));
                        picker.glow_spawned.push(entity);
                    }
                }
                let can_remove = !picker.glow_spawned.is_empty();
                if ui
                    .add_enabled(can_remove, egui::Button::new("Remove"))
                    .clicked()
                {
                    for entity in picker.glow_spawned.drain(..) {
                        if let Ok(mut entity) = commands.get_entity(entity) {
                            entity.despawn();
                        }
                    }
                }
            });

            // enhancement glow: the original client's CRTModProgEquipPow —
            // a sphere-map highlight tinted by a per-tier color pair the
            // glow pulses between (see bmt/sheen.rs). Applied to the
            // weapon's sheen materials rather than spawned. Picking a tier
            // (re)clones the materials; sliders mutate the clones live.
            let mut color_changed = false;
            let mut sliders_changed = false;
            ui.horizontal(|ui| {
                ui.label("Enhancement glow:");
                if ui
                    .selectable_value(&mut picker.shine, None, "None")
                    .changed()
                {
                    color_changed = true;
                }
                for tier in ShineColor::ALL {
                    if ui
                        .selectable_value(&mut picker.shine, Some(tier), tier.label())
                        .changed()
                    {
                        // adopt the tier's vanilla defaults on selection
                        picker.shine_pulse = tier.pulse();
                        picker.shine_intensity = tier.colors().0.w;
                        picker.shine_scroll = ShineColor::SCROLL_UV_PER_SEC;
                        color_changed = true;
                    }
                }
            });
            if picker.shine.is_some() {
                ui.horizontal(|ui| {
                    sliders_changed |= ui
                        .add(
                            egui::Slider::new(&mut picker.shine_intensity, 0.0..=10.0)
                                .text("intensity"),
                        )
                        .changed();
                    sliders_changed |= ui
                        .add(egui::Slider::new(&mut picker.shine_scroll, -0.3..=0.3).text("scroll"))
                        .changed();
                    sliders_changed |= ui
                        .add(egui::Slider::new(&mut picker.shine_pulse, 0.0..=2.0).text("pulse"))
                        .changed();
                });
            }
            if color_changed {
                let params = picker.shine_params();
                // each tier scrolls its own streak texture; the sphere map
                // is only the neutral fallback when clearing the shine
                // non-color: intensity maps (see DdjSettings::non_color)
                let non_color = |settings: &mut crate::assets::ddj::DdjSettings| {
                    settings.non_color = true;
                };
                let path = match picker.shine {
                    Some(tier) => tier.texture_path(),
                    None => SHINE_SPHEREMAP_PATH,
                };
                let shine_texture = asset_server
                    .load_builder()
                    .with_settings(non_color)
                    .load(path);
                picker.shine_handles.clear();
                for wrapper in &glow_wrappers {
                    let updates = set_shine(
                        *wrapper,
                        params,
                        &shine_texture,
                        &children,
                        &sheen_meshes,
                        &mut sheen_materials,
                    );
                    for (entity, material) in updates {
                        picker.shine_handles.push(material.clone());
                        commands.entity(entity).insert(MeshMaterial3d(material));
                    }
                }
            } else if sliders_changed {
                let params = picker.shine_params();
                for handle in &picker.shine_handles {
                    if let Some(mut material) = sheen_materials.get_mut(handle) {
                        params.apply_to(&mut material.extension.settings);
                    }
                }
            }
        });
}

fn frame_camera(
    mut camera_query: Query<&mut Transform, Or<(With<PlayerCamera>, With<DebugCamera>)>>,
) {
    // frames both the character spot (40,0,0) and the world effect position
    let look_at = Vec3::new(55.0, 8.0, 0.0);
    let eye = Vec3::new(55.0, 25.0, -90.0);
    for mut transform in camera_query.iter_mut() {
        *transform = Transform::from_translation(eye).looking_at(look_at, Vec3::Y);
    }
}
