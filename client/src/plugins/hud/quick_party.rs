//! The quick-party board — the compact member list under the mini-info.
//!
//! Idea: this is the surface people actually mean by "the party list". Vanilla
//! declares it as `GDR_QUICKPARTYBOARD:CIFQuickPartyWnd` (`ginterface.txt:683`)
//! at screen `4,137` — directly beneath the mini-info panel at `4,7` — with
//! seven `CIFQuickPartySlot` rows at a 44 px pitch. Seven slots against a party
//! of eight is the board's whole reading: it lists **the others**, because your
//! own bars are the panel above it (`party::model::quick_party_rows`).
//!
//! Note what this is NOT. `ifpartymemberviewer.txt` looks like the obvious file
//! for "the member list" and is dead: it is the only 1 of 247 resinfo files
//! still using the legacy bracketed grammar (our `InterfaceTextLoader` rejects
//! it outright), and `:CIFPartyMemberViewer` has zero instantiation sites
//! anywhere in the corpus. It survives only as a second source for party
//! max = 8. This board is the live surface.
//!
//! Two art facts drive the layout and neither is guessable from the rects:
//!
//! - the slot background is a **pixel-exact atlas subrect**, not its own file.
//!   All seven slots carry the same UV quad against `ifcommon/window_all.ddj`
//!   (1024x512), and it resolves to `(741, 71, 122, 40)` — exactly the declared
//!   rect. So the background is cropped out of the atlas the mini-info already
//!   loads, not stretched from a sprite.
//! - `GDR_QPS_STATUS` shares the portrait's rect (`6,6,28,28`) because it is an
//!   **overlay tint**, not a second picture. Decoding the orphaned art settles
//!   what the family is: `qpt_face.ddj` is an opaque black disc (the backdrop),
//!   `qpt_face_warning` a translucent red disc, and `qpt_face_faraway{,_50..100}`
//!   an opacity ladder of one blue disc. None of them is a face.
//!
//! What the ladder's `_50..100` steps *mean* stays UNKNOWN, so this draws the
//! un-numbered `qpt_face_faraway.ddj` for "not in range" and never picks a
//! rung: the numbering suggests distance, but inventing a distance-to-opacity
//! mapping would be exactly the unsourced magic number ADR 0009 forbids.
//! "In range" is derived, not guessed — a member is in range when an entity
//! with their name is spawned near us.
//!
//! Deliberately not built here: the per-slot `CIFBuffViewer` (one of only three
//! in the corpus, and its own widget), and the `qpt_grope_select` selection
//! frame, which needs a party-member-to-target join this board does not own
//! yet. The board also carries no level and no masteries — see `refresh`.

use bevy::ecs::relationship::RelatedSpawnerCommands;
use bevy::picking::hover::Hovered;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::config::hud::PartyPortraitSource;
use crate::plugins::config::ClientConfig;
use crate::plugins::hud::context_menu::{
    close_context_menus, spawn_context_menu, ContextMenuItem, ContextMenuOwner, ContextMenuRoot,
    ContextMenuRow,
};
use crate::plugins::hud::flipbook::Flipbook;
use crate::plugins::hud::game_window::abs_node;
use crate::plugins::hud::gauge::{gauge_art_node, gauge_crop_node, gauge_fill_width};
use crate::plugins::hud::party::model::{
    member_vitals, quick_party_rows, we_lead, QUICK_PARTY_ROWS,
};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::entities::{DisplayName, EntityVitals, RemoteEntity};
use crate::plugins::net::party::{PartyAction, PartyRoster};
use crate::plugins::player::Player;
use crate::plugins::textdata::{ClientCharacterData, ClientUiStrings};
use crate::plugins::ui_v2::style::ImageButtonStyle;

// --- Layout (vanilla screen/row space) --------------------------------------

/// `ginterface.txt:683` — the board's screen anchor.
const BOARD_LEFT: f32 = 4.0;
const BOARD_TOP: f32 = 137.0;
/// Seven slots at `0,{0,44,...,264},122,40` — pitch 44, height 40, so a 4 px gap.
const SLOT_W: f32 = 122.0;
const SLOT_H: f32 = 40.0;
const SLOT_PITCH: f32 = 44.0;

const ATLAS: &str = "media://interface/ifcommon/window_all.ddj";
/// The slot background's atlas subrect, resolved from the UV quad all seven
/// slots share: `UV_LT 0.723633,0.138672` / `UV_RB 0.842773,0.216797` against
/// 1024x512 gives x 741..863 and y 71..111 — a 122x40 span, byte-for-byte the
/// declared rect.
const SLOT_ATLAS_RECT: (f32, f32, f32, f32) = (741.0, 71.0, 122.0, 40.0);

const ART_QUICK: &str = "media://interface/quickparty/";
const ART_COMMON: &str = "media://interface/ifcommon/";
const ART_PMI: &str = "media://interface/playerminiinfo/";

/// Slot children, row-local (`ifquickpartyslot.txt`).
const Q_PORTRAIT: (f32, f32, f32, f32) = (6.0, 6.0, 28.0, 28.0);
/// Same rect as the portrait — it is an overlay, not a second picture.
const Q_STATUS: (f32, f32, f32, f32) = (6.0, 6.0, 28.0, 28.0);
/// Negative x on purpose: the mark bleeds off the slot's left edge.
const Q_RACE: (f32, f32, f32, f32) = (-2.0, 2.0, 16.0, 16.0);
const Q_CROWN: (f32, f32, f32, f32) = (30.0, 6.0, 12.0, 12.0);
const Q_NAME: (f32, f32, f32, f32) = (43.0, 5.0, 73.0, 12.0);
const Q_HP: (f32, f32, f32, f32) = (42.0, 24.0, 76.0, 4.0);
const Q_MP: (f32, f32, f32, f32) = (42.0, 30.0, 76.0, 4.0);
const Q_EFFECT_HP: (f32, f32, f32, f32) = (54.0, 24.0, 64.0, 32.0);
const Q_EFFECT_MP: (f32, f32, f32, f32) = (54.0, 30.0, 64.0, 32.0);

/// `GDR_QPB_LOOK` is `Rect=0,0,0,0` with an art *stem*, so it is art-sized at
/// the board origin, overlapping slot 0.
const LOCK_RECT: (f32, f32, f32, f32) = (0.0, 0.0, 20.0, 20.0);

/// The quick board's caution sheets are **half the width** of the mini-info's
/// (`pmi_*_quick_effect_caution.ddj` is 256x64, the `_cha_` pair 512x64) while
/// declaring the same 8 frames on the same 4x2 grid at the same 100 ms. The
/// tile therefore comes out 64x32, exactly the declared rect — which is the
/// check that the transcription is right.
const CAUTION: Flipbook = Flipbook {
    frame_count: 8,
    cols: 4,
    rows: 2,
    frame_ms: 100.0,
    sheet: (256.0, 64.0),
};

// --- State ------------------------------------------------------------------

/// The board's own state. Deliberately **not** persisted: `GDR_QUICKPARTYBOARD`
/// is absent from vanilla's `wndpos` array, so the original does not remember
/// where it was moved to either.
#[derive(Resource, Default, Debug)]
pub struct QuickPartyState {
    /// `GDR_QPB_LOOK` — while locked the board cannot be dragged.
    pub locked: bool,
    /// The member whose context menu is open, if any.
    pub context_member: Option<u32>,
}

#[derive(Component)]
pub struct QuickPartyRoot;
#[derive(Component)]
pub struct QuickPartySlot(pub usize);
#[derive(Component)]
pub struct QuickName(pub usize);
#[derive(Component)]
pub struct QuickPortrait(pub usize);
#[derive(Component)]
pub struct QuickStatus(pub usize);
#[derive(Component)]
pub struct QuickRaceMark(pub usize);
#[derive(Component)]
pub struct QuickCrown(pub usize);
#[derive(Component)]
pub struct QuickHpFill(pub usize);
#[derive(Component)]
pub struct QuickMpFill(pub usize);
/// `true` = the HP overlay, `false` = the MP one.
#[derive(Component)]
pub struct QuickCaution(pub usize, pub bool);
#[derive(Component)]
pub struct QuickLockButton;

/// One drawn slot, resolved once per refresh.
#[derive(Default, Clone)]
struct SlotView {
    member_id: Option<u32>,
    name: String,
    hp: f32,
    mp: f32,
    portrait: Option<String>,
    european: bool,
    crown: bool,
    /// No entity with this member's name is spawned near us.
    far_away: bool,
    low_hp: bool,
    low_mp: bool,
}

#[derive(Resource, Default)]
pub struct QuickPartyViews(Vec<Option<SlotView>>);

// --- Spawning ---------------------------------------------------------------

pub fn spawn_quick_party_board(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Ok(camera) = cam_query.single() else {
        warn!("quick party: no 2d camera to attach to");
        return;
    };
    let s = hud_scale();
    let atlas: Handle<Image> = asset_server.load(ATLAS);

    let root = commands
        .spawn((
            QuickPartyRoot,
            Name::from("Quick Party Board"),
            GlobalZIndex(50),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(BOARD_LEFT * s),
                top: Val::Px(BOARD_TOP * s),
                width: Val::Px(SLOT_W * s),
                height: Val::Px(QUICK_PARTY_ROWS as f32 * SLOT_PITCH * s),
                ..default()
            },
            UiTargetCamera(camera),
        ))
        .id();

    commands.entity(root).with_children(|board| {
        for row in 0..QUICK_PARTY_ROWS {
            let y = row as f32 * SLOT_PITCH;
            board
                .spawn((
                    QuickPartySlot(row),
                    abs_node((0.0, y, SLOT_W, SLOT_H), s),
                    ImageNode {
                        image: atlas.clone(),
                        image_mode: NodeImageMode::Stretch,
                        rect: Some(atlas_rect(SLOT_ATLAS_RECT)),
                        ..default()
                    },
                    Visibility::Hidden,
                ))
                .observe(on_slot_click)
                .with_children(|slot| {
                    spawn_slot_children(slot, &asset_server, &fonts, row, s);
                });
        }

        // The lock button sits at the board origin, overlapping slot 0 — its
        // `Rect=0,0,0,0` means art-sized, and the base art is 20x20. The two
        // `_disable` variants are 24x24, which is why the size is stated from
        // the base rather than taken from whichever file loads first.
        board
            .spawn((
                QuickLockButton,
                Button,
                Hovered::default(),
                ImageButtonStyle {
                    normal: asset_server.load(format!("{ART_COMMON}quickparty_move_unlock.ddj")),
                    hover: asset_server
                        .load(format!("{ART_COMMON}quickparty_move_unlock_focus.ddj")),
                    press: asset_server
                        .load(format!("{ART_COMMON}quickparty_move_unlock_press.ddj")),
                    ..Default::default()
                },
                ImageNode {
                    image: asset_server.load(format!("{ART_COMMON}quickparty_move_unlock.ddj")),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                abs_node(LOCK_RECT, s),
            ))
            .observe(on_lock_button);
    });
}

fn spawn_slot_children(
    slot: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    row: usize,
    s: f32,
) {
    let img = |rect: (f32, f32, f32, f32), path: String| {
        (
            abs_node(rect, s),
            ImageNode {
                image: asset_server.load(path),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        )
    };

    slot.spawn((
        QuickPortrait(row),
        img(Q_PORTRAIT, format!("{ART_QUICK}qpt_face.ddj")),
    ));
    slot.spawn((
        QuickStatus(row),
        img(Q_STATUS, format!("{ART_QUICK}qpt_face_faraway.ddj")),
        Visibility::Hidden,
    ));
    slot.spawn((
        QuickRaceMark(row),
        img(Q_RACE, format!("{ART_COMMON}com_kindred_china16.ddj")),
    ));
    slot.spawn((
        QuickCrown(row),
        img(Q_CROWN, format!("{ART_COMMON}com_pt_leader.ddj")),
        Visibility::Hidden,
    ));
    slot.spawn((
        QuickName(row),
        Text::new(""),
        TextFont {
            font: fonts.two.clone().into(),
            font_size: FontSize::Px(8.0 * s),
            ..default()
        },
        TextColor(Color::WHITE),
        abs_node(Q_NAME, s),
        Pickable::IGNORE,
    ));

    for (marker_hp, rect, art) in [
        (true, Q_HP, format!("{ART_QUICK}qpt_hp.ddj")),
        (false, Q_MP, format!("{ART_QUICK}qpt_mp.ddj")),
    ] {
        let (w, h) = (rect.2 * s, rect.3 * s);
        let mut track = abs_node(rect, s);
        track.overflow = Overflow::clip();
        slot.spawn((track, Pickable::IGNORE))
            .with_children(|track| {
                let crop = (
                    gauge_crop_node(gauge_fill_width(1.0, w), h),
                    Pickable::IGNORE,
                );
                let mut entity = if marker_hp {
                    track.spawn((QuickHpFill(row), crop))
                } else {
                    track.spawn((QuickMpFill(row), crop))
                };
                entity.with_children(|crop| {
                    crop.spawn((
                        ImageNode {
                            image: asset_server.load(art.clone()),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                        gauge_art_node(w, h),
                        Pickable::IGNORE,
                    ));
                });
            });
    }

    for (is_hp, rect, art) in [
        (
            true,
            Q_EFFECT_HP,
            format!("{ART_PMI}pmi_hp_quick_effect_caution.ddj"),
        ),
        (
            false,
            Q_EFFECT_MP,
            format!("{ART_PMI}pmi_mp_quick_effect_caution.ddj"),
        ),
    ] {
        slot.spawn((
            QuickCaution(row, is_hp),
            abs_node(rect, s),
            ImageNode {
                image: asset_server.load(art),
                image_mode: NodeImageMode::Stretch,
                rect: Some(CAUTION.frame_rect(0)),
                ..default()
            },
            Visibility::Hidden,
            Pickable::IGNORE,
        ));
    }
}

/// An atlas subrect as a `Rect`, so the four-tuple stays in the same shape
/// every other rect in this file uses.
fn atlas_rect(rect: (f32, f32, f32, f32)) -> Rect {
    Rect::new(rect.0, rect.1, rect.0 + rect.2, rect.1 + rect.3)
}

pub fn cleanup_quick_party_board(
    mut commands: Commands,
    roots: Query<Entity, With<QuickPartyRoot>>,
) {
    for root in roots.iter() {
        commands.entity(root).despawn();
    }
}

// --- Refresh ----------------------------------------------------------------

/// Resolve the roster into one view per slot.
///
/// The board shows exactly what vanilla's slot template declares: portrait,
/// status tint, race mark, crown, name and the two bars. It carries **no level
/// and no guild** — there is no control for either in the 11-block template,
/// and at 122x40 with the name row 12 px tall there is nowhere to put them that
/// does not cover something. The roster page (P) is where those live.
pub fn compute_quick_party_views(
    roster: Option<Res<PartyRoster>>,
    config: Res<ClientConfig>,
    char_data: Res<ClientCharacterData>,
    local: Query<&DisplayName, With<Player>>,
    nearby: Query<(&DisplayName, Option<&EntityVitals>), (With<RemoteEntity>, Without<Player>)>,
    mut views: ResMut<QuickPartyViews>,
) {
    let Some(roster) = roster else {
        views.0.clear();
        return;
    };
    let settings = &config.hud.party;
    let caution = config.hud.low_vitals_caution_percent as f32 / 100.0;
    let local_name = local.single().ok().map(|name| name.0.as_str());

    // Index the spawned entities by name once. The lookup below used to scan
    // every `RemoteEntity`'s `DisplayName` with string equality *per member*, so
    // a full party in a crowded town paid members x entities string compares
    // every frame — this system has no needs-refresh gate because the smooth HP
    // bars have to stay live, which makes the inner loop's shape the thing that
    // matters.
    let spawned_by_name: HashMap<&str, Option<&EntityVitals>> = nearby
        .iter()
        .map(|(display, vitals)| (display.0.as_str(), vitals))
        .collect();

    views.0 = quick_party_rows(&roster, local_name)
        .iter()
        .map(|member| {
            member.map(|member| {
                let name = member.name.clone().unwrap_or_default();
                // The party wire and the entity stream share no id: a member's
                // `member_id` is a JID and a spawn is keyed by its network id,
                // so the only join available is the name — the same one the
                // minimap's party signs already use.
                let spawned = spawned_by_name.get(name.as_str()).copied();
                let precise_hp = spawned
                    .flatten()
                    .map(|vitals| vitals.fill())
                    .filter(|_| settings.smooth_vitals);
                let (hp, mp) = member_vitals(member, precise_hp, settings.smooth_vitals);
                let row_char = member.model_id.and_then(|id| char_data.get(&(id as i32)));
                SlotView {
                    member_id: member.member_id,
                    crown: member.member_id.is_some_and(|id| roster.is_leader(id)),
                    hp,
                    mp,
                    // `low_vitals_caution_percent` defaults to 0, i.e. off:
                    // the original ships the animation but no threshold, so a
                    // baked one would be an invented number driving a visible
                    // effect. The same knob already gates the mini-info's pair.
                    low_hp: caution > 0.0 && hp <= caution,
                    low_mp: caution > 0.0 && mp <= caution,
                    far_away: spawned.is_none(),
                    portrait: match settings.portrait_source {
                        PartyPortraitSource::Icon => row_char.and_then(|row| row.icon_path()),
                        PartyPortraitSource::Race | PartyPortraitSource::None => None,
                    },
                    european: row_char.is_some_and(|row| row.code_name().starts_with("CHAR_EU")),
                    name,
                }
            })
        })
        .collect();
}

#[allow(clippy::too_many_arguments)]
pub fn refresh_quick_party(
    views: Res<QuickPartyViews>,
    time: Res<Time>,
    asset_server: Res<AssetServer>,
    mut slots: Query<(&QuickPartySlot, &mut Visibility)>,
    mut names: Query<(&QuickName, &mut Text)>,
    mut hp: Query<(&QuickHpFill, &mut Node)>,
    mut mp: Query<(&QuickMpFill, &mut Node), Without<QuickHpFill>>,
    mut portraits: Query<
        (&QuickPortrait, &mut ImageNode, &mut Visibility),
        Without<QuickPartySlot>,
    >,
    mut status: Query<
        (&QuickStatus, &mut Visibility),
        (Without<QuickPartySlot>, Without<QuickPortrait>),
    >,
    mut races: Query<
        (&QuickRaceMark, &mut ImageNode, &mut Visibility),
        (
            Without<QuickPartySlot>,
            Without<QuickPortrait>,
            Without<QuickStatus>,
        ),
    >,
    mut crowns: Query<
        (&QuickCrown, &mut Visibility),
        (
            Without<QuickPartySlot>,
            Without<QuickPortrait>,
            Without<QuickStatus>,
            Without<QuickRaceMark>,
        ),
    >,
    mut cautions: Query<
        (&QuickCaution, &mut ImageNode, &mut Visibility),
        (
            Without<QuickPartySlot>,
            Without<QuickPortrait>,
            Without<QuickStatus>,
            Without<QuickRaceMark>,
            Without<QuickCrown>,
        ),
    >,
) {
    let s = hud_scale();
    let view = |row: usize| views.0.get(row).and_then(|view| view.as_ref());

    // Every write below is compared first. This system cannot take a
    // needs-refresh run condition -- the HP bars follow live vitals, so it has
    // to look every frame -- but *looking* every frame need not mean *writing*
    // every frame, and the difference is not cosmetic: a `Node` mutation marks
    // the UI tree dirty, and bevy_ui then re-runs the whole Taffy layout.
    // Unconditional writes here held `ui_layout_system` at a measured
    // 1.88 ms/frame across three capture windows (docs/perf-baselines.md), on a
    // board whose eight slots change a few times a minute.
    for (slot, mut visibility) in slots.iter_mut() {
        visibility.set_if_neq(visible(view(slot.0).is_some()));
    }
    for (marker, mut text) in names.iter_mut() {
        // Compared before the clone, so an unchanged name allocates nothing.
        let name = view(marker.0).map(|v| v.name.as_str()).unwrap_or_default();
        if text.0 != name {
            text.0 = name.to_string();
        }
    }
    for (marker, mut node) in hp.iter_mut() {
        let width = gauge_fill_width(view(marker.0).map(|v| v.hp).unwrap_or(0.0), Q_HP.2 * s);
        if node.width != width {
            node.width = width;
        }
    }
    for (marker, mut node) in mp.iter_mut() {
        let width = gauge_fill_width(view(marker.0).map(|v| v.mp).unwrap_or(0.0), Q_MP.2 * s);
        if node.width != width {
            node.width = width;
        }
    }
    for (marker, mut image, mut visibility) in portraits.iter_mut() {
        match view(marker.0) {
            Some(row) => {
                visibility.set_if_neq(Visibility::Inherited);
                // The fallback is the round backdrop plate — `qpt_face.ddj` is
                // an opaque black disc, not a face.
                let path = row
                    .portrait
                    .clone()
                    .unwrap_or_else(|| format!("{ART_QUICK}qpt_face.ddj"));
                let handle = asset_server.load(path);
                if image.image != handle {
                    image.image = handle;
                }
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    for (marker, mut visibility) in status.iter_mut() {
        visibility.set_if_neq(visible(view(marker.0).is_some_and(|row| row.far_away)));
    }
    for (marker, mut image, mut visibility) in races.iter_mut() {
        match view(marker.0) {
            Some(row) => {
                visibility.set_if_neq(Visibility::Inherited);
                let art = if row.european {
                    "com_kindred_europe16.ddj"
                } else {
                    "com_kindred_china16.ddj"
                };
                let handle = asset_server.load(format!("{ART_COMMON}{art}"));
                if image.image != handle {
                    image.image = handle;
                }
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    for (marker, mut visibility) in crowns.iter_mut() {
        visibility.set_if_neq(visible(view(marker.0).is_some_and(|row| row.crown)));
    }
    let frame = CAUTION.rect_at(time.elapsed_secs());
    for (marker, mut image, mut visibility) in cautions.iter_mut() {
        let lit = view(marker.0).is_some_and(|row| if marker.1 { row.low_hp } else { row.low_mp });
        visibility.set_if_neq(visible(lit));
        // The rect genuinely advances while lit -- this guard only spares the
        // frames where the animation lands on the same cell.
        if lit && image.rect != Some(frame) {
            image.rect = Some(frame);
        }
    }
}

fn visible(show: bool) -> Visibility {
    if show {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    }
}

// --- Interaction ------------------------------------------------------------

fn on_lock_button(_: On<Activate>, mut state: ResMut<QuickPartyState>) {
    state.locked = !state.locked;
}

/// Right-clicking a slot opens the member's context menu.
///
/// Vanilla's Banish / Dismiss / Appoint-master are **runtime** menu items:
/// `UIIT_CTL_BAN_PARTY`, `UIIT_STT_PARTY_DISSOLVE` and
/// `UIIT_CTL_PARTY_APPOINT_MASTER` appear in zero resinfo and zero 2dt files,
/// so the original assembles this menu in code exactly as this does. Only
/// Banish and Leave are offered: dismiss and appoint-master have no wire verb
/// in our tree, and offering a button that cannot send anything is worse than
/// not offering it.
fn on_slot_click(
    click: On<Pointer<Press>>,
    slots: Query<&QuickPartySlot>,
    views: Res<QuickPartyViews>,
    roster: Option<Res<PartyRoster>>,
    ui_strings: Res<ClientUiStrings>,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    cameras: Query<Entity, With<Camera2d>>,
    local: Query<&DisplayName, With<Player>>,
    roots: Query<Entity, With<ContextMenuRoot>>,
    mut owner: ResMut<ContextMenuOwner>,
    mut state: ResMut<QuickPartyState>,
    mut commands: Commands,
) {
    if click.button != PointerButton::Secondary {
        return;
    }
    let (Ok(slot), Some(camera), Some(roster)) =
        (slots.get(click.entity), cameras.iter().next(), roster)
    else {
        return;
    };
    let Some(row) = views.0.get(slot.0).and_then(|view| view.as_ref()) else {
        return;
    };
    close_context_menus(&mut commands, &roots, &mut owner);
    state.context_member = row.member_id;

    // Kicking is the leader's privilege, and we can tell whether that is us:
    // the roster names the leader's JID, and our own record is the one whose
    // name matches the local player's.
    let leads = we_lead(&roster, local.single().ok().map(|name| name.0.as_str()));

    let s = hud_scale();
    let items = [
        ContextMenuItem {
            label: ui_strings
                .get_or("UIIT_CTL_BAN_PARTY", "Banish")
                .to_string(),
            enabled: leads && row.member_id.is_some(),
        },
        ContextMenuItem {
            label: ui_strings
                .get_or("UIIT_CTL_LEAVE_PARTY", "Leave")
                .to_string(),
            enabled: true,
        },
    ];
    let y = BOARD_TOP + slot.0 as f32 * SLOT_PITCH;
    spawn_context_menu(
        &mut commands,
        &asset_server,
        &fonts,
        camera,
        Val::Px((BOARD_LEFT + SLOT_W) * s),
        Val::Px(y * s),
        Val::Px(0.0),
        &items,
        s,
    );
}

/// Fire the picked row and close the menu.
///
/// Polled on release against `Hovered` rather than observed, because that is
/// how every surface drives the shared popup and they all have to agree: they
/// spawn the *same* `ContextMenuRow` components, so a click while another menu
/// believed itself open would be read by two pollers. [`ContextMenuOwner`] is
/// the arbiter — this runs only while the board owns the popup — and
/// `context_member` is merely the payload the pick needs.
pub fn pick_quick_party_menu_row(
    buttons: Res<ButtonInput<MouseButton>>,
    rows: Query<(&ContextMenuRow, &Hovered)>,
    roots: Query<Entity, With<ContextMenuRoot>>,
    mut owner: ResMut<ContextMenuOwner>,
    mut state: ResMut<QuickPartyState>,
    mut actions: MessageWriter<PartyAction>,
    mut commands: Commands,
) {
    if !buttons.just_released(MouseButton::Left) || *owner != ContextMenuOwner::QuickParty {
        return;
    }
    let picked = rows
        .iter()
        .find(|(_, hovered)| hovered.get())
        .map(|(row, _)| row.0);
    match picked {
        Some(0) => {
            if let Some(member) = state.context_member {
                actions.write(PartyAction::Kick(member));
            }
        }
        Some(1) => {
            actions.write(PartyAction::Leave);
        }
        _ => {}
    }
    // Any left click dismisses the menu, picked or not.
    close_context_menus(&mut commands, &roots, &mut owner);
    state.context_member = None;
}

// --- Plugin -----------------------------------------------------------------

pub struct QuickPartyPlugin;

impl Plugin for QuickPartyPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<QuickPartyState>()
            .init_resource::<ContextMenuOwner>()
            .init_resource::<QuickPartyViews>()
            .add_systems(OnEnter(SceneState::GameWorld), spawn_quick_party_board)
            .add_systems(OnExit(SceneState::GameWorld), cleanup_quick_party_board)
            .add_systems(
                Update,
                (
                    // Chained: the painter reads what the compute pass wrote.
                    (compute_quick_party_views, refresh_quick_party).chain(),
                    pick_quick_party_menu_row,
                )
                    .run_if(
                        in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting)),
                    ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same headless guard the roster page carries, for the same reason: this
    /// painter holds six `&mut Visibility` queries, and Bevy proves them
    /// disjoint from their filters rather than from the markers never
    /// co-occurring. A missing `Without` here panics the schedule the first
    /// time the board is spawned (B0001), which is a live-session failure that
    /// costs a whole round-trip to find.
    #[test]
    fn the_painter_has_disjoint_queries() {
        let mut world = World::new();
        let mut schedule = Schedule::default();
        schedule.add_systems((
            compute_quick_party_views,
            refresh_quick_party,
            pick_quick_party_menu_row,
        ));
        schedule
            .initialize(&mut world)
            .expect("the quick-party painters must build a valid schedule");
    }

    /// The slot background is an atlas crop, and the UV quad the data ships
    /// resolves to exactly the declared rect. If either number drifts the
    /// board silently draws a slice of some other window.
    #[test]
    fn the_slot_background_is_the_declared_atlas_subrect() {
        let (uv_lt, uv_rb) = ((0.723633_f32, 0.138672_f32), (0.842773_f32, 0.216797_f32));
        let (sheet_w, sheet_h) = (1024.0_f32, 512.0_f32);
        let left = uv_lt.0 * sheet_w;
        let top = uv_lt.1 * sheet_h;
        let width = (uv_rb.0 - uv_lt.0) * sheet_w;
        let height = (uv_rb.1 - uv_lt.1) * sheet_h;

        assert!((left - SLOT_ATLAS_RECT.0).abs() < 0.01, "left {left}");
        assert!((top - SLOT_ATLAS_RECT.1).abs() < 0.01, "top {top}");
        assert!((width - SLOT_W).abs() < 0.01, "width {width}");
        assert!((height - SLOT_H).abs() < 0.01, "height {height}");

        let rect = atlas_rect(SLOT_ATLAS_RECT);
        assert_eq!(rect.min, Vec2::new(741.0, 71.0));
        assert_eq!(rect.max, Vec2::new(863.0, 111.0));
    }

    /// Seven rows at pitch 44 with a 40-tall slot: a 4 px gap, and the board is
    /// exactly as tall as the rows it holds.
    #[test]
    fn the_board_stacks_seven_slots_at_the_declared_pitch() {
        assert_eq!(SLOT_PITCH - SLOT_H, 4.0);
        let declared = [0.0, 44.0, 88.0, 132.0, 176.0, 220.0, 264.0];
        for (row, expected) in declared.iter().enumerate() {
            assert_eq!(row as f32 * SLOT_PITCH, *expected);
        }
        assert_eq!(QUICK_PARTY_ROWS, declared.len());
    }

    /// The quick sheets are half the mini-info's width, and the tile that falls
    /// out is exactly the control's declared rect — the check that the
    /// flipbook keys were transcribed against the right file.
    #[test]
    fn the_caution_tile_matches_the_declared_effect_rect() {
        assert_eq!(CAUTION.tile(), (Q_EFFECT_HP.2, Q_EFFECT_HP.3));
        assert_eq!(CAUTION.tile(), (64.0, 32.0));
        assert_eq!(CAUTION.frame_count, CAUTION.cols * CAUTION.rows);
    }

    /// The status overlay shares the portrait's rect because it is a tint, not
    /// a second picture. Pinned because "two controls, same rect" is exactly
    /// the kind of thing a later reader tidies away.
    #[test]
    fn the_status_overlay_sits_on_the_portrait() {
        assert_eq!(Q_STATUS, Q_PORTRAIT);
    }
}
