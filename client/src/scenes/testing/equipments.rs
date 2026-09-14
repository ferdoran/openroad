use bevy::prelude::*;

use crate::plugins::camera::{spawn_player_camera, DebugCamera, PlayerCamera};
use crate::plugins::dynamic_resource_loader::{
    MirroredResource, PendingItemAttachment, PreferredAnimationGroup, UnloadedResource,
};
use crate::scenes::SceneState;
use crate::util::mesh::needs_winding_reversal;

/// Characters spawned by this scene, one per (race, armor type, weapon,
/// gender) combination, despawned on scene exit.
#[derive(Component)]
struct EquipmentTestCharacter;

pub struct EquipmentsScenePlugin;

impl Plugin for EquipmentsScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            OnEnter(SceneState::Equipments),
            (
                spawn_player_camera,
                spawn_equipment_characters,
                frame_camera,
            )
                .chain(),
        )
        .add_systems(OnExit(SceneState::Equipments), despawn_equipment_characters);
    }
}

/// The 6 pieces that make up a full armor set, by code-name suffix. Each
/// covers a different body region (cap / chest / legs / boots / shoulder /
/// gloves), verified against the real `attach_info` slot ids. Worn together
/// they form the whole armor look.
pub(crate) const ARMOR_PIECES: [&str; 6] = ["ha", "ba", "la", "fa", "sa", "aa"];

/// A weapon available to a race.
pub(crate) struct Weapon {
    pub(crate) name: &'static str,
    /// Weapon `.bsr` file stems under `item/<race>/weapon/`. Dual-wield
    /// weapons (EU daggers, EU axes) list two (`_l` + `_r`); all others one.
    pub(crate) files: &'static [&'static str],
    /// Character animation group (weapon class) selecting the stance.
    pub(crate) anim_group: &'static str,
    /// One-handed weapons leave the left hand free and carry a shield.
    pub(crate) one_handed: bool,
    /// Weapon class (item tid4) for skill cast requirements (skilldata
    /// cols 50/51): CH 2=sword 3=blade 4=spear 5=glaive 6=bow, EU 7=1h
    /// sword 8=2h sword 9=dual axe 10=warlock rod 11=staff 12=crossbow
    /// 13=dagger 14=harp 15=cleric rod.
    pub(crate) type_id: u8,
}

/// One of a race's 3 armor types, identified by its item category. A full
/// set of a category is [`ARMOR_PIECES`] worn at once.
pub(crate) struct ArmorSet {
    pub(crate) label: &'static str,
    pub(crate) category: &'static str,
}

pub(crate) struct RaceSpec {
    pub(crate) label: &'static str,
    /// Resource-path race segment: `china` or `europe`.
    pub(crate) race_dir: &'static str,
    pub(crate) male_body: &'static str,
    pub(crate) female_body: &'static str,
    pub(crate) armor: &'static [ArmorSet],
    pub(crate) weapons: &'static [Weapon],
    /// Highest equipment degree with models in Data.pk2. Degrees are
    /// contiguous from 1: every weapon/shield/armor stem exists at every
    /// degree (verified against the archive).
    pub(crate) max_degree: u32,
}

// Categories per race: CH clothes/light/heavy = Garment/Protector/Armor,
// EU clothes/light/heavy = Robe/Light Armor/Heavy Armor.
const CH_ARMOR: &[ArmorSet] = &[
    ArmorSet {
        label: "Garment",
        category: "clothes",
    },
    ArmorSet {
        label: "Protector",
        category: "light",
    },
    ArmorSet {
        label: "Armor",
        category: "heavy",
    },
];

const EU_ARMOR: &[ArmorSet] = &[
    ArmorSet {
        label: "Robe",
        category: "clothes",
    },
    ArmorSet {
        label: "Light Armor",
        category: "light",
    },
    ArmorSet {
        label: "Heavy Armor",
        category: "heavy",
    },
];

const CH_WEAPONS: &[Weapon] = &[
    Weapon {
        name: "sword",
        files: &["sword_01"],
        anim_group: "sword",
        one_handed: true,
        type_id: 2,
    },
    Weapon {
        name: "blade",
        files: &["blade_01"],
        anim_group: "sword",
        one_handed: true,
        type_id: 3,
    },
    Weapon {
        name: "spear",
        files: &["spear_01"],
        anim_group: "spear",
        one_handed: false,
        type_id: 4,
    },
    Weapon {
        name: "glaive",
        files: &["tblade_01"],
        anim_group: "spear",
        one_handed: false,
        type_id: 5,
    },
    Weapon {
        name: "bow",
        files: &["bow_01"],
        anim_group: "bow",
        one_handed: false,
        type_id: 6,
    },
];

const EU_WEAPONS: &[Weapon] = &[
    Weapon {
        name: "dagger",
        files: &["dagger_01_l", "dagger_01_r"],
        anim_group: "dagger",
        one_handed: false,
        type_id: 13,
    },
    Weapon {
        name: "onehand_sword",
        files: &["sword_01"],
        anim_group: "onehand_sword",
        one_handed: true,
        type_id: 7,
    },
    Weapon {
        name: "twohand_sword",
        files: &["tsword_01"],
        anim_group: "twohand_sword",
        one_handed: false,
        type_id: 8,
    },
    Weapon {
        name: "dual_axe",
        files: &["axe_01_l", "axe_01_r"],
        anim_group: "dual_axe",
        one_handed: false,
        type_id: 9,
    },
    Weapon {
        name: "crossbow",
        files: &["crossbow_01"],
        anim_group: "bow",
        one_handed: false,
        type_id: 12,
    },
    Weapon {
        name: "warlock_rod",
        files: &["darkstaff_01"],
        anim_group: "onehand_staff",
        one_handed: true,
        type_id: 10,
    },
    Weapon {
        name: "twohand_staff",
        files: &["tstaff_01"],
        anim_group: "twohand_staff",
        one_handed: false,
        type_id: 11,
    },
    Weapon {
        name: "harp",
        files: &["harp_01"],
        anim_group: "harf",
        one_handed: false,
        type_id: 14,
    },
    Weapon {
        name: "cleric_rod",
        files: &["staff_01"],
        anim_group: "onehand_staff",
        one_handed: true,
        type_id: 15,
    },
];

pub(crate) const RACES: &[RaceSpec] = &[
    RaceSpec {
        label: "CH",
        race_dir: "china",
        male_body: "data://res/char/china/chinaman_adventurer.bsr",
        female_body: "data://res/char/china/chinawoman_adventurer.bsr",
        armor: CH_ARMOR,
        weapons: CH_WEAPONS,
        max_degree: 11,
    },
    RaceSpec {
        label: "EU",
        race_dir: "europe",
        male_body: "data://res/char/europe/europeman_adventurer.bsr",
        female_body: "data://res/char/europe/europewoman_adventurer.bsr",
        armor: EU_ARMOR,
        weapons: EU_WEAPONS,
        max_degree: 12,
    },
];

// Character bodies are ~18-19 units tall and ~3.5 units wide (verified by
// parsing a real character .bsr's mesh bounding boxes). X_SPACING is 5 body
// widths; Z/RACE_GAP keep rows and race blocks clearly separated.
const X_SPACING: f32 = 17.5;
const Z_SPACING: f32 = 35.0;
const RACE_GAP: f32 = 90.0;

/// Equipment degree to spawn, from the `EQUIP_DEGREE` env var (default 1),
/// clamped to the race's `max_degree`. Every stem exists at every degree, so
/// swapping the `_01` suffix is enough to inspect any tier's models.
fn equip_degree(max_degree: u32) -> u32 {
    std::env::var("EQUIP_DEGREE")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(1)
        .clamp(1, max_degree)
}

fn spawn_equipment_characters(mut commands: Commands, asset_server: Res<AssetServer>) {
    let mut race_z = 0.0;
    for race in RACES {
        for (armor_idx, armor) in race.armor.iter().enumerate() {
            let row_z = race_z + armor_idx as f32 * Z_SPACING;
            let mut x = 0.0;
            for weapon in race.weapons {
                for (gender_dir, body) in [("man", race.male_body), ("woman", race.female_body)] {
                    spawn_character(
                        &mut commands,
                        &asset_server,
                        Vec3::new(x, 0.0, row_z),
                        race,
                        gender_dir,
                        body,
                        armor,
                        weapon,
                    );
                    x += X_SPACING;
                }
            }
        }
        race_z += race.armor.len() as f32 * Z_SPACING + RACE_GAP;
    }
}

fn spawn_character(
    commands: &mut Commands,
    asset_server: &AssetServer,
    position: Vec3,
    race: &RaceSpec,
    gender_dir: &str,
    body_path: &'static str,
    armor: &ArmorSet,
    weapon: &Weapon,
) {
    // Mirror on X (scale.x = -1) like every SRO resource so the character
    // renders with correct handedness; the shared winding rule then reverses
    // its meshes because the placement determinant is negative.
    let transform = Transform::from_translation(position)
        .with_scale(Vec3::new(-1.0, 1.0, 1.0))
        .with_rotation(Quat::from_rotation_y(std::f32::consts::PI));
    let mut char_commands = commands.spawn((
        transform,
        Visibility::default(),
        EquipmentTestCharacter,
        Name::from(format!(
            "{} {gender_dir} {} {}",
            race.label, armor.label, weapon.name
        )),
        UnloadedResource(asset_server.load(body_path)),
        PreferredAnimationGroup(weapon.anim_group.to_string()),
    ));
    if needs_winding_reversal(&transform.to_matrix()) {
        char_commands.insert(MirroredResource);
    }
    let char_entity = char_commands.id();

    let degree = equip_degree(race.max_degree);

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

    // weapon(s): dual-wield weapons bring two models
    for file in weapon.files {
        let file = file.replace("_01", &format!("_{degree:02}"));
        let path = format!("data://res/item/{}/weapon/{file}.bsr", race.race_dir);
        commands.spawn((
            PendingItemAttachment(asset_server.load(path)),
            ChildOf(char_entity),
            Name::from(format!("weapon {}", weapon.name)),
        ));
    }

    // one-handed weapons keep the left hand free for a shield
    if weapon.one_handed {
        let path = format!(
            "data://res/item/{}/shield/shield_{degree:02}.bsr",
            race.race_dir
        );
        commands.spawn((
            PendingItemAttachment(asset_server.load(path)),
            ChildOf(char_entity),
            Name::from("shield"),
        ));
    }
}

fn frame_camera(
    mut camera_query: Query<&mut Transform, Or<(With<PlayerCamera>, With<DebugCamera>)>>,
) {
    // The grid's widest row is EU's 18 characters (~298 units); 6 rows plus a
    // race gap run ~265 deep. Characters face -Z, so the camera sits on the
    // -Z side and looks back, near-level (slightly above mid-body) so weapon
    // and armor orientation reads without top-down distortion.
    let look_at = Vec3::new(149.0, 9.0, 132.0);
    let eye = Vec3::new(149.0, 35.0, 132.0 - 420.0);
    for mut transform in camera_query.iter_mut() {
        *transform = Transform::from_translation(eye).looking_at(look_at, Vec3::Y);
    }
}

fn despawn_equipment_characters(
    query: Query<Entity, With<EquipmentTestCharacter>>,
    mut commands: Commands,
) {
    for entity in query.iter() {
        commands.entity(entity).despawn();
    }
}
