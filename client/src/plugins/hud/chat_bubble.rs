//! World-space chat speech bubbles over the speaker's head.
//!
//! Idea: the pooled absolute-node overlay shape shared with
//! [`super::hitcount`] — both project their world anchor through
//! [`super::world_anchor`], which owns the cutoff and the projection so the
//! two cannot drift apart (#661) — with the body assembled from the original's own art
//! (`media://interface/chat/chatwindow_{small,big}_{side,middle}.ddj`) instead
//! of digit glyphs. That art fixes the geometry: it is a horizontal 3-slice,
//! not a 9-slice — an 8px cap at each end (`_side`, mirrored for the right end
//! because it is the only side texture and its corner is asymmetric) with the
//! 4px `_middle` tile repeated between them. Only two heights exist, 28px and
//! 48px, and their 20px difference is exactly one text line, so a bubble is
//! 4px of chrome + n*20px of lines + 4px of chrome and is never given an
//! interpolated height. See `docs/re/ui/chat-bubble-widget.md`.

use bevy::prelude::*;
use bevy::text::{LineHeight, TextLayoutInfo};
use bevy::ui::{UiTargetCamera, UiTransform, Val2};

use packets::agent::chat::ChatUpdate;

use crate::assets::FontAssets;
use crate::plugins::camera::PlayerCamera;
use crate::plugins::hud::world_anchor::project_world_anchor;
use crate::plugins::net::entities::NetworkEntities;

/// Concurrent bubbles; further speakers while all slots are busy are dropped.
const POOL_SIZE: usize = 16;
/// Width of an end cap — `chatwindow_*_side.ddj` is 8px wide.
const CAP_W: f32 = 8.0;
/// Width of the repeating middle tile — `chatwindow_*_middle.ddj` is 4px wide.
const TILE_W: f32 = 4.0;
/// Chrome on the top and bottom edges. The middle tile's alpha rows are a
/// 4-row taper, a constant run, then a 4-row taper — 4+20+4 in the 28px art
/// and 4+40+4 in the 48px one. Only the vertical chrome is 4px; horizontally
/// the fixed piece is the 8px cap.
const CHROME: f32 = 4.0;
/// Interior height of one text line — the constant run of the middle tile,
/// and equally the 48px art less the 28px art.
const LINE_H: f32 = 20.0;
/// The art ships a one-line and a two-line height and nothing taller.
const MAX_LINES: usize = 2;
/// Text width before wrapping. **Ours**: the wrap point is not in the data.
/// `GDR_CHATBUBBLEWINDOW_CHATBOX` carries `Rect="0,6,310,28"`, the only
/// text-width number in the tree, but that file's rects match no real art
/// dimension and are editor placeholders — so this is a choice anchored on
/// the least arbitrary number available, not a recovered value.
const MAX_TEXT_W: f32 = 310.0;
/// Matches the chat log (`super::chat::ui::MESSAGE_FONT_SIZE`). **Ours**: the
/// tree's `FontIndex=7` is unique to this widget in the whole resinfo corpus
/// and no `FontSize` is bound anywhere, so the original's size is unknown.
/// `docs/re/ui/hud-chat.md` records that `Media/fonts/` does ship real TTFs —
/// revisit this size when the real face is wired.
const FONT_SIZE: f32 = 12.0;
/// Anchor height above the entity origin, above the nameplate's 23 so the two
/// do not overlap. **Ours** — the original's offset is not in the data.
const HEAD_OFFSET: f32 = 34.0;
/// Seconds a bubble stays up. **Ours** — dwell time is not in the data.
const LIFETIME: f32 = 5.0;

/// One slot root of the bubble pool.
#[derive(Component)]
pub struct ChatBubbleSlot;

/// Which piece of the 3-slice body a slot child draws.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum BubbleSlice {
    Left,
    Middle,
    Right,
}

/// The message text inside a slot.
#[derive(Component)]
pub struct BubbleLabel;

/// Live state of a claimed slot.
#[derive(Component)]
pub struct ActiveBubble {
    age: f32,
    /// The speaker; the bubble follows it and ends when it despawns.
    anchor: Entity,
}

/// The 3-slice art, indexed by line count - 1 (small = 1 line, big = 2).
#[derive(Resource)]
pub struct ChatBubbleAssets {
    side: [Handle<Image>; MAX_LINES],
    middle: [Handle<Image>; MAX_LINES],
}

/// Bubble width for a laid-out text width: an 8px cap at each end around a
/// whole number of 4px middle tiles, so the narrowest bubble is 16px.
fn bubble_width(text_w: f32) -> f32 {
    let tiles = (text_w.max(0.0) / TILE_W).ceil();
    CAP_W * 2.0 + tiles * TILE_W
}

/// Bubble height for a line count. The art has exactly two heights, so this
/// only ever returns 28 or 48 — never an interpolation between them.
fn bubble_height(lines: usize) -> f32 {
    CHROME * 2.0 + LINE_H * lines.clamp(1, MAX_LINES) as f32
}

/// How many text lines a laid-out height covers, clamped to what the art can
/// draw. [`LineHeight::Px`] pins every line to [`LINE_H`], so this is exact.
fn bubble_lines(text_h: f32) -> usize {
    ((text_h / LINE_H).round() as usize).clamp(1, MAX_LINES)
}

/// Asset path of one 3-slice piece. `lines` picks the height variant, which
/// the art spells `small` (28px, one line) and `big` (48px, two lines).
/// A typo here is invisible until an asset fails to load at runtime, so the
/// four real corpus filenames are pinned by a test.
fn bubble_asset_path(lines: usize, piece: &str) -> String {
    let size = if lines >= MAX_LINES { "big" } else { "small" };
    format!("media://interface/chat/chatwindow_{size}_{piece}.ddj")
}

pub fn spawn_chat_bubble_pool(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found for chat bubbles");
        return;
    };

    // Index order matches `bubble_lines() - 1`: one-line art, two-line art.
    let load =
        |piece: &str| std::array::from_fn(|i| asset_server.load(bubble_asset_path(i + 1, piece)));
    commands.insert_resource(ChatBubbleAssets {
        side: load("side"),
        middle: load("middle"),
    });

    for _ in 0..POOL_SIZE {
        commands
            .spawn((
                ChatBubbleSlot,
                Node {
                    position_type: PositionType::Absolute,
                    // a message too long for the two-line art is clipped —
                    // the original's behaviour past two lines is unknown
                    overflow: Overflow::clip(),
                    ..default()
                },
                // anchor the bubble's bottom-centre on the projected point
                UiTransform::from_translation(Val2::new(Val::Percent(-50.0), Val::Percent(-100.0))),
                // above the nameplates (10), below the HUD windows (50)
                GlobalZIndex(15),
                Visibility::Hidden,
                Pickable::IGNORE,
                UiTargetCamera(camera),
            ))
            .with_children(|slot| {
                // The body is drawn before the text so the text sits on top.
                slot.spawn((
                    BubbleSlice::Left,
                    ImageNode::default(),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(0.0),
                        top: Val::Px(0.0),
                        width: Val::Px(CAP_W),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                // no width: pinned to both caps, so it spans whatever is left
                slot.spawn((
                    BubbleSlice::Middle,
                    ImageNode {
                        image_mode: NodeImageMode::Tiled {
                            tile_x: true,
                            tile_y: false,
                            stretch_value: 1.0,
                        },
                        ..default()
                    },
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(CAP_W),
                        right: Val::Px(CAP_W),
                        top: Val::Px(0.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                slot.spawn((
                    BubbleSlice::Right,
                    // the only side texture is the left cap; its corner is
                    // asymmetric, so the right end is the same art mirrored
                    ImageNode {
                        flip_x: true,
                        ..default()
                    },
                    Node {
                        position_type: PositionType::Absolute,
                        right: Val::Px(0.0),
                        top: Val::Px(0.0),
                        width: Val::Px(CAP_W),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                slot.spawn((
                    BubbleLabel,
                    Text::default(),
                    TextFont {
                        font: fonts.three.clone().into(),
                        font_size: FontSize::Px(FONT_SIZE),
                        ..default()
                    },
                    // every channel is white in the tree (all three controls
                    // carry FontColor="255,255,255,255"); colouring a bubble
                    // per channel would be non-original
                    TextColor(Color::WHITE),
                    // pins each line to the art's 20px line box, which is what
                    // makes the 28/48 heights exact
                    LineHeight::Px(LINE_H),
                    TextLayout::new(Justify::Left, LineBreak::WordBoundary),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(CAP_W),
                        top: Val::Px(CHROME),
                        max_width: Val::Px(MAX_TEXT_W),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            });
    }
}

pub fn cleanup_chat_bubbles(mut commands: Commands, slots: Query<Entity, With<ChatBubbleSlot>>) {
    for entity in slots.iter() {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<ChatBubbleAssets>();
}

/// The label of each slot, with the size its text laid out to.
type LabelQuery<'w, 's> =
    Query<'w, 's, (&'static mut Text, &'static TextLayoutInfo), With<BubbleLabel>>;

/// The 3-slice pieces of all slots.
type SliceQuery<'w, 's> = Query<
    'w,
    's,
    (&'static BubbleSlice, &'static mut ImageNode),
    (With<BubbleSlice>, Without<ChatBubbleSlot>),
>;

/// Claim a slot per incoming proximity line, then age, resize and project
/// every live bubble.
#[allow(clippy::too_many_arguments)]
pub fn update_chat_bubbles(
    mut commands: Commands,
    time: Res<Time>,
    assets: Option<Res<ChatBubbleAssets>>,
    entities: Res<NetworkEntities>,
    mut updates: MessageReader<ChatUpdate>,
    camera: Query<(&Camera, &GlobalTransform), With<PlayerCamera>>,
    anchors: Query<&GlobalTransform>,
    mut slots: Query<
        (
            Entity,
            &mut Node,
            &mut Visibility,
            Option<&mut ActiveBubble>,
        ),
        With<ChatBubbleSlot>,
    >,
    children: Query<&Children>,
    mut labels: LabelQuery,
    mut slices: SliceQuery,
) {
    let Some(assets) = assets else {
        updates.clear();
        return;
    };

    // --- claim a slot per speaker ---
    for update in updates.read() {
        // Only the proximity channels carry a sender id; the name-only
        // channels (party, guild, ...) have no entity to hang a bubble on.
        // NPC speech belongs here rather than with the dialog window: it is an
        // ambient broadcast sharing the same `u32 sender_id` wire shape as
        // All/AllGm (`docs/re/net/wired-opcode-audit.md:251`).
        // Own speech arrives here too — the log drops it as a duplicate of the
        // 0xB025 ack, but for a bubble it is the only source.
        let (ChatUpdate::All { sender_id, message }
        | ChatUpdate::AllGm { sender_id, message }
        | ChatUpdate::Npc { sender_id, message }) = update
        else {
            continue;
        };
        let Some(anchor) = entities.get(*sender_id) else {
            continue;
        };
        // A speaker has one bubble: talking again replaces it rather than
        // stacking a second slot on the same head.
        let slot = slots
            .iter()
            .find(|(_, _, _, active)| active.as_ref().is_some_and(|a| a.anchor == anchor))
            .map(|(entity, ..)| entity)
            .or_else(|| {
                slots
                    .iter()
                    .find(|(_, _, _, active)| active.is_none())
                    .map(|(entity, ..)| entity)
            });
        let Some(slot) = slot else {
            continue; // all slots busy — drop the bubble
        };
        for child in children.iter_descendants(slot) {
            if let Ok((mut text, _)) = labels.get_mut(child) {
                text.0 = message.clone();
            }
        }
        commands
            .entity(slot)
            .insert(ActiveBubble { age: 0.0, anchor });
    }

    // --- age, resize, follow, project ---
    let cam = camera.iter().find(|(c, _)| c.is_active);
    for (entity, mut node, mut visibility, active) in slots.iter_mut() {
        let Some(mut active) = active else {
            continue;
        };
        active.age += time.delta_secs();
        let expired = active.age >= LIFETIME;
        // the speaker walking out of the world takes its bubble with it
        let world = anchors
            .get(active.anchor)
            .map(|gt| gt.translation() + Vec3::Y * HEAD_OFFSET);
        let Ok(world) = world else {
            *visibility = Visibility::Hidden;
            commands.entity(entity).remove::<ActiveBubble>();
            continue;
        };
        if expired {
            *visibility = Visibility::Hidden;
            commands.entity(entity).remove::<ActiveBubble>();
            continue;
        }

        // Size from the laid-out text. It settles one frame after the text is
        // set, so an unmeasured bubble stays hidden rather than flashing at
        // the wrong size.
        let mut measured = None;
        for child in children.iter_descendants(entity) {
            if let Ok((_, info)) = labels.get(child) {
                if info.size.y <= 0.0 {
                    break;
                }
                let lines = bubble_lines(info.size.y);
                measured = Some((bubble_width(info.size.x), bubble_height(lines), lines));
                break;
            }
        }
        let Some((width, height, lines)) = measured else {
            if *visibility != Visibility::Hidden {
                *visibility = Visibility::Hidden;
            }
            continue;
        };
        for child in children.iter_descendants(entity) {
            if let Ok((slice, mut image)) = slices.get_mut(child) {
                let wanted = match slice {
                    BubbleSlice::Left | BubbleSlice::Right => assets.side[lines - 1].clone(),
                    BubbleSlice::Middle => assets.middle[lines - 1].clone(),
                };
                if image.image != wanted {
                    image.image = wanted;
                }
            }
        }

        let projected =
            cam.and_then(|(camera, cam_gt)| project_world_anchor(camera, cam_gt, world));
        let Some(px) = projected else {
            if *visibility != Visibility::Hidden {
                *visibility = Visibility::Hidden;
            }
            continue;
        };
        node.left = Val::Px(px.x);
        node.top = Val::Px(px.y);
        node.width = Val::Px(width);
        node.height = Val::Px(height);
        if *visibility != Visibility::Inherited {
            *visibility = Visibility::Inherited;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two art heights, from the DDS headers of
    /// `Media/interface/chat/chatwindow_{small,big}_middle.ddj` (4x28 and
    /// 4x48). A bubble may only ever be one of these.
    #[test]
    fn height_is_one_of_the_two_art_variants() {
        assert_eq!(bubble_height(1), 28.0);
        assert_eq!(bubble_height(2), 48.0);
        // a longer message still gets the two-line art, never a taller box
        assert_eq!(bubble_height(3), 48.0);
        assert_eq!(bubble_height(0), 28.0);
    }

    /// `width = 8 + n*4 + 8`, so the narrowest bubble is the two caps alone.
    #[test]
    fn width_is_two_caps_around_whole_tiles() {
        assert_eq!(bubble_width(0.0), 16.0);
        assert_eq!(bubble_width(4.0), 20.0);
        assert_eq!(bubble_width(8.0), 24.0);
        // partial tiles round up to a whole 4px tile
        assert_eq!(bubble_width(0.1), 20.0);
        assert_eq!(bubble_width(5.0), 24.0);
        // the widest text the tree allows, 310px, is 78 tiles
        assert_eq!(bubble_width(MAX_TEXT_W), 328.0);
    }

    /// `LineHeight::Px(LINE_H)` pins a line to 20px, so the laid-out height
    /// maps back to a line count exactly.
    #[test]
    fn lines_come_from_the_20px_line_box() {
        assert_eq!(bubble_lines(20.0), 1);
        assert_eq!(bubble_lines(40.0), 2);
        // clamped to what the art can draw
        assert_eq!(bubble_lines(60.0), 2);
        assert_eq!(bubble_lines(0.0), 1);
    }

    /// The art's 20px height difference is one line of text, which is what
    /// lets both variants share one chrome constant.
    #[test]
    fn art_height_difference_is_one_line() {
        assert_eq!(bubble_height(2) - bubble_height(1), LINE_H);
    }

    /// The exact four files in `Media/interface/chat/`. These names are the
    /// whole reason the widget renders: nothing in the PK2 data references
    /// them (`grep -a -rn chatwindow_ resinfo/` -> 0, 0 hits across all 42
    /// `res_ui/*.2dt`), so the code is the only place they are spelled and a
    /// typo would be silent until load. The one-line bubble takes the
    /// `small` art, the two-line bubble the `big` art.
    #[test]
    fn asset_paths_are_the_four_real_corpus_files() {
        assert_eq!(
            bubble_asset_path(1, "side"),
            "media://interface/chat/chatwindow_small_side.ddj"
        );
        assert_eq!(
            bubble_asset_path(1, "middle"),
            "media://interface/chat/chatwindow_small_middle.ddj"
        );
        assert_eq!(
            bubble_asset_path(2, "side"),
            "media://interface/chat/chatwindow_big_side.ddj"
        );
        assert_eq!(
            bubble_asset_path(2, "middle"),
            "media://interface/chat/chatwindow_big_middle.ddj"
        );
    }

    /// Every geometry constant read back out of the DDJ headers of those four
    /// files (uncompressed A8R8G8B8, so the dimensions are the DDS header's
    /// own words): `_side` is 8px wide, `_middle` is 4px wide, and the two
    /// height variants are 28px and 48px. The 4px chrome is likewise measured,
    /// not assumed -- the middle tile's alpha column runs 128,128,76,102 then
    /// a constant 128 run then the mirror of that taper, i.e. 4 + 20 + 4 rows
    /// in the small art and 4 + 40 + 4 in the big one.
    #[test]
    fn constants_match_the_ddj_headers() {
        assert_eq!(CAP_W, 8.0);
        assert_eq!(TILE_W, 4.0);
        assert_eq!(CHROME, 4.0);
        assert_eq!(LINE_H, 20.0);
        // the two art heights fall straight out of chrome + n lines
        assert_eq!(CHROME * 2.0 + LINE_H, 28.0);
        assert_eq!(CHROME * 2.0 + LINE_H * 2.0, 48.0);
        // and the art ships nothing taller
        assert_eq!(MAX_LINES, 2);
    }
}

/// Self-registration for the over-head chat bubbles (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct ChatBubblePlugin;

impl Plugin for ChatBubblePlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.add_systems(OnEnter(SceneState::GameWorld), spawn_chat_bubble_pool)
            .add_systems(OnExit(SceneState::GameWorld), cleanup_chat_bubbles)
            .add_systems(Update, update_chat_bubbles.run_if(super::hud_scenes));
    }
}
