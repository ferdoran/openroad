// https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVBMT

use std::io::Cursor;
use std::path::PathBuf;

use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::color::LinearRgba;
use bevy::pbr::ExtendedMaterial;
use bevy::prelude::{trace, AlphaMode, Color, Handle, Image, StandardMaterial};
use bevy::reflect::TypePath;
use bevy::render::render_resource::Face;
use bevy::utils::default;
use bytes::Buf;
use thiserror::Error;

use crate::assets::bmt::rim::{RimExtension, RimSettings};
use crate::assets::bmt::sheen::{
    SheenExtension, SheenSettings, ENV_SPHEREMAP_PATH, SHEEN_ALPHA_CUTOUT, SHINE_SPHEREMAP_PATH,
};

use crate::util::buf_ext::BufExt;

#[derive(Asset, TypePath, Debug, Default, Clone)]
#[allow(dead_code)]
pub struct SroMaterial {
    pub name: String,
    pub path: PathBuf,
    pub ambient: Color,
    pub diffuse: Color,
    pub specular: Color,
    pub emissive: Color,
    pub power: f32,
    pub flag: u32,
    pub diffuse_map_path: PathBuf,
    pub diffuse_map_float: f32, // could be lightness? contains at least same value as in Color
    pub diffuse_map_unk0: u8,
    pub diffuse_map_unk1: u8,
    pub diffuse_map_is_relative: bool,
}

impl<T: Buf> From<&mut T> for SroMaterial {
    fn from(value: &mut T) -> Self {
        Self {
            name: value.get_double_len_string(),
            diffuse: value.get_color_rgba(),
            ambient: value.get_color_rgba(),
            specular: value.get_color_rgba(),
            emissive: value.get_color_rgba(),
            power: value.get_f32_le(),
            flag: value.get_u32_le(),
            diffuse_map_path: value.get_path_buf_double_len(),
            diffuse_map_float: value.get_f32_le(),
            diffuse_map_unk0: value.get_u8(),
            diffuse_map_unk1: value.get_u8(),
            diffuse_map_is_relative: value.get_u8() == 1,
            ..default()
        }
    }
}

// PrimMtrlFlag decoded from a census of all 12,227 materials in Data.pk2
// (2026-07-09). Only 6 bits are ever set anywhere; nothing above 0x200 is used:
//   0x0001 bit0  two-sided / no backface cull        (26.3%)  is_two_sided
//   0x0004 bit2  D3D sun specular enable              (0.2%)  is_specular_emphasis
//   0x0008 bit3  self-illuminated / emissive          (0.9%)  is_emissive
//   0x0040 bit6  ALWAYS set - mandatory format marker (100%)  has_color_tint (misnomer)
//   0x0100 bit8  has diffuse map / texture           (96.4%)  has_diffuse_map
//   0x0200 bit9  has alpha channel                   (36.4%)  has_alpha_channel
// The wiki's "BumpMap" 0x2000 is never set in this build; bits 1,4,5,7,10-31 unused.
#[allow(dead_code)]
impl SroMaterial {
    /// PrimMtrlFlag Bit6 (0x40). The wiki labels this "ColorTint?", but the
    /// census found it set on 100% of materials - it is a mandatory
    /// "lit material" format marker, so this predicate is always true.
    pub fn has_color_tint(&self) -> bool {
        self.flag & 0x40 != 0
    }

    pub fn has_diffuse_map(&self) -> bool {
        self.flag & 0x100 != 0
    }

    pub fn has_alpha_channel(&self) -> bool {
        self.flag & 0x200 != 0
    }

    /// PrimMtrlFlag Bit14 (0x2000), documented as "BumpMap" on the wiki, but
    /// NOT set on any material in this client build's Data.pk2 - effectively
    /// dead (always false) here.
    pub fn has_bump_map(&self) -> bool {
        self.flag & 0x2000 != 0
    }

    /// PrimMtrlFlag Bit1 (0x1): render both faces (no backface cull).
    /// Set on hair, foliage/leaves, grass, flags, ropes, caps, and paper-ward
    /// talismans - thin card geometry authored as single-sided quads. Verified
    /// against Data.pk2: correlates exactly with two-sided geometry and is
    /// independent of the alpha bit (0x200).
    pub fn is_two_sided(&self) -> bool {
        self.flag & 0x1 != 0
    }

    /// PrimMtrlFlag Bit3 (0x8): self-illuminated / emissive. Set almost
    /// exclusively on building materials whose textures are named `*_winlight`,
    /// `window`, `light`, `jewel`, xmas lights - the panes/lamps that glow at
    /// night. The flag (not the emissive color field) is the render signal and
    /// drives `StandardMaterial::emissive`, always-on until a day/night cycle
    /// exists. Verified against Data.pk2 (104 materials, 92 of them buildings).
    pub fn is_emissive(&self) -> bool {
        self.flag & 0x8 != 0
    }

    /// PrimMtrlFlag Bit2 (0x4): classic D3D sun specular. Verified in
    /// sro_client.exe (material apply fn ~0xc82300): the flag turns on
    /// `D3DRS_SPECULARENABLE` for the draw (restore fn ~0xc82450 turns it
    /// back off) and the BMT specular color + power fill the D3DMATERIAL9,
    /// so the directional sun produces a fixed-function Phong highlight.
    /// Only 19 materials in all of Data.pk2 opt in (Jangan palace set,
    /// stone lion statues, horse skulls, ... - power 7-170, tinted colors
    /// like (0.5,0.25,0)); everything else stays pure diffuse.
    pub fn is_specular_emphasis(&self) -> bool {
        self.flag & 0x4 != 0
    }
}

#[derive(TypePath, Asset, Debug, Clone)]
pub struct JMXVBMT {
    pub materials: Vec<SroMaterial>,
}

impl JMXVBMT {
    pub fn from<T: Buf>(mut value: &mut T, path: PathBuf) -> Self {
        let count = value.get_u32_le();
        // debug!("{}: found {} materials", path.display(), count);
        let mut materials = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let mut mat = SroMaterial::from(&mut value);
            mat.path = path.clone();
            materials.push(mat);
        }

        Self { materials }
    }
}

/// The config-derived material bases every `.bmt` labeled sub-asset starts
/// from. The client bin inserts it from `graphics.{sheen,rim}` before the
/// asset plugins register; without it (tools, tests) [`BmtLoader`] falls
/// back to the faithful defaults (no ambient rim, neutral sheen).
#[derive(bevy::prelude::Resource, Clone, Copy, Debug)]
pub struct BmtMaterialDefaults {
    /// Base settings of the `.sheen`/`.sheen_cutout` sub-assets.
    pub sheen: SheenSettings,
    /// `Some` = build a `.rim` sub-asset per material with these settings
    /// and use it as the default for character-class meshes (see
    /// `PreparedMeshGroups::prepare`); `None` = rim disabled.
    pub rim: Option<RimSettings>,
}

impl Default for BmtMaterialDefaults {
    fn default() -> Self {
        Self {
            sheen: SheenSettings::default(),
            rim: None,
        }
    }
}

/// Carries the config-derived [`BmtMaterialDefaults`] so the sub-assets it
/// builds start from `graphics.{sheen,rim}` (the loader has no world access
/// at load time, so the settings are captured at registration via
/// [`FromWorld`]; no `Default` derive — bevy's blanket
/// `FromWorld for T: Default` would collide with the manual impl).
#[derive(bevy::reflect::TypePath)]
pub struct BmtLoader {
    defaults: BmtMaterialDefaults,
}

impl bevy::prelude::FromWorld for BmtLoader {
    fn from_world(world: &mut bevy::prelude::World) -> Self {
        // Inserted by the client bin before the asset plugins register (this
        // shared assets code can't see the config types — the parser-only
        // lib target has no `plugins` module); absent (tools, tests) →
        // faithful defaults.
        let defaults = world
            .get_resource::<BmtMaterialDefaults>()
            .copied()
            .unwrap_or_default();
        Self { defaults }
    }
}

#[derive(Error, Debug)]
pub enum BmtLoaderError {
    #[error("IO error: {0}")]
    IO(std::io::Error),
    #[error("invalid signature: {0}")]
    Signature(String),
}

// type MaterialSet = Vec<SroMaterial>;

impl AssetLoader for BmtLoader {
    type Asset = JMXVBMT;
    type Settings = ();
    type Error = BmtLoaderError;
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
            .map_err(|e| BmtLoaderError::IO(e))?;
        let mut cursor = Cursor::new(&buf);
        let sig = cursor.get_fixed_size_string(12);
        if sig != "JMXVBMT 0102" {
            return Err(BmtLoaderError::Signature(sig));
        }
        let mut path = load_context.path().path().to_path_buf();
        path.pop();
        let material = JMXVBMT::from(&mut cursor, path);

        for mat in &material.materials {
            // info!("material: {}", &mat.name);
            let image: Handle<Image> = if mat.diffuse_map_is_relative {
                let path = format!("data://{}", mat.diffuse_map_path.display());
                // info!("relative path: {}", path);
                // PathBuf::from("")
                load_context.load(path)
            } else {
                let path = format!("data://{}", mat.path.display());
                // info!("absolute path part1: '{}'", path);
                let path = format!("{}/{}", path, mat.diffuse_map_path.display());
                // info!("absolute path part2: '{}'", mat.diffuse_map_path.display());
                let p = format!("{}", mat.diffuse_map_path.display());
                if p.is_empty() {
                    trace!(
                        "empty material path in: {} for material {}",
                        load_context.path().path().display(),
                        &mat.name
                    );
                    continue;
                }
                // let mut path = PathBuf::from(format!("data://{}/{}", mat.path.display(), mat.diffuse_map_path.display()));
                // path.extend(&mat.diffuse_map_path);
                // info!("absolute path: {}", &path);
                load_context.load(path)
            };
            // let image = load_context.load_direct(path).await?;
            // let image: Handle<Image> = load_context.load(path);
            let standard_material =
                mat.to_standard_material(Some(image.clone()), MaterialVariant::Masked);
            load_context.add_labeled_asset(material_label(&mat.name), standard_material);
            // variants for resources whose texture alpha is a sheen mask
            // instead of transparency (see SroResource::alpha_is_sheen);
            // the sheen variant is an extended material whose fragment
            // shader turns that alpha into per-texel metallic (sheen.rs)
            let opaque_material =
                mat.to_standard_material(Some(image.clone()), MaterialVariant::Opaque);
            load_context.add_labeled_asset(opaque_material_label(&mat.name), opaque_material);
            // always-on rim variant for character-class meshes (the spawn
            // side picks it over the plain Masked material when rim is
            // enabled, see PreparedMeshGroups::prepare)
            if let Some(rim) = self.defaults.rim {
                let rim_material = ExtendedMaterial {
                    base: mat.to_standard_material(Some(image.clone()), MaterialVariant::Masked),
                    extension: RimExtension { settings: rim },
                };
                load_context.add_labeled_asset(rim_material_label(&mat.name), rim_material);
            }
            let sheen_material = ExtendedMaterial {
                base: mat.to_standard_material(Some(image.clone()), MaterialVariant::Opaque),
                extension: SheenExtension {
                    settings: self.defaults.sheen,
                    // the enhancement highlight sphere map is shared by all
                    // sheen materials; sampled only once a shine tint is set
                    shine_texture: load_probe(load_context, SHINE_SPHEREMAP_PATH),
                    // the base chrome probe is always sampled
                    env_texture: load_probe(load_context, ENV_SPHEREMAP_PATH),
                },
            };
            load_context.add_labeled_asset(sheen_material_label(&mat.name), sheen_material);
            // sheen with the original's alpha test (GREATEREQUAL ref 1):
            // for resources whose EnvMap mod flags exact-zero alpha as
            // cutout (e.g. glaive blade shapes punched out of the atlas).
            // The base must be Mask, not Opaque: cameras run a depth
            // prepass and the extension only replaces the main-pass
            // fragment, so an Opaque base writes prepass/shadow depth for
            // the very texels the main pass discards — geometry behind
            // can't render there and the cutouts turn into opaque black
            // holes (deg-7/8 CH shields). Mask makes the stock
            // prepass/shadow shaders discard the same texels, while the
            // main pass still receives the raw sampled alpha as the sheen
            // mask (pbr_input_from_standard_material applies no
            // alpha_discard).
            let mut cutout_base = mat.to_standard_material(Some(image), MaterialVariant::Opaque);
            cutout_base.alpha_mode = AlphaMode::Mask(SHEEN_ALPHA_CUTOUT);
            let sheen_cutout_material = ExtendedMaterial {
                base: cutout_base,
                extension: SheenExtension {
                    settings: SheenSettings {
                        alpha_cutout: SHEEN_ALPHA_CUTOUT,
                        ..self.defaults.sheen
                    },
                    shine_texture: load_probe(load_context, SHINE_SPHEREMAP_PATH),
                    env_texture: load_probe(load_context, ENV_SPHEREMAP_PATH),
                },
            };
            load_context.add_labeled_asset(
                sheen_cutout_material_label(&mat.name),
                sheen_cutout_material,
            );
        }

        Ok(material)
    }

    fn extensions(&self) -> &[&str] {
        &["bmt"]
    }
}

/// Load a sheen probe/streak texture as a non-color intensity map (see
/// [`DdjSettings::non_color`]): its stored bytes are the reflection
/// intensity, so an sRGB view would decode mid-gray 128 to 0.216 instead
/// of the intended 0.5 and halve the whole chrome term.
///
/// [`DdjSettings::non_color`]: crate::assets::ddj::DdjSettings
fn load_probe(load_context: &mut LoadContext<'_>, path: &'static str) -> Handle<Image> {
    load_context
        .load_builder()
        .with_settings(|settings: &mut crate::assets::ddj::DdjSettings| settings.non_color = true)
        .load(path)
}

/// Canonical label of a material's default variant within its .bmt. SRO
/// material names are case-insensitive: a mesh (`.bms`) may reference `Gyo`
/// while the `.bmt` stores it as `gyo` (or `waterghost` vs `WaterGhost`). Bevy's
/// labeled-asset lookup is case-*sensitive*, so both the producing side (the
/// loader below) and the requesting side (mesh assembly in `commands`) route the
/// name through this to lowercase it, making the labels match either way.
pub fn material_label(material_name: &str) -> String {
    material_name.to_ascii_lowercase()
}

/// Label of a material's opaque sub-asset variant within its .bmt.
pub fn opaque_material_label(material_name: &str) -> String {
    format!("{}.opaque", material_label(material_name))
}

/// Label of a material's always-on-rim sub-asset variant within its .bmt
/// (the Masked base plus the config ambient rim; only built when
/// [`BmtMaterialDefaults::rim`] is `Some`).
pub fn rim_material_label(material_name: &str) -> String {
    format!("{}.rim", material_label(material_name))
}

/// Label of a material's metallic-sheen sub-asset variant within its .bmt.
pub fn sheen_material_label(material_name: &str) -> String {
    format!("{}.sheen", material_label(material_name))
}

/// Label of the sheen variant that also cuts out exact-zero alpha texels
/// (for resources with the EnvMap alpha-test flag,
/// [`SroResource::sheen_alpha_test`](crate::assets::bsr::resource::SroResource::sheen_alpha_test)).
pub fn sheen_cutout_material_label(material_name: &str) -> String {
    format!("{}.sheen_cutout", material_label(material_name))
}

/// Emissive intensity of self-illuminated (flag 0x8) materials; visual
/// calibration constant (bevy's emissive is tonemapped scene-linear, so
/// plain 1.0 washes out next to daylight).
const EMISSIVE_BOOST: f32 = 2.0;

/// Reflectance of flag-0x4 (sun specular) materials; visual calibration
/// constant (0.5 = the 4% dielectric F0 PBR default, matching SheenSettings).
const SPECULAR_EMPHASIS_REFLECTANCE: f32 = 0.5;

/// D3D fixed-function Phong exponent -> perceptual roughness via the Phong ->
/// Beckmann width conversion alpha = sqrt(2 / (power + 2)). The census range
/// (power 7..170) maps to ~0.47..0.11. The 0.1 floor stays above bevy's
/// internal roughness minimum and avoids needle-thin aliasing highlights; the
/// 0.8 ceiling equals the pure-diffuse default.
fn phong_power_to_perceptual_roughness(power: f32) -> f32 {
    (2.0 / (power + 2.0)).sqrt().clamp(0.1, 0.8)
}

/// Which labeled sub-asset variant of a material to build. The third,
/// `"{name}.sheen"`, is not built here: it's an [`SroSheenMaterial`]
/// (extended material) wrapping the Opaque variant, whose fragment shader
/// turns the texture alpha into per-texel metallic — see `sheen.rs`.
///
/// [`SroSheenMaterial`]: crate::assets::bmt::sheen::SroSheenMaterial
#[derive(Clone, Copy, PartialEq)]
pub enum MaterialVariant {
    /// Default: texture alpha is transparency (alpha test).
    Masked,
    /// Texture alpha is data (env/sheen mask), render fully opaque
    /// (see `SroResource::alpha_is_sheen`).
    Opaque,
}

impl SroMaterial {
    /// Builds one rendering variant of this material; the .bmt loader adds
    /// all variants as labeled sub-assets and resources pick per their
    /// alpha semantics.
    pub fn to_standard_material(
        &self,
        texture: Option<Handle<Image>>,
        variant: MaterialVariant,
    ) -> StandardMaterial {
        match texture {
            Some(texture) => {
                let mut material = StandardMaterial::from(texture);
                material.alpha_mode =
                    if self.has_alpha_channel() && variant == MaterialVariant::Masked {
                        // the original engine's alpha test uses ref 0x80 with
                        // D3DCMP_GREATEREQUAL (seen in leaf Material ModData)
                        AlphaMode::Mask(0.5)
                    } else {
                        AlphaMode::Opaque
                    };
                // material.unlit = true;
                // The original engine renders non-EnvMap, non-flag-0x4
                // materials purely diffuse (fixed-function, no specular);
                // only the sheen variant's masked texels turn specular
                // (sro_sheen.wgsl mixes toward its own reflectance by the
                // mask). Any base reflectance makes skin/cloth catch
                // env-map highlights.
                material.reflectance = 0.0;
                material.metallic = 0.0;
                material.perceptual_roughness = 0.8;
                if self.is_specular_emphasis() && self.power > 0.0 {
                    // Flag 0x4 is the original's per-draw
                    // D3DRS_SPECULARENABLE (see is_specular_emphasis): the
                    // BMT specular color and Phong power fill a
                    // D3DMATERIAL9 and the sun produces a classic
                    // fixed-function highlight. Only 19 hero-prop materials
                    // opt in; the power=0 outlier (one flower) is excluded
                    // since a zero exponent is degenerate in both models.
                    // Sheen variants wrap this base, so on EnvMap resources
                    // these values still govern the mask~0 texels.
                    material.specular_tint = self.specular;
                    material.reflectance = SPECULAR_EMPHASIS_REFLECTANCE;
                    material.perceptual_roughness = phong_power_to_perceptual_roughness(self.power);
                }
                if self.is_emissive() {
                    // self-illuminated panes/lamps (window lights, jewels)
                    // glow in their own texture colors; always on until a
                    // day/night cycle exists to drive it. The factor keeps
                    // the glow visible under daylight tonemapping.
                    material.emissive_texture = Some(material.base_color_texture.clone().unwrap());
                    material.emissive = LinearRgba::WHITE * EMISSIVE_BOOST;
                }
                if self.is_two_sided() {
                    // Bit1 (0x1) meshes are single-sided quads (talismans, hair,
                    // foliage, flags). double_sided flips the normal on back
                    // faces so both sides light correctly instead of showing a
                    // dark unlit back.
                    material.double_sided = true;
                    material.cull_mode = None;
                } else {
                    material.cull_mode = Some(Face::Back);
                }
                material
            }
            None => StandardMaterial::from(self.ambient),
        }
    }
}
