use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext};
use bevy::prelude::{default, Handle};

use crate::assets::bms::mesh::JMXVBMS;
use crate::assets::bmt::material::JMXVBMT;
use crate::assets::bsk::JMXVBSK;
use crate::assets::bsr::bsr::{
    parse_bsr, parse_dyvertex_mods, parse_particle_mods, parse_sound_mods, AnimationData,
};
use crate::assets::bsr::resource::{
    BlendModEntry, DyVertexModEntry, EffectModOwner, ParticleModEntry, SoundModEntry, SroResource,
    TexAniModEntry,
};
use crate::assets::bsr::BsrLoaderError;

/// Loads a .bsr into an [`SroResource`]: the byte-level section walk lives
/// in [`parse_bsr`] (shared with standalone tools); this loader only turns
/// the returned sub-asset paths into asset handles and maps the particle
/// mod palette onto effect entries.
#[derive(Default, bevy::reflect::TypePath)]
pub struct BsrLoaderV2;

impl AssetLoader for BsrLoaderV2 {
    type Asset = SroResource;
    type Settings = ();
    type Error = BsrLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader
            .read_to_end(&mut buf)
            .await
            .map_err(BsrLoaderError::IO)?;
        let parsed = parse_bsr(&buf)?;

        let mut mesh = Vec::with_capacity(parsed.mesh_paths.len());
        for (i, path) in parsed.mesh_paths.iter().enumerate() {
            let a = load_context
                .load_builder()
                .load_untyped_value(format!("data://{}", path.display()))
                .await
                .expect("failed to load mesh")
                .take::<JMXVBMS>()
                .unwrap();
            let a = load_context.add_labeled_asset(format!("bms{}", i), a);
            mesh.push(a);
        }

        let materials: Vec<Handle<JMXVBMT>> = parsed
            .material_sets
            .iter()
            .map(|material_data| {
                load_context.load(format!("data://{}", material_data.path.display()))
            })
            .collect();

        let mut attachment_bone = None;
        let skeleton: Option<Handle<JMXVBSK>> =
            parsed.skeleton.as_ref().map(|(bsk_path, prim_bone)| {
                // for attachable items (e.g. weapons) this names the bone of
                // the *target* skeleton the item gets attached to
                if !prim_bone.is_empty() {
                    attachment_bone = Some(prim_bone.clone());
                }
                load_context.load(format!("data://{}", bsk_path.display()))
            });

        let animation = AnimationData {
            type_version: parsed.animation_type_version,
            type_user_define: parsed.animation_type_user_define,
            animations: parsed
                .animation_paths
                .iter()
                .map(|path| load_context.load(format!("data://{}", path.display())))
                .collect(),
        };

        let effect_mods = parse_particle_mods(&buf, parsed.header.mod_palette_offset as usize)
            .into_iter()
            .map(|raw| {
                let owner = match raw.set {
                    Some((1, anim_type, group)) => EffectModOwner::Animation { group, anim_type },
                    Some((0, ..)) => EffectModOwner::External,
                    // typ 2 ("ambient"), unexpected typ, or no decodable owner
                    _ => EffectModOwner::AlwaysOn,
                };
                ParticleModEntry {
                    path: raw.path.replace('\\', "/"),
                    bone: raw.bone,
                    offset: raw.offset,
                    delay_ms: raw.delay_ms,
                    night_only: raw.night_only,
                    scale: raw.scale,
                    owner,
                }
            })
            .collect();

        // Sound ModData tracks. Only the animation-linked (typ 1) sets are
        // kept: they are the ones a playing animation can drive (39,783 of
        // the corpus's 45,086 tracks). typ 0 sets are referenced externally
        // by skill/state logic and typ 2 ("ambient", 3 tracks) would need a
        // looping ambience channel — neither has a consumer, so keeping them
        // would only be dead data.
        let sound_mods = parse_sound_mods(&buf, parsed.header.mod_palette_offset as usize)
            .into_iter()
            .filter_map(|raw| {
                let Some((1, anim_type, group)) = raw.set else {
                    return None;
                };
                // 105 of the 1,896 corpus paths are authored with a leading
                // `sound\` component that has no counterpart in Data.pk2 (the
                // archive has no `sound/` directory); 104 of those resolve
                // once it is dropped.
                let normalized = raw.path.replace('\\', "/");
                let path = normalized
                    .strip_prefix("sound/")
                    .unwrap_or(&normalized)
                    .to_string();
                Some(SoundModEntry {
                    path,
                    key_time_ms: raw.key_time_ms,
                    group,
                    anim_type,
                })
            })
            .collect();

        // DyVertex (soft-body) flags. Every set type is kept: this is a
        // static property of the material, not something an animation drives,
        // and the census gives no set-type split for it.
        let dyvertex_mods = parse_dyvertex_mods(&buf, parsed.header.mod_palette_offset as usize)
            .into_iter()
            .map(|raw| DyVertexModEntry {
                mtrl_idx: u32::try_from(raw.mtrl_idx).ok(),
            })
            .collect();

        let texani_mods = parsed
            .texani_mods
            .iter()
            // only always-on ("ambient" system set / no decodable owner)
            // entries; animation-linked and external sets are not driven yet
            .filter(|raw| matches!(raw.set, Some((2, ..)) | None))
            // UnkUInt06 == 1 means the transform belongs to the MultiTex
            // second stage (117 of its 118 corpus carriers also hold a
            // MultiTex mod on the same MtrlIdx). We have no MultiTex
            // consumer, so applying it would scroll the base diffuse
            // instead of the authored overlay — drop until we do.
            .filter(|raw| !raw.multi_tex_stage)
            .filter(|raw| raw.uv_speed != bevy::math::Vec2::ZERO)
            .map(|raw| {
                if raw.non_translation {
                    bevy::log::warn!(
                        "{}: TexAni matrix has non-translation terms, applying UV scroll only",
                        load_context.path()
                    );
                }
                TexAniModEntry {
                    mtrl_idx: u32::try_from(raw.mtrl_idx).ok(),
                    uv_speed: raw.uv_speed,
                }
            })
            .collect();

        // D3DBLEND: 2 = ONE, 5 = SRCALPHA, 6 = INVSRCALPHA. Only the two
        // pairs the corpus actually pairs with TexAni are mapped; anything
        // else keeps the default masked rendering.
        let blend_mods = parsed
            .material_mods
            .iter()
            .filter(|raw| matches!(raw.set, Some((2, ..)) | None))
            .filter_map(|raw| {
                let alpha_mode = match (raw.src_blend, raw.dst_blend) {
                    (5, 6) => bevy::prelude::AlphaMode::Blend,
                    (5, 2) => bevy::prelude::AlphaMode::Add,
                    _ => return None,
                };
                Some(BlendModEntry {
                    mtrl_idx: u32::try_from(raw.mtrl_idx).ok(),
                    alpha_mode,
                })
            })
            .collect();

        let resource = SroResource {
            header: parsed.header,
            object_info: parsed.object_info,
            collision_mesh: parsed.collision_mesh,
            mesh,
            animation,
            materials,
            skeleton,
            attachment_bone,
            attach_info: parsed.attach_info,
            alpha_is_sheen: parsed.alpha_is_sheen,
            sheen_alpha_test: parsed.sheen_alpha_test,
            primitive_group: parsed.primitive_group,
            primitive_animation_group: parsed.primitive_animation_group,
            effect_mods,
            texani_mods,
            blend_mods,
            sound_mods,
            dyvertex_mods,
            ..default()
        };
        Ok(resource)
    }

    fn extensions(&self) -> &[&str] {
        &["bsr"]
    }
}
