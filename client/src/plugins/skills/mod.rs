//! The skill system: skill book (learn/level/withdraw), cast pipeline, and
//! status effects.
//!
//! Idea: everything UI-facing goes through two seams so the same window and
//! underbar drive both worlds. Casts are [`cast::CastRequest`] messages —
//! online they become the 0x7074 packet, offline (the Skills test scene
//! inserts [`LocalCombat`]) they run a local presentation loop built from
//! skilleffect.txt + the player plugin's one-shot animation path. Learned
//! state is the [`book::SkillBook`] resource — seeded from CHARACTER_DATA
//! online, from the egui panel offline.

use bevy::prelude::*;

use crate::assets::bsr::resource::SroResource;
use crate::plugins::net::inventory::{Inventory, WEAPON_SLOT};
use crate::plugins::player::Player;
use crate::plugins::textdata::{ClientItemData, ClientSkillEffects};
use crate::scenes::SceneState;

pub mod book;
pub mod cast;
pub mod status;

/// Keeps skill projectile MODELS (bow arrows) loaded and cached, so a cast's
/// short-lived projectile finds them already resident instead of losing the
/// async `.bsr` load race (which silently renders nothing).
#[derive(Resource, Default)]
pub struct PreloadedSkillModels(#[allow(dead_code)] Vec<Handle<SroResource>>);

/// Warm the projectile model cache once the skilleffect table arrives.
fn preload_skill_models(
    skill_effects: Res<ClientSkillEffects>,
    asset_server: Res<AssetServer>,
    mut preloaded: ResMut<PreloadedSkillModels>,
    mut done: Local<bool>,
) {
    if *done || !skill_effects.is_changed() {
        return;
    }
    let paths = skill_effects.model_paths();
    if paths.is_empty() {
        return;
    }
    preloaded.0 = paths
        .into_iter()
        .map(|path| asset_server.load(format!("data://{path}")))
        .collect();
    *done = true;
    debug!("skills: preloaded {} projectile models", preloaded.0.len());
}

/// Present = combat is simulated locally (no server): casts resolve into
/// animations/damage on this client, with every hit dealing `damage_per_hit`
/// (the Skills test scene's flat-10 rule).
#[derive(Resource)]
pub struct LocalCombat {
    pub damage_per_hit: u32,
}

impl Default for LocalCombat {
    fn default() -> Self {
        Self { damage_per_hit: 10 }
    }
}

/// What the local player is wielding, derived from the equipped weapon's
/// itemdata row. Both fields are `None` when the hands are empty, when
/// itemdata has not loaded yet, or before the inventory arrives at all —
/// every consumer falls back to its own default there.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq)]
pub struct EquippedWeapon {
    /// Weapon class (item tid4), for skilldata's cast-weapon requirement
    /// (cols 50/51). `None` skips the check — the server enforces it anyway.
    pub class: Option<u8>,
    /// Attack reach in **world units** (itemdata col 94). Steers the local
    /// approach only; the server owns the real range check.
    pub reach: Option<f32>,
}

impl EquippedWeapon {
    /// How far from its target an engagement stops, in world units.
    ///
    /// The one place that precedence lives, so the gap-close and the cast
    /// prediction cannot disagree about what "in range" means: an authored
    /// skill range wins, else the equipped weapon's own reach, else the
    /// unarmed fallback. That middle step is what
    /// `docs/formats/textdata-skilldata.md` means by "col 21 Action_Range: 0
    /// on player weapon attacks = use weapon range".
    ///
    /// Floored at [`ATTACK_GAP_STOP`], because col 94 measures reach *past the
    /// bodies* — a sword authors 6 and a dagger 3 — while the gap-close
    /// measures centre to centre, so used raw it would walk a swordsman inside
    /// the monster. The floor binds only for melee, whose authored reaches are
    /// all ≤ 18; a bow's 180 passes straight through, which is the whole point
    /// of this function.
    ///
    /// Resolved at each point of use rather than latched when the engagement
    /// starts, so swapping weapons mid-fight takes effect.
    pub fn engagement_reach(&self, authored: Option<f32>) -> f32 {
        use crate::plugins::combat::ATTACK_GAP_STOP;
        authored
            .or(self.reach)
            .map_or(ATTACK_GAP_STOP, |reach| reach.max(ATTACK_GAP_STOP))
    }
}

/// Derive [`EquippedWeapon`] from what the player is wearing in the weapon
/// slot.
///
/// Keyed off the `Inventory` component rather than the 0x3038 equip push,
/// because the inventory is the settled answer: the equip packet *precedes*
/// the 0xB034 move that fills the slot (see `on_entity_equip`), so hooking
/// 0x3038 would mean re-deriving the swap and unequip rules `apply_move`
/// already encodes.
///
/// Runs every frame instead of gating on `Changed<Inventory>`: itemdata may
/// still be loading when the inventory arrives, and a change-gated system
/// would then latch `None` and never revisit it. The cost is one `single()`
/// and one map lookup, and it self-heals the moment the table lands.
fn track_equipped_weapon(
    players: Query<&Inventory, With<Player>>,
    item_data: Res<ClientItemData>,
    mut equipped: ResMut<EquippedWeapon>,
) {
    let Ok(inventory) = players.single() else {
        return;
    };
    // tid4 only names a weapon class on a weapon row (TID1/2/3 = 3/1/6) —
    // the same gate `ItemDataRow::animation_group` applies. Anything else in
    // the slot is treated as empty hands rather than as a reach of zero.
    let weapon = inventory
        .get(WEAPON_SLOT)
        .and_then(|item| item_data.get(&(item.ref_id as i32)))
        .filter(|row| matches!(row.type_ids(), Some((3, 1, 6, _))));
    equipped.set_if_neq(EquippedWeapon {
        class: weapon
            .and_then(|row| row.type_ids())
            .map(|(_, _, _, tid4)| tid4 as u8),
        reach: weapon.and_then(|row| row.attack_reach()),
    });
}

/// Scenes where the skill system runs: the networked game plus the offline
/// Skills test scene.
fn in_skill_scenes(state: Res<State<SceneState>>) -> bool {
    matches!(**state, SceneState::GameWorld | SceneState::Skills)
}

pub struct SkillsPlugin;

impl Plugin for SkillsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<book::SkillBook>()
            .init_resource::<book::SkillGroupIndex>()
            .init_resource::<EquippedWeapon>()
            .init_resource::<PreloadedSkillModels>()
            .init_resource::<cast::PendingHits>()
            .init_resource::<cast::SkillCooldowns>()
            .init_resource::<cast::AutoAttackResume>()
            .init_resource::<cast::CastInstances>()
            .init_resource::<cast::ChainCast>()
            .init_resource::<cast::PredictedCasts>()
            .init_resource::<cast::SkillSwingTimings>()
            .init_resource::<cast::PendingSwings>()
            .init_resource::<cast::RunningSwings>()
            .add_message::<cast::CastRequest>()
            .add_message::<cast::SkillSwing>()
            .add_message::<cast::SkillSwingStarted>()
            .add_message::<cast::LocalCastEnded>()
            .add_message::<cast::CancelSwing>()
            // model preload is scene-agnostic (warms the cache at load time)
            .add_systems(Update, preload_skill_models)
            // Its own registration, not the `in_skill_scenes` tuple below:
            // that tuple is at Bevy's 20-system limit, and this is
            // GameWorld-only anyway — the offline Skills scene sets the
            // resource itself from its own weapon picker.
            .add_systems(
                Update,
                track_equipped_weapon.run_if(in_state(SceneState::GameWorld)),
            )
            .add_systems(
                Update,
                (
                    book::index_skill_groups,
                    book::seed_skillbook_from_character_info
                        .run_if(in_state(SceneState::GameWorld)),
                    book::apply_learn_responses.run_if(in_state(SceneState::GameWorld)),
                    status::apply_network_buffs.run_if(in_state(SceneState::GameWorld)),
                    cast::apply_cooldown_replay.run_if(in_state(SceneState::GameWorld)),
                    cast::dispatch_casts,
                    cast::advance_chain_casts,
                    cast::retract_refused_casts,
                    cast::resume_auto_attack,
                    cast::track_skill_swing_timing,
                    cast::advance_pending_swings,
                    cast::spawn_cast_effects,
                    cast::resolve_oneshot_cast_lifetime,
                    cast::move_skill_projectiles,
                    cast::apply_pending_hits,
                    status::apply_self_buffs,
                    status::enforce_incapacitation,
                    status::expire_status_effects,
                    status::expire_buffs,
                    status::sync_ailment_effects,
                )
                    .run_if(in_skill_scenes),
            )
            // Bone-parented draw effects hold their world aim against the
            // animated bone chain right before this frame's propagation.
            .add_systems(
                PostUpdate,
                cast::orient_aimed_bone_effects
                    .before(bevy::transform::TransformSystems::Propagate)
                    .run_if(in_skill_scenes),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::textdata::itemdata::{ItemData, ItemDataRow};
    use crate::plugins::combat::ATTACK_GAP_STOP;
    use crate::plugins::net::inventory::EQUIP_SLOT_COUNT;
    use packets::agent::character_data::{InventoryItem, ItemTypeData, RentInfo};
    use std::collections::HashMap;

    const BOW_REF: i32 = 3800;
    const POTION_REF: i32 = 10;

    /// An itemdata row carrying just the type tuple and the raw col-94 reach.
    fn row(tids: (u32, u32, u32, u32), reach: &str) -> ItemDataRow {
        let mut fields = vec![String::new(); 95];
        fields[9] = tids.0.to_string();
        fields[10] = tids.1.to_string();
        fields[11] = tids.2.to_string();
        fields[12] = tids.3.to_string();
        fields[94] = String::from(reach);
        ItemDataRow(fields)
    }

    fn item_data() -> ClientItemData {
        ClientItemData::from_data(ItemData(HashMap::from([
            // a CH bow, and a potion that is not equipment at all
            (BOW_REF, row((3, 1, 6, 6), "180")),
            (POTION_REF, row((3, 3, 3, 1), "0")),
        ])))
    }

    fn wearing(ref_id: Option<i32>) -> Inventory {
        let mut slots = vec![None; EQUIP_SLOT_COUNT as usize];
        if let Some(ref_id) = ref_id {
            slots[WEAPON_SLOT as usize] = Some(InventoryItem {
                slot: WEAPON_SLOT,
                rent: RentInfo::default(),
                ref_id: ref_id as u32,
                data: ItemTypeData::TransformScroll { mask_ref_id: 0 },
            });
        }
        Inventory {
            slots,
            avatar_slots: vec![],
            gold: 0,
        }
    }

    fn app_wearing(inventory: Inventory, data: ClientItemData) -> App {
        let mut app = App::new();
        app.init_resource::<EquippedWeapon>()
            .insert_resource(data)
            .add_systems(Update, track_equipped_weapon);
        app.world_mut().spawn((Player, inventory));
        app
    }

    fn equipped(app: &App) -> EquippedWeapon {
        *app.world().resource::<EquippedWeapon>()
    }

    #[test]
    fn engagement_reach_prefers_authored_then_weapon_then_the_fallback() {
        let bow = EquippedWeapon {
            class: Some(6),
            reach: Some(180.0),
        };
        assert_eq!(bow.engagement_reach(Some(200.0)), 200.0, "authored wins");
        assert_eq!(bow.engagement_reach(None), 180.0, "else the weapon");
        assert_eq!(
            EquippedWeapon::default().engagement_reach(None),
            ATTACK_GAP_STOP,
            "unarmed falls back"
        );
    }

    /// Corpus values: a sword authors 6 and a dagger 3, because col 94 is
    /// reach *past the bodies* while the gap-close measures centre to centre.
    /// Taken raw those would walk a swordsman inside the monster, so melee
    /// holds at the floor and only genuinely long weapons move the stop point.
    #[test]
    fn a_melee_reach_is_floored_rather_than_walked_into_the_target() {
        let sword = EquippedWeapon {
            class: Some(2),
            reach: Some(6.0),
        };
        let dagger = EquippedWeapon {
            class: Some(13),
            reach: Some(3.0),
        };
        let spear = EquippedWeapon {
            class: Some(4),
            reach: Some(18.0),
        };

        assert_eq!(sword.engagement_reach(None), ATTACK_GAP_STOP);
        assert_eq!(dagger.engagement_reach(None), ATTACK_GAP_STOP);
        // ...but a reach genuinely past the floor is kept
        assert_eq!(spear.engagement_reach(None), 18.0);
    }

    #[test]
    fn a_worn_bow_yields_its_class_and_reach() {
        let mut app = app_wearing(wearing(Some(BOW_REF)), item_data());
        app.update();

        assert_eq!(
            equipped(&app),
            EquippedWeapon {
                class: Some(6),
                reach: Some(180.0),
            }
        );
    }

    #[test]
    fn empty_hands_yield_nothing() {
        let mut app = app_wearing(wearing(None), item_data());
        app.update();

        assert_eq!(equipped(&app), EquippedWeapon::default());
    }

    /// A non-weapon in the weapon slot means empty hands, never a reach of
    /// zero — a zero would park the approach inside the monster.
    #[test]
    fn a_non_weapon_in_the_weapon_slot_is_treated_as_unarmed() {
        let mut app = app_wearing(wearing(Some(POTION_REF)), item_data());
        app.update();

        assert_eq!(equipped(&app), EquippedWeapon::default());
    }

    /// Why this system does not gate on `Changed<Inventory>`: itemdata can
    /// land *after* the inventory does, and a change-gated system would latch
    /// the unresolved answer and never revisit it.
    #[test]
    fn itemdata_arriving_after_the_inventory_is_still_picked_up() {
        let mut app = app_wearing(wearing(Some(BOW_REF)), ClientItemData::default());
        app.update();
        assert_eq!(
            equipped(&app),
            EquippedWeapon::default(),
            "nothing to resolve it against yet"
        );

        app.insert_resource(item_data());
        app.update();

        assert_eq!(
            equipped(&app).reach,
            Some(180.0),
            "the table landed, so the weapon must resolve"
        );
    }
}
