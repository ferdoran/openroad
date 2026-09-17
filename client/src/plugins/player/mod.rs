use bevy::animation::graph::{AnimationGraphHandle, AnimationNodeIndex};
use bevy::prelude::*;

use packets::agent::prelude::PlayerPickupAnimation;

use crate::assets::ban::JMXVBAN;
use crate::assets::bsk::JMXVBSK;
use crate::assets::bsr::resource::{
    SroResource, ANIM_GROUP_RIDING, ANIM_TYPE_ATTACKS, ANIM_TYPE_DAMAGE, ANIM_TYPE_DIE,
    ANIM_TYPE_DOWN_DAMAGE, ANIM_TYPE_DOWN_DIE, ANIM_TYPE_DOWN_ENTER, ANIM_TYPE_DOWN_LOOP,
    ANIM_TYPE_DOWN_UP, ANIM_TYPE_PICKUP, ANIM_TYPE_RUN, ANIM_TYPE_STAND, ANIM_TYPE_STUN,
    ANIM_TYPE_WALK,
};
use crate::commands::{animation_events, AnimEvent, SpawnedFromResource};
use crate::plugins::combat::{AttackSwing, DamagePopup, EntityDied};
use crate::plugins::cursor::interactions::GameCursorTarget;
use crate::plugins::dynamic_resource_loader::MirroredResource;
use crate::plugins::dynamic_resource_loader::{
    PendingItemAttachment, PreferredAnimationGroup, UnloadedResource,
};
use crate::plugins::effects::spawn::DelayedEffect;
use crate::plugins::hud::death::player_is_dead;
use crate::plugins::nav::{NavLocation, NavMeshRaycast, NavStep};
use crate::plugins::net::character_info::MovementSpeed;
use crate::plugins::net::entities::{NetworkEntities, RemoteEntity, RemoteMovement};
use crate::plugins::skills::cast::{PendingHit, PendingHits, SkillSwing, SkillSwingStarted};
use crate::plugins::skills::status::{Frozen, Stunned};
use crate::plugins::world_origin::WorldOrigin;
use crate::scenes::world_scene::SpawnPoints;
use crate::scenes::{in_playable_world, SceneState};
use crate::util::mesh::{mirrored, needs_winding_reversal};

pub mod collisions;

/// A player character's run speed as **characterdata** authors it
/// (`CHAR_CH_MAN*` Speed2 = 50). Every other row shares this scale —
/// `COS_C_HORSE1` 90, the `COS_C_*` line up to 150, monsters ~22 — and it is
/// the same unit as world positions: **units per second**, with a region
/// spanning 1920 of them.
const DATA_RUN_SPEED: f32 = 50.0;

/// What the live server multiplies a player's authored speed by before sending
/// it. Measured from `packet_dump/0x30d0.log`: characterdata 50 arrives as a
/// base 100 (110/120 when buffed), while a COS arrives at its authored value
/// unscaled — see `docs/re/systems/mount.md`.
const SERVER_PLAYER_SPEED_FACTOR: f32 = 2.0;

/// Move speed for the offline sandbox (`SceneState::WorldSandbox` and the other
/// serverless scenes), where nothing supplies a [`MovementSpeed`]; in the
/// networked scene the server-authoritative component wins.
///
/// Deliberately the speed a *live base player* has, so the sandbox moves at the
/// same absolute scale as the real game and speeds can be compared between the
/// two. Locally spawned entities keep their raw characterdata speeds for the
/// same reason — mixing a tuned sandbox constant with authored data made a
/// horse (90) move at less than half the player's, inverting the data's ratio.
const PLAYER_MOVE_SPEED: f32 = DATA_RUN_SPEED * SERVER_PLAYER_SPEED_FACTOR;
const PLAYER_TARGET_EPSILON: f32 = 2.0;
/// How quickly the character yaws toward its movement direction, as a slerp
/// fraction per second (higher = snappier turn).
const PLAYER_TURN_SPEED: f32 = 12.0;

/// The 6 pieces that make up a full armor set, by code-name suffix (cap /
/// chest / legs / boots / shoulder / gloves). Worn together they form the
/// whole armor look. Mirrors the equipment test scene.
const ARMOR_PIECES: [&str; 6] = ["ha", "ba", "la", "fa", "sa", "aa"];

/// A weapon a race can wield, selecting both the model(s) and the animation
/// stance the character uses.
pub struct WeaponSpec {
    pub label: &'static str,
    /// Weapon `.bsr` file stems under `item/<race>/weapon/`. Dual-wield
    /// weapons list two (`_l` + `_r`); all others one.
    pub files: &'static [&'static str],
    /// Character animation group (weapon class) selecting the stance.
    pub anim_group: &'static str,
    /// One-handed weapons leave the left hand free and carry a shield.
    pub one_handed: bool,
}

/// One of a race's armor types, a full set being [`ARMOR_PIECES`] of its
/// item category worn at once.
pub struct ArmorSpec {
    pub label: &'static str,
    pub category: &'static str,
}

/// Everything appearance-related that varies per race: the body meshes for
/// each gender plus the armor and weapon options available to that race.
pub struct RaceSpec {
    pub label: &'static str,
    /// Resource-path race segment: `china` or `europe`.
    pub race_dir: &'static str,
    pub male_body: &'static str,
    pub female_body: &'static str,
    pub armor: &'static [ArmorSpec],
    pub weapons: &'static [WeaponSpec],
}

const CH_ARMOR: &[ArmorSpec] = &[
    ArmorSpec {
        label: "Garment",
        category: "clothes",
    },
    ArmorSpec {
        label: "Protector",
        category: "light",
    },
    ArmorSpec {
        label: "Armor",
        category: "heavy",
    },
];

const EU_ARMOR: &[ArmorSpec] = &[
    ArmorSpec {
        label: "Robe",
        category: "clothes",
    },
    ArmorSpec {
        label: "Light Armor",
        category: "light",
    },
    ArmorSpec {
        label: "Heavy Armor",
        category: "heavy",
    },
];

const CH_WEAPONS: &[WeaponSpec] = &[
    WeaponSpec {
        label: "Sword",
        files: &["sword_01"],
        anim_group: "sword",
        one_handed: true,
    },
    WeaponSpec {
        label: "Blade",
        files: &["blade_01"],
        anim_group: "sword",
        one_handed: true,
    },
    WeaponSpec {
        label: "Spear",
        files: &["spear_01"],
        anim_group: "spear",
        one_handed: false,
    },
    WeaponSpec {
        label: "Glaive",
        files: &["tblade_01"],
        anim_group: "spear",
        one_handed: false,
    },
    WeaponSpec {
        label: "Bow",
        files: &["bow_01"],
        anim_group: "bow",
        one_handed: false,
    },
];

const EU_WEAPONS: &[WeaponSpec] = &[
    WeaponSpec {
        label: "Dagger",
        files: &["dagger_01_l", "dagger_01_r"],
        anim_group: "dagger",
        one_handed: false,
    },
    WeaponSpec {
        label: "One-hand Sword",
        files: &["sword_01"],
        anim_group: "onehand_sword",
        one_handed: true,
    },
    WeaponSpec {
        label: "Two-hand Sword",
        files: &["tsword_01"],
        anim_group: "twohand_sword",
        one_handed: false,
    },
    WeaponSpec {
        label: "Dual Axe",
        files: &["axe_01_l", "axe_01_r"],
        anim_group: "dual_axe",
        one_handed: false,
    },
    WeaponSpec {
        label: "Crossbow",
        files: &["crossbow_01"],
        anim_group: "bow",
        one_handed: false,
    },
    WeaponSpec {
        label: "Warlock Rod",
        files: &["darkstaff_01"],
        anim_group: "onehand_staff",
        one_handed: true,
    },
    WeaponSpec {
        label: "Two-hand Staff",
        files: &["tstaff_01"],
        anim_group: "twohand_staff",
        one_handed: false,
    },
    WeaponSpec {
        label: "Harp",
        files: &["harp_01"],
        anim_group: "harf",
        one_handed: false,
    },
    WeaponSpec {
        label: "Cleric Rod",
        files: &["staff_01"],
        anim_group: "onehand_staff",
        one_handed: true,
    },
];

/// The races a player character can be created from, indexed by
/// [`PlayerConfig::race`].
pub const RACES: &[RaceSpec] = &[
    RaceSpec {
        label: "China",
        race_dir: "china",
        male_body: "data://res/char/china/chinaman_adventurer.bsr",
        female_body: "data://res/char/china/chinawoman_adventurer.bsr",
        armor: CH_ARMOR,
        weapons: CH_WEAPONS,
    },
    RaceSpec {
        label: "Europe",
        race_dir: "europe",
        male_body: "data://res/char/europe/europeman_adventurer.bsr",
        female_body: "data://res/char/europe/europewoman_adventurer.bsr",
        armor: EU_ARMOR,
        weapons: EU_WEAPONS,
    },
];

/// The current player's appearance/equipment, edited live from the player
/// debug window and read by [`setup_player`] when the character is spawned.
/// The indices point into [`RACES`] and the selected race's armor/weapon
/// lists; [`PlayerConfig::clamp`] keeps them valid when the race changes.
#[derive(Resource, Clone, Copy)]
pub struct PlayerConfig {
    pub race: usize,
    pub female: bool,
    pub armor: usize,
    pub weapon: usize,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        // China man, Garment, Sword + Shield — the original hardcoded look.
        PlayerConfig {
            race: 0,
            female: false,
            armor: 0,
            weapon: 0,
        }
    }
}

impl PlayerConfig {
    pub fn race(&self) -> &'static RaceSpec {
        &RACES[self.race]
    }

    pub fn armor(&self) -> &'static ArmorSpec {
        &self.race().armor[self.armor]
    }

    pub fn weapon(&self) -> &'static WeaponSpec {
        &self.race().weapons[self.weapon]
    }

    /// Directory segment for gender-specific item paths (`man` / `woman`).
    pub fn gender_dir(&self) -> &'static str {
        if self.female {
            "woman"
        } else {
            "man"
        }
    }

    pub fn body(&self) -> &'static str {
        let race = self.race();
        if self.female {
            race.female_body
        } else {
            race.male_body
        }
    }

    /// Keep armor/weapon indices in range after the race (which has its own
    /// armor/weapon lists) changes.
    pub fn clamp(&mut self) {
        self.race = self.race.min(RACES.len() - 1);
        let race = self.race();
        self.armor = self.armor.min(race.armor.len() - 1);
        self.weapon = self.weapon.min(race.weapons.len() - 1);
    }
}

pub enum PlayerCommand {
    MoveTo(Vec3),
    // Suggestions:
    // Attack(BattleTarget),
    // UseSkill(Skill)
}

#[derive(Default, Resource)]
pub struct PlayerCommands(Vec<PlayerCommand>);

impl PlayerCommands {
    pub fn has_any_commands(&self) -> bool {
        return self.0.len() > 0;
    }
    /// Whether the player is currently walking toward a target. The `MoveTo`
    /// order stays queued while moving and is cleared once the target is
    /// reached or the path is blocked, so this drives the run animation.
    pub fn is_moving(&self) -> bool {
        self.0
            .iter()
            .any(|command| matches!(command, PlayerCommand::MoveTo(_)))
    }

    /// The queued walk destination, if any — what the combat gap-close
    /// checks for staleness against a drifting target.
    pub fn move_destination(&self) -> Option<Vec3> {
        self.0.iter().find_map(|command| match command {
            PlayerCommand::MoveTo(position) => Some(*position),
        })
    }
    pub fn move_to(&mut self, position: Vec3) {
        // Check if there's an old MoveTo command and replace it if found
        if let Some(index) = self
            .0
            .iter()
            .position(|command| matches!(command, PlayerCommand::MoveTo(_)))
        {
            self.0[index] = PlayerCommand::MoveTo(position);
        } else {
            // If no old MoveTo command exists, push a new one
            self.0.push(PlayerCommand::MoveTo(position));
        }
    }
    /// Cancel any pending movement (e.g. when the server rejects a move).
    pub fn stop(&mut self) {
        self.0
            .retain(|command| !matches!(command, PlayerCommand::MoveTo(_)));
    }
}

/// A request to move the player to a render-space point, emitted on click-to-move
/// (`nav::decal::place_decal_on_click`). It's always applied locally right away
/// (optimistic prediction); in GameWorld the networked path additionally sends a
/// movement request and reconciles against the server's response (see
/// `game_scene::send_movement_request` / `on_movement_response`).
#[derive(Message)]
pub struct PlayerMoveOrder(pub Vec3);

pub struct PlayerPlugin;
impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerCommands>()
            .init_resource::<PlayerConfig>()
            .init_resource::<PendingHitReactions>()
            .init_resource::<PendingKnockdowns>()
            .add_message::<PlayerMoveOrder>()
            .add_systems(OnEnter(SceneState::WorldSandbox), setup_player)
            .add_systems(
                Update,
                (
                    // Dead players must not walk. This is the single choke point
                    // every movement source funnels through (click-to-move, the
                    // local move order, and combat's close_attack_gap chase), so
                    // gating it here stops them all — the corpse used to slide
                    // after the mob because the chase kept re-queueing a move
                    // every frame while the die pose was frozen.
                    handle_player_commands.run_if(not(player_is_dead)),
                    // Before the movement path: a stance that no longer matches
                    // the character's weapon is dropped here so the rebuild
                    // happens in the same frame rather than the next one.
                    rebuild_stance_on_weapon_change,
                    update_character_animation,
                    // After the movement path (which skips riders) and before
                    // the swing systems, same as the other pose drivers.
                    play_mounted_pose,
                    reset_pose_after_dismount,
                    play_attack_swings,
                    play_skill_swings,
                    advance_charging_casts,
                    play_pickup_animations,
                    // Before the flinch: a body going down must not also play
                    // the standing hit reaction from the very same packet.
                    play_knockdowns,
                    play_hit_reactions,
                    advance_knockdowns,
                    play_death_animations,
                    revive_player_animation,
                    interrupt_incapacitated_casts,
                    cancel_swings,
                )
                    .chain()
                    .run_if(in_playable_world),
            )
            // Optimistic: start moving on the click; the server response
            // reconciles (or a rejection stops the player) in game_scene.
            .add_systems(Update, apply_local_move_order.run_if(in_playable_world));
    }
}

fn apply_local_move_order(
    mut orders: MessageReader<PlayerMoveOrder>,
    mut player_commands: ResMut<PlayerCommands>,
    rider: Option<Res<crate::plugins::cos::RiderState>>,
) {
    // Mounted: the click drives the COS (cos::riding routes it), and the
    // rider is transform-slaved — walking the player would fight the slave.
    if rider.is_some_and(|r| r.0.is_some()) {
        orders.clear();
        return;
    }
    for order in orders.read() {
        player_commands.move_to(order.0);
    }
}

/// The locally controlled character.
///
/// Requires [`NavLocation`]: movement is resolved against the surface the
/// player is standing on, and `handle_player_commands` queries that component
/// mutably — a `Player` spawned without it would silently stop moving.
#[derive(Component, Clone, Copy)]
#[require(NavLocation)]
pub struct Player;

impl Player {
    pub fn new() -> Self {
        Player {}
    }
}

fn setup_player(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    config: Res<PlayerConfig>,
    origin: Res<WorldOrigin>,
) {
    spawn_player_character(
        &mut commands,
        &asset_server,
        &config,
        // Spawn on the world's start point (jangan), where the terrain and the
        // player camera live, so the follow camera has the character in view.
        Transform::from_translation(origin.to_render(SpawnPoints::jangan())),
    );
}

/// Spawn the playable character the same way the equipment test scene does:
/// a stable gameplay parent carries Player/movement state while the SRO body
/// resource and its pending item attachments load underneath it. The body,
/// armor set and weapon are chosen from `config`, so both the initial spawn
/// and the debug window's respawn go through here.
pub fn spawn_player_character(
    commands: &mut Commands,
    asset_server: &AssetServer,
    config: &PlayerConfig,
    transform: Transform,
) -> Entity {
    let race = config.race();
    let weapon = config.weapon();
    let gender_dir = config.gender_dir();

    // The SRO -> Bevy handedness mirror, applied here rather than by the four
    // callers (which all pass a translation-only transform). Every other SRO
    // resource is placed under it — map objects, dungeon props, the char-select
    // previews, every test scene — and `util::mesh` is the single source of
    // truth for why. The in-world character paths were the ones the migration
    // missed, which is what put the weapon in the wrong hand and mirrored
    // asymmetric armour.
    let transform = mirrored(transform);
    let player_entity = commands
        .spawn((
            Name::from("Player"),
            Player::new(),
            GameCursorTarget::default(),
            UnloadedResource(asset_server.load(config.body())),
            PreferredAnimationGroup(weapon.anim_group.to_string()),
            transform,
            Visibility::default(),
        ))
        .id();
    if needs_winding_reversal(&transform.to_matrix()) {
        commands.entity(player_entity).insert(MirroredResource);
    }

    // full armor set: all 6 pieces of the selected category worn together
    for piece in ARMOR_PIECES {
        let path = format!(
            "data://res/item/{}/{gender_dir}_item/{}_01_{piece}.bsr",
            race.race_dir,
            config.armor().category,
        );
        commands.spawn((
            PendingItemAttachment(asset_server.load(path)),
            ChildOf(player_entity),
            Name::from(format!("player armor {piece}")),
        ));
    }

    // weapon(s): dual-wield weapons bring two models
    for file in weapon.files {
        commands.spawn((
            PendingItemAttachment(asset_server.load(format!(
                "data://res/item/{}/weapon/{file}.bsr",
                race.race_dir
            ))),
            ChildOf(player_entity),
            Name::from("player weapon"),
        ));
    }

    // one-handed weapons keep the left hand free for a shield
    if weapon.one_handed {
        commands.spawn((
            PendingItemAttachment(asset_server.load(format!(
                "data://res/item/{}/shield/shield_01.bsr",
                race.race_dir
            ))),
            ChildOf(player_entity),
            Name::from("player shield"),
        ));
    }

    player_entity
}

fn handle_player_commands(
    mut player_commands: ResMut<PlayerCommands>,
    time: Res<Time>,
    nav_raycast: NavMeshRaycast,
    mut player_query: Query<
        (
            Entity,
            &mut Transform,
            Option<&MovementSpeed>,
            &mut NavLocation,
        ),
        With<Player>,
    >,
) {
    if !player_commands.has_any_commands() {
        return;
    }
    if let Ok((_entity, mut transform, speed, mut nav_location)) = player_query.single_mut() {
        let Some(PlayerCommand::MoveTo(target)) = player_commands.0.first() else {
            return;
        };
        let target = *target;
        let current_xz = transform.translation.xz();
        let target_xz = target.xz();
        let to_target = target_xz - current_xz;
        let distance = to_target.length();

        if distance <= PLAYER_TARGET_EPSILON {
            // Snap to the target XZ, but take the height from the nav surface,
            // not the click target's Y. The target's Y came from the cursor
            // raycast against the terrain height field, which sits *below* an
            // ice sheet — using it directly dropped the player through the ice
            // onto the lake bed on arrival. `ground()` resolves the surface the
            // player is actually on (ice included; see plugins::nav).
            let (height, location) = nav_raycast
                .ground(target_xz, transform.translation.y, *nav_location)
                .unwrap_or((target.y, *nav_location));
            transform.translation = Vec3::new(target_xz.x, height, target_xz.y);
            *nav_location = location;
            player_commands.0.clear();
            return;
        }

        let direction = to_target / distance;

        // Turn to face the walking direction (yaw only, around Y). The SRO body
        // faces -Z, so we add PI to the +Z yaw to point the front along the move
        // direction; slerp gives a smooth turn instead of an instant snap.
        let target_rotation =
            Quat::from_rotation_y(direction.x.atan2(direction.y) + std::f32::consts::PI);
        transform.rotation = transform.rotation.slerp(
            target_rotation,
            (PLAYER_TURN_SPEED * time.delta_secs()).min(1.0),
        );

        // Server-authoritative speed when present (networked scene), the local
        // constant otherwise (offline sandbox).
        let move_speed = speed
            .map(|s| s.current())
            .filter(|v| *v > 0.0)
            .unwrap_or(PLAYER_MOVE_SPEED);
        let step = (move_speed * time.delta_secs()).min(distance);
        let next_xz = current_xz + direction * step;

        // One stateful query does both the wall test and the ground snap,
        // resolved against the surface the player is actually standing on (see
        // plugins::nav::location).
        match nav_raycast.step(transform.translation, next_xz, *nav_location) {
            NavStep::Moved { position, location } => {
                transform.translation = position;
                *nav_location = location;
            }
            // Walked into a wall: hold position and drop the move order.
            NavStep::Blocked => {
                player_commands.0.clear();
            }
            // Nav data for the destination isn't streamed in yet. Hold position
            // and retry next frame — never treat this as a wall, or the player
            // gets stuck at streaming seams.
            NavStep::Unknown => {}
        }
    }
}

/// The stand/run/attack clips of a spawned character body, held in one
/// animation graph so [`drive_character_animation`] can cross between them by
/// just switching the played node instead of rebuilding a clip every time
/// movement starts/stops or a swing lands.
/// Which locomotion clip an entity should be playing, decided by its owner
/// (input for the local player, the server's motion state for remotes) and
/// resolved to an animation type by [`Gait::anim_type`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Gait {
    Standing,
    /// The server put this entity in the walking motion state
    /// (`MOTION_STATE_WALK`).
    Walking,
    Running,
}

impl Gait {
    /// The animation type to play. `has_walk` is whether the resource's group
    /// actually resolves a WALK clip: only 958 of the corpus's groups do
    /// (`docs/re/formats/anim-state-coverage.md`), and a walking entity
    /// without one keeps the run clip it used before this existed rather than
    /// freezing on stand.
    fn anim_type(self, has_walk: bool) -> u32 {
        match self {
            Gait::Standing => ANIM_TYPE_STAND,
            Gait::Walking if has_walk => ANIM_TYPE_WALK,
            Gait::Walking | Gait::Running => ANIM_TYPE_RUN,
        }
    }
}

#[derive(Component)]
struct MovementAnims {
    stand: AnimationNodeIndex,
    run: AnimationNodeIndex,
    /// The walk clip (type 1), when the resource's group has one.
    walk: Option<AnimationNodeIndex>,
    /// Whichever basic-attack swings (ANI_ATTACK1..4) the resource's group
    /// resolves — may be empty for props that animate but never fight.
    attacks: Vec<AttackAnim>,
    /// Whichever hit reactions (DAMAGE1/DAMAGE2, types 3/9) the resource's
    /// group resolves, in that order — sparse like [`Self::attacks`], and
    /// empty for bodies that ship no flinch at all.
    damage: Vec<AnimationNodeIndex>,
    /// The death clip (type 4), when the resource has one.
    die: Option<AnimationNodeIndex>,
    /// The pickup bow-down clip (type 38), when the resource has one.
    pickup: Option<AnimationNodeIndex>,
    /// The dizzy/stun loop (type 79), when the resource has one — 181 of the
    /// corpus's groups carry it. Held while the entity is `Stunned`/`Frozen`;
    /// a group without one keeps the stand pose it used before.
    stun: Option<AnimationNodeIndex>,
    /// Animation type id currently playing (see [`ANIM_TYPE_STAND`]/
    /// [`ANIM_TYPE_RUN`]); [`play_attack_swings`] parks it at `u32::MAX` so
    /// the movement path re-plays stand/run once a one-shot ends.
    current: u32,
    /// The animation group these clips were resolved from — the *requested*
    /// group (`PreferredAnimationGroup`), not whatever `find_animation` fell
    /// back to.
    ///
    /// This is what makes the set invalidatable. The graph below is built
    /// exactly once per wrapper and [`drive_character_animation`] short-circuits
    /// on the presence of this component, so a weapon swap used to leave a
    /// swordsman running the sword stance while holding a bow — nothing
    /// compared the built set against the stance the character now wants.
    /// [`rebuild_stance_on_weapon_change`] does that comparison.
    group: Option<String>,
}

/// The gait a remote entity is driven with: it only distinguishes walk from
/// run while it is actually moving — a parked entity stands no matter which
/// motion state the server last sent.
fn remote_gait(moving: bool, walking: bool) -> Gait {
    match (moving, walking) {
        (false, _) => Gait::Standing,
        (true, true) => Gait::Walking,
        (true, false) => Gait::Running,
    }
}

/// One resolved basic-attack swing clip plus its combat-hit moments.
struct AttackAnim {
    node: AnimationNodeIndex,
    /// Seconds into the clip of each typ-1 (combat hit) animation event,
    /// ascending — when the Nth damage instance visually lands.
    hit_times: Vec<f32>,
}

/// How strongly a one-shot clip owns the body.
///
/// Every clip that takes the wrapper away from the movement path is a one-shot,
/// but they are not equals: a 40 ms flinch and a 2.4 s combo used to wear the
/// same bare marker, so "a reaction must never cut a cast" could not be stated
/// at all — each system re-implemented its own `dead || one_shot || charging`
/// guard, two systems forgot to, and the rule drifted. Ranking the clips lets
/// [`may_take_body`] decide once, in one place.
///
/// Ordering is the whole point of the type: derive `Ord` follows declaration
/// order, lowest first.
///
/// Death is deliberately absent: it is not a one-shot but a *state*
/// ([`DeadWrapper`]) that every system here checks before anything else. The
/// knockdown is a state too ([`KnockedDown`]), but *entering* it is a clip like
/// any other and does need a rank — leaving it out is exactly what broke enemy
/// knockdowns.
#[derive(Component, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum ClipPriority {
    /// A flinch — the shortest and most interruptible thing a body plays.
    Reaction,
    /// A basic-attack swing, a pickup, a mount pose: the body's own doing, but
    /// yieldable to a cast.
    Action,
    /// A skill cast (its wind-up included). A chain's clip runs 2.4-5.1 s and
    /// must survive every hit the caster takes across it.
    Cast,
    /// The knockdown sequence. The server has said this body is on the ground,
    /// so it takes the body from anything — flinch, swing or cast. Nothing
    /// outranks it, so [`may_take_body`] never refuses one.
    Knockdown,
}

/// Marks a body wrapper that is playing a one-shot clip of the given rank;
/// movement animation switching pauses until the player reports it finished.
#[derive(Component)]
pub(crate) struct OneShotAttack(pub(crate) ClipPriority);

/// Whether a clip of rank `want` may take a body currently owned by `running`.
///
/// Equal rank wins, so a new swing replaces the previous swing and a new cast
/// replaces a cast; a strictly lower rank waits. An idle body (`None`) is
/// always available.
fn may_take_body(running: Option<ClipPriority>, want: ClipPriority) -> bool {
    match running {
        None => true,
        Some(held) => {
            if want < held {
                // The audit that produced the ranking covered every system in
                // this module; a refusal here therefore means something ELSE
                // reached the wrapper, and the log is how it names itself.
                debug!("animation: {want:?} clip yields to a running {held:?} clip");
                false
            } else {
                true
            }
        }
    }
}

/// Body wrappers a one-shot clip currently owns — an attack swing in flight,
/// or a cast still in its charge phase.
pub(crate) type BusyWrappers<'w, 's> =
    Query<'w, 's, (), Or<(With<OneShotAttack>, With<ChargingCast>)>>;

/// Whether `owner`'s body wrapper is mid one-shot.
///
/// The guard for "don't predict a swing right now": an optimistic swing played
/// over a running one restarts the clip from frame 0 and re-fires every sound
/// keyed to it. The wrapper is a CHILD of the owner (bodies spawn
/// asynchronously under the root), so this hops through `Children` rather than
/// testing the owner itself. The marker components stay private to this
/// module; callers ask the question instead of reading the state.
pub(crate) fn is_mid_one_shot(
    owner: Entity,
    children: &Query<&Children>,
    busy: &BusyWrappers,
) -> bool {
    children
        .get(owner)
        .into_iter()
        .flat_map(|children| children.iter())
        .any(|child| busy.contains(child))
}

/// Whether `owner`'s body is mid **cast** — a skill clip (its wind-up
/// included), or anything ranked above one.
///
/// The narrower sibling of [`is_mid_one_shot`], and the one to ask before
/// holding back a NETWORK send. `is_mid_one_shot` is rank-blind: it also
/// reports a ~0.3 s flinch, which [`ClipPriority`] itself calls "the shortest
/// and most interruptible thing a body plays". Stalling the wire on one of
/// those would hold the player's next action for something the animation
/// layer is happy to interrupt.
pub(crate) fn is_mid_cast(
    owner: Entity,
    children: &Query<&Children>,
    casting: &CastingWrappers,
) -> bool {
    children
        .get(owner)
        .into_iter()
        .flat_map(|children| children.iter())
        .any(|child| match casting.get(child) {
            Ok((running, charging)) => {
                charging || running.is_some_and(|r| r.0 >= ClipPriority::Cast)
            }
            Err(_) => false,
        })
}

/// Body wrappers with their one-shot rank, for [`is_mid_cast`]. Separate from
/// [`BusyWrappers`] because that one only has to answer yes/no; this one has
/// to read the rank.
pub(crate) type CastingWrappers<'w, 's> =
    Query<'w, 's, (Option<&'static OneShotAttack>, Has<ChargingCast>)>;

/// Marks a dead body wrapper: the die clip owns the player permanently, the
/// movement path never touches it again (the root despawns via `Dying`).
#[derive(Component)]
struct DeadWrapper;

/// Build an [`AnimationClip`] for the resource's `idx`-th animation, retargeted
/// to its own skeleton. `None` if the `.ban` file hasn't finished loading.
fn build_movement_clip(
    resource: &SroResource,
    skeleton: &JMXVBSK,
    idx: usize,
    ban_assets: &Assets<JMXVBAN>,
    animation_clips: &mut Assets<AnimationClip>,
) -> Option<Handle<AnimationClip>> {
    let ban = ban_assets.get(resource.animation.animations.get(idx)?)?;
    let clip = ban.to_animation_clip(skeleton, &resource.object_info.name);
    Some(animation_clips.add(clip))
}

/// The `wrapper_query` shared by [`update_character_animation`] and
/// [`drive_character_animation`]: every spawned body wrapper, regardless of who
/// owns it, matched by [`SpawnedFromResource`] + its [`AnimationPlayer`].
type CharacterWrapperQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static SpawnedFromResource,
        &'static mut AnimationPlayer,
        &'static mut AnimationGraphHandle,
        Option<&'static mut MovementAnims>,
        Option<&'static OneShotAttack>,
        Has<DeadWrapper>,
        Has<ChargingCast>,
    ),
>;

/// Plays a character's run animation while it is moving and the idle stand
/// animation otherwise, for a single owner's `children`. The body wrapper
/// (spawned asynchronously by the dynamic resource loader) carries the
/// [`AnimationPlayer`]; on first sight we build a graph holding both stand and
/// run clips, then just swap the played node when the movement state flips.
///
/// `group` is the animation group to resolve clips from (a weapon stance for
/// players; `None` for NPCs/monsters, which fall back to the "default" group).
/// Shared by the local player and every remote entity via
/// [`update_character_animation`].
#[allow(clippy::too_many_arguments)]
fn drive_character_animation(
    commands: &mut Commands,
    children: &Children,
    group: Option<&str>,
    gait: Gait,
    mounted: bool,
    stunned: bool,
    wrapper_query: &mut CharacterWrapperQuery,
    knocked: &Query<(), With<KnockedDown>>,
    sro_resources: &Assets<SroResource>,
    bsk_assets: &Assets<JMXVBSK>,
    ban_assets: &Assets<JMXVBAN>,
    animation_clips: &mut Assets<AnimationClip>,
    animation_graphs: &mut Assets<AnimationGraph>,
    budget: &mut i32,
) {
    for child in children.iter() {
        let Ok((
            entity,
            spawned,
            mut anim_player,
            mut graph_handle,
            movement,
            running,
            dead,
            charging,
        )) = wrapper_query.get_mut(child)
        else {
            continue;
        };
        // A corpse keeps its die clip's last frame until the root despawns.
        if dead {
            continue;
        }
        // A mid-charge wrapper holds its ready clip's last frame until the
        // shot fires (the ready clip is played once, so `all_finished()`
        // would otherwise resume stand mid-wind-up).
        if charging {
            continue;
        }
        // A knocked-down body belongs to `advance_knockdowns` until it has
        // finished getting up; the same reasoning as `charging` above, but
        // across three clips instead of one.
        if knocked.contains(entity) {
            continue;
        }

        // Already set up: switch the played clip only when movement state flips.
        if let Some(mut movement) = movement {
            // A one-shot attack owns the player until it finishes; `current`
            // is parked at the sentinel, so the switch below then fires.
            if running.is_some() {
                if !anim_player.all_finished() {
                    continue;
                }
                commands.entity(entity).try_remove::<OneShotAttack>();
            }
            // A stunned body holds the dizzy loop (79) instead of stand/run for
            // as long as the status lasts. Groups without the clip (all but
            // 181 of the corpus) fall through to the normal gait, which is
            // what every entity did before this.
            if stunned {
                if let Some(stun) = movement.stun {
                    if movement.current != ANIM_TYPE_STUN {
                        anim_player.stop_all();
                        anim_player.play(stun).repeat();
                        movement.current = ANIM_TYPE_STUN;
                    }
                    continue;
                }
            }
            // A rider's pose belongs to `play_mounted_pose` (the `cart` group).
            // Note this skips only the stand/run *switch* — the graph build
            // below must still run for someone who is already mounted when
            // their body loads (relog in the saddle), or they would animate
            // never at all.
            if mounted {
                continue;
            }
            let desired = gait.anim_type(movement.walk.is_some());
            if movement.current == desired {
                continue;
            }
            let node = match desired {
                ANIM_TYPE_WALK => movement.walk.unwrap_or(movement.run),
                ANIM_TYPE_RUN => movement.run,
                _ => movement.stand,
            };
            anim_player.stop_all();
            anim_player.play(node).repeat();
            movement.current = desired;
            continue;
        }

        // First sight of the loaded body: build the combined stand/run graph.
        let Some(resource) = sro_resources.get(&spawned.0) else {
            continue;
        };
        let Some(skeleton_handle) = &resource.skeleton else {
            continue;
        };
        let Some(skeleton) = bsk_assets.get(skeleton_handle) else {
            continue;
        };

        // The graph is built exactly once, so wait until every attack/die/
        // pickup .ban this group resolves has decoded — otherwise
        // late-loading clips would be silently missing forever. (Checked
        // before any clip build to avoid re-adding clip assets on retry
        // frames.) Attack entries carry their whole event list (#276); the
        // hit-synced damage popups read the typ-1 subset out of it, and the
        // clip timeline `to_animation_clip` builds is the keytime / 1000.
        let attack_entries: Vec<(u32, usize, Vec<AnimEvent>)> = ANIM_TYPE_ATTACKS
            .iter()
            .filter_map(|&typ| Some((typ, resource.find_animation_entry(group, typ)?)))
            .map(|(typ, anim)| (typ, anim.file_index as usize, animation_events(anim)))
            .collect();
        // The flinch clips, in DAMAGE1, DAMAGE2 order; sparse (only ~480/267
        // of the corpus's groups carry them, docs/re/formats/
        // anim-state-coverage.md §2), so an empty list is normal.
        let damage_entries: Vec<(u32, usize)> = ANIM_TYPE_DAMAGE
            .iter()
            .filter_map(|&typ| Some((typ, resource.find_animation(group, typ)?)))
            .collect();
        let die_idx = resource.find_animation(group, ANIM_TYPE_DIE);
        let pickup_idx = resource.find_animation(group, ANIM_TYPE_PICKUP);
        // Optional like WALK — 181 of the corpus's groups carry a stun loop
        // (`docs/re/formats/anim-state-coverage.md` §2); the rest keep standing.
        let stun_idx = resource.find_animation(group, ANIM_TYPE_STUN);
        if attack_entries
            .iter()
            .map(|(_, idx, _)| *idx)
            .chain(damage_entries.iter().map(|(_, idx)| *idx))
            .chain(die_idx)
            .chain(pickup_idx)
            .chain(stun_idx)
            .any(|idx| {
                resource
                    .animation
                    .animations
                    .get(idx)
                    .is_some_and(|handle| ban_assets.get(handle).is_none())
            })
        {
            continue;
        }

        // Budgeted: this entity's clips are all decoded and ready to build,
        // but building spends real CPU on decoding into an AnimationClip —
        // see ANIMATION_BUILDS_PER_FRAME. A skipped entity costs nothing to
        // retry: it simply keeps `MovementAnims` absent, which is exactly
        // the condition that routed it into this branch.
        if *budget <= 0 {
            continue;
        }
        *budget -= 1;

        let Some(stand_idx) = resource.find_animation(group, ANIM_TYPE_STAND) else {
            continue;
        };
        let Some(stand_clip) =
            build_movement_clip(resource, skeleton, stand_idx, ban_assets, animation_clips)
        else {
            continue;
        };
        // Fall back to the stand clip for weapon stances that have no run animation.
        // (Only stand/run are mapped today; movement uses the server's run speed
        // via `MovementSpeed` — selecting the walk *animation* is a follow-up.)
        let run_clip = resource
            .find_animation(group, ANIM_TYPE_RUN)
            .and_then(|idx| {
                build_movement_clip(resource, skeleton, idx, ban_assets, animation_clips)
            })
            .unwrap_or_else(|| stand_clip.clone());
        // Optional, unlike stand/run: a group without a WALK clip keeps
        // running, which is what every entity did before #275.
        let walk_clip = resource
            .find_animation(group, ANIM_TYPE_WALK)
            .and_then(|idx| {
                build_movement_clip(resource, skeleton, idx, ban_assets, animation_clips)
            });

        let mut graph = AnimationGraph::new();
        let stand = graph.add_clip(stand_clip, 1.0, graph.root);
        let run = graph.add_clip(run_clip, 1.0, graph.root);
        // Rebuilding the graph orphans the AnimationLibrary built at spawn
        // (its node indices point into the discarded graph), which would
        // silently break the animation-gated effects (death smoke on
        // "default"/4). Collect replacement entries for every node we add
        // and swap the library alongside the graph.
        let group_name = group.unwrap_or("default");
        let mut library_entries: Vec<crate::commands::AnimationLibraryEntry> = Vec::new();
        let library_entry = |anim_type: u32, node: AnimationNodeIndex, events: Vec<AnimEvent>| {
            crate::commands::AnimationLibraryEntry {
                group: group_name.to_string(),
                anim_type,
                label: format!("{group_name}/{anim_type}"),
                node,
                events,
            }
        };
        // Locomotion entries were given an empty event list before #276, which
        // is precisely where the 741 type-2 (footstep-shaped) events live —
        // WALK and RUN. They reach the library now; nothing consumes them yet.
        let movement_events = |typ: u32| {
            resource
                .find_animation_entry(group, typ)
                .map(animation_events)
                .unwrap_or_default()
        };
        library_entries.push(library_entry(
            ANIM_TYPE_STAND,
            stand,
            movement_events(ANIM_TYPE_STAND),
        ));
        library_entries.push(library_entry(
            ANIM_TYPE_RUN,
            run,
            movement_events(ANIM_TYPE_RUN),
        ));
        let walk = walk_clip.map(|clip| {
            let node = graph.add_clip(clip, 1.0, graph.root);
            library_entries.push(library_entry(
                ANIM_TYPE_WALK,
                node,
                movement_events(ANIM_TYPE_WALK),
            ));
            node
        });

        // Whichever basic-attack swings this group has (played one-shot by
        // `play_attack_swings`; sparse — monsters usually carry only two).
        let attacks: Vec<AttackAnim> = attack_entries
            .into_iter()
            .filter_map(|(typ, idx, events)| {
                let clip =
                    build_movement_clip(resource, skeleton, idx, ban_assets, animation_clips)?;
                let node = graph.add_clip(clip, 1.0, graph.root);
                let entry = library_entry(typ, node, events);
                let hit_times = entry
                    .hit_events()
                    .into_iter()
                    .map(|ms| ms as f32 / 1000.0)
                    .collect();
                library_entries.push(entry);
                Some(AttackAnim { node, hit_times })
            })
            .collect();
        // Hit reactions, played one-shot by `play_hit_reactions`.
        let damage: Vec<AnimationNodeIndex> = damage_entries
            .into_iter()
            .filter_map(|(typ, idx)| {
                let clip =
                    build_movement_clip(resource, skeleton, idx, ban_assets, animation_clips)?;
                let node = graph.add_clip(clip, 1.0, graph.root);
                library_entries.push(library_entry(typ, node, Vec::new()));
                Some(node)
            })
            .collect();
        let die = die_idx.and_then(|idx| {
            let clip = build_movement_clip(resource, skeleton, idx, ban_assets, animation_clips)?;
            let node = graph.add_clip(clip, 1.0, graph.root);
            library_entries.push(library_entry(ANIM_TYPE_DIE, node, Vec::new()));
            Some(node)
        });
        let pickup = pickup_idx.and_then(|idx| {
            let clip = build_movement_clip(resource, skeleton, idx, ban_assets, animation_clips)?;
            let node = graph.add_clip(clip, 1.0, graph.root);
            library_entries.push(library_entry(ANIM_TYPE_PICKUP, node, Vec::new()));
            Some(node)
        });
        // The dizzy loop held while `Stunned`/`Frozen`, by
        // `interrupt_incapacitated_casts`.
        let stun = stun_idx.and_then(|idx| {
            let clip = build_movement_clip(resource, skeleton, idx, ban_assets, animation_clips)?;
            let node = graph.add_clip(clip, 1.0, graph.root);
            library_entries.push(library_entry(ANIM_TYPE_STUN, node, Vec::new()));
            Some(node)
        });
        let graph = animation_graphs.add(graph);

        let desired = gait.anim_type(walk.is_some());
        let node = match desired {
            ANIM_TYPE_WALK => walk.unwrap_or(run),
            ANIM_TYPE_RUN => run,
            _ => stand,
        };
        anim_player.stop_all();
        anim_player.play(node).repeat();
        *graph_handle = AnimationGraphHandle(graph);
        // `try_insert`, not `insert` — the rule for every entity command in
        // this module.
        //
        // These systems all act on a character's body WRAPPER, reached through
        // `children_query` from an entity the query yielded this tick. Commands
        // are deferred, and the despawn pipelines run in the same tick: a
        // corpse's `Dying` timer expiring, a 0x3016 despawn, a COS unsummon and
        // combat's own death handling all despawn the character root, taking
        // its wrappers with it. A plain `insert` then panics on apply
        // ("Entity despawned"), which is a crash for something that is simply
        // a race we lose harmlessly — the animation state of an entity that no
        // longer exists does not matter. Same reasoning as the `try_insert` in
        // `combat::queue_damage_popups`.
        commands
            .entity(entity)
            // exempt from distance-gated animation culling: this system
            // holds &mut AnimationGraphHandle on the wrapper and would
            // silently stop matching if the handle were swapped out
            .try_insert((
                MovementAnims {
                    stand,
                    run,
                    walk,
                    attacks,
                    damage,
                    die,
                    pickup,
                    stun,
                    current: desired,
                    group: group.map(str::to_owned),
                },
                // replace the spawn-time library so animation-gated effects
                // (death smoke) keep matching the new graph's nodes
                crate::commands::AnimationLibrary {
                    entries: library_entries,
                },
                crate::plugins::animation_culling::AnimationCullExempt,
            ));
    }
}

/// First-sight AnimationGraph builds budgeted per frame, across every
/// remote entity `update_character_animation` walks this call. A single
/// build decodes every stand/run/walk/attack/damage/die/pickup/stun `.ban`
/// clip a group resolves (`drive_character_animation`'s first-sight
/// branch) — real CPU decode work, not just a spawn. Many `RemoteEntity`s
/// can lose their `MovementAnims` at once (a region crossing into a
/// crowded town/battlefield, or a batch of nearby-entity spawns landing
/// together), which would otherwise build all of them in the same frame.
/// Same idiom as the terrain budgets (`OBJECT_SPAWNS_PER_FRAME`,
/// `GROUP_BUILDS_PER_FRAME`) that fixed the measured 208ms region-crossing
/// hitch (`docs/perf-remote.md`). Unlike those, this value is **not**
/// backed by a live capture yet — it's a conservative starting estimate
/// (a build is multi-clip-decode work, closer in cost to
/// `GROUP_BUILDS_PER_FRAME`'s per-unit cost than to the plain-spawn
/// `OBJECT_SPAWNS_PER_FRAME`) and should be retuned once this is profiled
/// live.
const ANIMATION_BUILDS_PER_FRAME: i32 = 4;

/// Drives stand/run animation for the local player and every remote entity from
/// the same code path. "Moving" is [`PlayerCommands::is_moving`] for the player
/// and a live [`RemoteMovement`] target for remotes; both share one
/// `wrapper_query` so the graph build/swap logic lives in one place
/// ([`drive_character_animation`]). Items carry no [`RemoteMovement`], so the
/// remote query skips them.
///
/// The local player's own build is never budgeted — it's a single entity
/// and should never lag behind its own input. `ANIMATION_BUILDS_PER_FRAME`
/// only caps bursts across the remote entities below.
#[allow(clippy::too_many_arguments)]
fn update_character_animation(
    mut commands: Commands,
    player_commands: Res<PlayerCommands>,
    player_query: Query<
        (
            &Children,
            &PreferredAnimationGroup,
            Has<crate::plugins::cos::RiderOf>,
            Has<Stunned>,
            Has<Frozen>,
        ),
        With<Player>,
    >,
    remote_query: Query<
        (
            &Children,
            Option<&PreferredAnimationGroup>,
            &RemoteMovement,
            Has<crate::plugins::cos::RiderOf>,
            Has<Stunned>,
            Has<Frozen>,
        ),
        With<RemoteEntity>,
    >,
    mut wrapper_query: CharacterWrapperQuery,
    knocked: Query<(), With<KnockedDown>>,
    sro_resources: Res<Assets<SroResource>>,
    bsk_assets: Res<Assets<JMXVBSK>>,
    ban_assets: Res<Assets<JMXVBAN>>,
    mut animation_clips: ResMut<Assets<AnimationClip>>,
    mut animation_graphs: ResMut<Assets<AnimationGraph>>,
) {
    if let Ok((children, group, mounted, stunned, frozen)) = player_query.single() {
        let mut player_budget = i32::MAX;
        drive_character_animation(
            &mut commands,
            children,
            Some(&group.0),
            // The local player has no walk/run toggle yet (the client always
            // moves at the server's run speed), so it never asks for the walk
            // clip — wiring the toggle is its own change.
            if player_commands.is_moving() {
                Gait::Running
            } else {
                Gait::Standing
            },
            mounted,
            stunned || frozen,
            &mut wrapper_query,
            &knocked,
            &sro_resources,
            &bsk_assets,
            &ban_assets,
            &mut animation_clips,
            &mut animation_graphs,
            &mut player_budget,
        );
    }

    let mut budget = ANIMATION_BUILDS_PER_FRAME;
    for (children, group, movement, mounted, stunned, frozen) in remote_query.iter() {
        drive_character_animation(
            &mut commands,
            children,
            group.map(|g| g.0.as_str()),
            remote_gait(movement.target.is_some(), movement.walking),
            mounted,
            stunned || frozen,
            &mut wrapper_query,
            &knocked,
            &sro_resources,
            &bsk_assets,
            &ban_assets,
            &mut animation_clips,
            &mut animation_graphs,
            &mut budget,
        );
    }
}

/// Drop a body wrapper's animation set when the stance it was built from is no
/// longer the stance the character wants, so [`drive_character_animation`]
/// rebuilds it on the next frame.
///
/// Idea: the set is built once and [`drive_character_animation`] short-circuits
/// on its presence, while [`PreferredAnimationGroup`] is *replaced* on every
/// live equip (`hud::inventory::model::on_entity_equip`). Nothing connected the
/// two, so a character who swapped a sword for a bow kept the sword's idle, run
/// and swing clips for the rest of the session — the animations were correct
/// for a weapon they were no longer holding.
///
/// Comparing the built group against the wanted one, rather than reacting to
/// `Changed<PreferredAnimationGroup>`, is deliberate: it is self-correcting.
/// A change that arrives while the character is in the saddle is not lost (the
/// mismatch simply persists until they dismount), and a wrapper that spawns
/// after the change is already built from the right group and matches on sight.
///
/// **Riders and corpses are skipped.** `play_mounted_pose` and
/// `play_death_animations` own those wrappers' graphs outright; removing the
/// set under them would let the movement path rebuild the handle they are
/// holding, dropping the saddle pose or reviving a death animation mid-clip.
fn rebuild_stance_on_weapon_change(
    owners: Query<(
        &Children,
        &PreferredAnimationGroup,
        Has<crate::plugins::cos::RiderOf>,
    )>,
    wrappers: Query<(&MovementAnims, Has<DeadWrapper>)>,
    mut commands: Commands,
) {
    for (children, wanted, mounted) in owners.iter() {
        for child in children.iter() {
            let Ok((anims, dead)) = wrappers.get(child) else {
                continue;
            };
            if !stance_is_stale(anims.group.as_deref(), &wanted.0, mounted, dead) {
                continue;
            }
            debug!(
                "animation: stance {:?} -> {} — rebuilding the clip set",
                anims.group, wanted.0
            );
            commands
                .entity(child)
                .try_remove::<(MovementAnims, OneShotAttack)>();
        }
    }
}

/// Whether a built clip set must be thrown away — the decision
/// [`rebuild_stance_on_weapon_change`] makes, split out so it can be pinned
/// without standing up an animation graph.
fn stance_is_stale(built: Option<&str>, wanted: &str, mounted: bool, dead: bool) -> bool {
    !mounted && !dead && built != Some(wanted)
}

/// The riding clip a wrapper is currently playing.
///
/// Kept separate from [`MovementAnims::current`] because the two groups
/// collide numerically: `cart`'s idle is type 0, exactly like `sword`'s, so
/// `current` alone cannot tell "standing on foot" from "sitting in the
/// saddle". Its presence is also what marks the wrapper as pose-driven.
#[derive(Component, Debug, Clone, Copy)]
struct MountedPose(u32);

/// Put every rider in the saddle pose: the `cart` group's idle while the mount
/// stands, its walk clip while the mount moves ([`ANIM_GROUP_RIDING`]).
///
/// Riders are skipped by [`drive_character_animation`]'s stand/run switch, so
/// this is the only thing driving their wrapper while mounted.
///
/// The clip is looked up with `strict_group`, since the wrapper's library
/// normally holds the *weapon* group only and a loose lookup would hand back
/// the sword idle — the rider would stand upright in the saddle again.
#[allow(clippy::too_many_arguments)]
fn play_mounted_pose(
    riders: Query<(&Children, &crate::plugins::cos::RiderOf)>,
    mounts: Query<&RemoteMovement>,
    poses: Query<&MountedPose>,
    mut wrapper_query: CharacterWrapperQuery,
    mut libraries: Query<&mut crate::commands::AnimationLibrary>,
    sro_resources: Res<Assets<SroResource>>,
    bsk_assets: Res<Assets<JMXVBSK>>,
    ban_assets: Res<Assets<JMXVBAN>>,
    mut animation_clips: ResMut<Assets<AnimationClip>>,
    mut animation_graphs: ResMut<Assets<AnimationGraph>>,
    mut commands: Commands,
) {
    for (children, rider_of) in riders.iter() {
        // The mount's own movement decides the rider's clip: the rider never
        // walks itself while mounted.
        let moving = mounts
            .get(rider_of.0)
            .is_ok_and(|movement| movement.target.is_some());
        let desired = if moving {
            ANIM_TYPE_WALK
        } else {
            ANIM_TYPE_STAND
        };

        for child in children.iter() {
            let Ok((entity, spawned, mut anim_player, graph_handle, movement, running, dead, _)) =
                wrapper_query.get_mut(child)
            else {
                continue;
            };
            // A corpse keeps its die clip (the mount may still be walking).
            if dead {
                continue;
            }
            // The saddle pose is an ordinary body action and must not cut a
            // cast short. This runs every frame, so it simply lands the moment
            // the cast releases the body.
            if !may_take_body(running.map(|r| r.0), ClipPriority::Action) {
                continue;
            }
            // Wait for the graph the movement path builds on first sight.
            let Some(mut movement) = movement else {
                continue;
            };
            if poses.get(entity).is_ok_and(|pose| pose.0 == desired) {
                continue;
            }
            let graph_handle = &*graph_handle;
            let Some((node, _)) = resolve_or_add_clip(
                desired,
                Some(ANIM_GROUP_RIDING),
                true,
                entity,
                &spawned.0,
                graph_handle,
                &mut libraries,
                &sro_resources,
                &bsk_assets,
                &ban_assets,
                &mut animation_clips,
                &mut animation_graphs,
            ) else {
                // Clip still streaming in (or a body without the group): try
                // again next frame rather than freezing on the wrong pose.
                continue;
            };
            anim_player.stop_all();
            anim_player.play(node).repeat();
            commands.entity(entity).try_insert(MountedPose(desired));
            // Park the movement path's state so it re-plays stand/run once the
            // rider dismounts — the same sentinel one-shot attacks use.
            movement.current = u32::MAX;
        }
    }
}

/// Hand the wrapper back to the movement path when a rider dismounts: drop the
/// pose marker and park the movement state, so stand/run is re-played next
/// frame instead of the rider walking around in the saddle pose.
fn reset_pose_after_dismount(
    mut dismounted: RemovedComponents<crate::plugins::cos::RiderOf>,
    children_query: Query<&Children>,
    mut anims: Query<&mut MovementAnims>,
    mut commands: Commands,
) {
    for owner in dismounted.read() {
        for child in children_query
            .get(owner)
            .into_iter()
            .flat_map(|children| children.iter())
        {
            if let Ok(mut anims) = anims.get_mut(child) {
                anims.current = u32::MAX;
            }
            commands.entity(child).try_remove::<MountedPose>();
        }
    }
}

/// Fallback per-hit stagger when a swing has no (or too few) typ-1 events.
const HIT_FALLBACK_STAGGER: f32 = 0.3;

/// Play a one-shot attack clip on a swinging entity's body wrapper — driven by
/// 0xB070 action updates ([`AttackSwing`]), so the server schedules swings for
/// the local player and remotes alike. Cycles through the group's resolved
/// swing variants, then releases the swing's damage popups with each one
/// delayed to the chosen clip's matching combat-hit keytime, so numbers appear
/// when the hit visually lands. Ordered after [`update_character_animation`]
/// so the movement path can't overwrite a freshly started clip in the same
/// frame; stand/run resumes once the one-shot finishes.
fn play_attack_swings(
    mut commands: Commands,
    mut swings: MessageReader<AttackSwing>,
    children_query: Query<&Children>,
    mut wrapper_query: CharacterWrapperQuery,
    mut popups: MessageWriter<DamagePopup>,
    mut cycle: Local<usize>,
) {
    for swing in swings.read() {
        // The clip's hit keytimes, once a wrapper takes the swing.
        let mut hit_times: Option<Vec<f32>> = None;
        for child in children_query
            .get(swing.owner)
            .into_iter()
            .flat_map(|children| children.iter())
        {
            let Ok((entity, _, mut anim_player, _, movement, _, dead, _)) =
                wrapper_query.get_mut(child)
            else {
                continue;
            };
            let Some(mut movement) = movement else {
                continue;
            };
            if dead || movement.attacks.is_empty() {
                continue;
            }
            if !swing.replay_clip {
                // The echo of a swing the client predicted at the click. The
                // clip is already running — restarting it would snap it back to
                // frame 0 mid-stroke and re-trigger every sound keyed to it. All
                // this swing is here for is to time its numbers, so read the
                // keytimes off whichever attack clip is actually playing and
                // leave the animation alone. Falls back to the clip the cycle
                // would have picked if none is (the prediction never took).
                hit_times = Some(
                    movement
                        .attacks
                        .iter()
                        .find(|attack| anim_player.is_playing_animation(attack.node))
                        .unwrap_or(&movement.attacks[*cycle % movement.attacks.len()])
                        .hit_times
                        .clone(),
                );
                break;
            }
            let attack = &movement.attacks[*cycle % movement.attacks.len()];
            *cycle = cycle.wrapping_add(1);
            anim_player.stop_all();
            anim_player.play(attack.node);
            hit_times = Some(attack.hit_times.clone());
            movement.current = u32::MAX;
            // Taking the wrapper over cancels any charge still scheduled on
            // it: a surviving `ChargingCast` would fire its own
            // `stop_all()+play()` later and cut this swing off mid-stroke.
            commands
                .entity(entity)
                .try_remove::<ChargingCast>()
                .try_insert(OneShotAttack(ClipPriority::Action));
            break;
        }
        let hit_times = hit_times.unwrap_or_default();
        // The one hop where a damage number can go missing without vanishing:
        // it is still written, but with a delay taken from the clip's combat-hit
        // keytimes, so a bad keytime parks it out of sight instead of dropping
        // it. Logged with the delays so a "numbers stopped showing" report can
        // be read off the log rather than guessed at.
        debug!(
            "combat: releasing {} popup(s) on {} hit keytime(s) {:?}",
            swing.popups.len(),
            hit_times.len(),
            hit_times
        );
        for q in &swing.popups {
            let mut popup = q.popup.clone();
            popup.delay = match hit_times.get(q.hit_index) {
                Some(&t) => t,
                // eventless clip / unloaded body: stagger past the last
                // known hit moment instead of dropping everything at once
                None => {
                    hit_times.last().copied().unwrap_or(0.0)
                        + (q.hit_index + 1 - hit_times.len()) as f32 * HIT_FALLBACK_STAGGER
                }
            };
            popups.write(popup);
        }
    }
}

/// Play a skill's shot animation as a one-shot on the caster's body wrapper
/// (driven by the skills plugin's [`SkillSwing`], local simulation only).
///
/// Idea: the movement-graph rebuild in [`drive_character_animation`] only
/// carries stand/run/attack/die/pickup, so skill clips are added lazily on
/// first cast: resolve the aniset's (group, type) through the resource's
/// group table, build the clip, push it into the wrapper's live graph AND
/// its [`AnimationLibrary`] (keeping `sync_animation_effects` consistent).
/// Damage popups + scheduled hits are released per combat-hit keytime of
/// the resolved clip — one hit per typ-1 event, so multi-hit skills deal
/// their per-hit damage N times. Falls back to basic-attack cycling when
/// the skill has no resolvable clip (monster casters, missing groups).
#[allow(clippy::too_many_arguments)]
fn play_skill_swings(
    mut commands: Commands,
    time: Res<Time>,
    mut swings: MessageReader<SkillSwing>,
    children_query: Query<&Children>,
    mut wrapper_query: CharacterWrapperQuery,
    mut libraries: Query<&mut crate::commands::AnimationLibrary>,
    sro_resources: Res<Assets<SroResource>>,
    bsk_assets: Res<Assets<JMXVBSK>>,
    ban_assets: Res<Assets<JMXVBAN>>,
    mut animation_clips: ResMut<Assets<AnimationClip>>,
    mut animation_graphs: ResMut<Assets<AnimationGraph>>,
    transforms: Query<&GlobalTransform>,
    mut popups: MessageWriter<DamagePopup>,
    mut pending: ResMut<PendingHits>,
    mut started: MessageWriter<SkillSwingStarted>,
    mut cycle: Local<usize>,
) {
    for swing in swings.read() {
        let mut hit_times: Vec<f32> = Vec::new();
        let mut wrapper_entity = None;
        // The shot clip's own length, so `RunningSwings` can tell "the next
        // segment of the chain still playing" from "the skill pressed again".
        let mut duration_secs = 0.0_f32;
        let group = swing.anim_group.as_deref();
        for child in children_query
            .get(swing.owner)
            .into_iter()
            .flat_map(|children| children.iter())
        {
            let Ok((entity, spawned, mut anim_player, graph_handle, movement, running, dead, _)) =
                wrapper_query.get_mut(child)
            else {
                continue;
            };
            let Some(mut movement) = movement else {
                continue;
            };
            if dead {
                continue;
            }
            // A cast outranks every other one-shot, so this never refuses
            // today; it is here so the rule is stated in all five clip
            // players rather than four, and so anything ranked above `Cast`
            // later gets honoured without a second audit.
            if !may_take_body(running.map(|r| r.0), ClipPriority::Cast) {
                continue;
            }
            let graph_handle = &*graph_handle;

            // resolve the shot clip (else a basic-attack fallback for
            // offensive monster casts; imbues play no clip)
            let shot = swing
                .anim_type
                .and_then(|typ| {
                    resolve_or_add_clip(
                        typ,
                        group,
                        false,
                        entity,
                        &spawned.0,
                        graph_handle,
                        &mut libraries,
                        &sro_resources,
                        &bsk_assets,
                        &ban_assets,
                        &mut animation_clips,
                        &mut animation_graphs,
                    )
                })
                .or_else(|| {
                    (swing.damage.is_some() && !movement.attacks.is_empty()).then(|| {
                        let attack = &movement.attacks[*cycle % movement.attacks.len()];
                        *cycle = cycle.wrapping_add(1);
                        (attack.node, attack.hit_times.clone())
                    })
                });

            let charging = swing.charge_secs > 0.05;
            if charging {
                // charge phase, sub-phase 1: hold on the ready clip (if any)
                // for `preparing_secs`, then swap to the wait clip (if any)
                // for the remainder — the shot swap is scheduled by
                // `advance_charging_casts` once both sub-phases elapse.
                if let Some(ready_node) = swing.ready_anim_type.and_then(|typ| {
                    resolve_or_add_clip(
                        typ,
                        group,
                        false,
                        entity,
                        &spawned.0,
                        graph_handle,
                        &mut libraries,
                        &sro_resources,
                        &bsk_assets,
                        &ban_assets,
                        &mut animation_clips,
                        &mut animation_graphs,
                    )
                }) {
                    anim_player.stop_all();
                    // play once (RepeatAnimation::Never) so it holds its
                    // last frame; the ChargingCast guard in
                    // drive_character_animation keeps it from resuming stand
                    anim_player.play(ready_node.0);
                }
                let wait_node = swing.wait_anim_type.and_then(|typ| {
                    resolve_or_add_clip(
                        typ,
                        group,
                        false,
                        entity,
                        &spawned.0,
                        graph_handle,
                        &mut libraries,
                        &sro_resources,
                        &bsk_assets,
                        &ban_assets,
                        &mut animation_clips,
                        &mut animation_graphs,
                    )
                });
                movement.current = u32::MAX;
                let shot_node = shot.as_ref().map(|(node, _)| *node);
                let preparing_secs = swing.preparing_secs.clamp(0.0, swing.charge_secs);
                // always schedule the charge end so the ready/wait hold can't
                // run forever (imbues have a wind-up but no shot clip)
                commands.entity(entity).try_insert((
                    OneShotAttack(ClipPriority::Cast),
                    ChargingCast {
                        timer: Timer::from_seconds(preparing_secs, TimerMode::Once),
                        next: ChargingPhase::Wait {
                            wait_node: wait_node.map(|(node, _)| node),
                            shot_node,
                            shot_secs: swing.charge_secs - preparing_secs,
                        },
                    },
                ));
                if let Some((_, times)) = &shot {
                    hit_times = times.clone();
                }
            } else if let Some((node, times)) = &shot {
                // no wind-up: fire the shot immediately
                anim_player.stop_all();
                anim_player.play(*node);
                movement.current = u32::MAX;
                // Same reason as `play_attack_swings`: a charge left over from
                // a superseded cast must not fire a late clip swap.
                commands
                    .entity(entity)
                    .try_remove::<ChargingCast>()
                    .try_insert(OneShotAttack(ClipPriority::Cast));
                hit_times = times.clone();
            }
            duration_secs = shot
                .as_ref()
                .and_then(|(node, _)| {
                    clip_duration(*node, graph_handle, &animation_graphs, &animation_clips)
                })
                .unwrap_or(0.0);
            wrapper_entity = Some(entity);
            break;
        }

        // one damage instance per combat-hit keytime (at least one),
        // delayed past the charge wind-up
        if hit_times.is_empty() {
            hit_times.push(HIT_FALLBACK_STAGGER);
        }
        if let Some(damage) = &swing.damage {
            let world = transforms
                .get(damage.target)
                .map(|gt| gt.translation())
                .unwrap_or_default();
            let now = time.elapsed_secs_f64();
            for &t in &hit_times {
                let at = swing.charge_secs + t;
                popups.write(DamagePopup {
                    anchor: damage.target,
                    world,
                    delay: at,
                    amount: damage.per_hit,
                    critical: false,
                    blocked: false,
                    killing_blow: false,
                    set: damage.set,
                    // The offline simulation has no wire to read displacement
                    // arms from, so it never knocks anything down.
                    knockdown: None,
                });
                pending.0.push(PendingHit {
                    target: damage.target,
                    amount: damage.per_hit,
                    at: now + at as f64,
                });
            }
        }
        // Real server-computed damage (online path): release each queued
        // popup at its own hit's keytime rather than at packet arrival —
        // the same recipe `play_attack_swings` uses for basic attacks
        // (`popup.delay = hit_times[hit_index]`), plus the charge offset a
        // skill's wind-up needs and a basic attack never has.
        for q in &swing.popups {
            let mut popup = q.popup.clone();
            popup.delay = swing.charge_secs
                + hit_times
                    .get(q.hit_index)
                    .copied()
                    .unwrap_or_else(|| *hit_times.last().expect("hit_times is never empty here"));
            popups.write(popup);
        }
        if let Some(wrapper) = wrapper_entity {
            started.write(SkillSwingStarted {
                owner: swing.owner,
                wrapper,
                skill_id: swing.skill_id,
                codename: swing.codename.clone(),
                charge_secs: swing.charge_secs,
                hit_times,
                duration_secs,
                // `swing.target`, NOT the damage template's: online casts
                // carry no `damage` (their numbers ride `popups`), so reading
                // the target off it left every online cast target-less and
                // silently dropped all of its projectile/impact effects.
                target: swing.target.or(swing.damage.as_ref().map(|d| d.target)),
                flying_speed: swing.flying_speed,
                instance: swing.instance,
            });
        }
    }
}

/// A wrapper mid-charge: `timer` runs out `next`'s sub-phase, at which point
/// [`advance_charging_casts`] steps to the next one (or removes the
/// component, once the shot has fired or there was nothing to fire).
#[derive(Component)]
pub(crate) struct ChargingCast {
    timer: Timer,
    next: ChargingPhase,
}

/// What a [`ChargingCast`]'s timer does when it elapses.
#[derive(Clone, Copy)]
enum ChargingPhase {
    /// The preparing sub-phase ended: swap to the wait clip if one is
    /// authored (`None` leaves whatever's already showing — the held ready
    /// pose, or nothing — alone, which is exactly the old single-phase
    /// behavior for a skill with no `AniWait`), then run `shot_secs` before
    /// the shot.
    Wait {
        wait_node: Option<AnimationNodeIndex>,
        shot_node: Option<AnimationNodeIndex>,
        shot_secs: f32,
    },
    /// The casting sub-phase ended: fire the shot clip, or — if there is
    /// none (imbues) — release the one-shot so stand/run resumes.
    Shot {
        shot_node: Option<AnimationNodeIndex>,
    },
}

/// Step a [`ChargingCast`] through its sub-phases as each one's timer
/// completes, ending on the shot clip (or releasing the hold, if there is
/// none).
fn advance_charging_casts(
    time: Res<Time>,
    mut commands: Commands,
    mut charging: Query<(Entity, &mut ChargingCast, &mut AnimationPlayer)>,
) {
    for (entity, mut charge, mut player) in charging.iter_mut() {
        if !charge.timer.tick(time.delta()).just_finished() {
            continue;
        }
        match charge.next {
            ChargingPhase::Wait {
                wait_node,
                shot_node,
                shot_secs,
            } => {
                if let Some(node) = wait_node {
                    player.stop_all();
                    player.play(node);
                }
                charge.timer = Timer::from_seconds(shot_secs.max(0.0), TimerMode::Once);
                charge.next = ChargingPhase::Shot { shot_node };
            }
            ChargingPhase::Shot { shot_node } => match shot_node {
                Some(node) => {
                    player.stop_all();
                    player.play(node);
                    commands.entity(entity).try_remove::<ChargingCast>();
                }
                // no shot clip: end the one-shot so stand/run resumes
                None => {
                    commands
                        .entity(entity)
                        .try_remove::<ChargingCast>()
                        .try_remove::<OneShotAttack>();
                }
            },
        }
    }
}

/// Lazily add a (group, type) clip to a wrapper's live animation graph and
/// its [`AnimationLibrary`]. Returns the new node with its combat-hit
/// keytimes (seconds), or `None` when the resource/skeleton/clip isn't
/// available (e.g. the `.ban` still streaming in — the cast then falls back
/// to a basic attack and the next cast retries).
#[allow(clippy::too_many_arguments)]
fn add_skill_clip_to_graph(
    resource_handle: &Handle<SroResource>,
    graph_handle: &AnimationGraphHandle,
    library: &mut crate::commands::AnimationLibrary,
    group: Option<&str>,
    typ: u32,
    sro_resources: &Assets<SroResource>,
    bsk_assets: &Assets<JMXVBSK>,
    ban_assets: &Assets<JMXVBAN>,
    animation_clips: &mut Assets<AnimationClip>,
    animation_graphs: &mut Assets<AnimationGraph>,
) -> Option<(AnimationNodeIndex, Vec<f32>)> {
    let resource = sro_resources.get(resource_handle)?;
    let skeleton = bsk_assets.get(resource.skeleton.as_ref()?)?;
    let entry = resource.find_animation_entry(group, typ)?;
    let idx = entry.file_index as usize;
    let clip = build_movement_clip(resource, skeleton, idx, ban_assets, animation_clips)?;
    let mut graph = animation_graphs.get_mut(&graph_handle.0)?;
    let root = graph.root;
    let node = graph.add_clip(clip, 1.0, root);
    let group_name = group.unwrap_or("default");
    // Skill entries carry the same event list as attacks — their typ-1 events
    // are the skill hit frame (`skilleffect.txt` StartEvent N), and their
    // typ-4 events ride along unread (#276).
    let library_entry = crate::commands::AnimationLibraryEntry {
        group: group_name.to_string(),
        anim_type: typ,
        label: format!("{group_name}/{typ} (skill)"),
        node,
        events: animation_events(entry),
    };
    let hit_times = library_entry
        .hit_events()
        .into_iter()
        .map(|ms| ms as f32 / 1000.0)
        .collect();
    library.entries.push(library_entry);
    Some((node, hit_times))
}

/// Find a (group, type) clip in a wrapper's [`AnimationLibrary`], adding it
/// to the live graph on first use. Returns the node with its combat-hit
/// keytimes (seconds).
///
/// `strict_group` decides what happens when the wanted group has no clip of
/// that type: normally any group's clip of the same type will do (a monster
/// casting a skill it has no dedicated stance for still swings), but a caller
/// that needs *that* group's clip must pass `true`. The riding pose does:
/// `drive_character_animation` rebuilds the library with the weapon group
/// only, so a loose `cart`/stand lookup would silently return the sword idle —
/// the rider would stand in the saddle again.
#[allow(clippy::too_many_arguments)]
fn resolve_or_add_clip(
    typ: u32,
    group: Option<&str>,
    strict_group: bool,
    entity: Entity,
    resource_handle: &Handle<SroResource>,
    graph_handle: &AnimationGraphHandle,
    libraries: &mut Query<&mut crate::commands::AnimationLibrary>,
    sro_resources: &Assets<SroResource>,
    bsk_assets: &Assets<JMXVBSK>,
    ban_assets: &Assets<JMXVBAN>,
    animation_clips: &mut Assets<AnimationClip>,
    animation_graphs: &mut Assets<AnimationGraph>,
) -> Option<(AnimationNodeIndex, Vec<f32>)> {
    let mut library = libraries.get_mut(entity).ok()?;
    if let Some(entry) = library
        .entries
        .iter()
        .find(|e| e.anim_type == typ && group.is_none_or(|g| e.group == g))
        .or_else(|| {
            (!strict_group)
                .then(|| library.entries.iter().find(|e| e.anim_type == typ))
                .flatten()
        })
    {
        return Some((
            entry.node,
            entry
                .hit_events()
                .into_iter()
                .map(|ms| ms as f32 / 1000.0)
                .collect(),
        ));
    }
    add_skill_clip_to_graph(
        resource_handle,
        graph_handle,
        &mut library,
        group,
        typ,
        sro_resources,
        bsk_assets,
        ban_assets,
        animation_clips,
        animation_graphs,
    )
}

/// A freshly stunned/frozen entity loses its in-flight one-shot cast: the
/// wrapper snaps back to stand and every not-yet-started skill emission
/// anchored under the entity is cancelled.
fn interrupt_incapacitated_casts(
    stunned_roots: Query<Entity, Or<(Added<Stunned>, Added<Frozen>)>>,
    children_query: Query<&Children>,
    mut wrapper_query: CharacterWrapperQuery,
    pending_effects: Query<(Entity, &ChildOf), With<DelayedEffect>>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    for root in stunned_roots.iter() {
        for child in children_query
            .get(root)
            .into_iter()
            .flat_map(|children| children.iter())
        {
            let Ok((entity, _, mut anim_player, _, movement, running, dead, _)) =
                wrapper_query.get_mut(child)
            else {
                continue;
            };
            let Some(mut movement) = movement else {
                continue;
            };
            if dead || running.is_none() {
                continue;
            }
            info!("skills: interrupting cast of a stunned entity");
            anim_player.stop_all();
            // Straight into the dizzy loop where the group has one, so the
            // cancelled cast does not flash a frame of stand before
            // `drive_character_animation` puts the body there anyway.
            match movement.stun {
                Some(stun) => {
                    anim_player.play(stun).repeat();
                    movement.current = ANIM_TYPE_STUN;
                }
                None => {
                    anim_player.play(movement.stand).repeat();
                    movement.current = ANIM_TYPE_STAND;
                }
            }
            commands
                .entity(entity)
                .try_remove::<OneShotAttack>()
                .try_remove::<ChargingCast>();
        }
        // pending (delayed) emissions under the stunned entity never fire
        for (effect, child_of) in pending_effects.iter() {
            let mut current = child_of.parent();
            for _ in 0..16 {
                if current == root {
                    commands.entity(effect).despawn();
                    break;
                }
                match parents.get(current) {
                    Ok(parent) => current = parent.parent(),
                    Err(_) => break,
                }
            }
        }
    }
}

/// Play the pickup bow-down on whoever 0x3036 names (broadcast for the local
/// player and others alike) — a one-shot through the same [`OneShotAttack`]
/// resume path as attack swings.
fn play_pickup_animations(
    mut commands: Commands,
    mut reader: MessageReader<PlayerPickupAnimation>,
    net: Res<NetworkEntities>,
    children_query: Query<&Children>,
    mut wrapper_query: CharacterWrapperQuery,
) {
    for msg in reader.read() {
        let Some(owner) = net.get(msg.unique_id) else {
            continue;
        };
        for child in children_query
            .get(owner)
            .into_iter()
            .flat_map(|children| children.iter())
        {
            let Ok((entity, _, mut anim_player, _, movement, running, dead, _)) =
                wrapper_query.get_mut(child)
            else {
                continue;
            };
            let Some(mut movement) = movement else {
                continue;
            };
            let (false, Some(pickup)) = (dead, movement.pickup) else {
                continue;
            };
            // Looting is an ordinary action: it must not cut a cast in half.
            // The 0x3036 that drives it is the server's own confirmation, so
            // dropping the clip loses nothing but the animation.
            if !may_take_body(running.map(|r| r.0), ClipPriority::Action) {
                continue;
            }
            anim_player.stop_all();
            anim_player.play(pickup);
            movement.current = u32::MAX;
            commands
                .entity(entity)
                .try_insert(OneShotAttack(ClipPriority::Action));
            break;
        }
    }
}

/// Hit reactions still waiting out their popup's delay: `(body owner, seconds
/// left)`. The flinch has to land *with* the number, and auto-attack damage is
/// deliberately deferred to the swing clip's hit keytimes
/// ([`play_attack_swings`]), so reacting at message time would flinch before
/// the blow visibly connects.
#[derive(Resource, Default)]
struct PendingHitReactions(Vec<(Entity, f32)>);

/// How long a popup's hit reaction waits, or `None` when this hit gets no
/// flinch. Three hits get none:
///
/// - a **killing blow**, answered by the die clip ([`play_death_animations`]);
///   playing a reaction first would cut the death animation into a twitch.
/// - a **knockdown**, answered by the DOWN sequence ([`play_knockdowns`]);
///   a standing flinch would stand the body back up mid-fall.
/// - an **avoided hit** (`blocked`), which landed nothing: there is no blow to
///   recoil from. The original answers a block with its own DEFENCE clip (anim
///   type 22, shipped by 78 of the corpus's groups), which we deliberately do
///   not play — see `docs/re/formats/anim-state-coverage.md`. Reusing the
///   DAMAGE flinch instead would be wrong twice over: wrong clip, and a body
///   twitching through its own combo on every blocked hit, which is exactly
///   the interruption this pass is here to remove.
///
/// Deciding it here is what keeps them in step: the knockdown rides the very
/// popup this reads, so both are queued from one place with one delay and
/// cannot drift apart.
fn reaction_delay(popup: &DamagePopup) -> Option<f32> {
    (!popup.killing_blow && !popup.blocked && popup.knockdown.is_none())
        .then(|| popup.delay.max(0.0))
}

/// Tick every queued reaction by `dt` and take out the ones now due, in queue
/// order. Same-frame duplicates stay duplicated on purpose — the play system
/// decides what a second hit does to a body already flinching.
fn drain_due_reactions(queue: &mut Vec<(Entity, f32)>, dt: f32) -> Vec<Entity> {
    let mut due = Vec::new();
    queue.retain_mut(|(entity, remaining)| {
        *remaining -= dt;
        if *remaining <= 0.0 {
            due.push(*entity);
            false
        } else {
            true
        }
    });
    due
}

/// Play the hit reaction (DAMAGE1/DAMAGE2) on whoever a damage popup names,
/// timed to that popup's delay — the second band of #275's animation-state
/// coverage, and the one the corpus says every fighting body ships.
///
/// It is a one-shot through the same [`OneShotAttack`] resume path as attack
/// swings, and it never interrupts a swing, a cast wind-up or a corpse: the
/// original flinches the *victim*, and a reaction that cancelled the victim's
/// own attack would let a fast attacker lock a target out of fighting back.
fn play_hit_reactions(
    mut commands: Commands,
    time: Res<Time>,
    mut popups: MessageReader<DamagePopup>,
    mut pending: ResMut<PendingHitReactions>,
    children_query: Query<&Children>,
    mut wrapper_query: CharacterWrapperQuery,
    mut knocked: Query<&mut KnockedDown>,
    mut cycle: Local<usize>,
) {
    for popup in popups.read() {
        if let Some(delay) = reaction_delay(popup) {
            pending.0.push((popup.anchor, delay));
        }
    }
    for owner in drain_due_reactions(&mut pending.0, time.delta_secs()) {
        for child in children_query
            .get(owner)
            .into_iter()
            .flat_map(|children| children.iter())
        {
            let Ok((entity, _, mut anim_player, _, movement, running, dead, charging)) =
                wrapper_query.get_mut(child)
            else {
                continue;
            };
            let Some(mut movement) = movement else {
                continue;
            };
            // A charge holds its ready clip's last frame, so it owns the body
            // even before a rank is on it.
            if dead || charging || !may_take_body(running.map(|r| r.0), ClipPriority::Reaction) {
                continue;
            }
            // Already on the ground: the prone flinch (DOWN_DAMAGE) replaces
            // the standing one, and a body with no prone flinch simply does
            // not react — playing DAMAGE1/2 would stand it back up mid-fall.
            if let Ok(mut down) = knocked.get_mut(entity) {
                if let Some(node) = down.damage {
                    anim_player.stop_all();
                    anim_player.play(node);
                    down.phase = DownPhase::Flinching;
                }
                break;
            }
            if movement.damage.is_empty() {
                continue;
            }
            let node = movement.damage[*cycle % movement.damage.len()];
            *cycle = cycle.wrapping_add(1);
            anim_player.stop_all();
            anim_player.play(node);
            movement.current = u32::MAX;
            commands
                .entity(entity)
                .try_insert(OneShotAttack(ClipPriority::Reaction));
            break;
        }
    }
}

/// Where a knocked-down body is in its fall/lie/rise sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DownPhase {
    /// `DOWN` (62) — going down, played once.
    Falling,
    /// `DOWN_RM` (63) — lying there, held for [`KnockedDown::hold`].
    Prone,
    /// `DOWN_DAMAGE` (64) — hit while prone; returns to `Prone`/`GettingUp`.
    Flinching,
    /// `DOWN_UP` (65) — getting up, played once before motion resumes.
    GettingUp,
}

/// A body wrapper currently knocked down.
///
/// Idea: a knockdown is the one reaction that is **not** a single clip, so the
/// binary [`OneShotAttack`] marker cannot express it — it is a fall, a dwell
/// and a recovery, and a hit or a death landing in the middle of it wants
/// different clips again. This carries that little state machine.
///
/// The five clips are resolved **once**, when the knockdown starts, and cached
/// here as nodes. Only ~370 of the corpus's 7,714 animation groups ship the
/// DOWN block, so every one is optional; caching them also lets
/// [`play_death_animations`] reach `DOWN_DIE` without taking the whole clip
/// resolution parameter set.
#[derive(Component, Debug, Clone, Copy)]
struct KnockedDown {
    phase: DownPhase,
    /// Seconds of prone dwell left. Zero by default — see
    /// `config::combat::CombatSettings::knockdown_hold_seconds` for why the
    /// original's dwell is not invented here.
    hold: f32,
    prone: Option<AnimationNodeIndex>,
    damage: Option<AnimationNodeIndex>,
    get_up: Option<AnimationNodeIndex>,
    /// `DOWN_DIE` (66), used by [`play_death_animations`] instead of DIE1 when
    /// the killing blow lands on a body that is already down.
    die: Option<AnimationNodeIndex>,
}

/// The authored length of the clip behind a graph node, in seconds.
///
/// Used as the last data-derived fallback for the prone dwell: one loop of the
/// artists' own lying-down animation. Returns `None` for a node that is not a
/// clip or whose asset has not streamed in yet.
fn clip_duration(
    node: AnimationNodeIndex,
    graph_handle: &AnimationGraphHandle,
    graphs: &Assets<AnimationGraph>,
    clips: &Assets<AnimationClip>,
) -> Option<f32> {
    let graph = graphs.get(&graph_handle.0)?;
    match &graph.get(node)?.node_type {
        AnimationNodeType::Clip(handle) => Some(clips.get(handle)?.duration()),
        _ => None,
    }
}

/// How long a knocked-down body lies there, in seconds.
///
/// Resolution order, most specific first — every step is either configured or
/// data-derived, so no number is invented:
///
/// 1. `combat.knockdown_hold_seconds` when the user set one (>= 0).
/// 2. The victim's own characterdata `KO_RecoverTime`
///    ([`CharacterDataRow::knockdown_recover_secs`]) — the only value in the
///    game's data that answers this question.
/// 3. The `DOWN_RM` clip's authored length: one loop of the animation made for
///    exactly this pose.
/// 4. Zero — fall straight into the recovery. Reached only when the body ships
///    no prone loop at all, in which case there is nothing to hold *on*.
fn knockdown_hold_secs(
    configured: f32,
    row: Option<&crate::assets::textdata::characterdata::CharacterDataRow>,
    prone: Option<AnimationNodeIndex>,
    graph_handle: &AnimationGraphHandle,
    graphs: &Assets<AnimationGraph>,
    clips: &Assets<AnimationClip>,
) -> f32 {
    if configured >= 0.0 {
        return configured;
    }
    if let Some(secs) = row.and_then(|row| row.knockdown_recover_secs()) {
        return secs;
    }
    prone
        .and_then(|node| clip_duration(node, graph_handle, graphs, clips))
        .unwrap_or(0.0)
}

/// Knockdowns waiting out their popup's delay: `(victim, seconds left)`.
///
/// The same shape and the same reason as [`PendingHitReactions`]: the body has
/// to go down *when the blow lands*, not when the packet arrives. Auto-attack
/// damage is deliberately deferred to the swing clip's hit keytimes
/// ([`play_attack_swings`]), so a knockdown played at message time dropped the
/// victim a few hundred milliseconds before the sword reached it.
#[derive(Resource, Default)]
struct PendingKnockdowns(Vec<(Entity, f32)>);

/// Start the knockdown sequence on whoever a displacement hit named, timed to
/// that hit's popup delay.
///
/// Falls back to doing nothing when the victim's group ships no `DOWN` clip.
/// Note the flinch is already suppressed for these hits by [`reaction_delay`],
/// so a model without the block plays *no* reaction rather than the standing
/// one — the alternative would be a flinch that contradicts the knockdown the
/// server just reported.
#[allow(clippy::too_many_arguments)]
fn play_knockdowns(
    mut commands: Commands,
    time: Res<Time>,
    config: Res<crate::plugins::config::ClientConfig>,
    mut popups: MessageReader<DamagePopup>,
    mut pending: ResMut<PendingKnockdowns>,
    char_refs: Query<&crate::plugins::net::entities::CharacterRef>,
    char_data: Res<crate::plugins::textdata::ClientCharacterData>,
    children_query: Query<&Children>,
    mut wrapper_query: CharacterWrapperQuery,
    mut libraries: Query<&mut crate::commands::AnimationLibrary>,
    sro_resources: Res<Assets<SroResource>>,
    bsk_assets: Res<Assets<JMXVBSK>>,
    ban_assets: Res<Assets<JMXVBAN>>,
    mut animation_clips: ResMut<Assets<AnimationClip>>,
    mut animation_graphs: ResMut<Assets<AnimationGraph>>,
) {
    for popup in popups.read() {
        if popup.knockdown.is_some() {
            pending.0.push((popup.anchor, popup.delay.max(0.0)));
        }
    }
    for owner in drain_due_reactions(&mut pending.0, time.delta_secs()) {
        for child in children_query
            .get(owner)
            .into_iter()
            .flat_map(|children| children.iter())
        {
            let Ok((
                entity,
                spawned,
                mut anim_player,
                graph_handle,
                movement,
                running,
                dead,
                // A body mid-wind-up is precisely one that should go down, so
                // unlike everywhere else `ChargingCast` is not consulted here —
                // it is cleared below instead.
                _,
            )) = wrapper_query.get_mut(child)
            else {
                continue;
            };
            // A corpse does not get up again.
            if dead {
                continue;
            }
            // A knockdown takes the body from ANYTHING — flinch, swing, cast,
            // wind-up. This system anchors on the **victim**, not on the
            // caster, so guarding it the way `play_hit_reactions` is guarded
            // reads the wrong body: an enemy is mid-attack-swing most of the
            // time it is in combat, and refusing there silently dropped nearly
            // every enemy knockdown. And the case that guard was written for —
            // a monster knocking *us* down mid-combo — is the correct
            // behaviour, not a hazard: a body on the ground has no business
            // finishing its swing. (The displacement arms it keys off appear 0
            // times in all 987 hit records of `packet_dump/0xb070.log`, so the
            // hazard was never evidenced; the behaviour it broke was.)
            if !may_take_body(running.map(|r| r.0), ClipPriority::Knockdown) {
                continue;
            }
            let Some(mut movement) = movement else {
                continue;
            };
            let mut resolve = |typ: u32| {
                resolve_or_add_clip(
                    typ,
                    movement.group.as_deref(),
                    false,
                    entity,
                    &spawned.0,
                    &graph_handle,
                    &mut libraries,
                    &sro_resources,
                    &bsk_assets,
                    &ban_assets,
                    &mut animation_clips,
                    &mut animation_graphs,
                )
                .map(|(node, _)| node)
            };
            // No fall clip, no knockdown — the flinch covers it.
            let Some(enter) = resolve(ANIM_TYPE_DOWN_ENTER) else {
                continue;
            };
            // Resolve every clip first so `resolve`'s mutable borrow of the
            // asset collections ends before the dwell reads one back.
            let prone = resolve(ANIM_TYPE_DOWN_LOOP);
            let damage = resolve(ANIM_TYPE_DOWN_DAMAGE);
            let get_up = resolve(ANIM_TYPE_DOWN_UP);
            let die = resolve(ANIM_TYPE_DOWN_DIE);
            let row = char_refs
                .get(owner)
                .ok()
                .and_then(|char_ref| char_data.get(&(char_ref.0 as i32)));
            let down = KnockedDown {
                phase: DownPhase::Falling,
                hold: knockdown_hold_secs(
                    config.combat.knockdown_hold_seconds,
                    row,
                    prone,
                    &graph_handle,
                    &animation_graphs,
                    &animation_clips,
                ),
                prone,
                damage,
                get_up,
                die,
            };
            anim_player.stop_all();
            anim_player.play(enter);
            // Park the movement sentinel exactly like the one-shot path, so
            // `drive_character_animation` replays stand/run when this ends.
            movement.current = u32::MAX;
            // Same hygiene as the two swing players: a marker left over from
            // the clip this just replaced would outlive the fall. The stale
            // `OneShotAttack` is inert while `KnockedDown` is on the wrapper
            // (`drive_character_animation` skips it) but parks the body on an
            // `all_finished()` for a long-gone clip once `advance_knockdowns`
            // releases the state; a stale `ChargingCast` is worse, since
            // `advance_charging_casts` is not gated on `KnockedDown` and would
            // fire its scheduled clip swap onto a body lying on the ground.
            commands
                .entity(entity)
                .try_remove::<(OneShotAttack, ChargingCast)>()
                .try_insert(down);
            break;
        }
    }
}

/// Advance the knockdown state machine: fall -> (lie) -> get up -> done.
///
/// Each transition waits on the previous clip actually finishing rather than a
/// timer, so a long fall is never cut off; only the optional prone dwell is
/// time-based.
fn advance_knockdowns(
    mut commands: Commands,
    time: Res<Time>,
    mut wrappers: Query<(
        Entity,
        &mut AnimationPlayer,
        &mut KnockedDown,
        Option<&mut MovementAnims>,
    )>,
) {
    /// What this frame does to one knocked-down body.
    enum Step {
        /// Still playing; leave it alone.
        Wait,
        /// Settle into the prone loop for the configured dwell.
        Lie(AnimationNodeIndex),
        /// Start the recovery clip.
        Rise(AnimationNodeIndex),
        /// Sequence over (or nothing left to play): hand the body back to the
        /// ordinary movement path.
        Done,
    }

    for (entity, mut anim_player, mut down, movement) in wrappers.iter_mut() {
        // Once the fall (or a prone flinch) has played out, lie there if there
        // is a dwell configured and a loop clip to spend it on, else get up.
        let after_clip = || match (down.hold > 0.0, down.prone, down.get_up) {
            (true, Some(prone), _) => Step::Lie(prone),
            (_, _, Some(get_up)) => Step::Rise(get_up),
            _ => Step::Done,
        };
        let step = match down.phase {
            DownPhase::Falling | DownPhase::Flinching => {
                if anim_player.all_finished() {
                    after_clip()
                } else {
                    Step::Wait
                }
            }
            DownPhase::Prone => {
                down.hold -= time.delta_secs();
                if down.hold > 0.0 {
                    Step::Wait
                } else {
                    match down.get_up {
                        Some(get_up) => Step::Rise(get_up),
                        None => Step::Done,
                    }
                }
            }
            DownPhase::GettingUp => {
                if anim_player.all_finished() {
                    Step::Done
                } else {
                    Step::Wait
                }
            }
        };
        match step {
            Step::Wait => {}
            Step::Lie(prone) => {
                anim_player.stop_all();
                anim_player.play(prone).repeat();
                down.phase = DownPhase::Prone;
            }
            Step::Rise(get_up) => {
                anim_player.stop_all();
                anim_player.play(get_up);
                down.phase = DownPhase::GettingUp;
            }
            Step::Done => {
                commands.entity(entity).try_remove::<KnockedDown>();
                anim_player.stop_all();
                if let Some(mut movement) = movement {
                    // Re-arm the sentinel so the movement path picks stand/run
                    // up again on the next frame.
                    movement.current = u32::MAX;
                }
            }
        }
    }
}

/// Play the death clip on a dying entity's body wrapper (driven by
/// [`EntityDied`] from the despawn handlers). The wrapper is marked
/// [`DeadWrapper`] so the movement path never resumes stand/run; the corpse
/// holds the clip's last frame until the root's `Dying` timer despawns it.
/// Playing the typ-4 clip also lets the animation-effects system fire the
/// death smoke on monsters that define one.
fn play_death_animations(
    mut commands: Commands,
    mut deaths: MessageReader<EntityDied>,
    children_query: Query<&Children>,
    mut wrapper_query: CharacterWrapperQuery,
    knocked: Query<&KnockedDown>,
) {
    for EntityDied(owner) in deaths.read() {
        for child in children_query
            .get(*owner)
            .into_iter()
            .flat_map(|children| children.iter())
        {
            let Ok((entity, _, mut anim_player, _, movement, _, dead, _)) =
                wrapper_query.get_mut(child)
            else {
                continue;
            };
            if dead {
                continue;
            }
            // A body killed while it is already on the ground dies from there
            // (DOWN_DIE, 66) instead of playing the standing death, which
            // would snap it upright for the last clip of its life.
            let down_die = knocked.get(entity).ok().and_then(|down| down.die);
            // Without a die clip keep whatever plays (usually stand) rather
            // than stopping into a bind pose.
            if let Some(die) = down_die.or_else(|| movement.and_then(|m| m.die)) {
                anim_player.stop_all();
                anim_player.play(die);
            }
            // The knockdown sequence is over either way — nothing gets up.
            commands
                .entity(entity)
                .try_remove::<KnockedDown>()
                .try_insert(DeadWrapper);
        }
    }
}

/// Local-player revive (#143): once the death flag clears, drop the frozen die
/// pose — remove [`DeadWrapper`] from the player's body wrapper and re-arm the
/// movement path (`current` sentinel) so it plays stand/run again next frame.
/// Only the local player's die pose is ever un-frozen this way; monster corpses
/// keep theirs until they despawn.
fn revive_player_animation(
    death: Res<crate::plugins::hud::death::PlayerDeath>,
    players: Query<&Children, With<Player>>,
    dead_wrappers: Query<(), With<DeadWrapper>>,
    mut anims: Query<&mut MovementAnims>,
    mut commands: Commands,
) {
    if !death.is_changed() || death.dead {
        return;
    }
    for children in players.iter() {
        for child in children.iter() {
            if !dead_wrappers.contains(child) {
                continue;
            }
            if let Ok(mut anims) = anims.get_mut(child) {
                anims.current = u32::MAX; // force a stand/run re-play next frame
            }
            commands.entity(child).try_remove::<DeadWrapper>();
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// A weapon swap must invalidate the clip set. Before this the set was
    /// built once and never compared against the stance the character wanted,
    /// so a swordsman who drew a bow kept swinging a sword all session.
    #[test]
    fn a_stance_the_character_no_longer_wants_is_stale() {
        assert!(stance_is_stale(Some("sword"), "bow", false, false));
        // ...and one that still matches is left alone, or the set would be
        // rebuilt every single frame
        assert!(!stance_is_stale(Some("bow"), "bow", false, false));
        // a wrapper whose set predates any stance at all still needs building
        assert!(stance_is_stale(None, "sword", false, false));
    }

    /// Riders and corpses are skipped: `play_mounted_pose` and
    /// `play_death_animations` own those graphs outright, and rebuilding under
    /// them drops the saddle pose or restarts a death clip mid-animation.
    #[test]
    fn a_rider_or_a_corpse_keeps_its_stance_however_stale() {
        assert!(!stance_is_stale(Some("sword"), "bow", true, false));
        assert!(!stance_is_stale(Some("sword"), "bow", false, true));
    }

    /// The gait -> animation-type mapping is the whole of #275's walk wiring:
    /// a walking entity plays WALK only when its resource group actually has
    /// one, and otherwise keeps the RUN clip it played before — never stand,
    /// which would look like it froze mid-stroll.
    #[test]
    fn gait_picks_walk_only_when_the_group_has_one() {
        assert_eq!(Gait::Standing.anim_type(true), ANIM_TYPE_STAND);
        assert_eq!(Gait::Standing.anim_type(false), ANIM_TYPE_STAND);
        assert_eq!(Gait::Walking.anim_type(true), ANIM_TYPE_WALK);
        assert_eq!(Gait::Walking.anim_type(false), ANIM_TYPE_RUN);
        assert_eq!(Gait::Running.anim_type(true), ANIM_TYPE_RUN);
        assert_eq!(Gait::Running.anim_type(false), ANIM_TYPE_RUN);
    }

    /// A parked entity stands whatever its motion state says — the server
    /// leaves the gait set after a stroll ends, so keying the animation off
    /// the gait alone would leave it walking on the spot.
    /// A killing blow gets no flinch — the die clip owns that body — and every
    /// other hit waits out exactly the delay the popup carries, which for
    /// auto-attacks is the swing clip's hit keytime.
    #[test]
    fn only_survivable_hits_flinch_and_they_wait_for_their_popup() {
        assert_eq!(reaction_delay(&test_popup(0.35, false, false)), Some(0.35));
        assert_eq!(reaction_delay(&test_popup(0.0, false, false)), Some(0.0));
        // negative delays cannot come from a keytime, but a fallback stagger
        // subtraction could produce one: clamp rather than fire never
        assert_eq!(reaction_delay(&test_popup(-1.0, false, false)), Some(0.0));
        assert_eq!(reaction_delay(&test_popup(0.35, true, false)), None);
    }

    /// A knockdown suppresses the standing flinch on the same hit. Both ride
    /// one popup, so the decision is made once and the two cannot drift — a
    /// DAMAGE clip here would stand the body back up mid-fall.
    #[test]
    fn a_knockdown_hit_gets_no_standing_flinch() {
        assert_eq!(reaction_delay(&test_popup(0.35, false, true)), None);
        // ...and it is the knockdown doing it, not the delay
        assert_eq!(reaction_delay(&test_popup(0.35, false, false)), Some(0.35));
    }

    /// The knockdown inherits the popup's delay, so the body goes down on the
    /// frame the blow lands rather than when the packet arrived.
    #[test]
    fn a_knockdown_waits_out_the_same_delay_as_its_number() {
        let popup = test_popup(0.35, false, true);
        assert!(popup.knockdown.is_some());
        let mut queue = vec![(popup.anchor, popup.delay.max(0.0))];
        assert_eq!(drain_due_reactions(&mut queue, 0.34), Vec::new());
        assert_eq!(drain_due_reactions(&mut queue, 0.01), vec![popup.anchor]);
    }

    fn test_popup(delay: f32, killing_blow: bool, knockdown: bool) -> DamagePopup {
        DamagePopup {
            anchor: Entity::from_raw_u32(1).unwrap(),
            world: Vec3::ZERO,
            delay,
            amount: 7,
            critical: false,
            blocked: false,
            killing_blow,
            set: crate::plugins::combat::HitcountSet::Received,
            knockdown: knockdown.then_some(crate::plugins::combat::KnockdownHit {
                arm: 4,
                position: packets::agent::prelude::HitPosition {
                    region: 0,
                    x: 0,
                    y: 0,
                    z: 0,
                },
            }),
        }
    }

    /// The queue fires each reaction on the frame its delay runs out, keeps
    /// the ones still counting down, and preserves two hits on one body as two
    /// separate reactions.
    #[test]
    fn queued_reactions_fire_when_their_delay_runs_out() {
        let a = Entity::from_raw_u32(1).unwrap();
        let b = Entity::from_raw_u32(2).unwrap();
        let mut queue = vec![(a, 0.1), (b, 0.5), (a, 0.1)];
        assert_eq!(drain_due_reactions(&mut queue, 0.05), Vec::new());
        assert_eq!(queue.len(), 3);
        assert_eq!(drain_due_reactions(&mut queue, 0.05), vec![a, a]);
        assert_eq!(queue.len(), 1);
        assert_eq!(drain_due_reactions(&mut queue, 1.0), vec![b]);
        assert!(queue.is_empty());
    }

    #[test]
    fn a_parked_entity_stands_whatever_its_motion_state_says() {
        assert_eq!(remote_gait(false, true), Gait::Standing);
        assert_eq!(remote_gait(false, false), Gait::Standing);
        assert_eq!(remote_gait(true, true), Gait::Walking);
        assert_eq!(remote_gait(true, false), Gait::Running);
    }

    /// The sandbox runs at live scale: its player speed is what the server
    /// sends a base character (authored 50, doubled), and locally spawned
    /// entities keep their authored speeds untouched. Speeds and positions
    /// share one unit — units/second, 1920 to a region — so any conversion
    /// between "data" and "sandbox" scale would be a bug, not a feature.
    #[test]
    fn the_sandbox_runs_at_live_scale() {
        assert_eq!(PLAYER_MOVE_SPEED, 100.0);
        assert_eq!(
            PLAYER_MOVE_SPEED,
            DATA_RUN_SPEED * SERVER_PLAYER_SPEED_FACTOR
        );
        // A region takes ~19s to cross on foot at that speed.
        assert!(((1920.0 / PLAYER_MOVE_SPEED) - 19.2).abs() < 0.1);
    }

    /// The rule the whole ranking exists for: a hit reaction must never cut a
    /// cast. A chain's clip runs 2.4-5.1 s, and a flinch on every blow taken
    /// across it restarted the body mid-combo.
    #[test]
    fn a_reaction_never_takes_a_body_from_a_cast() {
        assert!(!may_take_body(
            Some(ClipPriority::Cast),
            ClipPriority::Reaction
        ));
        assert!(!may_take_body(
            Some(ClipPriority::Action),
            ClipPriority::Reaction
        ));
    }

    /// Looting, mounting and basic swings are ordinary actions: they wait for
    /// a cast, but not for a flinch.
    #[test]
    fn an_action_yields_only_to_a_cast() {
        assert!(!may_take_body(
            Some(ClipPriority::Cast),
            ClipPriority::Action
        ));
        assert!(may_take_body(
            Some(ClipPriority::Reaction),
            ClipPriority::Action
        ));
    }

    /// `is_mid_cast` answers the narrower question `is_mid_one_shot` cannot:
    /// only a cast (or the knockdown that outranks it) holds the body in the
    /// sense that should delay the player's NEXT ACTION on the wire. A flinch
    /// is "the shortest and most interruptible thing a body plays", so
    /// stalling combat on one would be worse than sending early.
    #[test]
    fn only_a_cast_rank_clip_counts_as_mid_cast() {
        fn mid_cast_with(clip: Option<ClipPriority>, charging: bool) -> bool {
            let mut world = World::new();
            let wrapper = world.spawn_empty().id();
            if let Some(priority) = clip {
                world.entity_mut(wrapper).insert(OneShotAttack(priority));
            }
            if charging {
                world.entity_mut(wrapper).insert(ChargingCast {
                    timer: Timer::from_seconds(1.0, TimerMode::Once),
                    next: ChargingPhase::Wait {
                        wait_node: None,
                        shot_node: None,
                        shot_secs: 0.0,
                    },
                });
            }
            // the body wrapper is a CHILD of the owner, as it is in the game
            let owner = world.spawn_empty().add_child(wrapper).id();
            let mut children = world.query::<&Children>();
            let mut casting = world.query::<(Option<&OneShotAttack>, Has<ChargingCast>)>();
            let (children, casting) = (children.query(&world), casting.query(&world));
            is_mid_cast(owner, &children, &casting)
        }

        assert!(
            !mid_cast_with(None, false),
            "an idle body holds nothing back"
        );
        assert!(
            !mid_cast_with(Some(ClipPriority::Reaction), false),
            "a flinch must not stall the wire"
        );
        assert!(
            !mid_cast_with(Some(ClipPriority::Action), false),
            "nor a basic swing — the server paces that loop itself"
        );
        assert!(mid_cast_with(Some(ClipPriority::Cast), false));
        assert!(
            mid_cast_with(Some(ClipPriority::Knockdown), false),
            "anything outranking a cast holds the body too"
        );
        assert!(
            mid_cast_with(None, true),
            "a cast still winding up counts before its shot clip starts"
        );
    }

    /// Equal rank wins, so a new swing replaces the previous swing and a new
    /// cast replaces a cast — and an idle body is always available.
    #[test]
    fn equal_rank_takes_the_body_and_so_does_anything_on_an_idle_one() {
        assert!(may_take_body(Some(ClipPriority::Cast), ClipPriority::Cast));
        assert!(may_take_body(
            Some(ClipPriority::Action),
            ClipPriority::Action
        ));
        assert!(may_take_body(None, ClipPriority::Reaction));
    }

    /// A cast outranks everything below it and is never made to wait.
    #[test]
    fn a_cast_takes_the_body_from_anything() {
        for held in [ClipPriority::Reaction, ClipPriority::Action] {
            assert!(may_take_body(Some(held), ClipPriority::Cast));
        }
    }

    /// **Regression (`01d38e7f`).** A knockdown takes the body from anything,
    /// including a cast. `play_knockdowns` anchors on the *victim*, and an
    /// enemy in combat is mid-attack-swing most of the time — guarding it like
    /// a flinch silently dropped nearly every enemy knockdown.
    #[test]
    fn a_knockdown_takes_the_body_from_anything_including_a_cast() {
        for held in [
            ClipPriority::Reaction,
            ClipPriority::Action,
            ClipPriority::Cast,
            ClipPriority::Knockdown,
        ] {
            assert!(
                may_take_body(Some(held), ClipPriority::Knockdown),
                "a knockdown must not be refused by a running {held:?} clip"
            );
        }
    }

    /// ...and nothing takes the body back off it. The fall/lie/rise sequence is
    /// then owned by `KnockedDown`, which `drive_character_animation` honours
    /// ahead of every rank here.
    #[test]
    fn nothing_below_a_knockdown_takes_the_body_from_one() {
        for want in [
            ClipPriority::Reaction,
            ClipPriority::Action,
            ClipPriority::Cast,
        ] {
            assert!(!may_take_body(Some(ClipPriority::Knockdown), want));
        }
    }

    /// An avoided hit landed nothing, so it gets no flinch — otherwise every
    /// blocked blow would twitch the body mid-combo, which is the interruption
    /// the ranking above is here to stop.
    #[test]
    fn an_avoided_hit_gets_no_flinch() {
        let hit = test_popup(0.4, false, false);
        // an ordinary hit of this shape still flinches...
        assert_eq!(reaction_delay(&hit), Some(0.4));
        // ...the same hit avoided does not.
        let blocked = DamagePopup {
            blocked: true,
            amount: 0,
            ..hit
        };
        assert_eq!(reaction_delay(&blocked), None);
    }
}

/// Retract a swing whose cast the server refused ([`CancelSwing`]): the body
/// drops back to stand and every not-yet-started emission under it is dropped.
///
/// Same shape as [`interrupt_incapacitated_casts`] minus the dizzy branch —
/// there is no status to hold here, the cast simply did not happen.
fn cancel_swings(
    mut cancels: MessageReader<crate::plugins::skills::cast::CancelSwing>,
    children_query: Query<&Children>,
    mut wrapper_query: CharacterWrapperQuery,
    pending_effects: Query<(Entity, &ChildOf), With<DelayedEffect>>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    for crate::plugins::skills::cast::CancelSwing(root) in cancels.read() {
        for child in children_query
            .get(*root)
            .into_iter()
            .flat_map(|children| children.iter())
        {
            let Ok((entity, _, mut anim_player, _, movement, running, dead, _)) =
                wrapper_query.get_mut(child)
            else {
                continue;
            };
            let Some(mut movement) = movement else {
                continue;
            };
            if dead || running.is_none() {
                continue;
            }
            debug!("skills: retracting the swing of a refused cast");
            anim_player.stop_all();
            anim_player.play(movement.stand).repeat();
            movement.current = ANIM_TYPE_STAND;
            commands
                .entity(entity)
                .try_remove::<OneShotAttack>()
                .try_remove::<ChargingCast>();
        }
        // emissions that had not fired yet never should
        for (effect, child_of) in pending_effects.iter() {
            let mut current = child_of.parent();
            for _ in 0..16 {
                if current == *root {
                    commands.entity(effect).despawn();
                    break;
                }
                match parents.get(current) {
                    Ok(parent) => current = parent.parent(),
                    Err(_) => break,
                }
            }
        }
    }
}
