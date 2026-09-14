//! COS status stack — the icon + HP bar slots left of the minimap.
//!
//! Idea: vanilla's `GDR_COS_MANAGER:CIFCOSManager` (ginterface.txt:182, ID 39)
//! is a zero-rect, code-driven HUD container that stacks one 44×56 slot per
//! active summon, each built from the `resinfo/ifcosstatus.txt` prototype:
//! `am_window.ddj` frame, a 32×32 icon at (6,7), the 36×4 `am_hp.ddj` gauge at
//! (4,47), the 36×4 `am_hgp.ddj` hunger gauge at (4,56) — pets only — and the
//! `cos_outline_2.ddj` selection overlay at (-11,-11). Because the manager has
//! no authored rect, the stack's anchor is OURS: directly left of the minimap
//! window (whose rect is authored at 892,6 on the 1024-wide canvas). Clicking a
//! slot selects the COS (the 0x7045 send piggybacks on `SelectedEntity`'s
//! change detection). The command bar under the stack is a separate widget —
//! see `super::cos_command`.
//!
//! Note the HGP gauge's authored y of 56 sits **below** the 44×56 frame art, so
//! a pet slot occupies 60px of layout while its frame stays 56. The slot root
//! is therefore a plain `Node` with the frame as an absolutely-positioned
//! child, not (as it was) an `ImageNode` doing double duty as the layout box.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::{Activate, Button};

use crate::plugins::cos::ActiveCosList;
use crate::plugins::cursor::interactions::entity_select::SelectedEntity;
use crate::plugins::hud::cos::state::CosState;
use crate::plugins::hud::game_window::scaled;
use crate::plugins::hud::gauge::{gauge_art_node, gauge_crop_node, gauge_fill_width};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::entities::{EntityVitals, NetworkEntities};
use crate::plugins::textdata::{ClientItemData, ClientItemIndex};

// --- Layout ------------------------------------------------------------------

/// The 44×56 slot prototype's own size (`am_window.ddj`).
const SLOT_W: f32 = 44.0;
pub const SLOT_H: f32 = 56.0;
/// A pet slot is 4px taller than its frame, because `GDR_COSSTAT_SGAUGE` is
/// authored at y=56 — flush under the frame rather than inside it.
const PET_SLOT_H: f32 = HGP_RECT.1 + HGP_RECT.3;
/// Slot-space rects, verbatim from `resinfo/ifcosstatus.txt`. Both gauges are
/// `w=h=0` there ("size to the art"), and both arts measure 36×4
/// (`docs/re/ui/hp-mp-gauge-widget.md:199-200`).
const ICON_RECT: (f32, f32, f32, f32) = (6.0, 7.0, 32.0, 32.0);
const HP_RECT: (f32, f32, f32, f32) = (4.0, 47.0, 36.0, 4.0);
const HGP_RECT: (f32, f32, f32, f32) = (4.0, 56.0, 36.0, 4.0);
/// The selection outline overhangs the slot by 11px on each side
/// (`Rect="-11,-11,0,0"` — w=h=0 means "size to the art").
///
/// The art is `icon/etc/COS_outline_2.ddj`, **measured 65×77**, so that is the
/// extent rather than the symmetric `SLOT_W+22` × `SLOT_H+22` (66×78) an
/// earlier reading assumed: the plate is one pixel short on each axis because
/// its opaque centre is the 44×56 slot at x 11..54 / y 11..65, and stretching
/// it to 66×78 slid the fringe a pixel off the frame it wraps.
const OUTLINE_W: f32 = 65.0;
const OUTLINE_H: f32 = 77.0;
const OUTLINE_RECT: (f32, f32, f32, f32) = (-11.0, -11.0, OUTLINE_W, OUTLINE_H);

/// OUR anchoring: stack top aligned with the minimap (y 6), right edge 4px
/// left of the minimap window (authored at x 892 on the 1024 canvas).
pub const STACK_TOP: f32 = 6.0;
pub const STACK_RIGHT: f32 = 1024.0 - 892.0 + 4.0;
/// Vertical gap between stacked slots (ours; vanilla stacks are untraced).
const SLOT_GAP: f32 = 4.0;

const FRAME_DDJ: &str = "media://interface/animal/am_window.ddj";
const HP_DDJ: &str = "media://interface/animal/am_hp.ddj";
const HGP_DDJ: &str = "media://interface/animal/am_hgp.ddj";
const OUTLINE_DDJ: &str = "media://icon/etc/cos_outline_2.ddj";

// --- Components ---------------------------------------------------------------

#[derive(Component, Clone, Default)]
pub struct CosStatusRoot;
/// One slot, keyed by the COS unique id it shows.
#[derive(Component, Clone)]
pub struct CosSlot(pub u32);
#[derive(Component, Clone)]
pub struct CosSlotHpFill(pub u32);
/// The hunger gauge's crop node. Pets only, and hidden rather than emptied
/// while the COS has no HGP value at all.
#[derive(Component, Clone)]
pub struct CosSlotHgpFill(pub u32);
#[derive(Component, Clone)]
pub struct CosSlotOutline(pub u32);

// --- Spawn / cleanup ----------------------------------------------------------

pub fn spawn_cos_status(mut commands: Commands, cam_query: Query<Entity, With<Camera2d>>) {
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found for the COS status stack");
        return;
    };
    let s = hud_scale();
    commands
        .spawn((
            CosStatusRoot,
            Name::from("COS Status Stack"),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(STACK_TOP * s),
                right: Val::Px(STACK_RIGHT * s),
                width: Val::Px(SLOT_W * s),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(SLOT_GAP * s),
                ..default()
            },
            GlobalZIndex(40),
            Visibility::default(),
            UiTargetCamera(camera),
        ))
        .with_children(|_| {});
}

pub fn cleanup_cos_status(
    mut commands: Commands,
    roots: Query<Entity, With<CosStatusRoot>>,
    bars: Query<Entity, With<super::cos_command::CosCommandBar>>,
) {
    for entity in roots.iter().chain(bars.iter()) {
        commands.entity(entity).despawn();
    }
}

// --- Sync ----------------------------------------------------------------------

/// Rebuild the slot children whenever the active list changes (a handful of
/// nodes; not worth diffing). The command bar under the stack is built by
/// `super::cos_command`.
#[allow(clippy::too_many_arguments)]
pub fn sync_cos_status_slots(
    list: Res<ActiveCosList>,
    mut last_uids: Local<Vec<u32>>,
    roots: Query<Entity, With<CosStatusRoot>>,
    asset_server: Res<AssetServer>,
    item_index: Option<Res<ClientItemIndex>>,
    item_data: Option<Res<ClientItemData>>,
    char_data: Option<Res<crate::plugins::textdata::ClientCharacterData>>,
    mut commands: Commands,
) {
    let Ok(root) = roots.single() else { return };
    let uids: Vec<u32> = list.0.iter().map(|c| c.unique_id).collect();
    if *last_uids == uids {
        return;
    }
    last_uids.clone_from(&uids);

    // Rebuild the stack.
    commands.entity(root).despawn_children();
    for status in &list.0 {
        // Slot icon: the COS's *own* characterdata icon (`AssocFileIcon_128`,
        // column 54), which 5,579 of the 5,587 COS rows carry. The summon
        // item's icon is the fallback and used to be the only source, which is
        // why a growth pet showed nothing: its laddered stages
        // (`COS_P_WOLF_001`, `_002`, ...) are characterdata rows with no 1:1
        // item, so `ITEM_<codename>` never resolved. Note the whole 140-row
        // wolf ladder shares one icon — growth stages are not drawn apart here.
        let row = char_data
            .as_deref()
            .and_then(|cd| cd.get(&(status.ref_id as i32)));
        let icon = row.and_then(|row| row.icon_path()).or_else(|| {
            let item_ref = item_index
                .as_deref()?
                .id(&format!("ITEM_{}", row?.code_name()))?;
            item_data.as_deref()?.get(&item_ref)?.icon_path()
        });
        let uid = status.unique_id;
        let is_pet = status.kind.is_pet();
        let s = hud_scale();
        let (icon_l, icon_t, icon_w, icon_h) = scaled(ICON_RECT, s);
        let (out_l, out_t, out_w, out_h) = scaled(OUTLINE_RECT, s);
        let slot_h = if is_pet { PET_SLOT_H } else { SLOT_H };
        let mut slot = commands.spawn((
            CosSlot(uid),
            Name::from(format!("cos slot {uid}")),
            Button,
            Hovered::default(),
            Node {
                width: Val::Px(SLOT_W * s),
                height: Val::Px(slot_h * s),
                ..default()
            },
            ChildOf(root),
        ));
        slot.observe(
            move |_: On<Activate>,
                  index: Res<NetworkEntities>,
                  mut selected: ResMut<SelectedEntity>,
                  mut subject: ResMut<super::cos_command::CosBarSubject>| {
                // Selecting via the slot; the 0x7045 send rides the change.
                selected.0 = index.get(uid);
                // ...and the command bar switches to this summon, since its
                // cells are chosen per COS kind. Kept separate from
                // `SelectedEntity` on purpose — see `CosBarSubject`.
                subject.0 = Some(uid);
            },
        );
        slot.with_children(|slot| {
            // Selection outline, FIRST so it renders BEHIND the frame and the
            // icon.
            //
            // The art is not a hollow ring: `COS_outline_2.ddj` is 65x77 and
            // measures as an 11px green (226,255,34) glow fringe whose alpha
            // ramps 0 -> 217 inward, wrapped around a centre of **fully opaque
            // white** — and that white centre is exactly the 44x56 slot rect
            // (x 11..54, y 11..65). A plate whose middle is precisely the thing
            // it decorates is a backdrop: it is meant to be occluded by the
            // slot so only the fringe shows. Drawn last, its white centre
            // painted straight over the icon and the whole slot read as a white
            // blur inside a green halo.
            slot.spawn((
                CosSlotOutline(uid),
                ImageNode {
                    image: asset_server.load(OUTLINE_DDJ),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(out_l),
                    top: Val::Px(out_t),
                    width: Val::Px(out_w),
                    height: Val::Px(out_h),
                    ..default()
                },
                Visibility::Hidden,
                Pickable::IGNORE,
            ));
            // The frame art, at its own 44x56 — a pet slot's layout box is
            // taller than this, so the two are separate nodes.
            slot.spawn((
                ImageNode {
                    image: asset_server.load(FRAME_DDJ),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Px(SLOT_W * s),
                    height: Val::Px(SLOT_H * s),
                    ..default()
                },
                Pickable::IGNORE,
            ));
            if let Some(icon) = icon {
                slot.spawn((
                    ImageNode {
                        image: asset_server.load(icon),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(icon_l),
                        top: Val::Px(icon_t),
                        width: Val::Px(icon_w),
                        height: Val::Px(icon_h),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            }
            // Both gauges use the shared three-node CIFGauge recipe
            // (`hud/gauge.rs`, #630): the art is never resized, the crop node
            // is the only thing a fill drives. These two used to write the
            // percentage straight onto the `ImageNode`, which is exactly the
            // bitmap stretch that recipe exists to prevent.
            for (rect, art, hunger) in [(HP_RECT, HP_DDJ, false), (HGP_RECT, HGP_DDJ, true)] {
                if hunger && !is_pet {
                    // Vehicles do not starve, so vanilla's hunger gauge has
                    // nothing to show for them.
                    continue;
                }
                let (l, t, w, h) = scaled(rect, s);
                slot.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(l),
                        top: Val::Px(t),
                        width: Val::Px(w),
                        height: Val::Px(h),
                        overflow: bevy::ui::Overflow::clip(),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|track| {
                    let mut crop = track.spawn((gauge_crop_node(Val::Px(w), h), Pickable::IGNORE));
                    if hunger {
                        crop.insert((CosSlotHgpFill(uid), Visibility::Hidden));
                    } else {
                        crop.insert(CosSlotHpFill(uid));
                    }
                    crop.with_children(|crop| {
                        crop.spawn((
                            gauge_art_node(w, h),
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
    }
}

// --- Per-frame refresh ----------------------------------------------------------

/// HP fills from live [`EntityVitals`] (fallback: the 0x30C8 seed in the
/// status entry), HGP from [`CosState`], and outline visibility from the
/// selection.
#[allow(clippy::type_complexity)]
pub fn update_cos_status(
    list: Res<ActiveCosList>,
    cos_state: Res<CosState>,
    index: Res<NetworkEntities>,
    selected: Res<SelectedEntity>,
    vitals: Query<&EntityVitals>,
    mut fills: Query<(&CosSlotHpFill, &mut Node), Without<CosSlotHgpFill>>,
    mut hunger: Query<(&CosSlotHgpFill, &mut Node, &mut Visibility), Without<CosSlotOutline>>,
    mut outlines: Query<(&CosSlotOutline, &mut Visibility), Without<CosSlotHgpFill>>,
) {
    let s = hud_scale();
    for (fill, mut node) in fills.iter_mut() {
        let ratio = index
            .get(fill.0)
            .and_then(|e| vitals.get(e).ok())
            .map(EntityVitals::fill)
            .or_else(|| {
                list.get(fill.0)
                    .map(|s| s.hp as f32 / s.hp_max.max(1) as f32)
            })
            .unwrap_or(1.0);
        let width = gauge_fill_width(ratio, HP_RECT.2 * s);
        if node.width != width {
            node.width = width;
        }
    }
    for (fill, mut node, mut visibility) in hunger.iter_mut() {
        // No HGP at all is not a starving pet: a pick pet carries no growth
        // block, so its first value arrives only with 0x30C9 arm 4. Hide the
        // gauge until then rather than drawing an empty one.
        let fraction = cos_state.get(fill.0).and_then(|cos| cos.hgp_fraction());
        let wanted = match fraction {
            Some(_) => Visibility::Inherited,
            None => Visibility::Hidden,
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
        let width = gauge_fill_width(fraction.unwrap_or(0.0), HGP_RECT.2 * s);
        if node.width != width {
            node.width = width;
        }
    }
    for (outline, mut visibility) in outlines.iter_mut() {
        let is_selected = selected
            .0
            .is_some_and(|entity| index.get(outline.0) == Some(entity));
        let wanted = if is_selected {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
}

/// Self-registration for the COS status stack (#558): the HUD registry names
/// one plugin per widget, so the stack's systems live here rather than in the
/// registry. Spawned in the offline sandbox too — the dev COS spawner rides
/// there without a server.
pub struct CosStatusPlugin;

impl Plugin for CosStatusPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.add_systems(OnEnter(SceneState::GameWorld), spawn_cos_status)
            .add_systems(OnExit(SceneState::GameWorld), cleanup_cos_status)
            .add_systems(OnEnter(SceneState::WorldSandbox), spawn_cos_status)
            .add_systems(OnExit(SceneState::WorldSandbox), cleanup_cos_status)
            .add_systems(
                Update,
                (sync_cos_status_slots, update_cos_status).run_if(crate::scenes::in_playable_world),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::cos::{CosStatus, RiderState};
    use crate::plugins::hud::cos::state::HGP_FULL;
    use packets::agent::pet::CosKind;

    fn app_with(summons: Vec<CosStatus>) -> App {
        // `AssetServer::load` schedules on the IO task pool, which `App::new`
        // alone does not create.
        bevy::tasks::IoTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Image>()
            .init_resource::<ActiveCosList>()
            .init_resource::<CosState>()
            .init_resource::<RiderState>()
            .init_resource::<NetworkEntities>()
            .init_resource::<SelectedEntity>()
            .add_systems(Update, (sync_cos_status_slots, update_cos_status));
        app.world_mut().spawn((CosStatusRoot, Node::default()));
        app.world_mut().resource_mut::<ActiveCosList>().0 = summons;
        app
    }

    fn summon(unique_id: u32, ref_id: u32, kind: CosKind) -> CosStatus {
        CosStatus {
            unique_id,
            ref_id,
            kind,
            name: Some("Summon".into()),
            hp: 50,
            hp_max: 100,
            local_only: true,
        }
    }

    /// Query disjointness is only proven at system init, and a miss is a
    /// first-frame B0001 panic in GameWorld — run both systems once with a
    /// populated stack (same guard as the target window's test).
    #[test]
    fn the_hud_systems_have_no_conflicting_queries() {
        let mut app = app_with(vec![summon(7, 2137, CosKind::Vehicle)]);
        app.update(); // builds the slots
        app.update(); // refreshes them
        let fills: Vec<_> = app
            .world_mut()
            .query::<&CosSlotHpFill>()
            .iter(app.world())
            .collect();
        assert_eq!(fills.len(), 1, "one slot per active summon");
    }

    /// The selection outline must be the FIRST child of its slot, i.e. drawn
    /// behind everything else in it.
    ///
    /// `COS_outline_2.ddj` is not a hollow ring: its 11px green fringe wraps a
    /// centre of fully opaque white that is exactly the 44x56 slot rect, so it
    /// only works as a backdrop. Spawned last it painted that white centre over
    /// the icon, and selecting a COS turned its slot into a white blur inside a
    /// green halo. Bevy UI draws siblings in spawn order, so child index 0 is
    /// the whole fix — and the thing a refactor would silently undo.
    #[test]
    fn the_selection_outline_is_drawn_behind_the_slot_contents() {
        let mut app = app_with(vec![summon(7, 2137, CosKind::Vehicle)]);
        app.update();

        let slot = app
            .world_mut()
            .query_filtered::<Entity, With<CosSlot>>()
            .iter(app.world())
            .next()
            .expect("one slot");
        let children: Vec<Entity> = app
            .world()
            .entity(slot)
            .get::<Children>()
            .expect("slot has children")
            .iter()
            .collect();
        assert!(
            children.len() > 1,
            "the slot draws more than just the outline"
        );
        assert!(
            app.world().entity(children[0]).contains::<CosSlotOutline>(),
            "the outline must be child 0 (behind the frame, icon and gauges), \
             otherwise its opaque white centre covers them"
        );
    }

    /// The outline extent is the art's own size, which is what the authored
    /// `Rect="-11,-11,0,0"` (w=h=0 = "size to the art") asks for — measured
    /// 65x77, not the symmetric 66x78 that `SLOT_W+22` / `SLOT_H+22` implies.
    #[test]
    fn the_outline_extent_is_the_measured_art_size() {
        assert_eq!((OUTLINE_W, OUTLINE_H), (65.0, 77.0));
        // ...and its opaque centre is exactly the slot, which is the evidence
        // that it belongs behind one: 65 - 2*11 = 43 ≈ 44, 77 - 2*11 = 55 ≈ 56.
        assert_eq!(OUTLINE_RECT.0, -11.0);
        assert_eq!(OUTLINE_RECT.1, -11.0);
        assert!(OUTLINE_W >= SLOT_W && OUTLINE_H >= SLOT_H);
    }

    /// The hunger gauge is a pet's, not a vehicle's — vanilla's own exclusion,
    /// and the reason a pet slot is 4px taller than its frame.
    #[test]
    fn only_pets_get_the_hunger_gauge() {
        let mut app = app_with(vec![
            summon(7, 2137, CosKind::Vehicle),
            summon(8, 6106, CosKind::GrowthPet),
        ]);
        app.update();
        app.update();

        let hp = app
            .world_mut()
            .query::<&CosSlotHpFill>()
            .iter(app.world())
            .count();
        let hgp: Vec<u32> = app
            .world_mut()
            .query::<&CosSlotHgpFill>()
            .iter(app.world())
            .map(|fill| fill.0)
            .collect();
        assert_eq!(hp, 2, "both summons have an HP bar");
        assert_eq!(hgp, vec![8], "only the pet has a hunger bar");
    }

    /// An unknown hunger reads as *hidden*, not as *empty*: a pick pet carries
    /// no growth block, so a starving-looking bar would be a lie until its
    /// first 0x30C9 arm 4.
    #[test]
    fn an_unknown_hunger_hides_the_gauge_rather_than_emptying_it() {
        let mut app = app_with(vec![summon(8, 6106, CosKind::GrowthPet)]);
        app.update();
        app.update();
        let hidden = app
            .world_mut()
            .query::<(&CosSlotHgpFill, &Visibility)>()
            .iter(app.world())
            .all(|(_, v)| *v == Visibility::Hidden);
        assert!(hidden, "no HGP value yet -> no gauge");

        // ...and the captured wolf's 9932 brings it back, nearly full.
        app.world_mut()
            .resource_mut::<CosState>()
            .cos
            .push(crate::plugins::hud::cos::state::Cos {
                unique_id: 8,
                ref_obj_id: 6106,
                kind: CosKind::GrowthPet,
                body: packets::agent::pet::CosBody {
                    hp: 360,
                    unk_b: 0,
                    growth: None,
                    unk_f: None,
                    name: None,
                    inventory_size: 0,
                    items: Vec::new(),
                    unk_g: None,
                    unk_h: None,
                },
                hgp: Some(9_932),
                exp: 77,
                level: Some(1),
            });
        app.update();
        let (width, visibility) = app
            .world_mut()
            .query::<(&CosSlotHgpFill, &Node, &Visibility)>()
            .iter(app.world())
            .map(|(_, node, v)| (node.width, *v))
            .next()
            .expect("the pet has a hunger gauge");
        assert_eq!(visibility, Visibility::Inherited);
        let track = HGP_RECT.2 * hud_scale();
        // 99.32 % of the track, cropped rather than stretched (#630).
        assert_eq!(width, Val::Px(track.round()));

        // Halve it and the crop halves with it — proof the fill drives the crop
        // node and not the art's own width.
        app.world_mut()
            .resource_mut::<CosState>()
            .get_mut(8)
            .unwrap()
            .hgp = Some(HGP_FULL / 2);
        app.update();
        let width = app
            .world_mut()
            .query::<(&CosSlotHgpFill, &Node)>()
            .iter(app.world())
            .map(|(_, node)| node.width)
            .next()
            .unwrap();
        assert_eq!(width, Val::Px((track / 2.0).round()));
    }
}
