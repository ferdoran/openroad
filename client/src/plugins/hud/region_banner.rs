//! Zone-entry region banner (`GDR_REGION_INFO_VIEW:CIFRegionView`, id 83 —
//! `resinfo/ginterface.txt:104`, live behind `EUROPE_SYSTEM`, `define.txt:7`).
//!
//! Idea: the original shows a decorative plate (`interface/game/area_deco.ddj`,
//! 708x104 ARGB8888) carrying three `CIFStatic` lines — region name,
//! description, level range — for a moment after the player crosses into a new
//! region. Two thirds of the widget's geometry is unusable, so this module
//! transcribes only what the data actually says and marks the rest:
//!
//! * The parent `Rect="981,282,32,192"` in `ginterface.txt` is **not**
//!   geometry: it is byte-identical to the dead `#ifdef EVENT_FESTIVAL` block
//!   above it (`:92`), and a 32px-wide box cannot hold 655px children. It is
//!   editor scratch, the same hazard class as `Color=`, and is deliberately not
//!   consumed here.
//! * All three children are authored at `Rect="0,0,w,h"` — same origin, fully
//!   overlapping — so their *sizes* are data and their *placement* is code-side
//!   in the original too. The stacked layout and the screen anchor below are
//!   therefore openroad decisions, labelled as such.
//!
//! What IS data and is used verbatim: the three control sizes, their `FontColor`
//! (ARGB) and `HAlign`, the plate's native art size, and the region name from
//! `textdata/textzonename.txt` (the same table the minimap's area label binds).
//! The description and level-range lines have **no known source** — no per-region
//! text column exists in `refregion.txt`, `regioninfo.txt`, `regioncode.txt` or
//! `textzonename.txt`, and the only Lv-range strings belong to the world-map
//! Area tab — so they stay empty rather than invented
//! (`docs/re/ui/region-banner.md` §9).
//!
//! The dwell time is code-side in the original as well; it is a config knob
//! (`hud.region_banner_seconds`, 0 disables the banner) so the non-original
//! choice is not baked in. The name is also logged, so a transient overlay is
//! never the only announcement of a zone change.

use bevy::prelude::*;
use bevy::ui::UiTargetCamera;

use crate::assets::FontAssets;
use crate::plugins::config::ClientConfig;
use crate::plugins::hud::minimap::MinimapDungeonContext;
use crate::plugins::map::terrain::REGION_SIZE;
use crate::plugins::player::Player;
use crate::plugins::textdata::ClientZoneNames;
use crate::plugins::world_origin::WorldOrigin;

const BANNER_DDJ: &str = "media://interface/game/area_deco.ddj";

/// `area_deco.ddj`, drawn at its native size (708x104, ARGB8888).
const PLATE: (f32, f32) = (708.0, 104.0);

/// `GDR_STA_REGIONNAME` `Rect="0,0,655,29"`, `FontColor="255,61,253,255"`
/// (ARGB), `HAlign=0` (left) — `resinfo/ifregionview.txt:5`.
const NAME_SIZE: (f32, f32) = (655.0, 29.0);
const NAME_COLOR: Color = Color::srgb_u8(61, 253, 255);
/// `GDR_STA_REGIONEXPLAIN` `Rect="0,0,691,13"`, white, `HAlign=1` (centered).
const EXPLAIN_SIZE: (f32, f32) = (691.0, 13.0);
/// `GDR_STA_LVEXPLAIN` `Rect="0,0,427,11"`, `FontColor="255,255,255,92"`,
/// `HAlign=1`.
const LV_SIZE: (f32, f32) = (427.0, 11.0);
const LV_COLOR: Color = Color::srgb_u8(255, 255, 92);

/// Font sizes from the only font-index legend in the data —
/// `server_dep/silkroad/event/event_interface.txt:2`
/// (`//titlefont : 0 = "9", 1 = "8", 2 = "12", 3 = "11", 4 = "15"`) read as
/// point sizes, which is [S], not [V]: the resinfo `FontIndex` -> face mapping
/// is a lane-wide UNKNOWN. The three controls carry FontIndex 4 / 3 / 1.
const NAME_FONT: f32 = 15.0;
const EXPLAIN_FONT: f32 = 11.0;
const LV_FONT: f32 = 8.0;

/// Screen anchor — an **openroad decision**, not data (see the module note):
/// the plate is centered horizontally and hangs this far below the top edge,
/// clear of the mini-info panel and the minimap.
const BANNER_TOP: f32 = 96.0;

#[derive(Component)]
pub struct RegionBannerRoot;

/// Which region the banner last announced, and how long the current banner has
/// left. `None` until the first region is seen — entering the world does not
/// announce the starting region twice.
#[derive(Resource, Default)]
pub struct RegionBannerState {
    pub current: Option<u16>,
    pub remaining: f32,
}

/// Overworld region id for an SRO-space position: X sector in bits 0-7, Z in
/// bits 8-14 (`util::region`). Returns `None` outside the 256x128 sector grid.
pub fn overworld_region_id(sro_x: f32, sro_z: f32) -> Option<u16> {
    let xsec = (sro_x / REGION_SIZE).floor();
    let zsec = (sro_z / REGION_SIZE).floor();
    if !(0.0..=255.0).contains(&xsec) || !(0.0..=127.0).contains(&zsec) {
        return None;
    }
    Some(((zsec as u16) << 8) | (xsec as u16))
}

/// Detect the region change and spawn / expire the banner.
#[allow(clippy::too_many_arguments)]
pub fn update_region_banner(
    time: Res<Time>,
    config: Res<ClientConfig>,
    origin: Res<WorldOrigin>,
    zone_names: Res<ClientZoneNames>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    dungeon: Option<Res<MinimapDungeonContext>>,
    player: Query<&Transform, With<Player>>,
    cameras: Query<Entity, With<Camera2d>>,
    roots: Query<Entity, With<RegionBannerRoot>>,
    mut state: ResMut<RegionBannerState>,
    mut commands: Commands,
) {
    let dwell = config.hud.region_banner_seconds;
    if dwell <= 0.0 {
        return;
    }

    let region = match &dungeon {
        Some(ctx) => Some(ctx.region_id),
        None => player.single().ok().and_then(|tf| {
            let sro = origin.to_sro(tf.translation);
            overworld_region_id(-sro.x, sro.z)
        }),
    };

    if let Some(region) = region.filter(|region| state.current != Some(*region)) {
        state.current = Some(region);
        // An unnamed region announces nothing: textzonename covers 602 of the
        // grid, and a blank plate would be worse than no plate.
        if let Some(name) = zone_names.name(region).filter(|name| !name.is_empty()) {
            // WCAG: the overlay is transient, so the log carries it too.
            info!("entered region {region}: {name}");
            for root in roots.iter() {
                commands.entity(root).despawn();
            }
            if let Some(camera) = cameras.iter().next() {
                spawn_banner(&mut commands, camera, &asset_server, &fonts, name);
                state.remaining = dwell;
            }
        }
    }

    if state.remaining > 0.0 {
        state.remaining -= time.delta_secs();
        if state.remaining <= 0.0 {
            state.remaining = 0.0;
            for root in roots.iter() {
                commands.entity(root).despawn();
            }
        }
    }
}

fn spawn_banner(
    commands: &mut Commands,
    camera: Entity,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    name: &str,
) {
    // The three statics are stacked inside the plate instead of sharing the
    // authored 0,0 origin (see the module note); their sizes are the data's.
    let name_top = (PLATE.1 - NAME_SIZE.1 - EXPLAIN_SIZE.1 - LV_SIZE.1) / 2.0;
    commands
        .spawn((
            RegionBannerRoot,
            Name::from("Region Banner"),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(BANNER_TOP),
                left: Val::Percent(50.0),
                margin: UiRect::left(Val::Px(-PLATE.0 / 2.0)),
                width: Val::Px(PLATE.0),
                height: Val::Px(PLATE.1),
                ..default()
            },
            ImageNode {
                image: asset_server.load(BANNER_DDJ),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            GlobalZIndex(60),
            Pickable::IGNORE,
            UiTargetCamera(camera),
        ))
        .with_children(|plate| {
            // GDR_STA_REGIONNAME — HAlign 0, left-aligned in its slot
            plate.spawn(banner_line(
                name.to_string(),
                fonts.nine.clone(),
                NAME_FONT,
                NAME_COLOR,
                NAME_SIZE,
                name_top,
                Justify::Left,
            ));
            // GDR_STA_REGIONEXPLAIN — no known text source, left empty
            plate.spawn(banner_line(
                String::new(),
                fonts.nine.clone(),
                EXPLAIN_FONT,
                Color::WHITE,
                EXPLAIN_SIZE,
                name_top + NAME_SIZE.1,
                Justify::Center,
            ));
            // GDR_STA_LVEXPLAIN — no known per-region level table, left empty
            plate.spawn(banner_line(
                String::new(),
                fonts.nine.clone(),
                LV_FONT,
                LV_COLOR,
                LV_SIZE,
                name_top + NAME_SIZE.1 + EXPLAIN_SIZE.1,
                Justify::Center,
            ));
        });
}

fn banner_line(
    text: String,
    font: Handle<Font>,
    font_size: f32,
    color: Color,
    size: (f32, f32),
    top: f32,
    justify: Justify,
) -> impl Bundle {
    (
        Text(text),
        TextFont {
            font: font.into(),
            font_size: FontSize::Px(font_size),
            ..default()
        },
        TextColor(color),
        TextLayout::justify(justify),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px((PLATE.0 - size.0) / 2.0),
            top: Val::Px(top),
            width: Val::Px(size.0),
            height: Val::Px(size.1),
            ..default()
        },
        Pickable::IGNORE,
    )
}

pub fn cleanup_region_banner(
    mut commands: Commands,
    roots: Query<Entity, With<RegionBannerRoot>>,
    mut state: ResMut<RegionBannerState>,
) {
    for root in roots.iter() {
        commands.entity(root).despawn();
    }
    *state = RegionBannerState::default();
}

#[cfg(test)]
mod test {
    use super::*;

    /// The parent rect in `ginterface.txt` is a copy of the dead
    /// `EVENT_FESTIVAL` block's (`:92`), so nothing in this module may be
    /// derived from it. Pin the plate to the ART size instead — the widget's
    /// only trustworthy extent.
    #[test]
    fn the_dead_parent_rect_is_not_consumed() {
        assert_eq!(PLATE, (708.0, 104.0), "area_deco.ddj's native size");
        // 981,282,32,192 must not appear as geometry anywhere here.
        assert_ne!(PLATE, (32.0, 192.0));
        assert_ne!(BANNER_TOP, 282.0);
        // A 32px-wide parent cannot contain the authored children; that is the
        // arithmetic that proves the rect is scratch.
        assert!(NAME_SIZE.0 > 32.0 && EXPLAIN_SIZE.0 > 32.0 && LV_SIZE.0 > 32.0);
    }

    /// Sizes and colours are the only usable numbers in `ifregionview.txt`
    /// (the rects' x/y are all 0,0 — see the module note).
    #[test]
    fn control_sizes_match_the_authored_tree() {
        assert_eq!(NAME_SIZE, (655.0, 29.0));
        assert_eq!(EXPLAIN_SIZE, (691.0, 13.0));
        assert_eq!(LV_SIZE, (427.0, 11.0));
        // FontColor is ARGB: "255,61,253,255" and "255,255,255,92".
        assert_eq!(NAME_COLOR, Color::srgb_u8(61, 253, 255));
        assert_eq!(LV_COLOR, Color::srgb_u8(255, 255, 92));
        // All three fit inside the plate they are drawn on.
        assert!(EXPLAIN_SIZE.0 <= PLATE.0);
        assert!(NAME_SIZE.1 + EXPLAIN_SIZE.1 + LV_SIZE.1 <= PLATE.1);
    }

    /// Region-change detection: the id must pack the way `util::region`
    /// decodes it, or the banner would look up the wrong zone name.
    #[test]
    fn region_id_packs_the_sector_grid() {
        use crate::util::region::RegionIdExt;

        // Jangan is region 25000 = sector (168, 97) — the doc's worked example.
        let id = overworld_region_id(168.0 * REGION_SIZE + 1.0, 97.0 * REGION_SIZE + 1.0);
        assert_eq!(id, Some(25000));
        assert_eq!(25000u16.to_x_z(), (168, 97));
        // Crossing a border changes the id; staying inside one does not.
        assert_eq!(
            overworld_region_id(168.0 * REGION_SIZE, 97.0 * REGION_SIZE),
            overworld_region_id(169.0 * REGION_SIZE - 1.0, 97.0 * REGION_SIZE)
        );
        assert_ne!(
            overworld_region_id(168.0 * REGION_SIZE, 97.0 * REGION_SIZE),
            overworld_region_id(169.0 * REGION_SIZE, 97.0 * REGION_SIZE)
        );
        // Off-grid positions announce nothing; bit 15 is the dungeon flag and
        // must never be produced by overworld math.
        assert_eq!(overworld_region_id(-1.0, 0.0), None);
        assert_eq!(overworld_region_id(0.0, 128.0 * REGION_SIZE), None);
    }
}

/// Self-registration for the zone-entry region banner (#347) (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct RegionBannerPlugin;

impl Plugin for RegionBannerPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<RegionBannerState>()
            .add_systems(OnExit(SceneState::GameWorld), cleanup_region_banner)
            // live game world only — it announces a region change, which the
            // offline scenes never make (#347)
            .add_systems(
                Update,
                update_region_banner.run_if(in_state(SceneState::GameWorld)),
            );
    }
}
