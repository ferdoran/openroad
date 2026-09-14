use std::collections::HashMap;

use bevy::asset::{Asset, AssetId, Assets, Handle};
use bevy::ecs::hierarchy::Children;
use bevy::image::Image;
use bevy::math::{Vec2, Vec4};
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MeshMaterial3d};
use bevy::prelude::{Entity, Query, StandardMaterial};
use bevy::reflect::TypePath;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

/// Fallback highlight texture (a soft grayscale radial blob) bound while no
/// shine is active. The real enhancement glow samples a per-tier scrolling
/// streak texture instead — see [`ShineColor::texture_path`].
pub const SHINE_SPHEREMAP_PATH: &str = "data://prim/mtrl/etc/spheremap_highlight.ddj";

/// The grayscale chrome-probe sphere map of the original's base EnvMap pass
/// (`CRTModEnvMap`), sampled by every sheen material — see
/// [`SheenSettings::env_strength`].
pub const ENV_SPHEREMAP_PATH: &str = "data://prim/mtrl/etc/spheremap_gray.ddj";

/// The [`SheenSettings::alpha_cutout`] value matching the original's
/// alpha test (`GREATEREQUAL` ref 1): discard only exact-zero alpha.
pub const SHEEN_ALPHA_CUTOUT: f32 = 1.0 / 255.0;

// Idea: sheen resources (weapons, metal armor — every .bsr with an EnvMap
// mod, see `SroResource::alpha_is_sheen`) repurpose their diffuse texture's
// alpha channel as the original engine's environment-map mask: high alpha =
// polished metal (blade, plates), low alpha = matte (leather grip, cloth
// straps). This extension keeps the full standard PBR pipeline (skinning,
// shadows, prepass, light probes) and swaps the fragment shader
// (`sro_sheen.wgsl`) for one that reproduces the original's actual EnvMap
// stages: the grayscale chrome-probe sphere map (spheremap_gray, sampled
// by the view-space normal) is added into the albedo per texel by the
// mask, then multiplied by the lighting — the moving glint on metal
// parts, reading as lit metal rather than a glow. The shader can also
// drive per-texel PBR roughness/reflectance (sun glints) and metallic
// (off by default — it would eat the albedo chrome term) from the mask.
pub type SroSheenMaterial = ExtendedMaterial<StandardMaterial, SheenExtension>;

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone, Default)]
pub struct SheenExtension {
    #[uniform(100)]
    pub settings: SheenSettings,
    /// The enhancement highlight sphere map ([`SHINE_SPHEREMAP_PATH`]);
    /// sampled only when `settings.shine_color.a > 0`. A default (unset)
    /// handle binds bevy's fallback white image, harmless while shine is off.
    #[texture(101)]
    #[sampler(102)]
    pub shine_texture: Handle<Image>,
    /// The base EnvMap chrome probe ([`ENV_SPHEREMAP_PATH`]), always
    /// sampled. Must be set by the loader: the fallback white image would
    /// add a flat `env_strength` glow instead of a moving reflection.
    #[texture(103)]
    #[sampler(104)]
    pub env_texture: Handle<Image>,
}

/// Std140 layout, field order must match the `SheenSettings` struct in
/// `sro_sheen.wgsl`. All values are visual calibration constants.
#[derive(Clone, Copy, Debug, ShaderType)]
pub struct SheenSettings {
    /// PBR metallic at full mask (texture alpha 1.0). Defaults to 0: the
    /// original has no physically-based metal — its whole metal look IS
    /// the chrome-probe albedo term ([`env_strength`](Self::env_strength))
    /// times lighting, and Bevy's metallic would eat that term (metallic
    /// kills the diffuse lobe). Raise only for experiments.
    pub strength: f32,
    /// Perceptual roughness the surface shifts toward as the mask rises
    /// (the base material's roughness applies where the mask is 0).
    /// Defaults to the base's 0.8 (no-op): a lower value adds a dielectric
    /// gloss highlight on top of the chrome term, which reads as plastic
    /// rather than metal — the original has no specular lobe at all.
    pub shiny_roughness: f32,
    /// Reflectance at full mask. Defaults to the base's 0.0 (no-op) for
    /// the same reason as [`shiny_roughness`](Self::shiny_roughness).
    pub reflectance: f32,
    /// Discard texels whose alpha (the sheen mask) is below this; 0
    /// disables. The original's per-resource EnvMap alpha-test flag maps
    /// to `1/255` (`ALPHATESTENABLE`, `GREATEREQUAL`, ref 1): exact-zero
    /// alpha is a cutout, everything above stays opaque sheen (see
    /// `SroResource::sheen_alpha_test`).
    pub alpha_cutout: f32,
    /// Alchemy-enhancement shine: rgb = scene-linear tint (the original's
    /// TFACTOR color), a = intensity; zero (the default) disables it.
    /// The original `CRTModProgEquipPow` glow ping-pong-interpolates between
    /// this and [`shine_color2`](Self::shine_color2) — see [`ShineColor`].
    pub shine_color: Vec4,
    /// Second glow color the shine pulses toward (rgb; a = its intensity).
    /// Every original tier pulses between two distinct colors (e.g. +6
    /// gold↔orange); equal colors give a steady glow.
    pub shine_color2: Vec4,
    /// Extra scroll (uv/sec) added to the sphere coordinate. The original
    /// highlight is view-driven — it sweeps as the weapon/camera turns — so
    /// 0 is faithful; a small value adds an idle drift.
    pub shine_scroll: Vec2,
    /// Full color1→color2→color1 pulses per second (0 = steady color1).
    pub shine_pulse: f32,
    /// Intensity of the base EnvMap chrome-probe term (the original's
    /// `CRTModEnvMap` stages: spheremap_gray × TFACTOR, added into the
    /// albedo by the mask, then multiplied by lighting). Albedo-space:
    /// 0.5 IS the original's TFACTOR — corpus-verified 2026-08-09
    /// (`probe_envmap_moddata_census`): the EnvMap ModData Float0 is 0.5 on
    /// ~89% of all 2094 entries and exactly 0 on the `_sa` avatar sets
    /// (semantics UNKNOWN, see `docs/formats/bsr-jmxvres.md`); no payload
    /// field carries a per-resource tint. 0 disables.
    pub env_strength: f32,
    /// Always-on rim light (mobile-port parity, gap #1 in
    /// `docs/rendering-mobile-shader-comparison.md`): rgb = rim color,
    /// a = strength; a == 0 (the default) disables the term. Applied
    /// post-lighting with the same fresnel as `sro_rim.wgsl`. Populated
    /// from `graphics.rim` config by the .bmt loader.
    pub rim_color: Vec4,
    /// Fresnel exponent of [`rim_color`](Self::rim_color) (mobile ships 3).
    pub rim_power: f32,
    /// Sharpening exponent on the sampled enhancement-streak highlight
    /// (the mobile port's `streamerPOW`, gap #4): `pow(highlight, this)`.
    /// 1 = the faithful soft streak (no-op); higher values narrow the
    /// sweep into a crisp band. Populated from `graphics.sheen.shine_pow`.
    pub shine_pow: f32,
    /// How the rim term applies: 0 = absolute add (scene-linear constant —
    /// swamped by HDR daylight, kept for A/B), 1 = relative
    /// (`lit × (1 + rim·fresnel)`), which reproduces the mobile port's
    /// rim-to-lit *ratio* at any exposure/time of day.
    ///
    /// (A mobile-style metal boost — albedo-tinted reflection, `env_gain`,
    /// a replace mode — briefly lived here and was reverted by decision
    /// 2026-08-10: without a metallicity channel in 1.188 data it bled
    /// onto every EnvMap resource, most visibly the garment paper
    /// talismans. The chrome term is the faithful exe stage only.)
    pub rim_mode: f32,
}

impl Default for SheenSettings {
    fn default() -> Self {
        Self {
            strength: 0.0,
            shiny_roughness: 0.8,
            reflectance: 0.0,
            alpha_cutout: 0.0,
            shine_color: Vec4::ZERO,
            shine_color2: Vec4::ZERO,
            shine_scroll: Vec2::ZERO,
            shine_pulse: 0.0,
            env_strength: 0.5,
            rim_color: Vec4::ZERO,
            rim_power: 3.0,
            shine_pow: 1.0,
            rim_mode: 0.0,
        }
    }
}

/// Shine configuration applied to an item instance; the sliders in the
/// particles test scene drive these live.
#[derive(Clone, Copy, Debug)]
pub struct ShineParams {
    /// First and second glow colors (rgb + intensity); the shine pulses
    /// between them (see [`ShineColor::colors`]).
    pub color: Vec4,
    pub color2: Vec4,
    pub scroll: Vec2,
    /// Pulses per second between the two colors.
    pub pulse: f32,
}

impl Default for ShineParams {
    fn default() -> Self {
        Self {
            color: Vec4::ZERO,
            color2: Vec4::ZERO,
            scroll: Vec2::ZERO,
            pulse: 0.0,
        }
    }
}

impl ShineParams {
    /// Scale the glow's brightness, leaving its hue, scroll and pulse alone.
    ///
    /// The intensity lives in the colors' **alpha** (see [`ShineColor::colors`]
    /// — the shader multiplies `tint.rgb * tint.a`), so brightness is one
    /// factor on two `w` components and nothing else has to move.
    ///
    /// Exists because the tier intensities are scene-linear HDR values that
    /// only read correctly on a bloom-lit pipeline. With bloom off they are
    /// tonemapped flat inside the forward pass and the glow all but vanishes,
    /// so `graphics.sheen.intensity` scales them back up rather than forcing
    /// bloom on for an unrelated reason.
    #[must_use]
    pub fn scaled(mut self, intensity: f32) -> Self {
        let k = intensity.max(0.0);
        self.color.w *= k;
        self.color2.w *= k;
        self
    }

    pub fn apply_to(self, settings: &mut SheenSettings) {
        settings.shine_color = self.color;
        settings.shine_color2 = self.color2;
        settings.shine_scroll = self.scroll;
        settings.shine_pulse = self.pulse;
    }
}

/// Tint presets of the alchemy-enhancement shine sweeping across enhanced
/// weapons, keyed by color tier.
///
/// The two TFACTOR colors each tier ping-pongs between, the scrolling
/// highlight texture (`prim/mtrl/itemoption/option_texture*.ddj`) and the
/// UV scroll come from Media.pk2 `resinfo/itemtypenumber.txt`; the intensity
/// keeps the original's MODULATE2X step (×2 above white) on a scene-linear
/// HDR calibration factor. The white/pink/gold colors and their textures are
/// verbatim from that file; green and blue are the two colors of the stock
/// blue↔green tier (option_texture20), split into their own tiers to serve
/// the 5-tier opt-level mapping in [`Self::for_opt_level`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShineColor {
    /// +3/+4: white pulsing to pale yellow.
    White,
    /// +5/+6: pink pulsing to purple.
    Pink,
    /// +7/+8: gold pulsing to orange.
    Gold,
    /// +9/+10: green.
    Green,
    /// +11 and up: blue.
    Blue,
}

impl ShineColor {
    pub const ALL: [ShineColor; 5] = [
        ShineColor::White,
        ShineColor::Pink,
        ShineColor::Gold,
        ShineColor::Green,
        ShineColor::Blue,
    ];

    /// The glow tier for an item's enhancement (opt) level, in the paired
    /// scheme white(+3/+4) → pink(+5/+6) → gold(+7/+8) → green(+9/+10) →
    /// blue(+11+); `None` below +3 (no glow). (This pairing is the target
    /// scheme, not the stock itemtypenumber.txt single-step cycle.)
    pub fn for_opt_level(opt_level: u8) -> Option<Self> {
        Some(match opt_level {
            0..=2 => return None,
            3 | 4 => ShineColor::White,
            5 | 6 => ShineColor::Pink,
            7 | 8 => ShineColor::Gold,
            9 | 10 => ShineColor::Green,
            _ => ShineColor::Blue,
        })
    }

    /// The ready-to-apply shine parameters for this tier (canonical colors,
    /// scroll and pulse — the same values the dev picker starts from).
    pub fn params(self) -> ShineParams {
        let (color, color2) = self.colors();
        ShineParams {
            color,
            color2,
            scroll: Vec2::new(Self::SCROLL_UV_PER_SEC, -Self::SCROLL_UV_PER_SEC),
            pulse: self.pulse(),
        }
    }

    /// The original's UV scroll of the highlight texture, from
    /// itemtypenumber.txt type 1/2 (`0.005,-0.005`; types 3/4 use
    /// `0,0.005`). The file value is per engine tick; this is scaled ×60
    /// (uv/sec at the original's frame cadence), calibration-pending.
    pub const SCROLL_UV_PER_SEC: f32 = 0.3;

    pub fn label(self) -> &'static str {
        match self {
            ShineColor::White => "+3/+4 white",
            ShineColor::Pink => "+5/+6 pink",
            ShineColor::Gold => "+7/+8 gold",
            ShineColor::Green => "+9/+10 green",
            ShineColor::Blue => "+11+ blue",
        }
    }

    /// The tier's `(color1, color2)` glow pair (rgb + intensity), pulsed
    /// between by the shader.
    pub fn colors(self) -> (Vec4, Vec4) {
        const I: f32 = 4.0; // white intensity (scene-linear HDR calibration)
        let (a, b, intensity) = match self {
            ShineColor::White => ((1.0, 1.0, 1.0), (0.988, 1.0, 0.690), I),
            ShineColor::Pink => ((0.996, 0.173, 0.510), (0.902, 0.122, 0.761), 2.0 * I),
            ShineColor::Gold => ((0.992, 0.996, 0.510), (0.831, 0.231, 0.0), 2.0 * I),
            // green/blue: the two stock texture20 colors, given their own
            // pulse partners so each tier reads clearly as one color
            ShineColor::Green => ((0.094, 0.580, 0.380), (0.400, 0.900, 0.500), 2.0 * I),
            ShineColor::Blue => ((0.388, 0.502, 0.890), (0.100, 0.300, 0.950), 2.0 * I),
        };
        (
            Vec4::new(a.0, a.1, a.2, intensity),
            Vec4::new(b.0, b.1, b.2, intensity),
        )
    }

    /// Pulses per second between the two colors. The original ping-pongs
    /// over 1000 ms each way (a 2 s full cycle) at every tier.
    pub fn pulse(self) -> f32 {
        1.0
    }

    /// The tier's scrolling highlight texture (`data://` path into
    /// Data.pk2), as named in itemtypenumber.txt.
    pub fn texture_path(self) -> &'static str {
        match self {
            ShineColor::White | ShineColor::Pink => {
                "data://prim/mtrl/itemoption/option_texture13.ddj"
            }
            ShineColor::Gold => "data://prim/mtrl/itemoption/option_texture14.ddj",
            ShineColor::Green | ShineColor::Blue => {
                "data://prim/mtrl/itemoption/option_texture20.ddj"
            }
        }
    }
}

/// Applies (or with `None` clears) the alchemy-enhancement shine on every
/// sheen mesh in `root`'s hierarchy.
///
/// Enhancement is per item *instance*, but the sheen materials are labeled
/// `.bmt` sub-assets shared by every instance of the same item, so the
/// affected meshes get a clone of their material with the shine set instead
/// of a mutation (one clone per distinct source material, so meshes that
/// shared a material keep sharing the clone).
pub fn set_shine(
    root: Entity,
    params: ShineParams,
    sphere_map: &Handle<Image>,
    children: &Query<&Children>,
    mesh_materials: &Query<&MeshMaterial3d<SroSheenMaterial>>,
    materials: &mut Assets<SroSheenMaterial>,
) -> Vec<(Entity, Handle<SroSheenMaterial>)> {
    let mut clones: HashMap<AssetId<SroSheenMaterial>, Handle<SroSheenMaterial>> = HashMap::new();
    let mut updates = Vec::new();

    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if let Ok(entity_children) = children.get(entity) {
            stack.extend(entity_children.iter());
        }
        let Ok(material) = mesh_materials.get(entity) else {
            continue;
        };
        let source = material.0.id();
        let clone = clones.entry(source).or_insert_with(|| {
            let Some(mut cloned) = materials.get(source).cloned() else {
                // not loaded yet; keep the shared handle untouched
                return material.0.clone();
            };
            params.apply_to(&mut cloned.extension.settings);
            cloned.extension.shine_texture = sphere_map.clone();
            materials.add(cloned)
        });
        if *clone != material.0 {
            updates.push((entity, clone.clone()));
        }
    }
    updates
}

impl MaterialExtension for SheenExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/sro_sheen.wgsl".into()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The shader reads brightness out of the colors' alpha
    /// (`tint.rgb * tint.a`), so scaling must touch **only** `w` — a factor
    /// applied to `rgb` would wash the tier's hue toward white as it brightened.
    #[test]
    fn scaling_brightens_without_touching_the_hue() {
        let params = ShineColor::Gold.params();
        let scaled = params.scaled(3.0);
        assert_eq!(scaled.color.w, params.color.w * 3.0);
        assert_eq!(scaled.color2.w, params.color2.w * 3.0);
        assert_eq!(scaled.color.truncate(), params.color.truncate());
        assert_eq!(scaled.color2.truncate(), params.color2.truncate());
        // and the motion is untouched
        assert_eq!(scaled.scroll, params.scroll);
        assert_eq!(scaled.pulse, params.pulse);
    }

    /// 1.0 is the identity, so the knob's neutral position really is the old
    /// behaviour rather than "close to it".
    #[test]
    fn an_intensity_of_one_changes_nothing() {
        for tier in ShineColor::ALL {
            let params = tier.params();
            let scaled = params.scaled(1.0);
            assert_eq!(scaled.color, params.color, "{tier:?}");
            assert_eq!(scaled.color2, params.color2, "{tier:?}");
        }
    }

    /// A negative intensity in the config must not invert the glow into a
    /// subtractive term — the shader adds this straight onto the lit color.
    #[test]
    fn a_negative_intensity_clamps_to_dark() {
        let scaled = ShineColor::White.params().scaled(-5.0);
        assert_eq!(scaled.color.w, 0.0);
        assert_eq!(scaled.color2.w, 0.0);
    }
}
