//! The pet mini-info panel — `GDR_PMI_PET_MINI_INFO`, id 100.
//!
//! Idea: this is **not a window of its own**. `ifplayerminiinfo.txt` declares
//! it as one control at `53,55,154,40` *inside* the player mini-info panel,
//! with `DDJ="interface\ifcommon\window_all.ddj"` and UVs that resolve to the
//! atlas rect `867,71,154,40` — `0.846679 * 1024 = 867.0`,
//! `0.997070 * 1024 = 1021.0`, `0.138671 * 512 = 71.0`,
//! `0.216796 * 512 = 111.0`, i.e. exactly 154x40, the same UV→pixel identity
//! the parent panel's own frame rect uses. So it is spawned as a **child** of
//! `PlayerMiniInfoRoot` at that local rect (#303), not as a sibling window.
//!
//! Its seven controls come from `resinfo/ifpetminiinfo.txt`, panel-local.
//!
//! Where each readout comes from. `CosState` (#656) carries the summoned COS —
//! its name and its HGP (per-10,000, `0x30C9` type 4). **HP does not ride the
//! COS packets at all**, and that used to be read here as "the pet has no HP",
//! leaving the gauge empty. It is not: a pet is an ordinary remote entity, so
//! its current HP arrives on the same `EntityVitals` path every other entity
//! uses, and its maximum is a characterdata `MaxHP` lookup on the resolved ref
//! id — a growth stage is its own row, so the denominator follows the stage.
//! Neither number is invented, which is what the old note was guarding against;
//! only `0x30C8`'s two leading `[U]` u32s remain off-limits as an HP source.

use bevy::prelude::*;

use crate::assets::FontAssets;
use crate::plugins::cos::ActiveCosList;
use crate::plugins::hud::cos::state::CosState;
use crate::plugins::hud::game_window::abs_node;
use crate::plugins::hud::gauge::{gauge_art_node, gauge_crop_node, gauge_fill_width};
use crate::plugins::hud::player_mini_info::PlayerMiniInfoRoot;
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::entities::{EntityVitals, NetworkEntities};
use crate::plugins::textdata::{ClientCharacterData, ClientUiStrings};

// --- Layout constants -------------------------------------------------------

/// `GDR_PMI_PET_MINI_INFO` `Rect="53,55,154,40"`, parent-panel-local.
const PANEL_RECT: (f32, f32, f32, f32) = (53.0, 55.0, 154.0, 40.0);
/// The same block's UVs on the 1024x512 `window_all.ddj` atlas, in pixels.
const PANEL_ATLAS_RECT: (f32, f32, f32, f32) = (867.0, 71.0, 154.0, 40.0);
const WINDOW_ATLAS: &str = "media://interface/ifcommon/window_all.ddj";

/// `GDR_PET_MINI_PICTURE:CIFStaticWithPictureClip` `4,4,32,32`.
const PICTURE_RECT: (f32, f32, f32, f32) = (4.0, 4.0, 32.0, 32.0);
/// `GDR_PET_MINI_TXT_NAME:CIFStatic` `44,7,73,13`.
const NAME_RECT: (f32, f32, f32, f32) = (44.0, 7.0, 73.0, 13.0);
/// `GDR_PET_MINI_TXT_LEVEL:CIFStatic` `118,7,34,13`.
const LEVEL_RECT: (f32, f32, f32, f32) = (118.0, 7.0, 34.0, 13.0);
/// `GDR_PET_MINI_GAUGE_HP:CIFGauge` `41,23,0,0` and `_HGP` `41,29,0,0` — the
/// blocks carry `Rect` w,h `0,0`, so the extent is the art's: both
/// `pmi_pet_hp.ddj` and `pmi_pet_hgp.ddj` are **112x8** (DDS headers).
const GAUGE_SIZE: (f32, f32) = (112.0, 8.0);
const HP_GAUGE_POS: (f32, f32) = (41.0, 23.0);
const HGP_GAUGE_POS: (f32, f32) = (41.0, 29.0);
const HP_DDJ: &str = "media://interface/playerminiinfo/pmi_pet_hp.ddj";
const HGP_DDJ: &str = "media://interface/playerminiinfo/pmi_pet_hgp.ddj";
/// `GDR_PET_MINI_EFFECT_HP:CIFStatic` `42,24,107,32` — the animated low-HP
/// caution overlay. Not spawned: the *trigger* now exists (HP is live), but
/// the animation itself is not transcribed yet.
/// `GDR_PET_MINI_BUFF:CIFBuffViewer` `32,39,0,0` is left out for the original
/// reason — buff data for a COS has no wire.
const _UNBUILT: [&str; 2] = ["GDR_PET_MINI_EFFECT_HP", "GDR_PET_MINI_BUFF"];

const NAME_COLOR: Color = Color::srgb(0.92, 0.92, 0.92);

// --- Markers ----------------------------------------------------------------

/// Root of the pet panel; its `Visibility` is "the player has a COS out".
#[derive(Component)]
pub struct PetMiniInfoRoot;

#[derive(Component)]
pub struct PetMiniInfoName;

/// The level static — the pet's own level, characterdata `Lvl` as fallback.
#[derive(Component)]
pub struct PetMiniInfoLevel;

#[derive(Component)]
pub struct PetMiniInfoHgpFill;

/// The HP gauge's crop node. Filled from the pet entity's live
/// [`EntityVitals`], falling back to the summon-time seed in
/// [`ActiveCosList`] — the same two sources the status stack uses.
#[derive(Component, Clone, Copy)]
pub struct PetMiniInfoHpFill;

// --- Spawning ---------------------------------------------------------------

/// Attach the panel to the mini-info root the frame it appears.
pub fn spawn_pet_mini_info(
    roots: Query<Entity, Added<PlayerMiniInfoRoot>>,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    mut commands: Commands,
) {
    let s = hud_scale();
    for root in roots.iter() {
        let (ax, ay, aw, ah) = PANEL_ATLAS_RECT;
        commands.entity(root).with_children(|panel| {
            panel
                .spawn((
                    PetMiniInfoRoot,
                    abs_node(PANEL_RECT, s),
                    ImageNode {
                        image: asset_server.load(WINDOW_ATLAS),
                        rect: Some(Rect::new(ax, ay, ax + aw, ay + ah)),
                        ..default()
                    },
                    // hidden until a COS is summoned
                    Visibility::Hidden,
                    Pickable::IGNORE,
                ))
                .with_children(|pet| {
                    pet.spawn((
                        abs_node(PICTURE_RECT, s),
                        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
                        Pickable::IGNORE,
                    ));
                    pet.spawn((
                        PetMiniInfoName,
                        Text::new(""),
                        TextFont {
                            font: fonts.nine.clone().into(),
                            font_size: FontSize::Px(8.0 * s),
                            ..default()
                        },
                        TextColor(NAME_COLOR),
                        TextLayout::justify(Justify::Left),
                        abs_node(NAME_RECT, s),
                        Pickable::IGNORE,
                    ));
                    // `GDR_PET_MINI_TXT_LEVEL` — the pet's own level off
                    // 0x30C8's growth block, with characterdata `Lvl` as the
                    // fallback for the kinds that carry no block. Same source
                    // and same order as the info page's Level field.
                    pet.spawn((
                        PetMiniInfoLevel,
                        Text::new(""),
                        TextFont {
                            font: fonts.nine.clone().into(),
                            font_size: FontSize::Px(8.0 * s),
                            ..default()
                        },
                        TextColor(NAME_COLOR),
                        TextLayout::justify(Justify::Right),
                        abs_node(LEVEL_RECT, s),
                        Pickable::IGNORE,
                    ));
                    // Both gauges are the shared three-node CIFGauge recipe
                    // (`hud/gauge.rs`, #630): the art is never resized, the
                    // crop node is the only thing a fill drives.
                    for (pos, art, hgp) in [
                        (HP_GAUGE_POS, HP_DDJ, false),
                        (HGP_GAUGE_POS, HGP_DDJ, true),
                    ] {
                        let track = (pos.0, pos.1, GAUGE_SIZE.0, GAUGE_SIZE.1);
                        pet.spawn((abs_node(track, s), Pickable::IGNORE))
                            .with_children(|track| {
                                let mut crop = track.spawn((
                                    gauge_crop_node(Val::Px(0.0), GAUGE_SIZE.1 * s),
                                    Pickable::IGNORE,
                                ));
                                if hgp {
                                    crop.insert(PetMiniInfoHgpFill);
                                } else {
                                    crop.insert(PetMiniInfoHpFill);
                                }
                                crop.with_children(|crop| {
                                    crop.spawn((
                                        gauge_art_node(GAUGE_SIZE.0 * s, GAUGE_SIZE.1 * s),
                                        ImageNode {
                                            image: asset_server.load(art),
                                            ..default()
                                        },
                                        Pickable::IGNORE,
                                    ));
                                });
                            });
                    }
                });
        });
    }
}

/// Mirror the summoned pet onto the panel: visibility, name, and both gauges.
///
/// HP runs every frame rather than on `CosState` change, because it tracks the
/// entity's own vitals — which move without any COS packet arriving.
#[allow(clippy::type_complexity)]
pub fn refresh_pet_mini_info(
    cos: Res<CosState>,
    list: Res<ActiveCosList>,
    index: Res<NetworkEntities>,
    vitals: Query<&EntityVitals>,
    char_data: Res<ClientCharacterData>,
    ui_strings: Res<ClientUiStrings>,
    mut roots: Query<&mut Visibility, With<PetMiniInfoRoot>>,
    mut names: Query<&mut Text, (With<PetMiniInfoName>, Without<PetMiniInfoLevel>)>,
    mut levels: Query<&mut Text, (With<PetMiniInfoLevel>, Without<PetMiniInfoName>)>,
    mut hgp_fills: Query<&mut Node, (With<PetMiniInfoHgpFill>, Without<PetMiniInfoHpFill>)>,
    mut hp_fills: Query<&mut Node, (With<PetMiniInfoHpFill>, Without<PetMiniInfoHgpFill>)>,
) {
    let summoned = cos.active_pet();
    let track = GAUGE_SIZE.0 * hud_scale();

    // HP first: it is the half that must not wait for a COS packet.
    let hp_fraction = summoned
        .map(|pet| pet.unique_id)
        .map(|uid| {
            index
                .get(uid)
                .and_then(|entity| vitals.get(entity).ok())
                .map(EntityVitals::fill)
                .or_else(|| {
                    list.get(uid)
                        .map(|status| status.hp as f32 / status.hp_max.max(1) as f32)
                })
                .unwrap_or(1.0)
        })
        .unwrap_or(0.0);
    for mut node in hp_fills.iter_mut() {
        let width = gauge_fill_width(hp_fraction.clamp(0.0, 1.0), track);
        if node.width != width {
            node.width = width;
        }
    }

    if !cos.is_changed() {
        return;
    }
    for mut visibility in roots.iter_mut() {
        *visibility = if summoned.is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    for mut text in names.iter_mut() {
        // "No name" rather than a blank plate — same rule as the info page.
        text.0 = match summoned {
            Some(cos) => crate::plugins::cos::spawn::pet_display_name(cos.name(), &ui_strings),
            None => String::new(),
        };
    }
    for mut text in levels.iter_mut() {
        text.0 = summoned
            .and_then(|cos| {
                cos.level.map(u32::from).or_else(|| {
                    char_data
                        .get(&(cos.ref_obj_id as i32))
                        .and_then(|row| row.level())
                })
            })
            .map(|level| level.to_string())
            .unwrap_or_default();
    }
    let fraction = summoned.and_then(|cos| cos.hgp_fraction()).unwrap_or(0.0);
    for mut node in hgp_fills.iter_mut() {
        node.width = gauge_fill_width(fraction, track);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The atlas rect is the block's own UVs on the 1024x512 atlas, and it
    /// resolves to exactly the authored 154x40 — the same UV→pixel identity
    /// the parent panel's frame uses.
    #[test]
    fn the_atlas_rect_is_the_authored_uvs() {
        assert_eq!(PANEL_ATLAS_RECT.0, (0.846_679_f32 * 1024.0).round());
        assert_eq!(PANEL_ATLAS_RECT.1, (0.138_671_f32 * 512.0).round());
        assert_eq!((PANEL_ATLAS_RECT.2, PANEL_ATLAS_RECT.3), (154.0, 40.0));
        assert_eq!(
            (PANEL_ATLAS_RECT.2, PANEL_ATLAS_RECT.3),
            (PANEL_RECT.2, PANEL_RECT.3),
            "the atlas subrect and the control rect are the same extent"
        );
    }

    /// Every child rect stays inside the 154x40 host.
    #[test]
    fn every_child_fits_the_host_rect() {
        let mut boxes = vec![PICTURE_RECT, NAME_RECT, LEVEL_RECT];
        for pos in [HP_GAUGE_POS, HGP_GAUGE_POS] {
            boxes.push((pos.0, pos.1, GAUGE_SIZE.0, GAUGE_SIZE.1));
        }
        for (x, y, w, h) in boxes {
            assert!(
                x + w <= PANEL_RECT.2,
                "rect {x},{y},{w},{h} overflows width"
            );
            assert!(
                y + h <= PANEL_RECT.3,
                "rect {x},{y},{w},{h} overflows height"
            );
        }
    }

    /// The two gauges are the same 112x8 art, and vanilla's own numbers put
    /// HGP only 6 units below HP — so the bars overlap by 2. Kept verbatim
    /// rather than "corrected" to a 8-unit pitch.
    #[test]
    fn the_two_gauges_keep_the_authored_overlap() {
        assert_eq!(HGP_GAUGE_POS.1 - HP_GAUGE_POS.1, 6.0);
        assert_eq!(GAUGE_SIZE, (112.0, 8.0));
        assert!(HGP_GAUGE_POS.1 - HP_GAUGE_POS.1 < GAUGE_SIZE.1);
    }
}
