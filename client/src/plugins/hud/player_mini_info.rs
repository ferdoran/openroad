//! Player mini-info panel (top-left): portrait, name, level, HP/MP bars and
//! hwan/berserk pips.
//!
//! Idea: the layout is hand-transcribed from the vanilla UI definitions
//! (like char-select's `info_box` transcribes pscharacterselect.txt):
//! ginterface.txt's GDR_PLAYER_MINI_INFO places the 212x70 window and names
//! its frame art (a window_all.ddj atlas region), and every control in
//! `Media.pk2/resinfo/ifplayerminiinfo.txt` carries a `Rect=RECT,"x,y,w,h"`
//! in window space, used verbatim below and uniformly scaled. Values live in
//! the [`PlayerVitals`]
//! resource, seeded from CHARACTER_DATA (0x3013) and kept live by
//! `EntityBarsUpdate` (0x3057), `CharacterPointsUpdate` (0x304E, berserk pips)
//! and `CharacterStatsUpdate` (0x303D — the only packet carrying max HP/MP).
//!
//! The portrait is the actual player head: an offscreen `Camera3d` parented to
//! the player entity renders its meshes (tagged onto the Portrait render
//! layer) into a small target texture that the UI samples — so equipment and
//! facing come for free.

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::RenderTarget;
use bevy::light::AmbientLight;
use bevy::math::Vec3A;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::ui::{InteractionDisabled, Overflow, UiTargetCamera};
use bevy::ui_widgets::{Activate, Button};

use packets::agent::prelude::{
    CharacterPointsUpdate, CharacterStatsUpdate, EntityBarsUpdate, HwanActionRequest,
    HwanActionResponse, HwanLevelUpdate,
};
use packets::Packet;

use crate::assets::bsr::resource::SroResource;
use crate::assets::FontAssets;
use crate::commands::{MeshIndexMap, SpawnedFromResource};
use crate::net::connection::SilkroadConnection;
use crate::plugins::camera::CameraLayers;
use crate::plugins::config::ClientConfig;
use crate::plugins::hud::character_info::model::{balance_text, CharacterInfoState, PlayerStats};
use crate::plugins::hud::flipbook::Flipbook;
use crate::plugins::hud::game_window::scaled;
use crate::plugins::hud::gauge::{gauge, gauge_fill_width};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::character_info::CharacterInfo;
use crate::plugins::net::entities::NetworkId;
use crate::plugins::player::Player;
use crate::plugins::textdata::ClientUiStrings;
use crate::plugins::ui_v2::style::ImageButtonStyle;
use crate::plugins::ui_v2::widgets::label;

// --- Layout constants (resinfo/ifplayerminiinfo.txt, window space) ----------

/// Window size per ginterface.txt's GDR_PLAYER_MINI_INFO (Rect "4,7,212,70"):
/// the mini-info window is 212x70, placed at screen (4,7). The frame art is
/// the window_all.ddj atlas region below, drawn at local (0,0) — with that
/// anchoring the art's bar slots and ring hole line up 1:1 with the resinfo
/// control rects. (pmi_window.ddj is a legacy duplicate of this art WITHOUT
/// the baked-in translucent name strip — don't use it.)
const FRAME_W: f32 = 212.0;
const FRAME_H: f32 = 70.0;
/// Screen placement from ginterface.txt (x, y).
const WINDOW_POS: (f32, f32) = (4.0, 7.0);
/// Frame art: window_all.ddj atlas pixels (x, y, w, h), from ginterface.txt's
/// GDR_PLAYER_MINI_INFO UVs (the 1024x512 atlas maps UVs 1:1 to pixels).
const FRAME_ATLAS_RECT: (f32, f32, f32, f32) = (741.0, 0.0, 212.0, 70.0);
const WINDOW_ATLAS: &str = "media://interface/ifcommon/window_all.ddj";
// GDR_PMI_PICTURE. The original also draws a `pmi_face.ddj` backdrop disc
// behind the portrait; we render a live 3D portrait into this rect instead, so
// nothing loads that art — the comment used to claim we did (#303).
const PORTRAIT_RECT: (f32, f32, f32, f32) = (15.0, 7.0, 48.0, 48.0);
// GDR_PMI_TXT_ID
const NAME_RECT: (f32, f32, f32, f32) = (71.0, 4.0, 93.0, 15.0);
// GDR_PMI_TXT_LEVEL (FontColor 255,255,217,83 ARGB)
const LEVEL_RECT: (f32, f32, f32, f32) = (168.0, 7.0, 37.0, 15.0);
// GDR_PMI_GAUGE_HP / GDR_PMI_GAUGE_MP (fill art pmi_hp/pmi_mp.ddj, 124x12)
const HP_BAR_RECT: (f32, f32, f32, f32) = (79.0, 25.0, 124.0, 12.0);
const MP_BAR_RECT: (f32, f32, f32, f32) = (79.0, 41.0, 124.0, 12.0);
// GDR_PMI_TXT_HP / GDR_PMI_TXT_MP (centered "cur / max" overlays, +1px down)
const HP_TEXT_RECT: (f32, f32, f32, f32) = (79.0, 26.0, 124.0, 12.0);
const MP_TEXT_RECT: (f32, f32, f32, f32) = (79.0, 42.0, 124.0, 12.0);
// GDR_PMI_BTN_JAHWAN — the berserk activation button on top of the ring;
// vanilla shows it only while the hwan gauge is full (5 pips).
const JAHWAN_BUTTON_RECT: (f32, f32, f32, f32) = (16.0, -5.0, 20.0, 20.0);
const JAHWAN_BUTTON_NORMAL: &str = "media://interface/playerminiinfo/pmi_jahwan_button.ddj";
const JAHWAN_BUTTON_FOCUS: &str = "media://interface/playerminiinfo/pmi_jahwan_button_focus.ddj";
const JAHWAN_BUTTON_PRESS: &str = "media://interface/playerminiinfo/pmi_jahwan_button_press.ddj";
// GDR_PMI_CIRCLE0..4 — the hwan pip sockets on the portrait ring's arc
const PIP_POSITIONS: [(f32, f32); 5] = [
    (7.0, 13.0),
    (4.0, 28.0),
    (7.0, 43.0),
    (17.0, 54.0),
    (31.0, 60.0),
];
const PIP_SIZE: f32 = 8.0;

const LEVEL_COLOR: Color = Color::srgb_u8(255, 217, 83);
const NAME_FONT_SIZE: f32 = 10.0;
const BAR_FONT_SIZE: f32 = 9.0;

// --- Expanded stat drawer ----------------------------------------------------
//
// `GDR_PMI_BTN_CINFO` (id 20) toggles `GDR_PMI_STA_CINFOBG` (id 21), a panel
// hanging below the 70px frame that holds a STR/INT pair plus eight
// derived-stat rows. All rects are verbatim from `ifplayerminiinfo.txt` and are
// in window space, like every other rect in this module.

/// The toggle button, `ifplayerminiinfo.txt:72`. Only the base art is named in
/// the resinfo; the client derives the focus/press siblings by suffix, and both
/// exist on disk.
const CINFO_BUTTON_RECT: (f32, f32, f32, f32) = (41.0, 56.0, 20.0, 20.0);
const CINFO_BUTTON_NORMAL: &str = "media://interface/playerminiinfo/pmi_button.ddj";
const CINFO_BUTTON_FOCUS: &str = "media://interface/playerminiinfo/pmi_button_focus.ddj";
const CINFO_BUTTON_PRESS: &str = "media://interface/playerminiinfo/pmi_button_press.ddj";

/// `GDR_PMI_BTN_STATUP:CIFButton` (id 34) — `Rect="150,3,16,16"`,
/// `DDJ="interface\ifcommon\com_plus_button.ddj"`, `Style=0`.
///
/// The `Style=0` matters: the fifteen conditionally-drawn decorations of this
/// tree all carry `Style=64` (see `docs/re/ui/hud-player-mini-info.md` §3), so
/// this button is authored as **always present** and is greyed rather than
/// hidden when there is nothing to spend — the same treatment the C-window's
/// two `+` buttons already get, down to the `_disable` art.
const STATUP_BUTTON_RECT: (f32, f32, f32, f32) = (150.0, 3.0, 16.0, 16.0);
const PLUS_BUTTON_NORMAL: &str = "media://interface/ifcommon/com_plus_button.ddj";
const PLUS_BUTTON_FOCUS: &str = "media://interface/ifcommon/com_plus_button_focus.ddj";
const PLUS_BUTTON_PRESS: &str = "media://interface/ifcommon/com_plus_button_press.ddj";
const PLUS_BUTTON_DISABLE: &str = "media://interface/ifcommon/com_plus_button_disable.ddj";
/// The drawer backdrop, `:585`, and its `window_all.ddj` subrect recomputed from
/// the UVs at `:589-592` — (0.391602, 0.300781)-(0.523438, 0.669922) against
/// 1024x512 gives (401, 154) 135x189, exactly the declared extent.
const CINFO_BG_RECT: (f32, f32, f32, f32) = (72.0, 57.0, 135.0, 189.0);
const CINFO_BG_ATLAS_RECT: (f32, f32, f32, f32) = (401.0, 154.0, 135.0, 189.0);
/// Label colour, uniform across all ten labels (`FontColor="255,239,218,164"`).
/// resinfo colours are ARGB, so this is an opaque pale gold; the value cells are
/// plain white (`255,255,255,255`).
const CINFO_LABEL_COLOR: Color = Color::srgb_u8(239, 218, 164);
/// The eight derived-stat rows share their label/value rects; only y varies, on
/// a 20px pitch from 85 to 225. Parry at 225 is the last row and fits: 225 + 12
/// against the backdrop's 57 + 189 = 246.
const CINFO_LABEL_X: f32 = 82.0;
const CINFO_LABEL_W: f32 = 47.0;
const CINFO_VALUE_X: f32 = 132.0;
const CINFO_VALUE_W: f32 = 65.0;
const CINFO_ROW_H: f32 = 12.0;
const CINFO_ROW_YS: [f32; 8] = [85.0, 105.0, 125.0, 145.0, 165.0, 185.0, 205.0, 225.0];
/// The STR/INT pair shares one row at y=65 but has its own narrower rects, and
/// its two labels are centre-aligned where the derived-stat labels are
/// left-aligned (`HAlign=1` at `:564`/`:545` vs `HAlign=0`).
const CINFO_STR_LABEL_RECT: (f32, f32, f32, f32) = (82.0, 65.0, 15.0, 12.0);
const CINFO_STR_VALUE_RECT: (f32, f32, f32, f32) = (100.0, 65.0, 29.0, 12.0);
const CINFO_INT_LABEL_RECT: (f32, f32, f32, f32) = (145.0, 65.0, 23.0, 12.0);
const CINFO_INT_VALUE_RECT: (f32, f32, f32, f32) = (170.0, 65.0, 29.0, 12.0);

// --- Hwan (berserk) aura ------------------------------------------------------
//
// `GDR_PMI_EFFECT_JAHWAN` (id 46) is the glow the original lights around the
// whole panel while the player is in hwan. Its rect is `-4,-3,220,80` — larger
// than the 212x70 frame and starting outside it, which is what makes it read as
// an aura rather than a border — and its art is `pmi_jahwan_glow.ddj`, a
// 256x128 sheet whose authored UVs resolve to exactly 220x80
// (`docs/re/ui/hud-player-mini-info.md` §3). The doc records that region's
// *size*, not its origin, so the crop is taken at the sheet origin: the only
// placement that needs no invented offset, and the one the 220x80-of-256x128
// arithmetic implies.
//
// What is NOT modelled: the original tiers the aura by hwan level (§9), and
// what each tier looks like is [U]. `PlayerVitals::hwan_level` (0x30DF) has
// been parsed and stored since the panel was built but drove nothing; this
// wires it to the one thing the data does state — the glow is on while the
// level is non-zero — instead of inventing a per-tier ramp.

/// `ifplayerminiinfo.txt` id 46, window space like every rect here.
const JAHWAN_GLOW_RECT: (f32, f32, f32, f32) = (-4.0, -3.0, 220.0, 80.0);
const JAHWAN_GLOW_ART: &str = "media://interface/playerminiinfo/pmi_jahwan_glow.ddj";
/// The 220x80 region of the 256x128 sheet (see the note above).
const JAHWAN_GLOW_SHEET: (f32, f32) = (220.0, 80.0);

/// Marker on the hwan aura.
#[derive(Component, Default, Clone)]
pub struct MiniInfoJahwanGlow;

/// The aura is lit exactly while the server says the player is in hwan.
///
/// One predicate, kept separate from the system so the rule is testable: the
/// tier the original derives from the same value is [U], so a non-zero level
/// lights the one glow the art gives us.
pub fn jahwan_glow_lit(hwan_level: u8) -> bool {
    hwan_level > 0
}

/// Show or hide the hwan aura as 0x30DF changes.
pub fn update_jahwan_glow(
    vitals: Res<PlayerVitals>,
    mut glow: Query<&mut Visibility, With<MiniInfoJahwanGlow>>,
) {
    if !vitals.is_changed() {
        return;
    }
    let target = if jahwan_glow_lit(vitals.hwan_level) {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut visibility in glow.iter_mut() {
        if *visibility != target {
            *visibility = target;
        }
    }
}

// --- Low HP/MP caution overlays ----------------------------------------------
//
// `GDR_PMI_EFFECT_HP` (77) and `GDR_PMI_EFFECT_MP` (78) are the two animated
// warning overlays, and they are one of only six controls in the whole 247-file
// corpus that carry the resinfo grammar's flipbook keys
// (`docs/re/ui/timer-widgets.md`): `FrameCount=8 WidthCount=4 HeightCount=2
// Speed=100 EnableLoop=1 ImageWidth=512 ImageHeight=64`. The sheet arithmetic
// closes exactly — 512/4 = 128 and 64/2 = 32, which is the declared rect — so
// the animation is fully described by the data.
//
// What the data does NOT say is *when* it shows: no threshold appears anywhere
// in the tree (`docs/re/ui/hud-player-mini-info.md` §9). So the trigger is a
// config value rather than a baked constant, and it ships disabled: a guessed
// threshold would be an invented number driving a visible effect.

/// `ifplayerminiinfo.txt` ids 77 / 78, window space like every rect here.
const HP_CAUTION_RECT: (f32, f32, f32, f32) = (75.0, 25.0, 128.0, 32.0);
const MP_CAUTION_RECT: (f32, f32, f32, f32) = (75.0, 41.0, 128.0, 32.0);
const HP_CAUTION_ART: &str = "media://interface/playerminiinfo/pmi_hp_cha_effect_caution.ddj";
const MP_CAUTION_ART: &str = "media://interface/playerminiinfo/pmi_mp_cha_effect_caution.ddj";
/// `FrameCount`, `WidthCount`, `HeightCount` and `Speed` verbatim.
const CAUTION_FRAME_COUNT: usize = 8;
const CAUTION_SHEET_COLS: usize = 4;
const CAUTION_SHEET_ROWS: usize = 2;
const CAUTION_FRAME_MS: f32 = 100.0;
/// `ImageWidth`/`ImageHeight` of both sheets, confirmed against the DDJ headers.
const CAUTION_SHEET_W: f32 = 512.0;
const CAUTION_SHEET_H: f32 = 64.0;

/// Marker on the HP caution overlay.
#[derive(Component, Default, Clone)]
pub struct MiniInfoHpCaution;

/// Marker on the MP caution overlay.
#[derive(Component, Default, Clone)]
pub struct MiniInfoMpCaution;

/// The two overlays' keys as one description. The quick-party board authors
/// the *same* animation over a half-width sheet, which is why the arithmetic
/// lives in `hud::flipbook` with the sheet as data rather than as constants
/// here.
const CAUTION: Flipbook = Flipbook {
    frame_count: CAUTION_FRAME_COUNT,
    cols: CAUTION_SHEET_COLS,
    rows: CAUTION_SHEET_ROWS,
    frame_ms: CAUTION_FRAME_MS,
    sheet: (CAUTION_SHEET_W, CAUTION_SHEET_H),
};

/// Frame index at `elapsed` seconds, looping (`EnableLoop=1`).
fn caution_frame(elapsed_secs: f32) -> usize {
    CAUTION.frame_at(elapsed_secs)
}

/// Source rect of frame `n` on the 4x2 sheet, row-major.
fn caution_frame_rect(frame: usize) -> Rect {
    CAUTION.frame_rect(frame)
}

// --- Portrait framing -------------------------------------------------------

/// Portrait target resolution (2x the 48px slot for quality).
const PORTRAIT_RT_SIZE: u32 = 96;
/// The body-part slots that make up the portrait: 0 = Hair, 1 = Face (the
/// face mesh carries the head + neck/throat geometry). Slot table in
/// docs/formats/bsr-jmxvres.md. Only meshes from these slots — the
/// character's own, and those of any equipped item occupying them (helmets,
/// hats) — go onto the portrait render layer, so the portrait contains
/// exactly head + throat + head equipment and the camera can frame their own
/// bounding box instead of guessing a head position from body height.
const PORTRAIT_SLOTS: [u32; 2] = [0, 1];
/// How much taller the visible frame is than the head bounding box (the ring
/// is circular, so the corners need slack).
const PORTRAIT_MARGIN: f32 = 1.2;
/// Fallback focus/distance until the first measurement (adult head height).
const PORTRAIT_DEFAULT_FOCUS: Vec3 = Vec3::new(0.0, 17.3, 0.0);
const PORTRAIT_DEFAULT_DISTANCE: f32 = 4.5;
const PORTRAIT_FOV: f32 = 0.6;
/// Sideways camera offset (radians) compensating the stand pose's slight head
/// turn, so the frozen portrait faces the viewer. Flip the sign if the face
/// turns the wrong way.
const PORTRAIT_YAW: f32 = 0.15;
/// Seconds after the player's first mesh appears before the portrait camera
/// deactivates, freezing the render into a static image (the vanilla portrait
/// is not animated). Long enough for textures/equipment to stream in; raise
/// it if gear still pops in later.
const PORTRAIT_CAPTURE_DELAY: f32 = 1.0;

// --- State ------------------------------------------------------------------

/// Local player's HUD vitals, aggregated from CHARACTER_DATA (seed), 0x3057,
/// 0x304E and 0x303D. A resource (not a component) because 0x304E/0x303D
/// carry no unique id — they are inherently local-player-scoped — and so the
/// UI refresh can ride on resource change detection.
#[derive(Resource, Clone, Debug, Default)]
pub struct PlayerVitals {
    pub name: String,
    pub level: u8,
    pub hp: u32,
    pub mp: u32,
    /// `None` until the first `CharacterStatsUpdate` (0x303D) arrives; the
    /// display treats max == cur until then.
    pub max_hp: Option<u32>,
    pub max_mp: Option<u32>,
    /// Hwan/berserk gauge fill, 0-5.
    pub berserk_pips: u8,
    /// The server's hwan level for the local player (0x30DF). Stored but not
    /// yet rendered — the original drives the hwan aura tier from it.
    pub hwan_level: u8,
}

/// The portrait's render-target image, sampled by the UI portrait node.
#[derive(Resource)]
pub struct PortraitTarget(pub Handle<Image>);

// --- Markers ----------------------------------------------------------------

#[derive(Component, Default, Clone)]
pub struct PlayerMiniInfoRoot;
#[derive(Component, Default, Clone)]
pub struct MiniInfoNameText;
#[derive(Component, Default, Clone)]
pub struct MiniInfoLevelText;
#[derive(Component, Default, Clone)]
pub struct MiniInfoHpText;
#[derive(Component, Default, Clone)]
pub struct MiniInfoMpText;
#[derive(Component, Default, Clone)]
pub struct MiniInfoHpFill;
#[derive(Component, Default, Clone)]
pub struct MiniInfoMpFill;
/// Hwan pip with its slot index (0-4).
#[derive(Component, Default, Clone)]
pub struct MiniInfoPip(pub u8);
/// The berserk activation button (visible only at a full gauge).
#[derive(Component, Default, Clone)]
pub struct MiniInfoJahwanButton;
/// The stat-drawer toggle (`GDR_PMI_BTN_CINFO`).
#[derive(Component, Default, Clone)]
pub struct MiniInfoCinfoButton;
/// The stat-up button (`GDR_PMI_BTN_STATUP`). Hidden while the wallet is
/// empty, or greyed there under `hud.hide_statup_when_empty: false` — see
/// [`refresh_statup_button`].
#[derive(Component, Default, Clone)]
pub struct MiniInfoStatUpButton;
/// Root of the expanded stat drawer; its `Visibility` is the open/closed state.
#[derive(Component, Default, Clone)]
pub struct MiniInfoStatDrawer;
/// Which stat a drawer value cell shows.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum CinfoStat {
    #[default]
    Str,
    Int,
    PhyAtk,
    PhyDef,
    MagAtk,
    MagDef,
    PhyBal,
    MagBal,
    Hit,
    Parry,
}
/// A drawer value cell, tagged with the stat it renders.
#[derive(Component, Default, Clone)]
pub struct MiniInfoStatValue(pub CinfoStat);

/// Marker on the player entity once its portrait camera is attached.
#[derive(Component)]
pub struct PortraitRig;
#[derive(Component)]
pub struct PortraitCamera;
/// Countdown until the portrait camera deactivates, freezing the render into
/// a static image.
#[derive(Component)]
pub struct PortraitCapture(Timer);
/// Entity-id signature of the mesh set the current framing was computed
/// from. A set change (equipment swap, late-streaming gear) re-aims and
/// re-captures; animation-driven transform motion deliberately does not.
#[derive(Component, Default)]
pub struct PortraitAim {
    signature: u64,
}
/// Marker on player meshes already lifted onto the portrait render layer.
#[derive(Component)]
pub struct PortraitTagged;

// --- Spawn / cleanup --------------------------------------------------------

pub fn spawn_mini_info(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    mut images: ResMut<Assets<Image>>,
    mut vitals: ResMut<PlayerVitals>,
    ui_strings: Res<ClientUiStrings>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found for the player mini info");
        return;
    };

    // fresh session state (re-entry after logout must not show stale vitals)
    *vitals = PlayerVitals::default();

    let portrait = images.add(Image::new_target_texture(
        PORTRAIT_RT_SIZE,
        PORTRAIT_RT_SIZE,
        TextureFormat::Bgra8UnormSrgb,
        None,
    ));
    commands.insert_resource(PortraitTarget(portrait.clone()));

    commands
        .spawn_scene(mini_info(&asset_server, &fonts, &ui_strings, portrait))
        .insert(UiTargetCamera(camera));
}

pub fn cleanup_mini_info(mut commands: Commands, roots: Query<Entity, With<PlayerMiniInfoRoot>>) {
    for entity in roots.iter() {
        commands.entity(entity).despawn();
    }
    // the portrait camera itself is a child of the player entity and despawns
    // with it (cleanup_game_scene)
    commands.remove_resource::<PortraitTarget>();
}

fn mini_info(
    asset_server: &AssetServer,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
    portrait: Handle<Image>,
) -> impl Scene {
    let window: Handle<Image> = asset_server.load(WINDOW_ATLAS);
    let (fx, fy, fw, fh) = FRAME_ATLAS_RECT;
    let frame_crop = Rect::new(fx, fy, fx + fw, fy + fh);
    let hp_image: Handle<Image> = asset_server.load("media://interface/playerminiinfo/pmi_hp.ddj");
    let mp_image: Handle<Image> = asset_server.load("media://interface/playerminiinfo/pmi_mp.ddj");
    let pip_image: Handle<Image> =
        asset_server.load("media://interface/playerminiinfo/pmi_jahwan.ddj");

    let name_font = fonts.nine.clone();
    let level_font = fonts.nine.clone();
    let hp_font = fonts.nine.clone();
    let mp_font = fonts.nine.clone();

    let s = hud_scale();
    let name_size = NAME_FONT_SIZE * s;
    let bar_size = BAR_FONT_SIZE * s;

    let (por_l, por_t, por_w, por_h) = scaled(PORTRAIT_RECT, s);
    let (name_l, name_t, name_w, name_h) = scaled(NAME_RECT, s);
    let (lvl_l, lvl_t, lvl_w, lvl_h) = scaled(LEVEL_RECT, s);
    let (hpb_l, hpb_t, hpb_w, hpb_h) = scaled(HP_BAR_RECT, s);
    let (mpb_l, mpb_t, mpb_w, mpb_h) = scaled(MP_BAR_RECT, s);
    let (hpt_l, hpt_t, hpt_w, hpt_h) = scaled(HP_TEXT_RECT, s);
    let (mpt_l, mpt_t, mpt_w, mpt_h) = scaled(MP_TEXT_RECT, s);
    let (jbt_l, jbt_t, jbt_w, jbt_h) = scaled(JAHWAN_BUTTON_RECT, s);
    let jahwan_button: Handle<Image> = asset_server.load(JAHWAN_BUTTON_NORMAL);
    let (hpc_l, hpc_t, hpc_w, hpc_h) = scaled(HP_CAUTION_RECT, s);
    let (mpc_l, mpc_t, mpc_w, mpc_h) = scaled(MP_CAUTION_RECT, s);
    let hp_caution: Handle<Image> = asset_server.load(HP_CAUTION_ART);
    let mp_caution: Handle<Image> = asset_server.load(MP_CAUTION_ART);
    let (glow_l, glow_t, glow_w, glow_h) = scaled(JAHWAN_GLOW_RECT, s);
    let jahwan_glow: Handle<Image> = asset_server.load(JAHWAN_GLOW_ART);
    let jahwan_glow_crop = Rect::new(0.0, 0.0, JAHWAN_GLOW_SHEET.0, JAHWAN_GLOW_SHEET.1);
    let (cbt_l, cbt_t, cbt_w, cbt_h) = scaled(CINFO_BUTTON_RECT, s);
    let cinfo_normal: Handle<Image> = asset_server.load(CINFO_BUTTON_NORMAL);
    let cinfo_focus: Handle<Image> = asset_server.load(CINFO_BUTTON_FOCUS);
    let cinfo_press: Handle<Image> = asset_server.load(CINFO_BUTTON_PRESS);
    let cinfo_button = cinfo_normal.clone();
    let (sbt_l, sbt_t, sbt_w, sbt_h) = scaled(STATUP_BUTTON_RECT, s);
    let plus_normal: Handle<Image> = asset_server.load(PLUS_BUTTON_NORMAL);
    let plus_focus: Handle<Image> = asset_server.load(PLUS_BUTTON_FOCUS);
    let plus_press: Handle<Image> = asset_server.load(PLUS_BUTTON_PRESS);
    let plus_button = plus_normal.clone();

    bsn! {
        PlayerMiniInfoRoot
        Name("Player Mini Info")
        // the whole frame (ring, name strip, bar slots) in one atlas crop;
        // its ring interior is opaque black — the portrait backdrop
        ImageNode { image: {window}, image_mode: NodeImageMode::Stretch, rect: {Some(frame_crop)} }
        Node {
            position_type: PositionType::Absolute,
            top: px(WINDOW_POS.1 * s),
            left: px(WINDOW_POS.0 * s),
            width: px(FRAME_W * s),
            height: px(FRAME_H * s),
        }
        // above the world, below the loading overlay (200)
        GlobalZIndex(50)
        Pickable::IGNORE
        Children [
            // hwan aura (id 46). Declared first so it sits under the portrait,
            // the name and the gauges: the descriptor states no z-order for it,
            // and an aura drawn over the readouts would obscure them. Spawned
            // hidden; `update_jahwan_glow` is the only thing that shows it.
            (
                MiniInfoJahwanGlow
                ImageNode { image: {jahwan_glow}, image_mode: NodeImageMode::Stretch, rect: {Some(jahwan_glow_crop)} }
                Node { position_type: PositionType::Absolute, left: px(glow_l), top: px(glow_t), width: px(glow_w), height: px(glow_h) }
                Visibility::Hidden
                Pickable::IGNORE
            ),
            // the offscreen head render (transparent clear) over the frame's
            // black ring interior; the camera frames it to fit inside the ring
            (
                ImageNode { image: {portrait}, image_mode: NodeImageMode::Stretch }
                Node { position_type: PositionType::Absolute, left: px(por_l), top: px(por_t), width: px(por_w), height: px(por_h) }
                Pickable::IGNORE
            ),
            // name (vanilla HAlign 0: left-aligned in its slot)
            (
                label("", name_font, name_size)
                MiniInfoNameText
                TextColor(Color::WHITE)
                TextLayout::justify(Justify::Left)
                Node { position_type: PositionType::Absolute, left: px(name_l), top: px(name_t), width: px(name_w), height: px(name_h), justify_content: JustifyContent::FlexStart }
            ),
            (
                label("", level_font, name_size)
                MiniInfoLevelText
                TextColor({LEVEL_COLOR})
                TextLayout::justify(Justify::Left)
                Node { position_type: PositionType::Absolute, left: px(lvl_l), top: px(lvl_t), width: px(lvl_w), height: px(lvl_h), justify_content: JustifyContent::FlexStart }
            ),
            // HP gauge: track (authored rect) -> `gauge()` crop -> art at its
            // native 124x12, so `pmi_hp` is cropped and never squashed (#630).
            (
                Node {
                    position_type: PositionType::Absolute,
                    left: px(hpb_l),
                    top: px(hpb_t),
                    width: px(hpb_w),
                    height: px(hpb_h),
                    overflow: {Overflow::clip()},
                }
                Children [
                    (
                        MiniInfoHpFill
                        gauge(hp_image, hpb_w, hpb_h)
                    ),
                ]
            ),
            // MP gauge
            (
                Node {
                    position_type: PositionType::Absolute,
                    left: px(mpb_l),
                    top: px(mpb_t),
                    width: px(mpb_w),
                    height: px(mpb_h),
                    overflow: {Overflow::clip()},
                }
                Children [
                    (
                        MiniInfoMpFill
                        gauge(mp_image, mpb_w, mpb_h)
                    ),
                ]
            ),
            // Animated low-vitals warnings (ids 77/78). Spawned hidden; only
            // `update_caution_overlays` ever shows them, and only when the
            // config supplies a threshold.
            (
                MiniInfoHpCaution
                ImageNode { image: {hp_caution}, image_mode: NodeImageMode::Stretch, rect: {Some(caution_frame_rect(0))} }
                Node { position_type: PositionType::Absolute, left: px(hpc_l), top: px(hpc_t), width: px(hpc_w), height: px(hpc_h) }
                Visibility::Hidden
                Pickable::IGNORE
            ),
            (
                MiniInfoMpCaution
                ImageNode { image: {mp_caution}, image_mode: NodeImageMode::Stretch, rect: {Some(caution_frame_rect(0))} }
                Node { position_type: PositionType::Absolute, left: px(mpc_l), top: px(mpc_t), width: px(mpc_w), height: px(mpc_h) }
                Visibility::Hidden
                Pickable::IGNORE
            ),
            // "cur / max" overlays (vanilla HAlign 1: centered)
            (
                label("", hp_font, bar_size)
                MiniInfoHpText
                TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, left: px(hpt_l), top: px(hpt_t), width: px(hpt_w), height: px(hpt_h) }
            ),
            (
                label("", mp_font, bar_size)
                MiniInfoMpText
                TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, left: px(mpt_l), top: px(mpt_t), width: px(mpt_w), height: px(mpt_h) }
            ),
            // hwan/berserk pips along the portrait ring's arc; visibility is
            // driven by refresh_mini_info (hidden while the gauge is empty)
            (pip(pip_image.clone(), 0)),
            (pip(pip_image.clone(), 1)),
            (pip(pip_image.clone(), 2)),
            (pip(pip_image.clone(), 3)),
            (pip(pip_image, 4)),
            // the expanded stat drawer and its toggle; the drawer hangs below
            // the 70px frame and starts hidden. The toggle's hover/press art is
            // swapped by the shared `ImageButtonStyle` updater, so it needs no
            // system of its own. NOT Pickable::IGNORE — it's clickable.
            (stat_drawer(asset_server, fonts, ui_strings)),
            (
                MiniInfoCinfoButton
                Button
                Hovered
                ImageNode { image: {cinfo_button}, image_mode: NodeImageMode::Stretch }
                ImageButtonStyle { normal: {cinfo_normal}, hover: {cinfo_focus}, press: {cinfo_press} }
                Node {
                    position_type: PositionType::Absolute,
                    left: px(cbt_l),
                    top: px(cbt_t),
                    width: px(cbt_w),
                    height: px(cbt_h),
                }
            ),
            // the stat-up `+` next to the level slot. It opens the character
            // window rather than spending a point itself: one button cannot
            // choose between STR and INT, and that window is the only surface
            // in the client that offers the choice (its own two `+` buttons
            // send 0x7050/0x7051). Stated inference, not a transcription.
            (
                MiniInfoStatUpButton
                Button
                Hovered
                ImageNode { image: {plus_button}, image_mode: NodeImageMode::Stretch }
                ImageButtonStyle { normal: {plus_normal}, hover: {plus_focus}, press: {plus_press} }
                Node {
                    position_type: PositionType::Absolute,
                    left: px(sbt_l),
                    top: px(sbt_t),
                    width: px(sbt_w),
                    height: px(sbt_h),
                }
            ),
            // berserk activation button on the ring's top; shown by
            // refresh_mini_info only at a full gauge, hover/press art via
            // update_jahwan_button. NOT Pickable::IGNORE — it's clickable.
            (
                MiniInfoJahwanButton
                ImageNode { image: {jahwan_button}, image_mode: NodeImageMode::Stretch }
                Node {
                    position_type: PositionType::Absolute,
                    left: px(jbt_l),
                    top: px(jbt_t),
                    width: px(jbt_w),
                    height: px(jbt_h),
                }
            ),
        ]
    }
}

/// The drawer's rects are transcribed in window space, but its cells are
/// children of the backdrop, so shift them into backdrop-local space.
fn drawer_local(rect: (f32, f32, f32, f32), s: f32) -> (f32, f32, f32, f32) {
    scaled(
        (
            rect.0 - CINFO_BG_RECT.0,
            rect.1 - CINFO_BG_RECT.1,
            rect.2,
            rect.3,
        ),
        s,
    )
}

/// A derived-stat row's label rect at row `y` (`82,y,47,12`).
fn cinfo_label_rect(y: f32) -> (f32, f32, f32, f32) {
    (CINFO_LABEL_X, y, CINFO_LABEL_W, CINFO_ROW_H)
}

/// A derived-stat row's value rect at row `y` (`132,y,65,12`).
fn cinfo_value_rect(y: f32) -> (f32, f32, f32, f32) {
    (CINFO_VALUE_X, y, CINFO_VALUE_W, CINFO_ROW_H)
}

/// A drawer label cell: static text straight from the string table.
fn stat_label(
    font: Handle<Font>,
    text: &str,
    rect: (f32, f32, f32, f32),
    justify: Justify,
) -> impl Scene {
    let s = hud_scale();
    let size = BAR_FONT_SIZE * s;
    let (l, t, w, h) = drawer_local(rect, s);
    bsn! {
        label(text, font, size)
        TextColor(CINFO_LABEL_COLOR)
        TextLayout::justify(justify)
        Node {
            position_type: PositionType::Absolute,
            left: px(l),
            top: px(t),
            width: px(w),
            height: px(h),
        }
        Pickable::IGNORE
    }
}

/// A drawer value cell. Right-aligned (`HAlign=2`) and white, and filled by
/// [`refresh_stat_drawer`] from the stat sheet.
fn stat_value(font: Handle<Font>, stat: CinfoStat, rect: (f32, f32, f32, f32)) -> impl Scene {
    let s = hud_scale();
    let size = BAR_FONT_SIZE * s;
    let (l, t, w, h) = drawer_local(rect, s);
    bsn! {
        label("", font, size)
        MiniInfoStatValue({stat})
        TextColor(Color::WHITE)
        TextLayout::justify(Justify::Right)
        Node {
            position_type: PositionType::Absolute,
            left: px(l),
            top: px(t),
            width: px(w),
            height: px(h),
        }
        Pickable::IGNORE
    }
}

/// The expanded stat drawer: the backdrop crop plus the ten label/value pairs,
/// hidden until the toggle button is clicked.
fn stat_drawer(
    asset_server: &AssetServer,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
) -> impl Scene {
    let window: Handle<Image> = asset_server.load(WINDOW_ATLAS);
    let (ax, ay, aw, ah) = CINFO_BG_ATLAS_RECT;
    let bg_crop = Rect::new(ax, ay, ax + aw, ay + ah);
    let s = hud_scale();
    let (bg_l, bg_t, bg_w, bg_h) = scaled(CINFO_BG_RECT, s);
    let f = fonts.nine.clone();
    bsn! {
        MiniInfoStatDrawer
        Name("Mini Info Stat Drawer")
        ImageNode { image: {window}, image_mode: NodeImageMode::Stretch, rect: {Some(bg_crop)} }
        Node {
            position_type: PositionType::Absolute,
            left: px(bg_l),
            top: px(bg_t),
            width: px(bg_w),
            height: px(bg_h),
        }
        Visibility::Hidden
        Pickable::IGNORE
        Children [
            // y=65: the STR/INT pair, the only centre-aligned labels
            (stat_label(f.clone(), ui_strings.get_or("PARAM_STR", "Str"), CINFO_STR_LABEL_RECT, Justify::Center)),
            (stat_value(f.clone(), CinfoStat::Str, CINFO_STR_VALUE_RECT)),
            (stat_label(f.clone(), ui_strings.get_or("PARAM_INT", "Int"), CINFO_INT_LABEL_RECT, Justify::Center)),
            (stat_value(f.clone(), CinfoStat::Int, CINFO_INT_VALUE_RECT)),
            // y=85..225: the eight derived-stat rows, left labels, right values
            (stat_label(f.clone(), ui_strings.get_or("UIIT_STT_PHYSICAL_ATTACK", "Phy. atk"), cinfo_label_rect(CINFO_ROW_YS[0]), Justify::Left)),
            (stat_value(f.clone(), CinfoStat::PhyAtk, cinfo_value_rect(CINFO_ROW_YS[0]))),
            (stat_label(f.clone(), ui_strings.get_or("UIIT_STT_PHYSICAL_DEFENCE", "Phy. def."), cinfo_label_rect(CINFO_ROW_YS[1]), Justify::Left)),
            (stat_value(f.clone(), CinfoStat::PhyDef, cinfo_value_rect(CINFO_ROW_YS[1]))),
            (stat_label(f.clone(), ui_strings.get_or("UIIT_STT_MAGICAL_ATTACK", "Mag. atk"), cinfo_label_rect(CINFO_ROW_YS[2]), Justify::Left)),
            (stat_value(f.clone(), CinfoStat::MagAtk, cinfo_value_rect(CINFO_ROW_YS[2]))),
            (stat_label(f.clone(), ui_strings.get_or("UIIT_STT_MAGICAL_DEFENCE", "Mag. def."), cinfo_label_rect(CINFO_ROW_YS[3]), Justify::Left)),
            (stat_value(f.clone(), CinfoStat::MagDef, cinfo_value_rect(CINFO_ROW_YS[3]))),
            (stat_label(f.clone(), ui_strings.get_or("UIIT_STT_PHYSICAL_BALANCE", "Phy. balance"), cinfo_label_rect(CINFO_ROW_YS[4]), Justify::Left)),
            (stat_value(f.clone(), CinfoStat::PhyBal, cinfo_value_rect(CINFO_ROW_YS[4]))),
            (stat_label(f.clone(), ui_strings.get_or("UIIT_STT_MAGICAL_BALANCE", "Mag. balance"), cinfo_label_rect(CINFO_ROW_YS[5]), Justify::Left)),
            (stat_value(f.clone(), CinfoStat::MagBal, cinfo_value_rect(CINFO_ROW_YS[5]))),
            (stat_label(f.clone(), ui_strings.get_or("UIIT_STT_HIT_RATIO", "Hit rate"), cinfo_label_rect(CINFO_ROW_YS[6]), Justify::Left)),
            (stat_value(f.clone(), CinfoStat::Hit, cinfo_value_rect(CINFO_ROW_YS[6]))),
            (stat_label(f.clone(), ui_strings.get_or("UIIT_STT_PARRY_RATIO", "Parry ratio"), cinfo_label_rect(CINFO_ROW_YS[7]), Justify::Left)),
            (stat_value(f, CinfoStat::Parry, cinfo_value_rect(CINFO_ROW_YS[7]))),
        ]
    }
}

/// The drawer's value strings. Before the first 0x303D there is no sheet, so
/// every cell reads "-".
///
/// The two balance rows are the exception to "everything comes from the
/// packet": balance is not on the wire at all, and is derived from the sheet's
/// STR/INT and the character's level by [`balance_text`] — the same function
/// the character window uses, so the two surfaces cannot print different text
/// for one stat (#765). This supersedes
/// `docs/re/ui/hud-player-mini-info.md` §8.1's "leave the two balance fields
/// UNKNOWN", which predates the derivation shipping in `character_info`; the
/// sourcing lives in `docs/re/gamedata/stat-derivation-model.md:182-184`.
fn stat_value_text(sheet: Option<&CharacterStatsUpdate>, level: u8, stat: CinfoStat) -> String {
    let Some(s) = sheet else {
        return "-".to_string();
    };
    match stat {
        CinfoStat::Str => s.strength.to_string(),
        CinfoStat::Int => s.intelligence.to_string(),
        CinfoStat::PhyAtk => format!("{} ~ {}", s.phys_attack_min, s.phys_attack_max),
        CinfoStat::MagAtk => format!("{} ~ {}", s.mag_attack_min, s.mag_attack_max),
        CinfoStat::PhyDef => s.phys_defense.to_string(),
        CinfoStat::MagDef => s.mag_defense.to_string(),
        CinfoStat::Hit => s.hit_rate.to_string(),
        CinfoStat::Parry => s.parry_rate.to_string(),
        // Balance is not on the wire; it is derived from the stat and the
        // level's `MaxStat` ceiling, by the one function this drawer and the
        // character window share, so the two surfaces cannot disagree again
        // (#765) — they used to, and both are visible at once.
        CinfoStat::PhyBal => balance_text(level as u32, s.strength),
        CinfoStat::MagBal => balance_text(level as u32, s.intelligence),
    }
}

/// Attach the drawer toggle. `spawn_scene` defers entity creation, so the
/// observer is wired a frame later — the same pattern as the target window's
/// close button.
pub fn wire_cinfo_button(fresh: Query<Entity, Added<MiniInfoCinfoButton>>, mut commands: Commands) {
    for entity in fresh.iter() {
        commands.entity(entity).observe(
            |_: On<Activate>, mut drawer: Query<&mut Visibility, With<MiniInfoStatDrawer>>| {
                for mut visibility in drawer.iter_mut() {
                    *visibility = match *visibility {
                        Visibility::Hidden => Visibility::Inherited,
                        _ => Visibility::Hidden,
                    };
                }
            },
        );
    }
}

/// Attach the stat-up button. Same deferred-spawn dance as
/// [`wire_cinfo_button`]: the observer opens the character window, which is
/// where a point is actually spent.
pub fn wire_statup_button(
    fresh: Query<Entity, Added<MiniInfoStatUpButton>>,
    mut commands: Commands,
) {
    for entity in fresh.iter() {
        commands.entity(entity).observe(
            |_: On<Activate>, mut window: ResMut<CharacterInfoState>| {
                window.open = true;
            },
        );
    }
}

/// Update the stat-up button for the current wallet.
///
/// Two behaviours, selected by `hud.hide_statup_when_empty`:
///
/// * **hidden** (our default) — an empty wallet takes the button out of the
///   layout entirely, so its *appearance* is the "you have a point to spend"
///   cue. A permanently greyed control that is actionable for a few seconds per
///   level is clutter the rest of the time.
/// * **greyed** (the original) — the button's `Style=0` says it is always
///   drawn, all fifteen conditionally-drawn decorations of this tree carry
///   `Style=64`, and a `_disable` art variant ships for exactly this state.
///
/// Either way the enabled path mirrors `character_info::ui`'s treatment of its
/// own two `+` buttons, including that the style handles (not just the
/// `ImageNode`) have to change, because the shared button-visual system
/// repaints the node from the style every frame.
/// Whether the stat-up button occupies its slot at all — the one decision
/// `hud.hide_statup_when_empty` selects, split out so both branches can be
/// pinned without standing up the panel.
fn statup_display(enabled: bool, hide_when_empty: bool) -> Display {
    if enabled || !hide_when_empty {
        Display::Flex
    } else {
        Display::None
    }
}

pub fn refresh_statup_button(
    stats: Res<PlayerStats>,
    config: Option<Res<ClientConfig>>,
    asset_server: Res<AssetServer>,
    mut buttons: Query<
        (Entity, &mut ImageButtonStyle, &mut ImageNode, &mut Node),
        With<MiniInfoStatUpButton>,
    >,
    mut commands: Commands,
) {
    let enabled = stats.stat_points > 0;
    // Absent config (the offline preview scenes and the unit tests) takes the
    // shipped default rather than silently flipping to the original's look.
    let hide_when_empty = config
        .as_deref()
        .map_or(true, |c| c.hud.hide_statup_when_empty);
    for (entity, mut style, mut image, mut node) in buttons.iter_mut() {
        let display = statup_display(enabled, hide_when_empty);
        if node.display != display {
            node.display = display;
        }
        if enabled {
            style.normal = asset_server.load(PLUS_BUTTON_NORMAL);
            style.hover = asset_server.load(PLUS_BUTTON_FOCUS);
            style.press = asset_server.load(PLUS_BUTTON_PRESS);
            commands.entity(entity).remove::<InteractionDisabled>();
        } else {
            let disable: Handle<Image> = asset_server.load(PLUS_BUTTON_DISABLE);
            style.normal = disable.clone();
            style.hover = disable.clone();
            style.press = disable;
            commands.entity(entity).insert(InteractionDisabled);
        }
        image.image = style.normal.clone();
    }
}

/// Fill the drawer's value cells from the stat sheet the character-info model
/// already mirrors out of 0x303D, so this window needs no second reader. The
/// level comes from this panel's own [`PlayerVitals`] — the two balance rows
/// measure the stat against the ceiling that level allows.
pub fn refresh_stat_drawer(
    stats: Res<PlayerStats>,
    vitals: Res<PlayerVitals>,
    mut cells: Query<(&MiniInfoStatValue, &mut Text)>,
) {
    for (cell, mut text) in cells.iter_mut() {
        let new = stat_value_text(stats.sheet.as_ref(), vitals.level, cell.0);
        if text.0 != new {
            text.0 = new;
        }
    }
}

fn pip(image: Handle<Image>, index: u8) -> impl Scene {
    let s = hud_scale();
    let (x, y) = PIP_POSITIONS[index as usize];
    bsn! {
        MiniInfoPip({index})
        ImageNode { image: {image}, image_mode: NodeImageMode::Stretch }
        Node {
            position_type: PositionType::Absolute,
            left: px(x * s),
            top: px(y * s),
            width: px(PIP_SIZE * s),
            height: px(PIP_SIZE * s),
        }
        Pickable::IGNORE
    }
}

// --- Vitals updates ---------------------------------------------------------

/// Seed name/level/hp/mp/berserk from the `CharacterInfo` the game scene
/// inserts on the player entity once CHARACTER_DATA (0x3013) resolves.
pub fn seed_vitals_from_character_info(
    added: Query<&CharacterInfo, (With<Player>, Added<CharacterInfo>)>,
    mut vitals: ResMut<PlayerVitals>,
) {
    for info in added.iter() {
        if let Some(name) = &info.name {
            vitals.name = name.clone();
        }
        if let Some(stats) = &info.stats {
            vitals.level = stats.level;
            vitals.hp = stats.hp;
            vitals.mp = stats.mp;
            vitals.berserk_pips = stats.berserk_points.min(5);
            debug!(
                "mini-info: seeded from CHARACTER_DATA (level {} hp {} mp {} pips {})",
                stats.level, stats.hp, stats.mp, vitals.berserk_pips
            );
        }
    }
}

/// Apply 0x3057 vitals for the local player; updates for other entities
/// (monsters, remote players) are ignored here — a future health-bar feature
/// will consume those.
pub fn on_entity_bars_update(
    mut reader: MessageReader<EntityBarsUpdate>,
    player: Query<&NetworkId, With<Player>>,
    mut vitals: ResMut<PlayerVitals>,
) {
    for msg in reader.read() {
        let Ok(NetworkId(uid)) = player.single() else {
            continue;
        };
        if msg.unique_id != *uid {
            continue;
        }
        debug!(
            "mini-info: EntityBarsUpdate source={:#x} hp={:?} mp={:?} bad_status={:?}",
            msg.source, msg.hp, msg.mp, msg.bad_status
        );
        // compare-before-write so per-tick regen packets only dirty the UI on
        // an actual change
        if let Some(hp) = msg.hp {
            if vitals.hp != hp {
                vitals.hp = hp;
            }
        }
        if let Some(mp) = msg.mp {
            if vitals.mp != mp {
                vitals.mp = mp;
            }
        }
    }
}

/// Apply 0x304E: only the berserk variant feeds the mini info (gold/SP/stat
/// points land in other UI later).
pub fn on_points_update(
    mut reader: MessageReader<CharacterPointsUpdate>,
    mut vitals: ResMut<PlayerVitals>,
) {
    for msg in reader.read() {
        if let CharacterPointsUpdate::Berserk { amount, .. } = msg {
            debug!("mini-info: berserk gauge {}", amount);
            let pips = (*amount).min(5);
            if vitals.berserk_pips != pips {
                vitals.berserk_pips = pips;
            }
        }
    }
}

/// Apply 0x303D: the authoritative max HP/MP (and later, the stat sheet).
pub fn on_stats_update(
    mut reader: MessageReader<CharacterStatsUpdate>,
    mut vitals: ResMut<PlayerVitals>,
) {
    for msg in reader.read() {
        debug!(
            "mini-info: CharacterStatsUpdate max_hp={} max_mp={}",
            msg.max_hp, msg.max_mp
        );
        vitals.max_hp = Some(msg.max_hp);
        vitals.max_mp = Some(msg.max_mp);
        // keep the displayed current value sane if a max shrank below it
        vitals.hp = vitals.hp.min(msg.max_hp);
        vitals.mp = vitals.mp.min(msg.max_mp);
    }
}

// --- UI refresh -------------------------------------------------------------

/// Run condition: vitals changed, or the panel was just (re)spawned.
pub fn mini_info_needs_refresh(
    vitals: Res<PlayerVitals>,
    fresh: Query<(), Added<MiniInfoNameText>>,
) -> bool {
    vitals.is_changed() || !fresh.is_empty()
}

/// Gauge label for a current/maximum pair.
///
/// Idea: a maximum we have not been told is rendered as `?`, not as the
/// current value. `PlayerVitals.max_hp/max_mp` stay `None` until the first
/// `CharacterStatsUpdate` (0x303D) — 0x3013 carries no maxima — and the
/// previous `unwrap_or(cur)` turned that gap into the confident statement
/// "you are at full health". That is the one direction this must not fail
/// in: a character who joins at 12% HP was shown a full bar reading
/// `cur / cur`. Deliberate deviation from the original, which computes the
/// maximum locally from level and STR/INT: we cannot, because 0x3013 gives
/// us the level but no stats (`packets/src/agent/character_data.rs:64-85`),
/// and `leveldata::max_hp_or_mp` needs the stat. So we say "unknown" instead
/// of guessing — the format string is ours either way, the data has
/// `Text=""` on both value overlays.
fn gauge_text(cur: u32, max: Option<u32>) -> String {
    match max {
        Some(max) => format!("{cur} / {max}"),
        None => format!("{cur} / ?"),
    }
}

/// Fill fraction (0.0-1.0) for a current/maximum pair; an unknown or zero
/// maximum draws no fill rather than a full bar (see [`gauge_text`]).
fn gauge_fill(cur: u32, max: Option<u32>) -> f32 {
    match max {
        Some(max) if max > 0 => (cur as f32 / max as f32).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// Push [`PlayerVitals`] into the panel: texts, bar fill widths, pip
/// visibility. Until 0x303D supplies a max the bars stay empty and read
/// `cur / ?` — see [`gauge_text`].
#[allow(clippy::too_many_arguments)]
pub fn refresh_mini_info(
    vitals: Res<PlayerVitals>,
    name_q: Query<Entity, With<MiniInfoNameText>>,
    level_q: Query<Entity, With<MiniInfoLevelText>>,
    hp_text_q: Query<Entity, With<MiniInfoHpText>>,
    mp_text_q: Query<Entity, With<MiniInfoMpText>>,
    hp_fill_q: Query<Entity, With<MiniInfoHpFill>>,
    mp_fill_q: Query<Entity, With<MiniInfoMpFill>>,
    mut texts: Query<&mut Text>,
    mut nodes: Query<&mut Node>,
    mut pips: Query<(&MiniInfoPip, &mut Visibility), Without<MiniInfoJahwanButton>>,
    mut jahwan_button: Query<&mut Visibility, (With<MiniInfoJahwanButton>, Without<MiniInfoPip>)>,
) {
    let mut set_text = |entity: Entity, value: String| {
        if let Ok(mut text) = texts.get_mut(entity) {
            if text.0 != value {
                text.0 = value;
            }
        }
    };

    for entity in name_q.iter() {
        set_text(entity, vitals.name.clone());
    }
    for entity in level_q.iter() {
        set_text(entity, format!("Lv {}", vitals.level));
    }

    for entity in hp_text_q.iter() {
        set_text(entity, gauge_text(vitals.hp, vitals.max_hp));
    }
    for entity in mp_text_q.iter() {
        set_text(entity, gauge_text(vitals.mp, vitals.max_mp));
    }

    // The crop node carries the fill; its art child keeps its native size.
    let mut set_fill = |entity: Entity, cur: u32, max: Option<u32>, track_w: f32| {
        if let Ok(mut node) = nodes.get_mut(entity) {
            // Compared before writing, like `set_text` above: a `Node` write
            // marks the UI tree dirty and bevy_ui re-runs the whole Taffy
            // layout, so an unguarded gauge fill costs a relayout every frame
            // whether or not the bar moved.
            let width = gauge_fill_width(gauge_fill(cur, max), track_w);
            if node.width != width {
                node.width = width;
            }
        }
    };
    let hp_track_w = HP_BAR_RECT.2 * hud_scale();
    let mp_track_w = MP_BAR_RECT.2 * hud_scale();
    for entity in hp_fill_q.iter() {
        set_fill(entity, vitals.hp, vitals.max_hp, hp_track_w);
    }
    for entity in mp_fill_q.iter() {
        set_fill(entity, vitals.mp, vitals.max_mp, mp_track_w);
    }

    for (pip, mut visibility) in pips.iter_mut() {
        *visibility = if pip.0 < vitals.berserk_pips {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }

    // the activation button appears once the gauge is full
    for mut visibility in jahwan_button.iter_mut() {
        *visibility = if vitals.berserk_pips >= 5 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

/// Swap the berserk button art on hover/press and send the activation request.
///
/// The request is 0x70A7 with the single action byte the original's builder
/// writes (`sro_client.exe@0081e690:21,30`); the server answers with 0xB0A7,
/// handled by [`on_hwan_action_response`]. (`Interaction` has no bsn template
/// support, so it is inserted here rather than in the scene.)
pub fn update_jahwan_button(
    asset_server: Res<AssetServer>,
    uninit: Query<Entity, (With<MiniInfoJahwanButton>, Without<Interaction>)>,
    mut buttons: Query<
        (&Interaction, &mut ImageNode),
        (Changed<Interaction>, With<MiniInfoJahwanButton>),
    >,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut commands: Commands,
) {
    for entity in uninit.iter() {
        commands.entity(entity).insert(Interaction::default());
    }
    for (interaction, mut image) in buttons.iter_mut() {
        let path = match interaction {
            Interaction::Pressed => {
                debug!("mini-info: berserk button pressed, sending 0x70A7");
                send_hwan_activation(&conn);
                JAHWAN_BUTTON_PRESS
            }
            Interaction::Hovered => JAHWAN_BUTTON_FOCUS,
            Interaction::None => JAHWAN_BUTTON_NORMAL,
        };
        image.image = asset_server.load(path);
    }
}

fn send_hwan_activation(conn: &Query<&SilkroadConnection, With<AgentConnection>>) {
    let Ok(conn) = conn.single() else {
        warn!("mini-info: not sending berserk activation, no agent connection");
        return;
    };
    if let Err(e) = conn
        .get_sender()
        .send(Packet::from(HwanActionRequest::berserk()).into())
    {
        error!("network: failed to send berserk activation: {}", e.0);
    }
}

/// Apply 0xB0A7: the server's verdict on our 0x70A7. Only a failure is worth
/// surfacing — the state change itself arrives as 0x304E (gauge) / 0x30DF
/// (hwan level).
pub fn on_hwan_action_response(mut reader: MessageReader<HwanActionResponse>) {
    for msg in reader.read() {
        if msg.is_success() {
            debug!("mini-info: berserk activation accepted");
        } else {
            warn!(
                "mini-info: berserk activation refused (result {}, error {:?})",
                msg.result, msg.error_code
            );
        }
    }
}

/// Apply 0x30DF for the local player: the server's authoritative hwan level.
/// Updates naming other entities are ignored here, exactly like 0x3057 —
/// remote hwan auras are not modelled yet.
pub fn on_hwan_level_update(
    mut reader: MessageReader<HwanLevelUpdate>,
    player: Query<&NetworkId, With<Player>>,
    mut vitals: ResMut<PlayerVitals>,
) {
    for msg in reader.read() {
        let Ok(NetworkId(uid)) = player.single() else {
            continue;
        };
        if msg.unique_id != *uid {
            continue;
        }
        if vitals.hwan_level != msg.level {
            debug!("mini-info: hwan level {}", msg.level);
            vitals.hwan_level = msg.level;
        }
    }
}

// --- Portrait rig -----------------------------------------------------------

/// Attach the offscreen portrait camera to the player once both exist. Polling
/// rather than `OnEnter` because the player entity is spawned by another
/// plugin's enter systems (only visible after a command flush), and so a
/// respawned player re-acquires its rig.
pub fn attach_portrait_rig(
    target: Option<Res<PortraitTarget>>,
    player: Query<Entity, (With<Player>, Without<PortraitRig>)>,
    mut commands: Commands,
) {
    let Some(target) = target else {
        return;
    };
    for player_entity in player.iter() {
        let camera = commands
            .spawn((
                PortraitCamera,
                Name::from("Portrait Camera"),
                Camera3d::default(),
                Camera {
                    // transparent: the UI's backdrop disc shows through
                    clear_color: ClearColorConfig::Custom(Color::NONE),
                    // render before the main (0) and UI (1) cameras
                    order: -1,
                    ..default()
                },
                RenderTarget::from(target.0.clone()),
                Projection::Perspective(PerspectiveProjection {
                    fov: PORTRAIT_FOV,
                    near: 0.5,
                    far: 60.0,
                    ..default()
                }),
                RenderLayers::layer(CameraLayers::Portrait.into()),
                PortraitCapture(Timer::from_seconds(PORTRAIT_CAPTURE_DELAY, TimerMode::Once)),
                PortraitAim::default(),
                portrait_transform(PORTRAIT_DEFAULT_FOCUS, PORTRAIT_DEFAULT_DISTANCE),
                // portrait lighting is self-contained: world lights live on the
                // main layer and never reach this view
                AmbientLight {
                    color: Color::WHITE,
                    brightness: 500.0,
                    ..default()
                },
                ChildOf(player_entity),
            ))
            .id();
        // headlight: identity transform → shines wherever the camera looks
        commands.spawn((
            DirectionalLight {
                illuminance: 3_000.0,
                ..default()
            },
            RenderLayers::layer(CameraLayers::Portrait.into()),
            ChildOf(camera),
        ));
        commands.entity(player_entity).insert(PortraitRig);
        debug!("mini-info: portrait camera attached to player");
    }
}

/// Camera pose looking at `focus` (player space) from `distance` in front of
/// the face (the body faces -Z), yawed slightly to meet the stand pose's
/// head turn.
fn portrait_transform(focus: Vec3, distance: f32) -> Transform {
    let offset = Quat::from_rotation_y(PORTRAIT_YAW) * Vec3::new(0.0, 0.0, -distance);
    Transform::from_translation(focus + offset).looking_at(focus, Vec3::Y)
}

/// Re-frame the portrait when the player's MESH SET changes (first meshes
/// streaming in, equipment swapped on or off) and re-run the capture so the
/// frozen portrait reflects the current look.
///
/// The trigger is an entity-id signature of the tagged meshes, deliberately
/// NOT the measured height: skinned/bone-attached mesh transforms sway with
/// the stand animation, so a height-based trigger re-aims every frame — the
/// portrait bounces and the camera (with its per-view visibility pass) never
/// freezes. Height is measured once per set change, from the AABB corners in
/// player space (hats included).
pub fn aim_portrait_camera(
    players: Query<&GlobalTransform, With<Player>>,
    children: Query<&Children>,
    meshes: Query<(&GlobalTransform, &Aabb, &Visibility), With<PortraitTagged>>,
    mut cameras: Query<
        (
            &ChildOf,
            &mut Transform,
            &mut Camera,
            &mut PortraitCapture,
            &mut PortraitAim,
        ),
        With<PortraitCamera>,
    >,
) {
    for (child_of, mut transform, mut camera, mut capture, mut aim) in cameras.iter_mut() {
        let player = child_of.parent();
        let Ok(player_gt) = players.get(player) else {
            continue;
        };

        // cheap, order-independent signature of the current mesh set — no
        // writes (and no change ticks) unless it differs from the last aim.
        // Hidden meshes (hair replaced by a helmet) are excluded, so both the
        // attach and the hide re-trigger an aim.
        let mut signature = 0u64;
        let mut any = false;
        for entity in children.iter_descendants(player) {
            if let Ok((_, _, visibility)) = meshes.get(entity) {
                if *visibility != Visibility::Hidden {
                    signature ^= entity.to_bits();
                    any = true;
                }
            }
        }
        if !any || signature == aim.signature {
            continue;
        }

        // combined bounding box of the visible head meshes, in player space
        let to_player = player_gt.affine().inverse();
        let mut min = Vec3A::splat(f32::MAX);
        let mut max = Vec3A::splat(f32::MIN);
        for entity in children.iter_descendants(player) {
            let Ok((mesh_gt, aabb, visibility)) = meshes.get(entity) else {
                continue;
            };
            if *visibility == Visibility::Hidden {
                continue;
            }
            let to_local = to_player * mesh_gt.affine();
            for i in 0..8 {
                let corner = Vec3A::from(aabb.center)
                    + Vec3A::from(aabb.half_extents)
                        * Vec3A::new(
                            if i & 1 == 0 { -1.0 } else { 1.0 },
                            if i & 2 == 0 { -1.0 } else { 1.0 },
                            if i & 4 == 0 { -1.0 } else { 1.0 },
                        );
                let p = to_local.transform_point3a(corner);
                min = min.min(p);
                max = max.max(p);
            }
        }
        if min.y >= max.y {
            // AABBs not ready yet — leave the signature unset so we retry
            continue;
        }

        let focus = Vec3::from((min + max) * 0.5);
        let height = (max.y - min.y).max(0.2);
        // distance so the visible height at the focus plane is the head box
        // plus margin: 2*d*tan(fov/2) = height * margin
        let distance = height * PORTRAIT_MARGIN * 0.5 / (PORTRAIT_FOV * 0.5).tan();
        aim.signature = signature;
        *transform = portrait_transform(focus, distance);
        camera.is_active = true;
        capture.0.reset();
        debug!(
            "mini-info: portrait re-aimed (head height {:.2}, focus {:.1})",
            height, focus.y
        );
    }
}

/// Freeze the portrait once the player's meshes have been visible for
/// [`PORTRAIT_CAPTURE_DELAY`]: deactivating the camera keeps the last rendered
/// frame in the target texture, giving the static portrait of the vanilla
/// client (and dropping the per-frame render cost).
pub fn freeze_portrait_camera(
    time: Res<Time>,
    tagged: Query<(), With<PortraitTagged>>,
    mut cameras: Query<(&mut Camera, &mut PortraitCapture), With<PortraitCamera>>,
) {
    if tagged.is_empty() {
        return;
    }
    for (mut camera, mut capture) in cameras.iter_mut() {
        if camera.is_active && capture.0.tick(time.delta()).is_finished() {
            camera.is_active = false;
            debug!("mini-info: portrait frozen");
        }
    }
}

/// Lift the player's HEAD meshes onto the portrait layer (keeping the main
/// layer), so the portrait contains exactly head + throat + head equipment.
/// Selection is by body-part slot ([`PORTRAIT_SLOTS`]): a resource's
/// `attach_info.slots` maps slots to mesh indices — the character wrapper
/// contributes its hair/face meshes (via `MeshIndexMap`), and an attached
/// item contributes all of its meshes iff it occupies a head slot (helmets,
/// hats). Polling, because the body wrapper and equipment attachments stream
/// in over several frames; already-tagged meshes short-circuit.
pub fn tag_player_meshes_for_portrait(
    players: Query<Entity, With<Player>>,
    children: Query<&Children>,
    wrappers: Query<(&SpawnedFromResource, Option<&MeshIndexMap>)>,
    resources: Res<Assets<SroResource>>,
    mesh3d: Query<(), With<Mesh3d>>,
    tagged: Query<(), With<PortraitTagged>>,
    mut commands: Commands,
) {
    let mut targets: Vec<Entity> = Vec::new();
    for root in players.iter() {
        for entity in children.iter_descendants(root) {
            let Ok((spawned, mesh_map)) = wrappers.get(entity) else {
                continue;
            };
            let Some(resource) = resources.get(spawned.0.id()) else {
                continue;
            };
            let Some(attach) = &resource.attach_info else {
                continue;
            };
            if !attach.is_item {
                // the character body: its default hair + face part meshes
                let Some(map) = mesh_map else {
                    continue;
                };
                let before = targets.len();
                for (slot, mesh_idx) in &attach.slots {
                    if PORTRAIT_SLOTS.contains(slot) {
                        targets.extend(map.0.get(mesh_idx).copied());
                    }
                }
                if targets.len() == before {
                    // a body without hair/face slots — show all of it rather
                    // than an empty portrait
                    targets.extend(map.0.values().copied());
                }
            } else if attach
                .slots
                .iter()
                .any(|(slot, _)| PORTRAIT_SLOTS.contains(slot))
            {
                // a head-slot item (helmet/hat): all of its meshes
                targets.extend(
                    children
                        .iter_descendants(entity)
                        .filter(|e| mesh3d.contains(*e)),
                );
            }
        }
    }
    for entity in targets {
        if !tagged.contains(entity) {
            commands.entity(entity).insert((
                RenderLayers::from_layers(&[
                    CameraLayers::Main as usize,
                    CameraLayers::Portrait as usize,
                ]),
                PortraitTagged,
            ));
        }
    }
}

/// Show and animate the low-HP/MP warning overlays.
///
/// The threshold comes from `hud.low_vitals_caution_percent` and is `0` by
/// default, i.e. off: the original's trigger is not in the data
/// (`docs/re/ui/hud-player-mini-info.md` §9), so switching it on is an openroad
/// choice and the number is the user's, not an invented constant. Everything
/// else — the 8 frames, the 4x2 sheet, the 100 ms step, the loop — is the
/// original's own `FrameCount`/`WidthCount`/`HeightCount`/`Speed`/`EnableLoop`.
pub fn update_caution_overlays(
    time: Res<Time>,
    config: Res<ClientConfig>,
    vitals: Res<PlayerVitals>,
    mut hp_q: Query<
        (&mut Visibility, &mut ImageNode),
        (With<MiniInfoHpCaution>, Without<MiniInfoMpCaution>),
    >,
    mut mp_q: Query<
        (&mut Visibility, &mut ImageNode),
        (With<MiniInfoMpCaution>, Without<MiniInfoHpCaution>),
    >,
) {
    let threshold = config.hud.low_vitals_caution_percent as f32 / 100.0;
    let frame_rect = caution_frame(time.elapsed_secs());
    let rect = caution_frame_rect(frame_rect);

    let below = |cur: u32, max: Option<u32>| -> bool {
        if threshold <= 0.0 {
            return false;
        }
        // Without maxima there is no fraction to compare — do not guess one.
        let Some(max) = max else {
            return false;
        };
        max > 0 && (cur as f32 / max as f32) <= threshold
    };

    let hp_on = below(vitals.hp, vitals.max_hp);
    let mp_on = below(vitals.mp, vitals.max_mp);

    for (mut visibility, mut image) in hp_q.iter_mut() {
        *visibility = if hp_on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        image.rect = Some(rect);
    }
    for (mut visibility, mut image) in mp_q.iter_mut() {
        *visibility = if mp_on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        image.rect = Some(rect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hwan aura is the one thing `PlayerVitals::hwan_level` (0x30DF) can
    /// drive without inventing anything: on while the server says the player
    /// is in hwan, off otherwise. The original's per-level tiers are [U], so
    /// there is exactly one glow and no ramp (#303).
    #[test]
    fn the_hwan_aura_follows_the_servers_hwan_level() {
        assert!(!jahwan_glow_lit(0));
        for level in 1..=u8::MAX {
            assert!(
                jahwan_glow_lit(level),
                "hwan level {level} must light the aura"
            );
        }
    }

    /// The aura is authored `-4,-3,220,80`: it starts outside the 212x70 frame
    /// and is larger than it on both axes, which is what makes it an aura and
    /// not a border — and its crop is the 220x80 region of the 256x128 sheet.
    #[test]
    fn the_hwan_aura_overhangs_the_frame_on_every_side() {
        let (x, y, w, h) = JAHWAN_GLOW_RECT;
        assert!(x < 0.0 && y < 0.0);
        assert!(x + w > FRAME_W && y + h > FRAME_H);
        assert_eq!((w, h), JAHWAN_GLOW_SHEET);
        // the crop fits inside the 256x128 sheet it is taken from
        assert!(JAHWAN_GLOW_SHEET.0 <= 256.0 && JAHWAN_GLOW_SHEET.1 <= 128.0);
    }

    /// Regression (#303): before the first `CharacterStatsUpdate` (0x303D) the
    /// maximum is unknown, and the panel used to fall back to `unwrap_or(cur)`
    /// — a full bar reading `cur / cur` for a character who may be nearly
    /// dead. An unknown maximum must never render as a full bar.
    #[test]
    fn an_unknown_maximum_never_renders_a_full_bar() {
        assert_eq!(gauge_fill(120, None), 0.0);
        assert_eq!(gauge_text(120, None), "120 / ?");
        // a zero maximum is the same unknown, not "full"
        assert_eq!(gauge_fill(0, Some(0)), 0.0);
    }

    /// Once 0x303D lands, the gauge is the plain ratio again.
    #[test]
    fn a_known_maximum_fills_proportionally() {
        assert_eq!(gauge_fill(50, Some(200)), 0.25);
        assert_eq!(gauge_fill(200, Some(200)), 1.0);
        assert_eq!(gauge_text(50, Some(200)), "50 / 200");
        // the server may report cur above max (buffs drop); clamp, don't overflow
        assert_eq!(gauge_fill(300, Some(200)), 1.0);
    }

    /// The caution overlays' sheet arithmetic is the original's own and closes
    /// exactly: `ImageWidth 512 / WidthCount 4 = 128` and
    /// `ImageHeight 64 / HeightCount 2 = 32`, which is the declared rect of both
    /// ids 77 and 78.
    #[test]
    fn caution_sheet_arithmetic_closes_on_the_declared_rect() {
        let tile_w = CAUTION_SHEET_W / CAUTION_SHEET_COLS as f32;
        let tile_h = CAUTION_SHEET_H / CAUTION_SHEET_ROWS as f32;
        assert_eq!((tile_w, tile_h), (128.0, 32.0));
        assert_eq!((HP_CAUTION_RECT.2, HP_CAUTION_RECT.3), (tile_w, tile_h));
        assert_eq!((MP_CAUTION_RECT.2, MP_CAUTION_RECT.3), (tile_w, tile_h));
        assert_eq!(HP_CAUTION_RECT, (75.0, 25.0, 128.0, 32.0)); // id 77
        assert_eq!(MP_CAUTION_RECT, (75.0, 41.0, 128.0, 32.0)); // id 78
                                                                // FrameCount 8 must be exactly the 4x2 grid, with no dead tile.
        assert_eq!(CAUTION_FRAME_COUNT, CAUTION_SHEET_COLS * CAUTION_SHEET_ROWS);
    }

    /// `Speed=100` is milliseconds per frame and `EnableLoop=1` wraps; frame 4
    /// is the first tile of the second row on a 4-wide sheet.
    #[test]
    fn caution_frames_step_every_100ms_and_loop_row_major() {
        assert_eq!(caution_frame(0.0), 0);
        assert_eq!(caution_frame(0.099), 0);
        assert_eq!(caution_frame(0.1), 1);
        assert_eq!(caution_frame(0.75), 7);
        assert_eq!(caution_frame(0.8), 0); // EnableLoop=1
        assert_eq!(caution_frame(1.25), 4);

        assert_eq!(caution_frame_rect(0), Rect::new(0.0, 0.0, 128.0, 32.0));
        assert_eq!(caution_frame_rect(3), Rect::new(384.0, 0.0, 512.0, 32.0));
        assert_eq!(caution_frame_rect(4), Rect::new(0.0, 32.0, 128.0, 64.0));
        assert_eq!(caution_frame_rect(7), Rect::new(384.0, 32.0, 512.0, 64.0));
        // wraps rather than reading off the sheet
        assert_eq!(caution_frame_rect(8), caution_frame_rect(0));
    }

    /// The drawer geometry is a transcription of `ifplayerminiinfo.txt`, so pin
    /// it. The ladder is **nine** rows on a 20px pitch — issue #303's body lists
    /// only seven and drops Parry at y=225, which the backdrop's own height
    /// refutes.
    #[test]
    fn drawer_geometry_matches_ifplayerminiinfo() {
        assert_eq!(CINFO_BUTTON_RECT, (41.0, 56.0, 20.0, 20.0)); // :72  id 20
        assert_eq!(CINFO_BG_RECT, (72.0, 57.0, 135.0, 189.0)); // :585 id 21
        assert_eq!(CINFO_BG_ATLAS_RECT, (401.0, 154.0, 135.0, 189.0)); // :589-592
                                                                       // The atlas subrect is 1:1 with the declared extent.
        assert_eq!(
            (CINFO_BG_ATLAS_RECT.2, CINFO_BG_ATLAS_RECT.3),
            (CINFO_BG_RECT.2, CINFO_BG_RECT.3)
        );
        // Nine rows: the STR/INT pair at 65, then eight derived rows 85..225.
        assert_eq!(CINFO_STR_LABEL_RECT.1, 65.0);
        assert_eq!(CINFO_ROW_YS.len(), 8);
        assert_eq!(CINFO_ROW_YS[0], 85.0);
        assert_eq!(CINFO_ROW_YS[7], 225.0);
        for pair in CINFO_ROW_YS.windows(2) {
            assert_eq!(pair[1] - pair[0], 20.0, "uniform 20px pitch");
        }
        // Parry is the last row and fits: 225 + 12 <= 57 + 189.
        let last_bottom = CINFO_ROW_YS[7] + CINFO_ROW_H;
        assert!(last_bottom <= CINFO_BG_RECT.1 + CINFO_BG_RECT.3);
        // Stopping at the issue body's 7th row (y=205) would leave 29px of
        // backdrop empty below it — which is how that version refutes itself.
        assert_eq!(
            CINFO_BG_RECT.1 + CINFO_BG_RECT.3 - (CINFO_ROW_YS[6] + CINFO_ROW_H),
            29.0
        );
    }

    /// The STR/INT row is the one place the derived-row formula does not apply:
    /// its rects are narrower and its labels are centre-aligned.
    #[test]
    fn the_str_int_row_has_its_own_rects() {
        assert_eq!(CINFO_STR_LABEL_RECT, (82.0, 65.0, 15.0, 12.0));
        assert_eq!(CINFO_STR_VALUE_RECT, (100.0, 65.0, 29.0, 12.0));
        assert_eq!(CINFO_INT_LABEL_RECT, (145.0, 65.0, 23.0, 12.0));
        assert_eq!(CINFO_INT_VALUE_RECT, (170.0, 65.0, 29.0, 12.0));
        // Not the derived-row rects, which would misplace all four cells.
        assert_ne!(CINFO_STR_LABEL_RECT, cinfo_label_rect(65.0));
        assert_ne!(CINFO_STR_VALUE_RECT, cinfo_value_rect(65.0));
    }

    /// Cells are children of the backdrop, so window-space rects must be shifted
    /// by its origin or every row lands 72,57 too far.
    #[test]
    fn cells_are_placed_in_backdrop_local_space() {
        let (l, t, w, h) = drawer_local(cinfo_label_rect(CINFO_ROW_YS[0]), 1.0);
        assert_eq!((l, t), (10.0, 28.0)); // 82-72, 85-57
        assert_eq!((w, h), (CINFO_LABEL_W, CINFO_ROW_H));
    }

    /// `FontColor` is ARGB, so the label colour is an opaque pale gold rather
    /// than a 164-alpha cream.
    #[test]
    fn label_color_is_the_argb_rgb_components() {
        assert_eq!(CINFO_LABEL_COLOR, Color::srgb_u8(239, 218, 164));
    }

    /// The drawer reads the 0x303D sheet the character-info model already keeps.
    /// Balance is not in that packet and is derived from it instead (#765).
    #[test]
    fn value_cells_render_the_sheet_and_derive_balance() {
        let sheet = CharacterStatsUpdate {
            phys_attack_min: 10,
            phys_attack_max: 24,
            mag_attack_min: 7,
            mag_attack_max: 15,
            phys_defense: 33,
            mag_defense: 21,
            hit_rate: 44,
            parry_rate: 12,
            max_hp: 300,
            max_mp: 200,
            strength: 20,
            intelligence: 22,
        };
        let cell = |stat| stat_value_text(Some(&sheet), 1, stat);
        assert_eq!(cell(CinfoStat::PhyAtk), "10 ~ 24");
        assert_eq!(cell(CinfoStat::MagAtk), "7 ~ 15");
        assert_eq!(cell(CinfoStat::PhyDef), "33");
        assert_eq!(cell(CinfoStat::MagDef), "21");
        assert_eq!(cell(CinfoStat::Hit), "44");
        assert_eq!(cell(CinfoStat::Parry), "12");
        assert_eq!(cell(CinfoStat::Str), "20");
        assert_eq!(cell(CinfoStat::Int), "22");
        // balance is derived, not on the wire — see `balance_text` (#765)
        assert_eq!(cell(CinfoStat::PhyBal), balance_text(1, 20));
        assert_eq!(cell(CinfoStat::MagBal), balance_text(1, 22));
        // Before the first 0x303D there is no sheet at all.
        assert_eq!(stat_value_text(None, 1, CinfoStat::Hit), "-");
    }

    /// `refresh_stat_drawer` and `wire_cinfo_button` only ever run in-game, so
    /// run them once in an `App` to validate their params and query disjointness
    /// (a conflict here is a B0001 panic on the first frame, not a test failure).
    #[test]
    fn the_drawer_systems_have_valid_params() {
        let mut app = App::new();
        app.init_resource::<PlayerStats>()
            .init_resource::<PlayerVitals>()
            .add_systems(Update, (wire_cinfo_button, refresh_stat_drawer));
        app.update();
    }

    /// #765: the drawer's two balance cells must print exactly what the
    /// character window prints for the same sheet. They disagreed — `-` here,
    /// a percentage there — and both surfaces can be open at once.
    #[test]
    fn the_drawer_balance_cells_agree_with_the_character_window() {
        let sheet = CharacterStatsUpdate {
            phys_attack_min: 0,
            phys_attack_max: 0,
            mag_attack_min: 0,
            mag_attack_max: 0,
            phys_defense: 0,
            mag_defense: 0,
            hit_rate: 0,
            parry_rate: 0,
            max_hp: 0,
            max_mp: 0,
            strength: 20,
            intelligence: 22,
        };
        // the level is part of the answer now, so pin the agreement at several
        // of them — a drawer reading the wrong level would still match at one
        const LEVEL: u8 = 17;
        assert_eq!(
            stat_value_text(Some(&sheet), LEVEL, CinfoStat::PhyBal),
            balance_text(LEVEL as u32, sheet.strength)
        );
        assert_eq!(
            stat_value_text(Some(&sheet), LEVEL, CinfoStat::MagBal),
            balance_text(LEVEL as u32, sheet.intelligence)
        );
        assert_ne!(
            stat_value_text(Some(&sheet), 1, CinfoStat::PhyBal),
            stat_value_text(Some(&sheet), LEVEL, CinfoStat::PhyBal),
            "the reading must move with the level, or the level is being ignored"
        );
        // and it is a real value, not the old placeholder
        assert_ne!(stat_value_text(Some(&sheet), LEVEL, CinfoStat::PhyBal), "-");
        // no sheet at all still reads "-": unknown stays unknown
        assert_eq!(stat_value_text(None, LEVEL, CinfoStat::PhyBal), "-");
    }

    /// `GDR_PMI_BTN_STATUP` verbatim from `ifplayerminiinfo.txt`: `Rect=
    /// "150,3,16,16"`, `DDJ="interface\ifcommon\com_plus_button.ddj"`. Pinned
    /// because it is a transcription — a rect that drifts here is a rect nobody
    /// re-reads out of the resinfo.
    #[test]
    fn the_statup_button_matches_its_resinfo_block() {
        assert_eq!(STATUP_BUTTON_RECT, (150.0, 3.0, 16.0, 16.0));
        assert_eq!(
            PLUS_BUTTON_NORMAL,
            "media://interface/ifcommon/com_plus_button.ddj"
        );
        // the greyed state is the original's own art, not a tint we invented
        assert_eq!(
            PLUS_BUTTON_DISABLE,
            "media://interface/ifcommon/com_plus_button_disable.ddj"
        );
        // it sits inside the 212x70 frame, left of the level slot (168,7,37,15)
        let (x, y, w, h) = STATUP_BUTTON_RECT;
        assert!(
            x + w <= LEVEL_RECT.0,
            "the + must not overlap the level text"
        );
        assert!(
            y + h <= 70.0,
            "authored inside the frame, not in the drawer"
        );
    }

    /// A `+` that is almost never actionable is clutter, so an empty wallet
    /// takes the button out of the layout by default — and its appearance is
    /// then the "you have a point" cue. `hide_statup_when_empty: false`
    /// restores the original's always-drawn, greyed button.
    #[test]
    fn an_empty_wallet_hides_the_button_unless_the_original_look_is_asked_for() {
        assert_eq!(statup_display(false, true), Display::None);
        assert_eq!(statup_display(false, false), Display::Flex);
        // a point to spend always draws it, under either setting
        assert_eq!(statup_display(true, true), Display::Flex);
        assert_eq!(statup_display(true, false), Display::Flex);
    }

    /// The same param/disjointness smoke as the drawer systems: both stat-up
    /// systems touch `ImageButtonStyle` and `Commands`, and a conflict would be
    /// a first-frame B0001 panic in a live session, not a test failure.
    #[test]
    fn the_statup_systems_have_valid_params() {
        let mut app = App::new();
        // #665 fixture pairing: `TaskPoolPlugin` before `AssetPlugin`, because
        // `refresh_statup_button` loads its art by path and that reaches the
        // process-global `IoTaskPool`.
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_resource::<PlayerStats>()
        .init_resource::<CharacterInfoState>()
        .add_systems(Update, (wire_statup_button, refresh_statup_button));
        app.update();
    }
}

/// Self-registration for the player mini-info panel (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct PlayerMiniInfoPlugin;

impl Plugin for PlayerMiniInfoPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<PlayerVitals>()
            .add_systems(OnEnter(SceneState::GameWorld), spawn_mini_info)
            .add_systems(OnExit(SceneState::GameWorld), cleanup_mini_info)
            // UiTesting so the offline preview exercises the same
            // refresh/portrait systems with mock data.
            .add_systems(
                Update,
                (
                    seed_vitals_from_character_info,
                    on_entity_bars_update,
                    on_points_update,
                    on_stats_update,
                    on_hwan_action_response,
                    on_hwan_level_update,
                    attach_portrait_rig,
                    tag_player_meshes_for_portrait,
                    aim_portrait_camera,
                    freeze_portrait_camera,
                    update_jahwan_button,
                    wire_cinfo_button,
                    refresh_stat_drawer,
                    wire_statup_button,
                    refresh_statup_button,
                    update_caution_overlays,
                    update_jahwan_glow,
                    // the pet panel is a child control of this one (#303)
                    crate::plugins::hud::pet_mini_info::spawn_pet_mini_info,
                    crate::plugins::hud::pet_mini_info::refresh_pet_mini_info,
                    refresh_mini_info.run_if(mini_info_needs_refresh),
                )
                    .run_if(
                        in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting)),
                    ),
            );
    }
}
