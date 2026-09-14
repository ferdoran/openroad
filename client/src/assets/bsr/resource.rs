use bevy::math::{Vec2, Vec3};
use bevy::prelude::{AlphaMode, Asset, Handle};
use bevy::reflect::TypePath;

use crate::assets::bms::mesh::JMXVBMS;
use crate::assets::bmt::material::JMXVBMT;
use crate::assets::bsk::JMXVBSK;
use crate::assets::bsr::bsr::{
    AnimationData, ModSet, ObjectInfo, PrimitiveAnimationGroupData, PrimitiveAnimationTypeData,
    PrimitiveGroupData, ResAttachInfo, ResourceHeader,
};
use crate::assets::bsr::collision_mesh::CollisionMesh;

/// A particle effect the resource spawns (Particle ModData entry).
///
/// Deliberately stores only the path, not a pre-loaded handle: game data
/// contains dangling references (e.g. "monster\system_appear.efp" does not
/// exist in Particles.pk2), and a handle created in the .bsr loader would
/// make the missing file a hard dependency that wedges every loading state
/// waiting on the resource. The effect is resolved at spawn time instead
/// and missing/broken effects are dropped with a warning.
#[derive(Clone)]
pub struct ParticleModEntry {
    /// Normalized (forward-slash) path relative to Particles.pk2.
    pub path: String,
    /// Bone of the resource's own skeleton the effect is anchored to
    /// (e.g. the garment talisman's ward-tip glows on "Bone06"/"Bone04");
    /// `None` = the resource wrapper itself (lamps, mobs).
    pub bone: Option<String>,
    /// Spawn position in resource-local space.
    pub offset: Vec3,
    /// Keytime into the owning animation, in milliseconds (only meaningful
    /// for [`EffectModOwner::Animation`] entries, e.g. death smoke rising
    /// 2379ms into the die animation).
    pub delay_ms: u32,
    /// Only shows at night (street/building lamp glows). Spawned regardless
    /// until a day/night cycle exists; carried as a marker for it.
    pub night_only: bool,
    /// Uniform scale on the spawned effect — offset AND geometry (see
    /// [`crate::assets::bsr::RawParticleMod::scale`]).
    pub scale: f32,
    /// How the effect is triggered, from the owning mod set's header.
    pub owner: EffectModOwner,
}

/// How a resource effect (Particle ModData) is triggered, derived from the
/// typ field of its owning mod set (see `scan_mod_set_headers`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EffectModOwner {
    /// System set ("ambient", typ 2), or no decodable owner: always active
    /// (torch flames on map objects and the like).
    AlwaysOn,
    /// Animation-linked set (typ 1): plays while the animation of the given
    /// group + type id plays, `delay_ms` after it starts.
    Animation { group: String, anim_type: u32 },
    /// Named set referenced externally (typ 0): transformed movement-state
    /// variants and per-hit skill sets; never auto-played.
    External,
}

/// One sound track of a Sound ModData entry, kept only for the
/// animation-linked (typ 1) mod sets — the original's animation→SFX binding
/// (footsteps, weapon swings, mob voices, death thuds).
///
/// Like [`ParticleModEntry`] this stores the path, not a handle: game data
/// references sounds that are not in Data.pk2, and a handle built in the
/// .bsr loader would make a missing `.wav` a hard dependency of the model.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundModEntry {
    /// Normalized (forward-slash) path relative to Data.pk2.
    pub path: String,
    /// Milliseconds into the animation at which the track fires.
    pub key_time_ms: u32,
    /// Animation group of the owning mod set ("default", weapon class, ...).
    pub group: String,
    /// Animation type id within the group (0 = stand, 4 = die, ...).
    pub anim_type: u32,
}

/// A DyVertex ModData entry: the resource's flag that a material's mesh is
/// **soft-body simulated** (cloth, wind) rather than rigid — capes, robes,
/// wings, banners, sails, tent cloth. 1,015 of the corpus's 7,715 `.bsr`
/// carry one (`docs/re/formats/moddata-unhandled.md` §3).
///
/// The entry has no payload beyond its material index, so this is the whole
/// type. Nothing simulates it yet: the payload it activates is the mesh-side
/// `DyVertexData` block (`docs/formats/bms-jmxvbms.md:121`), and cloth
/// simulation is not in this tree. It is surfaced rather than left as a dead
/// palette variant so the flag is available the day that lands — and so that
/// "does this resource have cloth?" is answerable from the loaded model
/// instead of by re-scanning bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct DyVertexModEntry {
    /// Target material index, `None` = every material of the active set
    /// (the entry's `MtrlIdx == -1`).
    pub mtrl_idx: Option<u32>,
}

/// A TexAni ModData entry: continuously scrolls the UVs of the meshes
/// using the target material (waterfalls, dungeon canal water). Only
/// always-on ("ambient" set) entries are kept by the loader.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TexAniModEntry {
    /// Index into the active .bmt material set; `None` = all materials.
    pub mtrl_idx: Option<u32>,
    /// UV scroll in uv/sec (negative V = water flows down the sheet).
    pub uv_speed: Vec2,
}

/// A Material ModData entry's blend override: the D3D srcblend/dstblend
/// render states mapped onto the bevy alpha mode they correspond to.
/// Waterfall sheets are SRCALPHA/INVSRCALPHA ([`AlphaMode::Blend`]) or
/// SRCALPHA/ONE ([`AlphaMode::Add`], hot-spring falls, garden water) —
/// without this they render with the default hard alpha mask, which turns
/// the soft water gradients into stringy cutouts. Only always-on
/// ("ambient" set) entries with a recognized blend pair are kept.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendModEntry {
    /// Index into the active .bmt material set; `None` = all materials.
    pub mtrl_idx: Option<u32>,
    pub alpha_mode: AlphaMode,
}

#[derive(TypePath, Asset, Default)]
#[allow(dead_code)]
pub struct SroResource {
    pub header: ResourceHeader,
    pub object_info: ObjectInfo,
    pub collision_mesh: CollisionMesh,
    pub materials: Vec<Handle<JMXVBMT>>,
    pub mesh: Vec<Handle<JMXVBMS>>,
    pub animation: AnimationData,
    pub skeleton: Option<Handle<JMXVBSK>>,
    /// Bone name of the *target* skeleton this resource gets attached to
    /// (set for attachable items like weapons, e.g. "Bip01 R HandMid").
    pub attachment_bone: Option<String>,
    /// Attachment slots of a character or attachable item resource.
    pub attach_info: Option<ResAttachInfo>,
    /// The resource carries an EnvMap ModData entry: its textures' alpha
    /// channel is a sheen mask (weapons, metal armor), not transparency,
    /// so its meshes must use the opaque material variant.
    pub alpha_is_sheen: bool,
    /// The EnvMap entry's alpha-test flag: exact-zero alpha texels are
    /// cutouts (the original tests GREATEREQUAL ref 1) while the rest of
    /// the alpha range stays a sheen mask. Set e.g. on glaives whose
    /// blade shapes are punched out of a shared texture atlas.
    pub sheen_alpha_test: bool,
    pub primitive_group: Vec<PrimitiveGroupData>,
    pub primitive_animation_group: Vec<PrimitiveAnimationGroupData>,
    /// Particle effects referenced from the mod palette (death smoke,
    /// ambient flames, ...). MVP: spawned always-on at their offsets.
    pub effect_mods: Vec<ParticleModEntry>,
    /// Always-on texture UV animations from the mod palette (waterfalls,
    /// canal water); applied to the meshes using the target material.
    pub texani_mods: Vec<TexAniModEntry>,
    /// Always-on blend-state overrides from the mod palette's Material
    /// entries; applied alongside [`Self::texani_mods`].
    pub blend_mods: Vec<BlendModEntry>,
    /// Animation-linked sound tracks from the mod palette's Sound entries,
    /// played by `play_animation_sounds` at their keytime while the owning
    /// animation runs.
    pub sound_mods: Vec<SoundModEntry>,
    /// Soft-body (cloth/wind) flags from the mod palette's DyVertex entries.
    /// Surfaced, not simulated — see [`DyVertexModEntry`].
    pub dyvertex_mods: Vec<DyVertexModEntry>,
    pub system_mod_set: Vec<ModSet>,
    pub animation_mod_set: Vec<ModSet>,
    // pub unknown_buf: [u8; 40]
}

/// Animation type id of the city idle animation (see
/// [`PrimitiveAnimationTypeData::typ`] for other well-known ids).
pub const ANIM_TYPE_STAND: u32 = 0;

/// Animation type id of the running (moving) animation.
pub const ANIM_TYPE_RUN: u32 = 7;

/// Animation type id of the walking animation. Ships in 958 of the corpus's
/// AniGroup tables (`docs/re/formats/anim-state-coverage.md` §2) but is not
/// universal — resources without it keep running, so every consumer must treat
/// it as optional.
pub const ANIM_TYPE_WALK: u32 = 1;

/// Animation group holding the mounted (riding) poses, on every character
/// resource in the 1.188 corpus: `0` → `cart_stand01.ban` (a breathing idle),
/// `1` → `cart_walk.ban`, `7` → the same walk clip. Named `cart` because the
/// original shipped riding with the trade carts; there is no `horse`/`ride`
/// group. See `docs/re/systems/mount.md`.
pub const ANIM_GROUP_RIDING: &str = "cart";

/// Animation type ids of the basic-attack swings, in ANI_ATTACK1..4 order
/// (verified against real .bsr group tables; ATTACK5..16 are non-contiguous —
/// see the `ANI_ATTACK` arm of `textdata::skilleffect::slot_to_anim_type`).
/// Weapon groups carry a sparse subset — resolve each via
/// [`SroResource::find_animation`] and keep whichever exist.
pub const ANIM_TYPE_ATTACKS: [u32; 4] = [2, 5, 16, 17];

/// Animation type id of the death animation (its typ-4 clip also triggers
/// the death-smoke `AnimationEffects` automatically once played).
pub const ANIM_TYPE_DIE: u32 = 4;

/// Animation type id of the item pickup bow-down (verified: chinaman's
/// default group maps 38 → `chinaman_pickup.ban`).
pub const ANIM_TYPE_PICKUP: u32 = 38;

/// Animation type id of the stun/dizzy reaction loop. Ships in 181 of the
/// corpus's AniGroup tables (`docs/re/formats/anim-state-coverage.md` §2),
/// so like WALK it is optional — groups without it keep standing.
pub const ANIM_TYPE_STUN: u32 = 79;

/// Animation type ids of the two hit reactions (DAMAGE1/DAMAGE2, ×480/×267
/// in the corpus) and the knockdown block (DOWN, DOWN_RM, DOWN_DAMAGE,
/// DOWN_UP, DOWN_DIE — 62..=66, ×367–387 each), per
/// `docs/re/formats/anim-state-coverage.md` §2–§3.
///
/// DAMAGE1/DAMAGE2 are played as the victim's flinch off the damage popups
/// (`plugins::player::play_hit_reactions`).
///
/// The knockdown block's trigger is the displacement arms (4/5) of
/// 0xB070/0xB071 — see `plugins::combat::EntityKnockedDown`. That resolves the
/// "which server field selects DOWN" half of §9's UNKNOWN; how *long* a body
/// stays prone is still unstated by any packet in the corpus, so it is a
/// config knob (`hud`-side `knockdown_hold_seconds`) rather than an invented
/// constant.
pub const ANIM_TYPE_DAMAGE: [u32; 2] = [3, 9];
/// See [`ANIM_TYPE_DAMAGE`] — knockdown block, in DOWN..DOWN_DIE order.
pub const ANIM_TYPE_DOWN: [u32; 5] = [62, 63, 64, 65, 66];

/// Going down (62): the fall itself, played once on knockdown.
pub const ANIM_TYPE_DOWN_ENTER: u32 = ANIM_TYPE_DOWN[0];
/// Lying there (63): the prone loop, held for the configured hold.
pub const ANIM_TYPE_DOWN_LOOP: u32 = ANIM_TYPE_DOWN[1];
/// Hit while prone (64) — the flinch that replaces DAMAGE1/2 when down.
pub const ANIM_TYPE_DOWN_DAMAGE: u32 = ANIM_TYPE_DOWN[2];
/// Getting up (65): the recovery, played once before normal motion resumes.
pub const ANIM_TYPE_DOWN_UP: u32 = ANIM_TYPE_DOWN[3];
/// Dying while prone (66) — replaces DIE1 for a body killed on the ground.
pub const ANIM_TYPE_DOWN_DIE: u32 = ANIM_TYPE_DOWN[4];

impl SroResource {
    /// Index into the animation list for the given animation type, taken
    /// from the named animation group (e.g. the equipped weapon's class).
    /// Weapon groups are sparse, so missing types fall back to the
    /// "default" group like in the original engine.
    pub fn find_animation(&self, group: Option<&str>, typ: u32) -> Option<usize> {
        self.find_animation_entry(group, typ)
            .map(|anim| anim.file_index as usize)
    }

    /// Like [`Self::find_animation`], but returns the whole group entry —
    /// needed by consumers of the per-animation event list (combat hit
    /// keytimes).
    pub fn find_animation_entry(
        &self,
        group: Option<&str>,
        typ: u32,
    ) -> Option<&PrimitiveAnimationTypeData> {
        group
            .and_then(|name| self.find_animation_in_group(name, typ))
            .or_else(|| self.find_animation_in_group("default", typ))
    }

    fn find_animation_in_group(
        &self,
        group_name: &str,
        typ: u32,
    ) -> Option<&PrimitiveAnimationTypeData> {
        self.primitive_animation_group
            .iter()
            .find(|group| group.group_name == group_name)?
            .animations
            .iter()
            .find(|anim| anim.typ == typ && anim.file_index != u32::MAX)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Builds an AniGroup table the way the .bsr parser leaves it: `(typ,
    /// file_index)` pairs per named group, `u32::MAX` = the "no clip" sentinel
    /// real tables carry.
    fn resource(groups: &[(&str, &[(u32, u32)])]) -> SroResource {
        SroResource {
            primitive_animation_group: groups
                .iter()
                .map(|(name, entries)| PrimitiveAnimationGroupData {
                    group_name: (*name).to_string(),
                    animations: entries
                        .iter()
                        .map(|&(typ, file_index)| PrimitiveAnimationTypeData {
                            typ,
                            file_index,
                            ..Default::default()
                        })
                        .collect(),
                })
                .collect(),
            ..Default::default()
        }
    }

    /// Shape of a character's "default" group: locomotion plus the reaction
    /// band (DAMAGE1/2, STUN, DOWN block) this change names.
    fn reaction_resource() -> SroResource {
        resource(&[
            (
                "default",
                &[
                    (ANIM_TYPE_STAND, 0),
                    (ANIM_TYPE_RUN, 1),
                    (ANIM_TYPE_DAMAGE[0], 2),
                    (ANIM_TYPE_DAMAGE[1], 3),
                    (ANIM_TYPE_STUN, 4),
                    (ANIM_TYPE_DOWN[0], 5),
                    (ANIM_TYPE_DOWN[1], 6),
                    // knockdown death ships without a clip in plenty of
                    // groups: the sentinel, not a missing row.
                    (ANIM_TYPE_DOWN[4], u32::MAX),
                ],
            ),
            // Weapon groups are sparse — a sword stance re-animates its own
            // stand/run and one hit reaction, nothing else.
            ("sword", &[(ANIM_TYPE_STAND, 10), (ANIM_TYPE_DAMAGE[0], 11)]),
        ])
    }

    #[test]
    fn resolves_reaction_band_from_default_group() {
        let res = reaction_resource();
        assert_eq!(res.find_animation(None, ANIM_TYPE_DAMAGE[0]), Some(2));
        assert_eq!(res.find_animation(None, ANIM_TYPE_DAMAGE[1]), Some(3));
        assert_eq!(res.find_animation(None, ANIM_TYPE_STUN), Some(4));
        assert_eq!(res.find_animation(None, ANIM_TYPE_DOWN[0]), Some(5));
        assert_eq!(res.find_animation(None, ANIM_TYPE_DOWN[1]), Some(6));
    }

    #[test]
    fn sparse_weapon_group_falls_back_to_default_reactions() {
        let res = reaction_resource();
        // own entry wins ...
        assert_eq!(
            res.find_animation(Some("sword"), ANIM_TYPE_DAMAGE[0]),
            Some(11)
        );
        // ... everything the stance lacks falls back to "default"
        assert_eq!(res.find_animation(Some("sword"), ANIM_TYPE_STUN), Some(4));
        assert_eq!(
            res.find_animation(Some("sword"), ANIM_TYPE_DOWN[0]),
            Some(5)
        );
    }

    #[test]
    fn missing_reaction_states_stay_unknown() {
        let res = reaction_resource();
        // absent row: DOWN_UP is simply not in this table
        assert_eq!(res.find_animation(None, ANIM_TYPE_DOWN[3]), None);
        // present row carrying the u32::MAX sentinel: still no clip
        assert_eq!(res.find_animation(None, ANIM_TYPE_DOWN[4]), None);
        // a group without any reaction rows resolves nothing rather than
        // silently borrowing a neighbouring state's clip
        let bare = resource(&[("default", &[(ANIM_TYPE_STAND, 0)])]);
        assert_eq!(bare.find_animation(None, ANIM_TYPE_STUN), None);
        assert_eq!(bare.find_animation(None, ANIM_TYPE_DAMAGE[0]), None);
    }
}
