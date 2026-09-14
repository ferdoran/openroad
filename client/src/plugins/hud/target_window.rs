//! Target info panel — the HUD window shown top-center while an entity is
//! click-selected.
//!
//! Idea: one persistent panel built from the `window_all.ddj` frame crops named
//! in `resinfo/iftargetwindow.txt`: `CIFTargetWindowCommonEnemy` (196x51) for
//! regular targets, `CIFTargetWindowSpecialMob` (196x78, same art plus a
//! translucent lower strip — reserved for buffs) for monsters with a rarity
//! class, and `CIFTargetWindowPlayer` (196x36) for players. The inner layout of
//! the enemy frames is exe-code in vanilla, so it was measured off the
//! decoded frame art: the gold ring socket top-left holds a 20x20 `tw_gem_*`
//! (level-gap gem for monsters, `tw_gem_npc`/`tw_gem_player` otherwise), the
//! dark rounded plate holds the centered name plus the close (X) button
//! (GDR_TW_CLOSE), and the thin groove below is the 168x4 HP line — live
//! [`EntityVitals`] fill for monsters (characterdata max, 0x3057 updates) and
//! the static gold `tw_hp_npc` for NPCs. The player plate carries no gauge at
//! all — none in `iftw_player.txt`, no trough in its art, and remote player HP
//! is not on the wire — so it shows none. Monsters use the taller frame, whose
//! translucent lower strip shows a centered `tw_icon_*` + class label
//! (Normal/Champion/Giant/... — the per-spawn rarity byte). Selection ends on
//! despawn, the X button, or Esc (see `toggle_system_window`'s priority).

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::{Overflow, UiTargetCamera, UiTransform, Val2};
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::cos::{interacts_as_character, CosEntity};
use crate::plugins::cursor::interactions::entity_select::SelectedEntity;
use crate::plugins::hud::game_window::scaled;
use crate::plugins::hud::gauge::{gauge, gauge_fill_width, GaugeArt};
use crate::plugins::hud::player_mini_info::PlayerVitals;
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::entities::{
    CharacterRef, DisplayName, EntityAilments, EntityVitals, MonsterRarity, RemoteBuffs,
    RemoteEntity,
};
use crate::plugins::textdata::{ClientCharacterData, ClientSkillData, ClientUiStrings};
use crate::plugins::ui_v2::style::ImageButtonStyle;
use crate::plugins::ui_v2::widgets::label;

// --- Layout (window space, uniformly scaled like the other vanilla HUDs) -----
//
// Inner rects measured on the decoded CommonEnemy art (the SpecialMob crop is
// pixel-identical for the top 51 rows, so both share these offsets).

const FRAME_W: f32 = 196.0;
/// window_all.ddj atlas crops (1024x512) from iftargetwindow.txt's UVs. All
/// three resolve from the `#else` branch's UV pairs with sub-thousandth-pixel
/// error; `iftargetwindowex.txt` carries the same four variants at the same 196
/// widths with no `#ifdef` at all, which is why this is the branch we follow.
const FRAME_COMMON: (f32, f32, f32, f32) = (543.0, 0.0, 196.0, 51.0);
const FRAME_SPECIAL: (f32, f32, f32, f32) = (543.0, 52.0, 196.0, 78.0);
/// `GDR_TW_PLAYERWND:CIFTargetWindowPlayer` (`iftargetwindow.txt:200-216`,
/// `Rect="0,0,196,36"`). A short plate: `iftw_player.txt` declares only a
/// kindred mark and a name text — **no gauge control of any kind** — and the
/// decoded art has no gauge trough, so nothing is drawn at [`BAR_RECT`] for it.
/// The declared crop's first atlas row is a transparent gutter (the art proper
/// is 543,290,196x35); the data rect is kept because that is what the original
/// samples.
const FRAME_PLAYER: (f32, f32, f32, f32) = (543.0, 289.0, 196.0, 36.0);
const WINDOW_ATLAS: &str = "media://interface/ifcommon/window_all.ddj";

/// Screen offset of the top-centered panel.
const PANEL_TOP: f32 = 6.0;
/// The gold ring socket's hole — placed so the 20x20 gem art's visible disc
/// (center (9.5, 9) in its canvas) lands on the hole's centroid (16.5, 13.5).
const GEM_RECT: (f32, f32, f32, f32) = (7.0, 4.5, 20.0, 20.0);
/// The dark rounded name plate's interior; the name row centers inside it.
const NAME_ROW: (f32, f32, f32, f32) = (30.0, 5.0, 152.0, 20.0);
/// The recessed groove's fill line — exactly the 168x4 `tw_hp.ddj` art.
const BAR_RECT: (f32, f32, f32, f32) = (14.0, 37.0, 168.0, 4.0);
/// The close (X) button, from iftargetwindow.txt's GDR_TW_CLOSE rect.
const CLOSE_RECT: (f32, f32, f32, f32) = (176.0, 9.0, 16.0, 16.0);
/// Centered `[icon] Class` row on the special frame's translucent lower strip
/// (the strip spans rows ~52..72 of the art).
const RARITY_ROW: (f32, f32, f32, f32) = (0.0, 54.0, 196.0, 16.0);
const NAME_FONT_SIZE: f32 = 10.0;
const ICON_SIZE: f32 = 16.0;

/// `GDR_TW_BUFF:CIFBuffViewer` (`resinfo/iftargetwindow.txt:6`) is
/// `Rect="0,37,0,0"` — a control the original grows at runtime. The `0,0` is
/// the data saying "sized by its contents", so this row declares **no width and
/// no slot count**: it is a flex row whose children are the live buffs, and an
/// empty buff list renders nothing at all.
///
/// The descriptor's `top = 37` is exactly [`BAR_RECT`]'s top, the HP groove's
/// own row (`docs/re/ui/hud-target-window.md:58`). resinfo carries no z-order
/// key, so which of the two draws over the other is not in the data; the row is
/// spawned after the groove and therefore covers it while a target is buffed.
/// That is the descriptor's coincidence, not a chosen offset — moving the row
/// to a clear y would be an invented constant.
const BUFF_ROW_TOP: f32 = 37.0;
/// Icons grow rightward from the panel's left inset, which is the groove's own
/// left edge — the only inset the frame art gives us ([`BAR_RECT`]`.0`).
const BUFF_ROW_LEFT: f32 = BAR_RECT.0;

const CLOSE_DDJ: &str = "media://interface/ifcommon/com_windowclose.ddj";
const CLOSE_FOCUS_DDJ: &str = "media://interface/ifcommon/com_windowclose_focus.ddj";
const CLOSE_PRESS_DDJ: &str = "media://interface/ifcommon/com_windowclose_press.ddj";

// --- Components / assets ------------------------------------------------------

// (Clone + Default: bsn! templates construct components by value.)
#[derive(Component, Clone, Default)]
pub struct TargetWindowRoot;
#[derive(Component, Clone, Default)]
pub struct TwNameText;
#[derive(Component, Clone, Default)]
pub struct TwGemIcon;
#[derive(Component, Clone, Default)]
pub struct TwRarityRow;
#[derive(Component, Clone, Default)]
pub struct TwRarityIcon;
#[derive(Component, Clone, Default)]
pub struct TwRarityText;
#[derive(Component, Clone, Default)]
pub struct TwCloseButton;
#[derive(Component, Clone, Default)]
pub struct TwBarFill;
/// The `CIFBuffViewer` row; its children are rebuilt from the target's buffs.
#[derive(Component, Clone, Default)]
pub struct TwBuffRow;

/// Pre-loaded target window art.
#[derive(Resource)]
pub struct TargetWindowAssets {
    hp: Handle<Image>,
    hp_npc: Handle<Image>,
    gem_npc: Handle<Image>,
    gem_player: Handle<Image>,
    /// weak2, weak1, normal, strong1, strong2 (level gap ascending).
    gems: [Handle<Image>; 5],
    /// normal, champion, giant, elite, titan, unique.
    icons: [Handle<Image>; 6],
}

impl TargetWindowAssets {
    fn load(asset_server: &AssetServer) -> Self {
        let tw = |name: &str| asset_server.load(format!("media://interface/targetwindow/{name}"));
        Self {
            hp: tw("tw_hp.ddj"),
            hp_npc: tw("tw_hp_npc.ddj"),
            gem_npc: tw("tw_gem_npc.ddj"),
            gem_player: tw("tw_gem_player.ddj"),
            gems: [
                tw("tw_gem_weak2.ddj"),
                tw("tw_gem_weak1.ddj"),
                tw("tw_gem_normal.ddj"),
                tw("tw_gem_strong1.ddj"),
                tw("tw_gem_strong2.ddj"),
            ],
            icons: [
                tw("tw_icon_normal.ddj"),
                tw("tw_icon_champion.ddj"),
                tw("tw_icon_giant.ddj"),
                tw("tw_icon_elite.ddj"),
                tw("tw_icon_titan.ddj"),
                tw("tw_icon_unique.ddj"),
            ],
        }
    }

    /// The gem for a `target level - player level` gap (thresholds per the
    /// vanilla behavior): ≤-9 weak2, -8..-3 weak1, -2..2 normal, 3..8 strong1,
    /// ≥9 strong2.
    fn gem(&self, diff: i32) -> &Handle<Image> {
        let index = match diff {
            i32::MIN..=-9 => 0,
            -8..=-3 => 1,
            -2..=2 => 2,
            3..=8 => 3,
            _ => 4,
        };
        &self.gems[index]
    }

    /// The rarity badge for a spawn rarity class (party bit already masked;
    /// enum per skrillax's `EntityRarity`): 1 champion, 3/8 unique, 4 giant,
    /// 5 titan, 6/7 elite.
    fn icon(&self, kind: u8) -> &Handle<Image> {
        let index = match kind {
            1 => 1,
            4 => 2,
            6 | 7 => 3,
            5 => 4,
            3 | 8 => 5,
            _ => 0,
        };
        &self.icons[index]
    }
}

/// Display label for a rarity class (masked), from the string table's six
/// `UIIT_STT_MOB_*` keys (`textuisystem.txt:693-698`). Note that the shipped
/// data itself collides: `_ELITE` (697) and `_STRONG` (698) both read "Elite",
/// so class 7 is *not* "Strong".
///
/// The `"Normal"` default and the `"Party "` prefix are openroad inventions —
/// the original ships no `UIIT_STT_MOB_NORMAL` and no party-mob key (only the
/// `tw_icon_normal.ddj` badge art exists for the former).
fn rarity_label(strings: &ClientUiStrings, kind: u8, party: bool) -> String {
    let name = match kind {
        1 => strings.get_or("UIIT_STT_MOB_CHAMPION", "Champion"),
        3 | 8 => strings.get_or("UIIT_STT_MOB_UNIQUE", "Unique"),
        4 => strings.get_or("UIIT_STT_MOB_GIANT", "Giant"),
        5 => strings.get_or("UIIT_STT_MOB_TITAN", "Titan"),
        6 => strings.get_or("UIIT_STT_MOB_ELITE", "Elite"),
        7 => strings.get_or("UIIT_STT_MOB_STRONG", "Elite"),
        _ => "Normal",
    };
    if party {
        format!("Party {name}")
    } else {
        name.to_string()
    }
}

/// The atlas crop for a target. Players get their own short plate; monsters keep
/// the rarity-strip frame when they carry a class, otherwise the plain enemy
/// frame. NPCs stay on the enemy frame: which variant the original uses for an
/// NPC is unresolved, and it is entangled with the 196-vs-236 frame-width
/// question (`docs/re/ui/hud-target-window.md` §9-U1/U3). The JobPlayer variant
/// (543,210,196x58) is byte-verified but not selectable — no job or PK-rank
/// field exists anywhere in the packet layer (§9-U2).
/// A COS takes the player plate for the same reason it takes the player's
/// interaction path: it is a character to whoever is pointing at it, and the
/// NPC plate reads as "this thing has a dialog".
fn frame_for(kind: &RemoteEntity, has_rarity: bool, is_cos: bool) -> (f32, f32, f32, f32) {
    if interacts_as_character(kind, is_cos) {
        return FRAME_PLAYER;
    }
    match kind {
        _ if has_rarity => FRAME_SPECIAL,
        _ => FRAME_COMMON,
    }
}

// --- Spawn / cleanup ----------------------------------------------------------

pub fn spawn_target_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found for the target window");
        return;
    };
    let assets = TargetWindowAssets::load(&asset_server);

    let window: Handle<Image> = asset_server.load(WINDOW_ATLAS);
    let (fx, fy, fw, fh) = FRAME_COMMON;
    let frame_crop = Rect::new(fx, fy, fx + fw, fy + fh);
    let hp_image = assets.hp.clone();
    let gem_image = assets.gems[2].clone();
    let icon_image = assets.icons[0].clone();
    let name_font = fonts.nine.clone();
    let close_n: Handle<Image> = asset_server.load(CLOSE_DDJ);
    let close_f: Handle<Image> = asset_server.load(CLOSE_FOCUS_DDJ);
    let close_p: Handle<Image> = asset_server.load(CLOSE_PRESS_DDJ);
    let close_normal = close_n.clone();

    let s = hud_scale();
    let (gem_l, gem_t, gem_w, gem_h) = scaled(GEM_RECT, s);
    let (row_l, row_t, row_w, row_h) = scaled(NAME_ROW, s);
    let (bar_l, bar_t, bar_w, bar_h) = scaled(BAR_RECT, s);
    let (close_l, close_t, close_w, close_h) = scaled(CLOSE_RECT, s);
    let (rar_l, rar_t, rar_w, rar_h) = scaled(RARITY_ROW, s);
    let (buff_l, buff_t) = (BUFF_ROW_LEFT * s, BUFF_ROW_TOP * s);
    let scene = bsn! {
        TargetWindowRoot
        Name("Target Window")
        ImageNode { image: {window}, image_mode: NodeImageMode::Stretch, rect: {Some(frame_crop)} }
        // top-centered HUD window (the -50% x transform centers it on the
        // 50% left anchor)
        Node {
            position_type: PositionType::Absolute,
            top: px(PANEL_TOP * s),
            left: percent(50),
            width: px(FRAME_W * s),
            height: px(FRAME_COMMON.3 * s),
        }
        UiTransform::from_translation(Val2::new(Val::Percent(-50.0), Val::Px(0.0)))
        // between the nameplates (10) and the HUD windows (50)
        GlobalZIndex(30)
        Visibility::Hidden
        Pickable::IGNORE
        Children [
            // the gem in the gold ring socket (level gap / npc / player)
            (
                TwGemIcon
                ImageNode { image: {gem_image}, image_mode: NodeImageMode::Stretch }
                Node {
                    position_type: PositionType::Absolute,
                    left: px(gem_l),
                    top: px(gem_t),
                    width: px(gem_w),
                    height: px(gem_h),
                }
                Pickable::IGNORE
            ),
            // centered name on the plate
            (
                Node {
                    position_type: PositionType::Absolute,
                    left: px(row_l),
                    top: px(row_t),
                    width: px(row_w),
                    height: px(row_h),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                }
                Pickable::IGNORE
                Children [
                    (
                        label("", name_font.clone(), NAME_FONT_SIZE * s)
                        TwNameText
                        TextColor(Color::WHITE)
                    ),
                ]
            ),
            // close (X) button top-right (GDR_TW_CLOSE); NOT Pickable::IGNORE
            (
                TwCloseButton
                Button
                Hovered
                ImageNode { image: {close_normal}, image_mode: NodeImageMode::Stretch }
                ImageButtonStyle { normal: {close_n}, hover: {close_f}, press: {close_p} }
                Node {
                    position_type: PositionType::Absolute,
                    left: px(close_l),
                    top: px(close_t),
                    width: px(close_w),
                    height: px(close_h),
                }
            ),
            // centered "[icon] Class" row on the special frame's lower strip
            (
                TwRarityRow
                Node {
                    position_type: PositionType::Absolute,
                    left: px(rar_l),
                    top: px(rar_t),
                    width: px(rar_w),
                    height: px(rar_h),
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    column_gap: px(3.0 * s),
                }
                Visibility::Hidden
                Pickable::IGNORE
                Children [
                    (
                        TwRarityIcon
                        ImageNode { image: {icon_image}, image_mode: NodeImageMode::Stretch }
                        Node { width: px(ICON_SIZE * s), height: px(ICON_SIZE * s) }
                        Pickable::IGNORE
                    ),
                    (
                        label("", name_font, NAME_FONT_SIZE * s)
                        TwRarityText
                        TextColor(Color::WHITE)
                    ),
                ]
            ),
            // GDR_TW_BUFF: no width, no slot count — the row is whatever its
            // children are (see BUFF_ROW_TOP)
            (
                TwBuffRow
                Node {
                    position_type: PositionType::Absolute,
                    left: px(buff_l),
                    top: px(buff_t),
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::FlexStart,
                }
                Pickable::IGNORE
            ),
            // HP line: track sized to the groove -> `gauge()` crop -> art at
            // its native size, so `tw_hp.ddj`'s 1-texel rounded corners stay
            // on the bar's ends instead of riding the fill front (#630).
            (
                Node {
                    position_type: PositionType::Absolute,
                    left: px(bar_l),
                    top: px(bar_t),
                    width: px(bar_w),
                    height: px(bar_h),
                    overflow: {Overflow::clip()},
                }
                Pickable::IGNORE
                Children [
                    (
                        TwBarFill
                        gauge(hp_image, bar_w, bar_h)
                    ),
                ]
            ),
        ]
    };
    commands.spawn_scene(scene).insert(UiTargetCamera(camera));
    commands.insert_resource(assets);
}

/// Attach the close behavior to a freshly spawned X button (`spawn_scene`
/// defers entity creation, so the observer is wired one frame later).
pub fn wire_close_button(fresh: Query<Entity, Added<TwCloseButton>>, mut commands: Commands) {
    for entity in fresh.iter() {
        commands
            .entity(entity)
            .observe(|_: On<Activate>, mut selected: ResMut<SelectedEntity>| {
                selected.0 = None;
            });
    }
}

pub fn cleanup_target_window(mut commands: Commands, roots: Query<Entity, With<TargetWindowRoot>>) {
    for entity in roots.iter() {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<TargetWindowAssets>();
}

// --- Update -------------------------------------------------------------------

/// Drive the panel from the selection: shown while a target exists, content
/// (frame variant, name, gem, badge, bar fill) refreshed in place. All writes
/// compare first, so a still target costs a handful of equality checks.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn update_target_window(
    selected: Res<SelectedEntity>,
    assets: Option<Res<TargetWindowAssets>>,
    player_vitals: Res<PlayerVitals>,
    char_data: Option<Res<ClientCharacterData>>,
    ui_strings: Res<ClientUiStrings>,
    targets: Query<(
        &DisplayName,
        &RemoteEntity,
        Option<&MonsterRarity>,
        Option<&EntityVitals>,
        Option<&CharacterRef>,
        Has<CosEntity>,
    )>,
    mut root: Query<
        (&mut Node, &mut ImageNode, &mut Visibility),
        (With<TargetWindowRoot>, Without<TwBarFill>),
    >,
    mut texts: Query<&mut Text, (With<TwNameText>, Without<TwRarityText>)>,
    // The fill is the crop node now (#630): it carries the width and the
    // visibility, while the art it crops lives on its `GaugeArt` child.
    mut fills: Query<
        (&mut Node, &Children, &mut Visibility),
        (
            With<TwBarFill>,
            Without<TargetWindowRoot>,
            Without<TwRarityRow>,
        ),
    >,
    mut bar_art: Query<
        &mut ImageNode,
        (
            With<GaugeArt>,
            Without<TargetWindowRoot>,
            Without<TwGemIcon>,
            Without<TwRarityIcon>,
        ),
    >,
    mut gems: Query<
        &mut ImageNode,
        (
            With<TwGemIcon>,
            Without<TargetWindowRoot>,
            Without<TwBarFill>,
            Without<TwRarityIcon>,
        ),
    >,
    mut rarity_rows: Query<
        &mut Visibility,
        (
            With<TwRarityRow>,
            Without<TargetWindowRoot>,
            Without<TwBarFill>,
        ),
    >,
    mut rarity_icons: Query<
        &mut ImageNode,
        (
            With<TwRarityIcon>,
            Without<TargetWindowRoot>,
            Without<TwBarFill>,
            Without<TwGemIcon>,
        ),
    >,
    mut rarity_texts: Query<&mut Text, With<TwRarityText>>,
) {
    let Ok((mut root_node, mut frame, mut visibility)) = root.single_mut() else {
        return;
    };
    let Some(assets) = assets else {
        return;
    };

    let hide = |visibility: &mut Visibility| {
        if *visibility != Visibility::Hidden {
            *visibility = Visibility::Hidden;
        }
    };

    let Some((name, kind, rarity, vitals, char_ref, is_cos)) =
        selected.0.and_then(|entity| targets.get(entity).ok())
    else {
        hide(&mut visibility);
        return;
    };

    // --- Frame variant: monsters with a class get the taller strip frame,
    // players their own short plate; inner offsets are shared, so only the crop
    // and the height change ---
    let rarity = rarity.filter(|_| matches!(kind, RemoteEntity::Monster));
    let (fx, fy, fw, fh) = frame_for(kind, rarity.is_some(), is_cos);
    let crop = Rect::new(fx, fy, fx + fw, fy + fh);
    if frame.rect != Some(crop) {
        frame.rect = Some(crop);
    }
    let height = Val::Px(fh * hud_scale());
    if root_node.height != height {
        root_node.height = height;
    }

    // --- Name ---
    if let Ok(mut text) = texts.single_mut() {
        if text.0 != name.0 {
            text.0.clone_from(&name.0);
        }
    }

    // --- Ring gem: level gap for monsters, kind gem for NPCs/players ---
    let target_level = char_ref
        .and_then(|r| char_data.as_ref()?.get(&(r.0 as i32))?.level())
        .filter(|_| matches!(kind, RemoteEntity::Monster));
    if let Ok(mut gem) = gems.single_mut() {
        let wanted = match kind {
            // A COS wears the player gem for the same reason it wears the
            // player plate: the NPC gem announces a dialog it does not have.
            _ if interacts_as_character(kind, is_cos) => &assets.gem_player,
            RemoteEntity::Npc => &assets.gem_npc,
            _ => target_level.map_or(&assets.gems[2], |level| {
                assets.gem(level as i32 - player_vitals.level as i32)
            }),
        };
        if gem.image != *wanted {
            gem.image = wanted.clone();
        }
    }

    // --- Class strip: "[icon] Normal/Champion/Giant/..." (monsters only) ---
    if let Ok(mut row_visibility) = rarity_rows.single_mut() {
        match rarity {
            Some(r) => {
                if let Ok(mut icon) = rarity_icons.single_mut() {
                    let wanted = assets.icon(r.kind());
                    if icon.image != *wanted {
                        icon.image = wanted.clone();
                    }
                }
                if let Ok(mut text) = rarity_texts.single_mut() {
                    let label = rarity_label(&ui_strings, r.kind(), r.party());
                    if text.0 != label {
                        text.0 = label;
                    }
                }
                if *row_visibility != Visibility::Inherited {
                    *row_visibility = Visibility::Inherited;
                }
            }
            None => hide(&mut row_visibility),
        }
    }

    // --- HP line ---
    if let Ok((mut fill_node, fill_children, mut fill_visibility)) = fills.single_mut() {
        // The player plate has no gauge at all — no gauge control in
        // `iftw_player.txt`, no trough in the art — and remote player HP is not
        // on the wire either, so it is hidden rather than drawn full.
        //
        // A COS follows, and that is forced rather than chosen: its HP *is* on
        // the wire, but `the_player_plate_is_too_short_for_the_hp_groove` pins
        // the geometry — the groove ends at y=41 and the player plate is 36
        // tall, so a gauge on that plate would hang off the bottom of the
        // frame. Taking the player plate means taking its silence about HP.
        if interacts_as_character(kind, is_cos) {
            hide(&mut fill_visibility);
        } else {
            let art = if matches!(kind, RemoteEntity::Npc) {
                &assets.hp_npc
            } else {
                &assets.hp
            };
            for child in fill_children.iter() {
                if let Ok(mut fill_image) = bar_art.get_mut(child) {
                    if fill_image.image != *art {
                        fill_image.image = art.clone();
                    }
                }
            }
            // Monsters have live vitals; NPCs are static and show a full bar.
            let fill = vitals.map_or(1.0, EntityVitals::fill);
            let width = gauge_fill_width(fill, BAR_RECT.2 * hud_scale());
            if fill_node.width != width {
                fill_node.width = width;
            }
            if *fill_visibility != Visibility::Inherited {
                *fill_visibility = Visibility::Inherited;
            }
        }
    }

    if *visibility != Visibility::Inherited {
        *visibility = Visibility::Inherited;
    }
}

/// The icons a buff list renders, in wire order. Split out from the system so
/// the "no cap, no padding" rule of the descriptor's `0,0` size is testable:
/// the result is exactly as long as the ids that resolve to an icon.
fn buff_icons(ids: &[u32], icon_for: impl Fn(u32) -> Option<String>) -> Vec<String> {
    ids.iter().filter_map(|id| icon_for(*id)).collect()
}

/// Fill the `CIFBuffViewer` row from the selected target's [`RemoteBuffs`].
///
/// The row is rebuilt only when the id list changes (the same `Local`
/// signature the magic state board uses), and it is built from the list alone:
/// no slot count, no maximum, no placeholder — the descriptor's `0,0` size is
/// the data refusing to name a limit. A buff whose skilldata row supplies no
/// icon contributes no node, so nothing is drawn that we cannot source.
pub fn update_target_buff_row(
    selected: Res<SelectedEntity>,
    skill_data: Option<Res<ClientSkillData>>,
    asset_server: Res<AssetServer>,
    targets: Query<&RemoteBuffs>,
    ailing: Query<&EntityAilments>,
    rows: Query<Entity, With<TwBuffRow>>,
    mut shown: Local<(Vec<u32>, u32)>,
    mut commands: Commands,
) {
    let Ok(row) = rows.single() else {
        return;
    };
    let wanted: Vec<u32> = selected
        .0
        .and_then(|entity| targets.get(entity).ok())
        .map(|buffs| buffs.0.clone())
        .unwrap_or_default();
    // Debuffs come off the 0x3057 bad-status mask, the only live per-entity
    // ailment source (`RemoteBuffs` is the spawn record's skill buffs and is
    // never updated after spawn).
    let ailments = selected
        .0
        .and_then(|entity| ailing.get(entity).ok())
        .map(|a| a.0)
        .unwrap_or_default();
    if shown.0 == wanted && shown.1 == ailments.0 {
        return;
    }

    let side = Val::Px(ICON_SIZE * hud_scale());
    let icons = buff_icons(&wanted, |id| {
        skill_data
            .as_ref()
            .and_then(|data| data.get(&(id as i32)))
            .and_then(|row| row.icon_path())
    });
    // buffs first, then debuffs — the authored board keeps BLESS above CURSE
    // (`hud/magic_state_board.rs`), so the same order reads consistently here
    let icons: Vec<String> = icons
        .into_iter()
        .chain(ailments.ailments().map(|a| a.icon_path()))
        .collect();
    commands
        .entity(row)
        .despawn_related::<Children>()
        .with_children(|row| {
            for icon in icons {
                row.spawn((
                    Node {
                        width: side,
                        height: side,
                        ..default()
                    },
                    ImageNode {
                        image: asset_server.load(icon),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            }
        });
    *shown = (wanted, ailments.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame crops are transcriptions of `iftargetwindow.txt`'s UVs against
    /// the 1024x512 `window_all.ddj` atlas. The Player crop was byte-verified
    /// but never used before #309, so pin all three.
    #[test]
    fn frame_crops_match_the_atlas_uvs() {
        assert_eq!(FRAME_COMMON, (543.0, 0.0, 196.0, 51.0)); // :152 CommonEnemy
        assert_eq!(FRAME_SPECIAL, (543.0, 52.0, 196.0, 78.0)); // :171 SpecialMob
        assert_eq!(FRAME_PLAYER, (543.0, 289.0, 196.0, 36.0)); // :209 Player
                                                               // All of them share the atlas column and the frame width.
        for frame in [FRAME_COMMON, FRAME_SPECIAL, FRAME_PLAYER] {
            assert_eq!(frame.0, 543.0);
            assert_eq!(frame.2, FRAME_W);
        }
    }

    /// Every player and every NPC used to render with the monster frame, because
    /// the selection only looked at whether a rarity component was present.
    #[test]
    fn players_get_the_player_plate_and_monsters_keep_theirs() {
        assert_eq!(frame_for(&RemoteEntity::Player, false, false), FRAME_PLAYER);
        // Rarity is attached to every spawn from a characterdata fallback, so a
        // player carrying one must still not get the monster strip frame.
        assert_eq!(frame_for(&RemoteEntity::Player, true, false), FRAME_PLAYER);
        assert_eq!(
            frame_for(&RemoteEntity::Monster, true, false),
            FRAME_SPECIAL
        );
        assert_eq!(
            frame_for(&RemoteEntity::Monster, false, false),
            FRAME_COMMON
        );
        // NPCs stay on the enemy frame pending §9-U1/U3...
        assert_eq!(frame_for(&RemoteEntity::Npc, false, false), FRAME_COMMON);
        // ...but a COS spawns as an NPC and is a character to the player, so
        // it takes the player plate.
        assert_eq!(frame_for(&RemoteEntity::Npc, false, true), FRAME_PLAYER);
        assert_eq!(frame_for(&RemoteEntity::Npc, true, true), FRAME_PLAYER);
    }

    /// The data reason the bar is hidden for players: the HP groove ends at y=41,
    /// past the bottom of the 36px-tall player plate.
    #[test]
    fn the_player_plate_is_too_short_for_the_hp_groove() {
        assert!(BAR_RECT.1 + BAR_RECT.3 > FRAME_PLAYER.3);
        assert!(BAR_RECT.1 + BAR_RECT.3 <= FRAME_COMMON.3);
    }

    /// `UIIT_STT_MOB_STRONG` (class 7) reads "Elite" in the shipped data, the
    /// same as `_ELITE` (class 6). We printed "Strong", which no key supplies.
    #[test]
    fn rarity_labels_follow_the_string_table() {
        let strings = ClientUiStrings::default(); // no table loaded -> fallbacks
        assert_eq!(rarity_label(&strings, 6, false), "Elite");
        assert_eq!(rarity_label(&strings, 7, false), "Elite");
        assert_eq!(rarity_label(&strings, 1, false), "Champion");
        assert_eq!(rarity_label(&strings, 3, false), "Unique");
        assert_eq!(rarity_label(&strings, 8, false), "Unique");
        assert_eq!(rarity_label(&strings, 4, false), "Giant");
        assert_eq!(rarity_label(&strings, 5, false), "Titan");
        // Both of these are openroad inventions - no vanilla key exists.
        assert_eq!(rarity_label(&strings, 0, false), "Normal");
        assert_eq!(rarity_label(&strings, 1, true), "Party Champion");
    }

    /// `GDR_TW_BUFF` is `Rect="0,37,0,0"` (`iftargetwindow.txt:6`): the row's
    /// top is the descriptor's, and the `0,0` means there is no authored size
    /// to transcribe — so there must be no width and no slot-count constant in
    /// this module at all.
    #[test]
    fn the_buff_row_takes_its_top_from_the_descriptor_and_declares_no_size() {
        assert_eq!(BUFF_ROW_TOP, 37.0);
        // the descriptor's coincidence with the HP groove, recorded not smoothed
        assert_eq!(BUFF_ROW_TOP, BAR_RECT.1);
        // and it starts at the only inset the frame art gives us
        assert_eq!(BUFF_ROW_LEFT, BAR_RECT.0);
    }

    /// The row is exactly as long as the buff list: no cap, no padding, and no
    /// node for a buff whose skilldata row supplies no icon.
    #[test]
    fn buff_icons_follow_the_list_length_with_no_cap_and_no_padding() {
        let lookup = |id: u32| (id != 7).then(|| format!("media://icon/{id}.ddj"));
        assert!(buff_icons(&[], lookup).is_empty());
        assert_eq!(buff_icons(&[1, 2, 3], lookup).len(), 3);
        // the unresolvable id contributes nothing rather than an empty slot
        assert_eq!(buff_icons(&[1, 7, 3], lookup).len(), 2);
        // 64 buffs are 64 icons — a slot count would show up here
        let many: Vec<u32> = (100..164).collect();
        assert_eq!(buff_icons(&many, lookup).len(), many.len());
        assert_eq!(buff_icons(&[5], lookup)[0], "media://icon/5.ddj");
    }

    /// The row is a pure function of the buff list: no buffs, no children —
    /// not an empty trough. Driving the real system in an `App` also proves the
    /// new query set is disjoint (a B0001 panic no helper test would catch).
    #[test]
    fn the_buff_row_system_runs_and_renders_nothing_without_buffs() {
        let mut app = App::new();
        // TaskPoolPlugin BEFORE AssetPlugin: the system reaches
        // `asset_server.load()` as soon as a target has a resolvable buff icon,
        // and that path touches the IoTaskPool. The repo's existing
        // `AssetPlugin`-only app tests (nav/, net/entities.rs) are green only
        // because they never load by path — a model is a model only when it
        // exercises the same predicate.
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_resource::<SelectedEntity>()
        .add_systems(Update, update_target_buff_row);
        let row = app.world_mut().spawn(TwBuffRow).id();
        app.update();
        assert!(app.world().entity(row).get::<Children>().is_none());
    }

    /// `update_target_window` now writes `Visibility` through three queries that
    /// Bevy only proves disjoint at system-init time, so run it once in an `App`:
    /// a missing `Without` is a B0001 panic on the first frame in `GameWorld`,
    /// which no unit test on the helpers would catch.
    #[test]
    fn the_update_system_has_no_conflicting_queries() {
        let mut app = App::new();
        app.init_resource::<SelectedEntity>()
            .init_resource::<PlayerVitals>()
            .init_resource::<ClientUiStrings>()
            .add_systems(Update, update_target_window);
        app.update();
    }
}

/// Self-registration for the target window (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct TargetWindowPlugin;

impl Plugin for TargetWindowPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.add_systems(OnEnter(SceneState::GameWorld), spawn_target_window)
            .add_systems(OnExit(SceneState::GameWorld), cleanup_target_window)
            .add_systems(
                Update,
                (
                    wire_close_button,
                    update_target_window,
                    update_target_buff_row,
                )
                    .run_if(super::hud_scenes),
            );
    }
}
