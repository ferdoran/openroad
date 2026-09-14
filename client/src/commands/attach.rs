use std::f32::consts::FRAC_1_SQRT_2;

use bevy::asset::{AssetServer, Assets};
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::system::Command;
use bevy::log::{debug, trace, warn};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::{Entity, Handle, Mesh, Mut, Name, Quat, Transform, Visibility, World};

use crate::assets::bms::mesh::JMXVBMS;
use crate::assets::bsk::JMXVBSK;
use crate::assets::bsr::bsr::ResAttachInfo;
use crate::assets::bsr::resource::SroResource;
use crate::commands::{
    MeshIndexMap, PreparedMeshGroups, ReversedWinding, SkeletonBinding, SpawnResource,
    SpawnedFromResource,
};
use crate::plugins::map::objects::{SroBindPoses, SroMeshes};

/// SRO's canonical in-hand grip orientation (180° about the (0, 1, −1) axis):
/// the rotation that every correctly-held weapon and shield bakes into its
/// root bone origin. Applied to all hand attachments so the handful of CH
/// weapons with inconsistent origins seat the same way.
pub(crate) const HAND_GRIP: Quat = Quat::from_xyzw(0.0, FRAC_1_SQRT_2, -FRAC_1_SQRT_2, 0.0);

/// Attaches an item resource (weapon, shield, clothes, ...) to an already
/// spawned character resource so it follows the character's animations.
pub struct AttachResource {
    pub item: Handle<SroResource>,
    /// The wrapper entity a character `SpawnResource` created, carrying the
    /// [`SkeletonBinding`] of the character's bones.
    pub character_wrapper: Entity,
}

impl Command for AttachResource {
    type Out = ();

    fn apply(self, world: &mut World) {
        // Scope out the mesh/bind-pose dedup caches (and the asset
        // collections they manage) once for the whole attach, shared with
        // the nested `SpawnResource` (see its `apply` for why).
        world.init_resource::<SroMeshes>();
        world.init_resource::<SroBindPoses>();
        world.resource_scope(|world, mesh_cache: Mut<SroMeshes>| {
            world.resource_scope(|world, bind_pose_cache: Mut<SroBindPoses>| {
                world.resource_scope(|world, meshes: Mut<Assets<Mesh>>| {
                    world.resource_scope(
                        |world, inverse_bindposes: Mut<Assets<SkinnedMeshInverseBindposes>>| {
                            self.apply_with_caches(
                                world,
                                mesh_cache.into_inner(),
                                bind_pose_cache.into_inner(),
                                meshes.into_inner(),
                                inverse_bindposes.into_inner(),
                            );
                        },
                    );
                });
            });
        });
    }
}

impl AttachResource {
    fn apply_with_caches(
        self,
        world: &mut World,
        mesh_cache: &mut SroMeshes,
        bind_pose_cache: &mut SroBindPoses,
        meshes: &mut Assets<Mesh>,
        inverse_bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    ) {
        let Some(resource) = world.resource::<Assets<SroResource>>().get(self.item.id()) else {
            warn!("failed to get item resource for: {:?}", &self.item.path());
            return;
        };
        // The character can despawn while the item's resource loads (a remote
        // player walking out of range mid-attach); skip rather than reading a
        // dead wrapper (panic).
        if world.get_entity(self.character_wrapper).is_err() {
            return;
        }
        let name = resource.object_info.name.clone();
        let has_skeleton = resource.skeleton.is_some();
        let attachment_bone = resource.attachment_bone.clone();
        let item_attach = resource.attach_info.clone();

        // Attachments hang under the character (which is mirrored on X), so they
        // reverse their winding exactly when the character does. Inherit the
        // character's recorded flag; the item's own root transform is a pure
        // rotation and doesn't change the winding sign.
        let reverse_winding = world
            .entity(self.character_wrapper)
            .get::<ReversedWinding>()
            .map(|r| r.0)
            .unwrap_or(false);

        let Some(binding) = world
            .entity(self.character_wrapper)
            .get::<SkeletonBinding>()
        else {
            warn!(
                "character wrapper has no skeleton binding, cannot attach: {}",
                name
            );
            return;
        };

        if has_skeleton {
            // items with their own skeleton (e.g. weapons) hang onto a single
            // bone of the character and bring their own bones/meshes
            let Some(bone_name) = attachment_bone else {
                warn!("item {} has a skeleton but no attachment bone", name);
                return;
            };
            let Some(bone_entity) = binding.bones.get(&bone_name).copied() else {
                warn!(
                    "attachment bone {} of item {} not found in character skeleton",
                    bone_name, name
                );
                return;
            };

            // the item root bone's transform (e.g. a blade's 180° flip) gets
            // cancelled by the inverse bind pose during skinning, but the
            // original engine applies it - so it has to go on the wrapper
            let mut root_transform = resource
                .skeleton
                .as_ref()
                .and_then(|s| world.resource::<Assets<JMXVBSK>>().get(s))
                // The true root, composed from parent fields AND child lists:
                // multi-root skeletons leave several bones with an empty
                // parent field, so `find(is_empty)` picked an arbitrary one
                // and lost the synthetic `[root]` transform (#280).
                .and_then(|skeleton| skeleton.root_bone())
                .map(|root| Transform::from_matrix(root.get_origin_matrix()))
                .unwrap_or_default();

            // NOTE: this note used to say characters "now carry" the world's
            // `scale.x = -1` mirror. That was only half true — the char-select
            // previews and the test scenes did, but the in-world spawn paths
            // (local player, remote players, NPCs, monsters, COS) never had it,
            // so the same equipment seated into two different handednesses
            // depending on the scene. All of them are mirrored as of the
            // knockdown/mirror QoL pass, so previews and world finally agree
            // and one calibration can be right for both.
            //
            // The orientations below are deliberately UNCHANGED, and the
            // reasoning is worth keeping because "everything is mirrored now,
            // so flip every rotation" is the tempting wrong answer:
            //
            // - An attachment is a *child* of the mirrored root, so its world
            //   orientation went from `R` to `M·R` — a left-multiply, NOT the
            //   conjugation `M·R·M`. Only conjugation would turn a rotation
            //   about an axis into its negative, so the usual "mirrored
            //   rotations invert" intuition does not apply here.
            // - `HAND_GRIP` is 180 deg about (0,1,-1), and the bow's extra turn
            //   is 180 deg about Y. A 180 deg rotation about `a` equals one
            //   about `-a`, so both are mirror-invariant regardless.
            // - The ward's `-90 deg about Z` sends the item's local +X to world
            //   -Y (straight down). `M` negates X, and (0,-1,0) has no X
            //   component, so the hang direction does not move either.
            // - Front/back faces are preserved: the mirror flips face
            //   orientation and the inherited winding reversal flips it back.
            //   The CH/EU meshes are mirror images of *each other* in the data,
            //   so their relative difference — and the `rotation.x < 0.0` race
            //   test that keys off it — is unaffected.
            //
            // What genuinely changes is left/right *placement*, which is the
            // correction we wanted. Still worth a GPU pass: if a hand item shows
            // its dark back face or a ward hangs on the wrong side, this block
            // is where to look.
            //
            // Weapons and shields held in a hand should all seat with the same
            // grip. Almost every hand item bakes this exact rotation into its
            // root bone origin (verified across all EU weapons, the CH sword
            // and every shield), but a few CH weapons (blade/spear/glaive/bow)
            // carry inconsistent origins that render them rotated sideways.
            // Force the canonical grip for anything attached to a hand so they
            // all point the same way; non-hand items (e.g. the garment
            // "talisman" on the neck) keep their own origin.
            if bone_name.contains("Hand") {
                root_transform.rotation = HAND_GRIP;
                // The bow is rigged unlike the other hand weapons: with the
                // plain grip it comes out upside down, string toward the palm.
                // A half turn about its own long axis seats the grip in the
                // hand and stands it upright. (crossbow starts with "cross",
                // so it is not caught here.)
                if name.starts_with("bow") {
                    root_transform.rotation =
                        HAND_GRIP * Quat::from_rotation_y(std::f32::consts::PI);
                }
            } else {
                // Non-hand skeleton attachments - the garment "talisman" paper
                // wards on the neck. The wards are modelled extending along the
                // item's local +X; the neck bone's local +Y is world up, so a
                // -90 deg turn about Z sends +X to world-down and the wards hang
                // vertically beside the shoulders. The item's own root origin
                // (120 deg about a diagonal) instead sends +X to world-up, which
                // is why they rendered upside down.
                //
                // CH and EU wards have mirror-authored meshes (their root
                // origins are mirror images: EU 120 deg about (1,1,1), CH about
                // (-1,-1,1) - so the origin quaternion's x flips sign). With the
                // same wrapper rotation one race shows its lit front face and
                // the mirrored one shows its dark back face; spin the mirrored
                // (CH) ward 180 deg about its own hang axis to bring the front
                // face forward. EU keeps the plain rotation.
                let mut rotation = Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2);
                if root_transform.rotation.x < 0.0 {
                    rotation *= Quat::from_rotation_x(std::f32::consts::PI);
                }
                root_transform.rotation = rotation;
            }

            SpawnResource {
                resource: self.item,
                transform: root_transform,
                parent: Some(bone_entity),
                animation_group: None,
                // inherit the mirrored character's winding (see above)
                reverse_winding,
                material_variant: super::MaterialVariant::Base,
            }
            .apply_with_caches(
                world,
                mesh_cache,
                bind_pose_cache,
                meshes,
                inverse_bindposes,
            );
            trace!("attached {} to bone {}", name, bone_name);
        } else {
            // clothes/armor bring no skeleton of their own: their meshes are
            // skinned directly to the character's bones
            // `.first()`, not `[0]`: a resource can carry no material set at
            // all (the corpus ships 221-byte stub `.bsr` files), and indexing
            // those panicked rather than skipping them.
            let Some(material_set_path) =
                resource.materials.first().and_then(|handle| handle.path())
            else {
                debug!(
                    "no material set to skin to ({} entries); item not attached",
                    resource.materials.len()
                );
                return;
            };
            // Clothes are skinned to the character's skeleton, so that (not
            // the item's own, absent one) keys the shared bind-pose cache.
            let character_skeleton = world
                .entity(self.character_wrapper)
                .get::<SpawnedFromResource>()
                .and_then(|spawned| world.resource::<Assets<SroResource>>().get(spawned.0.id()))
                .and_then(|character| character.skeleton.clone());
            let bms_assets = world.resource::<Assets<JMXVBMS>>();
            let asset_server = world.resource::<AssetServer>();
            // worn clothes/armor are the character's visible body — they get
            // the always-on rim whenever it is enabled, regardless of their
            // res/item path (weapons/metal armor are sheen and carry the rim
            // inside the sheen material instead)
            let rim = world
                .get_resource::<crate::assets::bmt::material::BmtMaterialDefaults>()
                .is_some_and(|defaults| defaults.rim.is_some());
            // Per-part LOD tuning (`graphics.objects`), same read as the
            // resource-spawn path in `mod.rs`.
            let lod = world
                .get_resource::<crate::plugins::config::ClientConfig>()
                .map(|config| config.graphics.objects.clone())
                .unwrap_or_default();
            let mesh_groups = PreparedMeshGroups::prepare(
                asset_server,
                resource,
                bms_assets,
                material_set_path,
                &binding.bind_poses,
                true,
                // inherit the mirrored character's winding (see above)
                reverse_winding,
                character_skeleton.as_ref(),
                mesh_cache,
                bind_pose_cache,
                meshes,
                inverse_bindposes,
                &lod,
                rim,
            );
            let bones = binding.bones.clone();

            world
                .spawn((
                    Transform::default(),
                    Visibility::Inherited,
                    Name::from(name.clone()),
                    // lets consumers (e.g. the portrait rig's head-slot
                    // filter) resolve which item this wrapper renders
                    SpawnedFromResource(self.item.clone()),
                    ChildOf(self.character_wrapper),
                ))
                .with_children(|item_entity| {
                    mesh_groups.spawn(item_entity, &bones);
                });
            trace!("attached {} to character skeleton", name);
        }

        // the item now occupies body part slots, so the character's default
        // look of those parts (naked torso, hair, ...) has to be hidden
        if let Some(item_attach) = item_attach {
            Self::set_replaced_meshes_visibility(
                world,
                self.character_wrapper,
                &item_attach,
                &name,
                Visibility::Hidden,
            );
        }
    }
}

impl AttachResource {
    fn set_replaced_meshes_visibility(
        world: &mut World,
        character_wrapper: Entity,
        item_attach: &ResAttachInfo,
        item_name: &str,
        visibility: Visibility,
    ) {
        // only REPLACE (1) items supplant the character's default look; ADD
        // (2) items are overlays worn on top of bare skin (bracers over the
        // forearm, shoulder pads, capes, ...)
        if item_attach.attach_method != 1 {
            return;
        }

        let Some(char_handle) = world
            .entity(character_wrapper)
            .get::<SpawnedFromResource>()
            .map(|r| r.0.clone())
        else {
            warn!("character wrapper has no resource handle, cannot hide replaced meshes");
            return;
        };
        let Some(char_resource) = world
            .resource::<Assets<SroResource>>()
            .get(char_handle.id())
        else {
            warn!(
                "failed to get character resource for: {:?}",
                char_handle.path()
            );
            return;
        };
        // without attach info the character has no default part meshes that
        // could be replaced
        let Some(char_attach) = &char_resource.attach_info else {
            return;
        };

        let mesh_indices = item_attach
            .slots
            .iter()
            .filter_map(|(slot_id, _)| {
                // e.g. weapon slots have no default look on the character
                char_attach
                    .slots
                    .iter()
                    .find(|(s, _)| s == slot_id)
                    .map(|(_, mesh_idx)| *mesh_idx)
            })
            .collect::<Vec<_>>();

        let Some(mesh_map) = world.entity(character_wrapper).get::<MeshIndexMap>() else {
            warn!("character wrapper has no mesh index map, cannot hide replaced meshes");
            return;
        };
        let mesh_entities = mesh_indices
            .iter()
            .filter_map(|idx| match mesh_map.0.get(idx) {
                Some(entity) => Some(*entity),
                None => {
                    warn!("mesh {} not found on character", idx);
                    None
                }
            })
            .collect::<Vec<_>>();

        for entity in mesh_entities {
            world.entity_mut(entity).insert(visibility);
        }
        if !mesh_indices.is_empty() {
            trace!(
                "set default meshes {:?} replaced by {} to {:?}",
                mesh_indices,
                item_name,
                visibility
            );
        }
    }
}

/// The inverse of [`AttachResource`]: despawns whatever an earlier attach of
/// the same item resource spawned under the character (matched by the
/// [`SpawnedFromResource`] tag anywhere in the wrapper's subtree, which covers
/// both boned weapons under a hand bone and skinned clothes under the wrapper)
/// and re-shows the character's default part meshes the item had replaced.
pub struct DetachResource {
    pub item: Handle<SroResource>,
    /// The same wrapper entity the attach targeted (the character's
    /// [`SkeletonBinding`] holder).
    pub character_wrapper: Entity,
}

impl Command for DetachResource {
    type Out = ();

    fn apply(self, world: &mut World) {
        // Find every entity spawned from this item resource whose ancestor
        // chain reaches the character wrapper (excludes the same item worn by
        // other characters, e.g. the paper-doll clone).
        let mut spawned = world.query::<(Entity, &SpawnedFromResource)>();
        let candidates: Vec<Entity> = spawned
            .iter(world)
            .filter(|(e, from)| *e != self.character_wrapper && from.0.id() == self.item.id())
            .map(|(e, _)| e)
            .collect();
        let mut removed_any = false;
        for entity in candidates {
            let mut ancestor = entity;
            while let Some(parent) = world.entity(ancestor).get::<ChildOf>().map(|c| c.parent()) {
                if parent == self.character_wrapper {
                    world.entity_mut(entity).despawn();
                    removed_any = true;
                    break;
                }
                ancestor = parent;
            }
        }
        if !removed_any {
            warn!(
                "detach: no attachment of {:?} found under the character",
                self.item.path()
            );
        }

        // Bring the replaced default look (naked torso, hair, ...) back.
        let item_attach = world
            .resource::<Assets<SroResource>>()
            .get(self.item.id())
            .and_then(|resource| resource.attach_info.clone());
        if let Some(item_attach) = item_attach {
            if item_attach.attach_method == 1 {
                let name = self
                    .item
                    .path()
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "<unknown item>".into());
                AttachResource::set_replaced_meshes_visibility(
                    world,
                    self.character_wrapper,
                    &item_attach,
                    &name,
                    Visibility::Inherited,
                );
            }
        }
    }
}
