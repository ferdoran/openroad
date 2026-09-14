//! Floating combat damage numbers (hitcount digits).
//!
//! Idea: the same pooled absolute-node + `world_to_viewport` structure as
//! [`super::nameplates`], but each slot is a row of digit `ImageNode`s with a
//! per-popup lifetime: a [`DamagePopup`] claims a free slot, the amount's
//! decimal digits pick the images, then the slot floats up over ~1.2s with an
//! alpha fade, following its anchor entity while it lives (the last position
//! is cached so killing-blow numbers survive the despawn frame). Art is
//! `media://interface/hitcount/` — 36x60 digit canvases with the wavy
//! per-digit vertical offset baked in, so adjacent native-size layout matches
//! the original look. Color sets, pixel-verified: neutral = white = the local
//! player's own damage, `_enemy` = red = damage received from monsters,
//! `_player` = gray = other players' damage.

use bevy::prelude::*;
use bevy::ui::{UiTargetCamera, UiTransform, Val2};

use crate::plugins::camera::PlayerCamera;
use crate::plugins::combat::{DamagePopup, HitcountSet};

/// Concurrent popups; further hits while all slots are busy are dropped.
const POOL_SIZE: usize = 32;
/// Digits per popup (999,999 damage caps the era's numbers comfortably).
const MAX_DIGITS: usize = 6;
/// Seconds a popup lives.
const LIFETIME: f32 = 1.2;
/// Total upward drift over the lifetime, in logical px.
const RISE_PX: f32 = 40.0;
/// Anchor height above the entity origin, just over the nameplate point.
const HEAD_OFFSET: f32 = 26.0;
/// Popups farther than this from the camera are not shown.
const MAX_DISTANCE: f32 = 600.0;
/// Lifetime fraction after which the fade-out starts.
const FADE_START: f32 = 0.5;

/// One slot root of the popup pool.
#[derive(Component)]
pub struct HitcountSlot;

/// A digit or the critical-word image inside a slot.
#[derive(Component)]
pub struct HitcountGlyph;

/// Live state of a claimed slot.
#[derive(Component)]
pub struct ActivePopup {
    age: f32,
    anchor: Entity,
    /// Last known anchor head point (kept when the anchor despawns).
    world: Vec3,
}

/// The digit/critical image handles per [`HitcountSet`].
#[derive(Resource)]
pub struct HitcountAssets {
    digits: [[Handle<Image>; 10]; 3],
    critical: [Handle<Image>; 3],
    /// The `blocking` word, shown *instead of* a number when the defender
    /// avoided the hit (wire hit arm 2). Same three-suffix set as `critical`,
    /// and like it pre-composited — no `_shadow` layer underneath.
    blocking: [Handle<Image>; 3],
}

/// The digit glyphs a popup shows, most significant first.
///
/// Empty for an avoided hit: wire arm 2 carries no damage word at all, so its
/// `amount` is 0 and a plain decimal decomposition would render a literal "0"
/// under the BLOCK word.
fn digits_of(amount: u32, blocked: bool) -> Vec<usize> {
    if blocked {
        return Vec::new();
    }
    let mut n = amount as usize;
    let mut ds = Vec::new();
    loop {
        ds.push(n % 10);
        n /= 10;
        if n == 0 || ds.len() == MAX_DIGITS {
            break;
        }
    }
    ds.reverse();
    ds
}

fn set_index(set: HitcountSet) -> usize {
    match set {
        HitcountSet::Dealt => 0,
        HitcountSet::Received => 1,
        HitcountSet::Other => 2,
    }
}

pub fn spawn_hitcount_pool(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found for hitcount popups");
        return;
    };

    // Suffix order matches `set_index`: own damage, from-monster, from-player.
    let load_set = |suffix: &str| {
        std::array::from_fn(|d| {
            asset_server.load(format!(
                "media://interface/hitcount/hitcount_{suffix}{d}.ddj"
            ))
        })
    };
    commands.insert_resource(HitcountAssets {
        digits: [load_set(""), load_set("enemy_"), load_set("player_")],
        critical: ["", "_enemy", "_player"].map(|suffix| {
            asset_server.load(format!("media://interface/hitcount/critical{suffix}.ddj"))
        }),
        blocking: ["", "_enemy", "_player"].map(|suffix| {
            asset_server.load(format!("media://interface/hitcount/blocking{suffix}.ddj"))
        }),
    });

    for _ in 0..POOL_SIZE {
        commands
            .spawn((
                HitcountSlot,
                Node {
                    position_type: PositionType::Absolute,
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    ..default()
                },
                // anchor the number's bottom-centre on the projected point
                UiTransform::from_translation(Val2::new(Val::Percent(-50.0), Val::Percent(-100.0))),
                // above nameplates (10), below the HUD windows (50)
                GlobalZIndex(20),
                Visibility::Hidden,
                Pickable::IGNORE,
                UiTargetCamera(camera),
            ))
            .with_children(|slot| {
                // child 0: the digit row (digits claimed left-to-right)
                slot.spawn((
                    Node {
                        flex_direction: FlexDirection::Row,
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|row| {
                    for _ in 0..MAX_DIGITS {
                        row.spawn((
                            HitcountGlyph,
                            ImageNode::default(),
                            Node {
                                display: Display::None,
                                ..default()
                            },
                            Pickable::IGNORE,
                        ));
                    }
                });
                // child 1: the "critical" word below the number
                slot.spawn((
                    HitcountGlyph,
                    ImageNode::default(),
                    Node {
                        display: Display::None,
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            });
    }
}

pub fn cleanup_hitcounts(mut commands: Commands, slots: Query<Entity, With<HitcountSlot>>) {
    for entity in slots.iter() {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<HitcountAssets>();
}

/// The glyph image nodes of all slots (`Without` keeps it disjoint from the
/// slot-root query despite both touching `Node`).
type GlyphQuery<'w, 's> = Query<
    'w,
    's,
    (&'static mut ImageNode, &'static mut Node),
    (With<HitcountGlyph>, Without<HitcountSlot>),
>;

/// A popup waiting out its hit-moment delay before claiming a slot.
pub struct PendingPopup {
    remaining: f32,
    popup: DamagePopup,
}

/// Collect new popups, release each when its hit-moment delay elapses, then
/// age/float/fade every active slot.
#[allow(clippy::too_many_arguments)]
pub fn update_hitcounts(
    mut commands: Commands,
    time: Res<Time>,
    assets: Option<Res<HitcountAssets>>,
    mut popups: MessageReader<DamagePopup>,
    camera: Query<(&Camera, &GlobalTransform), With<PlayerCamera>>,
    anchors: Query<&GlobalTransform>,
    mut slots: Query<
        (Entity, &mut Node, &mut Visibility, Option<&mut ActivePopup>),
        With<HitcountSlot>,
    >,
    children: Query<&Children>,
    mut glyphs: GlyphQuery,
    mut pending: Local<Vec<PendingPopup>>,
) {
    let Some(assets) = assets else {
        popups.clear();
        pending.clear();
        return;
    };
    for popup in popups.read() {
        pending.push(PendingPopup {
            remaining: popup.delay,
            popup: popup.clone(),
        });
    }

    // --- release due popups into free slots ---
    let mut free = slots
        .iter()
        .filter_map(|(entity, _, _, active)| active.is_none().then_some(entity))
        .collect::<Vec<_>>()
        .into_iter();
    let dt = time.delta_secs();
    let mut i = 0;
    while i < pending.len() {
        pending[i].remaining -= dt;
        if pending[i].remaining > 0.0 {
            i += 1;
            continue;
        }
        let popup = pending.swap_remove(i).popup;
        let Some(slot) = free.next() else {
            continue; // all slots busy — drop the number
        };
        // Follow the anchor while it lives; a despawned anchor (killing
        // blows land after the corpse cleanup) falls back to the snapshot.
        let base = anchors
            .get(popup.anchor)
            .map(|gt| gt.translation())
            .unwrap_or(popup.world);
        let set = set_index(popup.set);
        let digits = digits_of(popup.amount, popup.blocked);
        let Ok(slot_children) = children.get(slot) else {
            continue;
        };
        // child 0 = digit row, child 1 = the word glyph (see the pool spawn)
        if let Ok(row_children) = children.get(slot_children[0]) {
            for (i, child) in row_children.iter().enumerate() {
                let Ok((mut image, mut node)) = glyphs.get_mut(child) else {
                    continue;
                };
                match digits.get(i) {
                    Some(&d) => {
                        image.image = assets.digits[set][d].clone();
                        image.color = Color::WHITE;
                        node.display = Display::Flex;
                    }
                    None => node.display = Display::None,
                }
            }
        }
        // One word slot, two mutually exclusive words: a critical needs a
        // number and an avoided hit has none, so the two can never coincide.
        if let Ok((mut image, mut node)) = glyphs.get_mut(slot_children[1]) {
            let word = if popup.blocked {
                Some(&assets.blocking[set])
            } else if popup.critical {
                Some(&assets.critical[set])
            } else {
                None
            };
            match word {
                Some(handle) => {
                    image.image = handle.clone();
                    image.color = Color::WHITE;
                    node.display = Display::Flex;
                }
                None => node.display = Display::None,
            }
        }
        commands.entity(slot).insert(ActivePopup {
            age: 0.0,
            anchor: popup.anchor,
            world: base + Vec3::Y * HEAD_OFFSET,
        });
    }

    // --- age, follow, project, fade ---
    let cam = camera.iter().find(|(c, _)| c.is_active);
    for (entity, mut node, mut visibility, active) in slots.iter_mut() {
        let Some(mut active) = active else {
            continue;
        };
        active.age += time.delta_secs();
        if active.age >= LIFETIME {
            *visibility = Visibility::Hidden;
            commands.entity(entity).remove::<ActivePopup>();
            continue;
        }
        if let Ok(gt) = anchors.get(active.anchor) {
            active.world = gt.translation() + Vec3::Y * HEAD_OFFSET;
        }
        let projected = cam.and_then(|(camera, cam_gt)| {
            if active.world.distance(cam_gt.translation()) > MAX_DISTANCE {
                return None;
            }
            camera.world_to_viewport(cam_gt, active.world).ok()
        });
        let Some(px) = projected else {
            if *visibility != Visibility::Hidden {
                *visibility = Visibility::Hidden;
            }
            continue;
        };
        let t = active.age / LIFETIME;
        // ease-out rise: fast at spawn, settling near the top
        let rise = RISE_PX * t * (2.0 - t);
        node.left = Val::Px(px.x);
        node.top = Val::Px(px.y - rise);
        let alpha = ((1.0 - t) / (1.0 - FADE_START)).min(1.0);
        for child in children.iter_descendants(entity) {
            if let Ok((mut image, _)) = glyphs.get_mut(child) {
                image.color = Color::WHITE.with_alpha(alpha);
            }
        }
        if *visibility != Visibility::Inherited {
            *visibility = Visibility::Inherited;
        }
    }
}

/// Self-registration shim for the floating damage numbers.
///
/// The systems above are this module's pre-#506 form, deliberately restored.
/// Only this wrapper is new: the HUD registry stopped taking loose systems in
/// #589 and now names one `Plugin` per window, a contract ~40 windows share,
/// so the three systems are registered here instead of in `hud/mod.rs`.
///
/// The schedules and the run condition are the ones this module had before
/// that refactor — `GameWorld` for the pool's lifetime and
/// `GameWorld | UiTesting` for the per-frame update. Notably NOT
/// `super::hud_scenes`, which is a later helper and would additionally run the
/// numbers in the offline `Skills` sandbox.
pub struct HitcountPlugin;

impl Plugin for HitcountPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.add_systems(OnEnter(SceneState::GameWorld), spawn_hitcount_pool)
            .add_systems(OnExit(SceneState::GameWorld), cleanup_hitcounts)
            .add_systems(
                Update,
                update_hitcounts.run_if(
                    in_state(SceneState::GameWorld).or_else(in_state(SceneState::UiTesting)),
                ),
            );
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// An avoided hit shows the BLOCK word alone. Its `amount` is 0 — the wire
    /// record carries no damage word — so the digit row must stay empty rather
    /// than print a "0" the server never sent.
    #[test]
    fn a_blocked_popup_renders_no_digits() {
        assert_eq!(digits_of(0, true), Vec::<usize>::new());
        // ...even if something ever hands it an amount.
        assert_eq!(digits_of(123, true), Vec::<usize>::new());
    }

    /// An ordinary amount still decomposes most-significant-first, and a
    /// genuine zero still prints one glyph.
    #[test]
    fn an_ordinary_amount_still_renders_its_digits() {
        assert_eq!(digits_of(0, false), vec![0]);
        assert_eq!(digits_of(1803, false), vec![1, 8, 0, 3]);
    }
}
