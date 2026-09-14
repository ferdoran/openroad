//! The vanilla MagicStateBoard: buff/debuff icons with remaining-time
//! gauges, sitting immediately right of the player mini info.
//!
//! Idea: transcribed from `resinfo/ifmagicstateboard.txt` + `ginterface.txt`
//! — the board anchors at window (220,10) (mini info ends at x 216); two
//! rows of nine 20×20 icon slots on a 21px pitch, each over a 20×4 time
//! gauge (`icon/stateodd/s_stateodd_time_gauge.ddj`, the blue draining
//! bar): BLESS (buffs) at y 0/20, CURSE (debuffs) at y 27/47. Buff icons
//! are the buff skill's own icon (skilldata col 61); debuff icons come from
//! the `icon/StateOdd/` status set. The board rebuilds when the buff/debuff
//! set changes and the gauge widths track their timers every frame.
//!
//! Sources today are the player's [`ActiveBuffs`] component and the
//! [`Stunned`]/[`Frozen`] status components — item-granted buffs have no
//! item-use pipeline yet.
//!
//! Two visuals here are openroad conventions with no counterpart in the data:
//! the `GlobalZIndex` (the resinfo grammar has no z-order key) and the slot /
//! gauge-track background fills (the tree authors no background controls).

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::UiTargetCamera;

use packets::agent::prelude::{Ailment, BadStatus};

use crate::plugins::hud::gauge::{gauge_art_node, gauge_crop_node, gauge_fill_width};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::entities::EntityAilments;
use crate::plugins::player::Player;
use crate::plugins::skills::status::{self, ActiveBuffs, Frozen, Stunned};

/// Board anchor in vanilla window units, straight from the registry:
/// `GDR_MAGICSTATEBOARD` `Rect="220,10,160,40"` (`ginterface.txt:407`, ID 22).
/// The `160,40` is a stale editor bound rather than the size — the tree's own
/// content bbox is 188×51. Do not hand-tune x: a previous 208 here was
/// justified by a transparent tail on the mini-info frame crop that the pixel
/// data does not show (#344).
const BOARD_POS: (f32, f32) = (220.0, 10.0);
const SLOT: f32 = 20.0;
const PITCH: f32 = 21.0;
const GAUGE_H: f32 = 4.0;
/// The curse (debuff) row's icon y (gauges sit at y + SLOT).
const CURSE_Y: f32 = 27.0;
/// Icon slots per authored row — `ifmagicstateboard.txt` authors exactly nine
/// per row and no wrap, so a tenth buff has no slot in the original either.
const SLOTS: usize = 9;

const GAUGE_DDJ: &str = "media://icon/stateodd/s_stateodd_time_gauge.ddj";
const STUN_ICON: &str = "media://icon/stateodd/s_stun_icon.ddj";
const FREEZE_ICON: &str = "media://icon/stateodd/s_freeze_icon.ddj";

#[derive(Component)]
pub struct MagicStateBoardRoot;

/// What a gauge's fill tracks; looked up per frame.
#[derive(Component, Clone, PartialEq)]
pub enum GaugeSource {
    /// A buff, identified by its skilleffect codename.
    Buff(String),
    Stunned,
    Frozen,
    /// A server-reported ailment (0x3057). Has no gauge: the wire carries the
    /// set bits, never a remaining duration.
    Ailment(Ailment),
}

/// A hoverable board icon (feeds the skill tooltip; buffs also cancel on
/// right-click).
#[derive(Component)]
pub struct BuffIcon(pub GaugeSource);

/// The state the current board children were built for.
#[derive(Default, PartialEq)]
pub struct BoardSignature {
    buffs: Vec<(String, Option<String>)>,
    stunned: bool,
    frozen: bool,
    /// The local player's 0x3057 bad-status mask — the live, server-owned
    /// ailment source, as against the two skilldata-inferred flags above.
    ailments: u32,
}

pub fn cleanup_magic_state_board(
    mut commands: Commands,
    roots: Query<Entity, With<MagicStateBoardRoot>>,
) {
    for root in roots.iter() {
        commands.entity(root).despawn();
    }
}

/// Spawn/rebuild the board when the player's buff/debuff set changes.
pub fn update_magic_state_board(
    buffs: Query<&ActiveBuffs, With<Player>>,
    stunned: Query<(), (With<Stunned>, With<Player>)>,
    frozen: Query<(), (With<Frozen>, With<Player>)>,
    ailing: Query<&EntityAilments, With<Player>>,
    roots: Query<Entity, With<MagicStateBoardRoot>>,
    cam_query: Query<Entity, With<Camera2d>>,
    asset_server: Res<AssetServer>,
    mut signature: Local<BoardSignature>,
    mut commands: Commands,
) {
    let current = BoardSignature {
        buffs: buffs
            .single()
            .map(|active| {
                active
                    .0
                    .iter()
                    .take(SLOTS)
                    .map(|buff| (buff.codename.clone(), buff.icon.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        stunned: !stunned.is_empty(),
        frozen: !frozen.is_empty(),
        ailments: ailing.single().map(|a| a.0 .0).unwrap_or(0),
    };
    let root = roots.single().ok();
    if root.is_some() && *signature == current {
        return;
    }

    let root = match root {
        Some(root) => root,
        None => {
            let Ok(camera) = cam_query.single() else {
                return;
            };
            commands
                .spawn((
                    MagicStateBoardRoot,
                    Name::from("Magic State Board"),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(BOARD_POS.0 * hud_scale()),
                        top: Val::Px(BOARD_POS.1 * hud_scale()),
                        ..default()
                    },
                    GlobalZIndex(50),
                    Pickable::IGNORE,
                    UiTargetCamera(camera),
                ))
                .id()
        }
    };

    let s = hud_scale();
    let mut slots: Vec<(usize, f32, Option<String>, GaugeSource)> = Vec::new();
    for (i, (codename, icon)) in current.buffs.iter().enumerate() {
        slots.push((i, 0.0, icon.clone(), GaugeSource::Buff(codename.clone())));
    }
    let mut curse = 0;
    if current.stunned {
        slots.push((curse, CURSE_Y, Some(STUN_ICON.into()), GaugeSource::Stunned));
        curse += 1;
    }
    if current.frozen {
        slots.push((
            curse,
            CURSE_Y,
            Some(FREEZE_ICON.into()),
            GaugeSource::Frozen,
        ));
        curse += 1;
    }
    // The server-owned ailments (0x3057). They carry no client-side timer —
    // the wire says which bits are set, never for how long — so they get an
    // icon with no gauge, unlike the two skilldata-inferred flags above.
    // Truncated at the authored nine slots like the bless row.
    for ailment in BadStatus(current.ailments).ailments() {
        if curse >= SLOTS {
            break;
        }
        slots.push((
            curse,
            CURSE_Y,
            Some(ailment.icon_path()),
            GaugeSource::Ailment(ailment),
        ));
        curse += 1;
    }

    commands
        .entity(root)
        .despawn_related::<Children>()
        .with_children(|board| {
            for (column, y, icon, source) in slots {
                let x = column as f32 * PITCH;
                // the icon is hoverable (skill tooltip with remaining time)
                // and, for buffs, right-clickable to cancel
                let mut slot = board.spawn((
                    BuffIcon(source.clone()),
                    Hovered::default(),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(x * s),
                        top: Val::Px(y * s),
                        width: Val::Px(SLOT * s),
                        height: Val::Px(SLOT * s),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
                ));
                slot.observe(on_buff_press);
                if let Some(icon) = icon {
                    slot.insert(ImageNode {
                        image: asset_server.load(icon),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    });
                }
                // the time gauge: dark track under the icon, blue fill
                // whose width tracks the remaining fraction
                board
                    .spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(x * s),
                            top: Val::Px((y + SLOT) * s),
                            width: Val::Px(SLOT * s),
                            height: Val::Px(GAUGE_H * s),
                            // The track clipped nothing before: the fill was
                            // the art node itself, so it shrank the bitmap
                            // instead of cropping it (#630).
                            overflow: Overflow::clip(),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.02, 0.02, 0.06, 0.9)),
                        Pickable::IGNORE,
                    ))
                    .with_children(|track| {
                        track
                            .spawn((
                                source,
                                gauge_crop_node(gauge_fill_width(1.0, SLOT * s), GAUGE_H * s),
                                Pickable::IGNORE,
                            ))
                            .with_children(|crop| {
                                crop.spawn((
                                    gauge_art_node(SLOT * s, GAUGE_H * s),
                                    ImageNode {
                                        image: asset_server.load(GAUGE_DDJ),
                                        image_mode: NodeImageMode::Stretch,
                                        ..default()
                                    },
                                    Pickable::IGNORE,
                                ));
                            });
                    });
            }
        });
    *signature = current;
}

/// Right-click on a BUFF icon cancels the buff (the curse row ignores it —
/// debuffs can't be clicked away). Local-sim only: no client→server buff
/// cancel opcode is documented (see [`status::cancel_buff`]).
fn on_buff_press(
    mut press: On<Pointer<Press>>,
    icons: Query<&BuffIcon>,
    mut buffs: Query<&mut ActiveBuffs, With<Player>>,
    mut commands: Commands,
) {
    if press.event.button != PointerButton::Secondary {
        return;
    }
    let Ok(BuffIcon(GaugeSource::Buff(codename))) = icons.get(press.entity) else {
        return;
    };
    press.propagate(false);
    if let Ok(mut active) = buffs.single_mut() {
        status::cancel_buff(&mut active, codename, &mut commands);
    }
}

/// Track the gauge fills against their timers every frame.
pub fn update_state_gauges(
    buffs: Query<&ActiveBuffs, With<Player>>,
    stunned: Query<&Stunned, With<Player>>,
    frozen: Query<&Frozen, With<Player>>,
    mut gauges: Query<(&GaugeSource, &mut Node)>,
) {
    for (source, mut node) in gauges.iter_mut() {
        let fraction = match source {
            GaugeSource::Buff(codename) => buffs
                .single()
                .ok()
                .and_then(|active| active.0.iter().find(|buff| &buff.codename == codename))
                .map(|buff| buff.timer.fraction_remaining()),
            GaugeSource::Stunned => stunned.single().ok().map(|s| s.0.fraction_remaining()),
            GaugeSource::Frozen => frozen.single().ok().map(|f| f.0.fraction_remaining()),
            // The wire reports which ailments are set, never for how long, so
            // there is no fraction to draw. Empty rather than full: a
            // permanently-full bar would read as "just applied, forever".
            GaugeSource::Ailment(_) => None,
        }
        .unwrap_or(0.0);
        // Drives the crop node (`gauge_crop_node`), never the art node.
        let width = gauge_fill_width(fraction, SLOT * hud_scale());
        if node.width != width {
            node.width = width;
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The registry entry is the only anchor this board has: it is absent from
    /// the 10-entry `wndpos` list (`docs/formats/wndpos.md:16-25`), so the
    /// player cannot move it and `220,10` is not a default but *the* position.
    /// Three modules in this lane have drifted by hand-tuning a constant whose
    /// own doc comment states the data value (#344), so pin it.
    #[test]
    fn the_anchor_is_the_registry_rect() {
        // GDR_MAGICSTATEBOARD, ID 22: Rect="220,10,160,40" (ginterface.txt:407)
        assert_eq!(BOARD_POS, (220.0, 10.0));
    }

    /// Slot metrics transcribed from `ifmagicstateboard.txt`: 36 controls, of
    /// which 18 are icon slots (two rows of nine) and 18 the gauges beneath
    /// them. The curse row's icons are authored at y 27 (e.g. `Rect="168,27,
    /// 20,20"` down to `Rect="42,27,20,20"`), a 21px pitch on 20x20 slots.
    #[test]
    fn slot_metrics_match_the_authored_tree() {
        assert_eq!((SLOT, PITCH, GAUGE_H), (20.0, 21.0, 4.0));
        assert_eq!(SLOTS, 9, "nine authored slots per row, no wrap");
        assert_eq!(CURSE_Y, 27.0);
        // Gauges are authored at y 20 (bless) and y 47 (curse); the spawn path
        // derives both as `y + SLOT`, so that identity must hold.
        assert_eq!(0.0 + SLOT, 20.0);
        assert_eq!(CURSE_Y + SLOT, 47.0);
    }
}

/// Self-registration for the buff/debuff board (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct MagicStateBoardPlugin;

impl Plugin for MagicStateBoardPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.add_systems(OnExit(SceneState::GameWorld), cleanup_magic_state_board)
            .add_systems(OnExit(SceneState::Skills), cleanup_magic_state_board)
            // buff/debuff board: only where the skills sim runs (GameWorld +
            // the Skills test scene)
            .add_systems(
                Update,
                (update_magic_state_board, update_state_gauges)
                    .chain()
                    .run_if(in_state(SceneState::GameWorld).or_else(in_state(SceneState::Skills))),
            );
    }
}
