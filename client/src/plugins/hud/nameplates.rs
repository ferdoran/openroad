//! Floating name labels over players, NPCs and monsters.
//!
//! Idea: each frame we project every named entity's head point into the
//! viewport with the game (3D follow) camera and drive a fixed pool of
//! absolute-positioned text nodes — the same "iterate world entities → move
//! pooled UI nodes, toggle Visibility" structure the minimap uses for its dots
//! ([`super::minimap::update_minimap_dots`]). Entity transforms are already in
//! render space (what the camera projects), so unlike the minimap there is no
//! world-origin conversion here. The label source is the uniform [`DisplayName`]
//! component, set at spawn for players (packet name) and NPCs/monsters
//! (localized characterdata name).

use bevy::ecs::system::SystemParam;
use bevy::light::NotShadowCaster;
use bevy::log::warn_once;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings};
use bevy::prelude::*;
use bevy::ui::{UiTargetCamera, UiTransform, Val2};

use crate::assets::FontAssets;
use crate::net::reader::GuildTag;
use crate::plugins::camera::PlayerCamera;
use crate::plugins::config::nameplates::NameplateColors;
use crate::plugins::cos::CosOwner;
use crate::plugins::cursor::interactions::entity_select::{HoveredEntity, SelectedEntity};
use crate::plugins::hud::chat::model::ChatState;
use crate::plugins::net::entities::{
    DisplayName, DropRarity, RemoteEntity, SealDrop, UniqueMonster,
};
use crate::plugins::net::stall::StallOwner;
use crate::plugins::player::Player;
use crate::plugins::settings::keymap::KEY_VIEW_DROP_ITEM;
use crate::plugins::settings::options::GameOptions;

/// Pooled nameplate nodes; labels beyond this many on screen at once are dropped.
const POOL_SIZE: usize = 96;
/// Height above an entity's origin (its feet) to anchor the label, in render
/// units. The follow camera targets height 20 (`CAMERA_TARGET_HEIGHT` in
/// `camera.rs`), so a head sits a little above that — tune visually.
const HEAD_OFFSET: f32 = 23.0;
/// Label anchor above a ground item (a small pile, not a standing body).
const ITEM_OFFSET: f32 = 8.0;
/// Labels farther than this from the camera are hidden.
const MAX_DISTANCE: f32 = 600.0;
/// How long an entity's line-of-sight verdict is reused before it is cast
/// again. Each verdict costs a `MeshRayCast` against the whole streamed static
/// world (an AABB broadphase plus per-triangle tests), and re-casting every
/// label every frame made the cost scale with crowd size — exactly when the
/// client can least afford it. A label reacting to cover ~0.15 s late is not
/// perceptible; a stutter in a crowded town is.
const OCCLUSION_REFRESH_SECS: f32 = 0.15;

/// Per-entity line-of-sight verdicts and when each is next due for a re-cast.
///
/// Refreshes are staggered by entity index so a crowd that spawns together does
/// not then re-cast together on one frame, which would just move the spike
/// rather than remove it.
#[derive(Default)]
pub struct OcclusionCache {
    /// entity -> (occluded, earliest time to cast again)
    verdicts: std::collections::HashMap<Entity, (bool, f32)>,
}

/// The line-of-sight machinery, bundled because `update_nameplates` sits at
/// Bevy's 16-system-parameter ceiling and these three always travel together.
#[derive(SystemParam)]
pub struct Occlusion<'w, 's> {
    raycast: MeshRayCast<'w, 's>,
    /// Characters (skinned meshes) and non-solid art (decals, effects, skybox —
    /// all `NotShadowCaster`) never hide a label.
    non_occluders: Query<'w, 's, (), Or<(With<NotShadowCaster>, With<SkinnedMesh>)>>,
    cache: Local<'s, OcclusionCache>,
}

impl OcclusionCache {
    /// Spread of the refresh deadline across the interval, from the entity id.
    fn stagger(entity: Entity) -> f32 {
        (entity.to_bits() % 16) as f32 / 16.0 * OCCLUSION_REFRESH_SECS
    }
}
const FONT_SIZE: f32 = 13.0;

/// `SROptionSet` display toggles that gate the plates (docs/formats/sroptionset.md,
/// `UIIT_STT_*_SIGN`): own name, other players, monsters, NPCs — and the guild
/// line. An id we have never received defaults to **enabled** (§9-U3: the
/// NAMEVIEW tab has 6 rows but only 5 known ids).
const OPT_OWN_NAME: u16 = 2010;
const OPT_OTHER_NAME: u16 = 2011;
const OPT_MONSTER_NAME: u16 = 2012;
const OPT_NPC_NAME: u16 = 2013;
const OPT_GUILD_NAME: u16 = 2014;

/// One slot of the nameplate pool.
#[derive(Component)]
pub struct Nameplate;

/// Marks the sub-line span child of a pooled plate: the lines *under* the
/// name. Today that is the guild tag and, since #782, the stall title of a
/// player running a stall — one span, so the pool stays one text entity per
/// plate instead of one per possible line.
#[derive(Component)]
pub struct NameplateSubLines;

/// Whether a display toggle is on. Ids we have never been told about default to
/// enabled, so a stock client shows everything (docs/re/ui/hud-nameplates.md §9-U3).
fn toggle_on(options: &GameOptions, id: u16) -> bool {
    options.gameplay.toggles.get(&id).copied().unwrap_or(true)
}

/// The toggle gating an entity kind's plate.
fn plate_toggle(kind: RemoteEntity) -> Option<u16> {
    match kind {
        RemoteEntity::Player => Some(OPT_OTHER_NAME),
        RemoteEntity::Monster => Some(OPT_MONSTER_NAME),
        RemoteEntity::Npc => Some(OPT_NPC_NAME),
        // ground items are governed by the KeyViewDropItem binding (id 3012),
        // not by a _SIGN toggle
        RemoteEntity::Item => None,
    }
}

/// Whether the **held** `KeyViewDropItem` binding (id 3012) is labelling every
/// dropped item in range at once — the original's bulk read, so a player can
/// take in a whole drop pile in one look (`docs/re/ui/hud-nameplates.md` §1 —
/// the tooltip is "Dropped items' names can be checked by clicking
/// corresponding button").
///
/// This is the *additional* mode, not the only one: a drop under the cursor is
/// always labelled. `KEY_VIEW_DROP_ITEM` ships with **no** default binding
/// (`OptionSet.csv` declares none), so making the bulk key the sole path left
/// drop names unreachable out of the box — hovering a drop showed nothing at
/// all until the player went and bound a key.
///
/// Typing in chat must not reveal the pile, hence the `input_open` guard the
/// six other keyed HUD modules use.
/// Keyboard/chat/config inputs of [`update_nameplates`], bundled into one
/// `SystemParam`: the system already sits at Bevy's 16-parameter ceiling, so
/// three loose `Res` arguments would not compile.
#[derive(SystemParam)]
pub struct NameplateInput<'w> {
    keys: Res<'w, ButtonInput<KeyCode>>,
    chat: Res<'w, ChatState>,
    config: Res<'w, crate::plugins::config::ClientConfig>,
    time: Res<'w, Time>,
}

fn drop_item_names_held(
    keys: &ButtonInput<KeyCode>,
    options: &GameOptions,
    chat_open: bool,
) -> bool {
    !chat_open
        && options
            .key_for(KEY_VIEW_DROP_ITEM)
            .is_some_and(|k| keys.pressed(k))
}

/// The lines under an entity's name, newline-prefixed so they append to the
/// name span: the guild tag first (when the toggle allows it), then the stall
/// title of a player running a stall (#782), then the summoner of a COS.
///
/// All are one span, and all are plain text — the stall title reaches a screen
/// reader as words for exactly that reason. **No stall toggle exists**: the
/// NAMEVIEW tab has six rows and only five ids are known
/// (`docs/re/ui/hud-nameplates.md` §9-U3), so gating this on an id would mean
/// inventing one. The guild toggle is untouched.
///
/// The COS owner is `[S]`: the spawn record carries `OwnerName` for every
/// non-horse COS (`docs/re/systems/pet-growth-cos.md` §3) and nothing else
/// consumes it, which is the whole of the evidence that vanilla draws it.
fn sub_lines_text(
    guild: Option<&GuildTag>,
    stall: Option<&StallOwner>,
    cos_owner: Option<&CosOwner>,
) -> String {
    let mut out = String::new();
    if let Some(tag) = guild {
        out.push('\n');
        out.push_str(&tag.name);
    }
    if let Some(stall) = stall.filter(|s| !s.title.trim().is_empty()) {
        out.push('\n');
        out.push_str(&stall.title);
    }
    if let Some(owner) = cos_owner.filter(|o| !o.0.trim().is_empty()) {
        out.push('\n');
        out.push_str(&owner.0);
    }
    out
}

/// The label colour per entity kind, from the configurable [`NameplateColors`].
///
/// Ground drops follow the same precedence the inventory tooltip uses for an
/// item's name (`inventory/tooltip.rs`): gold for Seal-grade, else blue when
/// the drop carries magic ("blue") options, else plain. The two inputs come
/// from different places because a drop record has no item body —
/// [`SealDrop`] is itemdata-derived (the `_RARE` code-name suffix, which also
/// drives the sparkle pillar), [`DropRarity`] is the spawn packet's rarity
/// byte.
fn label_color(
    colors: &NameplateColors,
    kind: RemoteEntity,
    unique: bool,
    drop: Option<&DropRarity>,
    seal: bool,
) -> Color {
    match kind {
        RemoteEntity::Monster if unique => colors.unique_monster,
        RemoteEntity::Monster => colors.monster,
        RemoteEntity::Npc => colors.npc,
        RemoteEntity::Player => colors.player,
        RemoteEntity::Item if seal => colors.item_rare,
        RemoteEntity::Item if drop.is_some_and(DropRarity::has_options) => colors.item_magic,
        RemoteEntity::Item => colors.item,
    }
}

pub fn spawn_nameplate_pool(
    mut commands: Commands,
    fonts: Res<FontAssets>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found for nameplates");
        return;
    };

    for _ in 0..POOL_SIZE {
        commands
            .spawn((
                Nameplate,
                Text::default(),
                TextFont {
                    font: fonts.nine.clone().into(),
                    font_size: FontSize::Px(FONT_SIZE),
                    ..default()
                },
                TextColor(Color::WHITE),
                TextLayout::justify(Justify::Center),
                // a 1px dark drop shadow keeps light text legible over the 3d scene
                TextShadow {
                    offset: Vec2::splat(1.0),
                    color: Color::BLACK,
                },
                Node {
                    position_type: PositionType::Absolute,
                    ..default()
                },
                // anchor the text's bottom-centre on the projected head point:
                // shift left by half its width and up by its full height
                UiTransform::from_translation(Val2::new(Val::Percent(-50.0), Val::Percent(-100.0))),
                // below the HUD windows (the mini-info panel sits at GlobalZIndex(50))
                GlobalZIndex(10),
                Visibility::Hidden,
                Pickable::IGNORE,
                UiTargetCamera(camera),
            ))
            // the sub-lines: an empty second span, filled per frame with the
            // guild tag and/or the stall title and blanked for everyone else
            .with_child((
                NameplateSubLines,
                TextSpan::default(),
                TextFont {
                    font: fonts.nine.clone().into(),
                    font_size: FontSize::Px(FONT_SIZE),
                    ..default()
                },
                TextColor(Color::WHITE),
            ));
    }
}

pub fn cleanup_nameplates(mut commands: Commands, plates: Query<Entity, With<Nameplate>>) {
    for entity in plates.iter() {
        commands.entity(entity).despawn();
    }
}

/// Project each labelled entity's head into the viewport and drive the pool:
/// on-screen, in-range labels take a slot; the remainder are hidden.
pub fn update_nameplates(
    colors: Res<NameplateColors>,
    options: Res<GameOptions>,
    hovered: Res<HoveredEntity>,
    selected: Res<SelectedEntity>,
    input: NameplateInput,
    fonts: Res<FontAssets>,
    camera: Query<(&Camera, &GlobalTransform), With<PlayerCamera>>,
    remotes: Query<(
        Entity,
        &GlobalTransform,
        &Transform,
        &DisplayName,
        &RemoteEntity,
        Has<UniqueMonster>,
        Option<&GuildTag>,
        Option<&StallOwner>,
        Option<&CosOwner>,
        Option<&DropRarity>,
        Has<SealDrop>,
    )>,
    local: Query<(Entity, &GlobalTransform, &DisplayName), With<Player>>,
    // The mount the local player is sitting on, so it can be skipped below.
    rider: Query<&crate::plugins::cos::RiderOf, With<Player>>,
    children: Query<&Children>,
    mut sub_line_spans: Query<(&mut TextSpan, &mut TextColor), With<NameplateSubLines>>,
    occlusion: Occlusion,
    mut plates: Query<
        (
            Entity,
            &mut Node,
            &mut Text,
            &mut TextFont,
            &mut TextColor,
            &mut Visibility,
            Has<Underline>,
        ),
        // The guild line is a CHILD entity of the plate (`.with_child` above), so
        // the two &mut TextColor queries never touch the same entity — but Bevy
        // cannot infer that from `With<Nameplate>` alone and panics at runtime
        // (B0001). Excluding the child archetype makes the disjointness explicit.
        (With<Nameplate>, Without<NameplateSubLines>),
    >,
    mut commands: Commands,
) {
    // The World dev sandbox spawns both PlayerCamera and DebugCamera; only one
    // is active, so project with whichever is currently rendering.
    let Some((camera, cam_gt)) = camera.iter().find(|(c, _)| c.is_active) else {
        return;
    };
    let cam_pos = cam_gt.translation();
    let viewport = camera.logical_viewport_size().unwrap_or(Vec2::ZERO);

    // Head anchor above the feet, tracking the model scale (giants & co.).
    let head =
        |gt: &GlobalTransform, scale_y: f32| gt.translation() + Vec3::Y * HEAD_OFFSET * scale_y;

    // World head point → viewport px, or None if too far, behind the camera, or
    // off-screen.
    let project = |world: Vec3| -> Option<Vec2> {
        if world.distance(cam_pos) > MAX_DISTANCE {
            return None;
        }
        let px = camera.world_to_viewport(cam_gt, world).ok()?;
        (px.x >= 0.0 && px.x <= viewport.x && px.y >= 0.0 && px.y <= viewport.y).then_some(px)
    };

    // Line-of-sight test against the static world (terrain, buildings, walls).
    // Characters (skinned meshes) and non-solid art (decals, effects, skybox —
    // all NotShadowCaster) never hide a label, so the label's own model and a
    // crowd in front don't flicker it, and the cast can early-exit on the
    // first solid hit.
    let Occlusion {
        mut raycast,
        non_occluders,
        cache: mut occlusion,
    } = occlusion;
    let occluder = |entity: Entity| !non_occluders.contains(entity);
    let now = input.time.elapsed_secs();
    // Entities that despawn are simply never queried again, so their entries go
    // stale rather than being removed at despawn time. Sweep whenever the map
    // outgrows what could plausibly be on screen; a stale entry's deadline is by
    // definition far in the past.
    if occlusion.verdicts.len() > POOL_SIZE * 4 {
        occlusion
            .verdicts
            .retain(|_, (_, due)| *due > now - OCCLUSION_REFRESH_SECS * 10.0);
    }
    let mut occluded = |entity: Entity, world: Vec3| -> bool {
        if let Some((verdict, due)) = occlusion.verdicts.get(&entity) {
            if now < *due {
                return *verdict;
            }
        }
        let delta = world - cam_pos;
        let distance = delta.length();
        let Ok(direction) = Dir3::new(delta) else {
            return false;
        };
        let settings = MeshRayCastSettings::default().with_filter(&occluder);
        let verdict = raycast
            .cast_ray(Ray3d::new(cam_pos, direction), &settings)
            .first()
            .is_some_and(|(_, hit)| hit.distance + 1.0 < distance);
        occlusion.verdicts.insert(
            entity,
            (
                verdict,
                now + OCCLUSION_REFRESH_SECS + OcclusionCache::stagger(entity),
            ),
        );
        verdict
    };

    // Underline the hovered and selected entities' labels.
    let underlined = |entity: Entity| hovered.0 == Some(entity) || selected.0 == Some(entity);

    let show_guild = toggle_on(&options, OPT_GUILD_NAME);
    let show_dropped_items = drop_item_names_held(&input.keys, &options, input.chat.input_open);
    let hover_bold = input.config.nameplates.hover_bold;

    // A COS spawns as an NPC to inherit movement/selection/plates, so the
    // mount you are riding would otherwise carry a plate right under the
    // camera. Skipped before projection so it doesn't consume a pool slot.
    let ridden = rider.single().ok().map(|rider| rider.0);

    let mut visible: Vec<(Vec2, String, Color, bool, String)> = Vec::new();
    for (entity, gt, transform, name, kind, unique, guild, stall, cos_owner, drop, seal) in
        remotes.iter()
    {
        if Some(entity) == ridden {
            continue;
        }
        if plate_toggle(*kind).is_some_and(|id| !toggle_on(&options, id)) {
            continue;
        }
        // Ground items label under the cursor, and — while the KeyViewDropItem
        // key is held — all of them at once (see `drop_item_names_held`).
        // Anchored just above the pile.
        let world = if matches!(kind, RemoteEntity::Item) {
            if !show_dropped_items && hovered.0 != Some(entity) {
                continue;
            }
            gt.translation() + Vec3::Y * ITEM_OFFSET
        } else {
            head(gt, transform.scale.y)
        };
        let Some(px) = project(world) else {
            continue;
        };
        if occluded(entity, world) {
            continue;
        }
        visible.push((
            px,
            name.0.clone(),
            label_color(&colors, *kind, unique, drop, seal),
            underlined(entity),
            sub_lines_text(guild.filter(|_| show_guild), stall, cos_owner),
        ));
        if visible.len() >= POOL_SIZE {
            warn_once!("nameplates: more than {POOL_SIZE} labels in view, dropping overflow");
            break;
        }
    }
    // our own character's name, if there's still room (never a hover target)
    if visible.len() < POOL_SIZE && toggle_on(&options, OPT_OWN_NAME) {
        if let Ok((entity, gt, name)) = local.single() {
            let world = head(gt, 1.0);
            if let Some(px) = project(world) {
                if !occluded(entity, world) {
                    // our own guild is not on the wire in the spawn path
                    visible.push((
                        px,
                        name.0.clone(),
                        colors.local_player,
                        false,
                        String::new(),
                    ));
                }
            }
        }
    }

    let mut pending = visible.into_iter();
    for (slot, mut node, mut text, mut font, mut color, mut visibility, has_underline) in
        plates.iter_mut()
    {
        // the plate's guild-line span child
        let sub_span = children
            .get(slot)
            .ok()
            .and_then(|kids| kids.iter().find(|kid| sub_line_spans.contains(*kid)));
        match pending.next() {
            Some((px, label, c, underline, sub_lines)) => {
                let left = Val::Px(px.x);
                let top = Val::Px(px.y);
                if node.left != left {
                    node.left = left;
                }
                if node.top != top {
                    node.top = top;
                }
                if text.0 != label {
                    text.0 = label;
                }
                if color.0 != c {
                    color.0 = c;
                }
                if *visibility != Visibility::Inherited {
                    *visibility = Visibility::Inherited;
                }
                if underline && !has_underline {
                    commands.entity(slot).insert(Underline);
                } else if !underline && has_underline {
                    commands.entity(slot).remove::<Underline>();
                }
                // Deviation from the original: hovered/selected labels read
                // bold. It is a readability improvement, config-gated per ADR
                // 0009 (`nameplates.hover_bold`, default on). The underline in
                // the same block is original-shaped and needs no flag.
                //
                // The weight rides the UI face's own `wght` axis rather than a
                // second font file — the bundled Arimo is variable, so bold no
                // longer means "a different typeface appears" (it used to swap
                // to Fira Sans Bold beside a non-Fira body face). A PK2 face
                // with no variable axis simply renders both states at its own
                // single weight, which is the original's look anyway.
                let wanted = if underline && hover_bold {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                };
                if font.weight != wanted {
                    font.weight = wanted;
                }
                let face: FontSource = fonts.nine.clone().into();
                if font.font != face {
                    font.font = face;
                }
                // sub-lines: same size and colour as the name (§9-U4), empty
                // for an entity with neither a guild nor a stall
                if let Some((mut span, mut span_color)) =
                    sub_span.and_then(|kid| sub_line_spans.get_mut(kid).ok())
                {
                    if span.0 != sub_lines {
                        span.0 = sub_lines;
                    }
                    if span_color.0 != c {
                        span_color.0 = c;
                    }
                }
            }
            None => {
                if *visibility != Visibility::Hidden {
                    *visibility = Visibility::Hidden;
                }
                if has_underline {
                    commands.entity(slot).remove::<Underline>();
                }
                // a freed slot must not keep the previous entity's sub-lines
                if let Some((mut span, _)) =
                    sub_span.and_then(|kid| sub_line_spans.get_mut(kid).ok())
                {
                    if !span.0.is_empty() {
                        span.0.clear();
                    }
                }
            }
        }
    }
}

/// Self-registration for the entity nameplates (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct NameplatesPlugin;

impl Plugin for NameplatesPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.init_resource::<crate::plugins::config::nameplates::NameplateColors>()
            .add_systems(
                PreUpdate,
                apply_nameplate_colors.run_if(crate::plugins::settings::live::config_changed),
            )
            .add_systems(OnEnter(SceneState::GameWorld), spawn_nameplate_pool)
            .add_systems(OnExit(SceneState::GameWorld), cleanup_nameplates)
            .add_systems(Update, update_nameplates.run_if(super::hud_scenes));
    }
}

/// Re-resolves the per-kind nameplate palette from `config.yaml`
/// ([`crate::plugins::settings::live`]).
pub fn apply_nameplate_colors(
    config: Res<crate::plugins::config::ClientConfig>,
    mut colors: ResMut<crate::plugins::config::nameplates::NameplateColors>,
) {
    *colors = config.nameplates.colors.resolved();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::config::nameplates::NameplateSettings;

    fn pressed(key: KeyCode) -> ButtonInput<KeyCode> {
        let mut input = ButtonInput::default();
        input.press(key);
        input
    }

    /// The key these tests bind. **Not a modifier**: `keycode_to_vk`
    /// (`settings/keymap.rs`) is a Win32 VK table that carries no
    /// Alt/Shift/Ctrl entry at all, so `bind_key(_, KeyCode::AltLeft)` returns
    /// `false` and stores nothing — an earlier draft of these tests asserted
    /// on exactly that and went red. Whether the original binds 3012 to a
    /// modifier is UNKNOWN (`OptionSet.csv` gives id 3012 no default at all);
    /// binding modifiers at all is a keymap-table gap, not a nameplate one.
    const HELD: KeyCode = KeyCode::F5;

    /// Unbound by default (`OptionSet.csv` gives id 3012 no default key), so a
    /// stock client never shows the *bulk* pile read.
    ///
    /// It does still label the drop under the cursor — that path does not go
    /// through this function (`update_nameplates` ORs it in), which is the
    /// point: making the unbound bulk key the only path is what left hover
    /// silent on a stock client.
    #[test]
    fn the_bulk_pile_read_stays_off_while_the_action_is_unbound() {
        let options = GameOptions::default();
        assert_eq!(options.key_for(KEY_VIEW_DROP_ITEM), None);
        assert!(!drop_item_names_held(&pressed(HELD), &options, false));
    }

    /// The whole point of #600: it is the HELD key, not the cursor, and it is
    /// all-or-nothing for every pile in range.
    #[test]
    fn drop_item_names_follow_the_held_key() {
        let mut options = GameOptions::default();
        assert!(options.bind_key(KEY_VIEW_DROP_ITEM, HELD));
        assert_eq!(options.key_for(KEY_VIEW_DROP_ITEM), Some(HELD));

        assert!(drop_item_names_held(&pressed(HELD), &options, false));
        // released
        assert!(!drop_item_names_held(
            &ButtonInput::default(),
            &options,
            false
        ));
        // some other key held
        assert!(!drop_item_names_held(
            &pressed(KeyCode::KeyZ),
            &options,
            false
        ));
    }

    /// Same seam the six other keyed HUD modules use: typing must not fire it.
    #[test]
    fn drop_item_names_ignore_the_key_while_chat_input_is_open() {
        let mut options = GameOptions::default();
        assert!(options.bind_key(KEY_VIEW_DROP_ITEM, HELD));
        assert!(!drop_item_names_held(&pressed(HELD), &options, true));
    }

    /// The bold hover face is a stated deviation (no bold cut in the PK2), so
    /// it must be reachable from config — and default to on.
    #[test]
    fn hover_bold_is_config_reachable_and_defaults_on() {
        assert!(NameplateSettings::default().hover_bold);
        let off: NameplateSettings =
            serde_yaml::from_str("hover_bold: false").expect("nameplate settings parse");
        assert!(!off.hover_bold);
        assert_eq!(
            off.colors.player,
            NameplateSettings::default().colors.player
        );
    }

    /// The stall title is a second line on the *existing* plate, not a second
    /// text system — and it composes with the guild tag rather than replacing
    /// it (#782).
    #[test]
    fn sub_lines_stack_the_guild_tag_and_then_the_stall_title() {
        let guild = GuildTag {
            name: "Nomads".to_string(),
            granted_nick: String::new(),
        };
        let stall = StallOwner {
            title: "Cheap elixirs".to_string(),
            decoration_id: 0,
        };

        assert_eq!(sub_lines_text(None, None, None), "");
        assert_eq!(sub_lines_text(Some(&guild), None, None), "\nNomads");
        assert_eq!(sub_lines_text(None, Some(&stall), None), "\nCheap elixirs");
        assert_eq!(
            sub_lines_text(Some(&guild), Some(&stall), None),
            "\nNomads\nCheap elixirs"
        );
    }

    /// A stall whose title is empty (or blank) adds no line at all — an empty
    /// second line would push the name up for no reason.
    #[test]
    fn a_blank_stall_title_adds_no_line() {
        let blank = StallOwner {
            title: "   ".to_string(),
            decoration_id: 3,
        };
        assert_eq!(sub_lines_text(None, Some(&blank), None), "");
    }

    /// A COS carries its summoner's name under its own. Same blank rule as the
    /// stall title — a horse's spawn record has no owner tail at all, so most
    /// COS reach this with `None`.
    #[test]
    fn a_cos_shows_its_summoner_under_its_name() {
        let owner = CosOwner("Roland".to_string());
        assert_eq!(sub_lines_text(None, None, Some(&owner)), "\nRoland");
        assert_eq!(sub_lines_text(None, None, Some(&CosOwner(" ".into()))), "");
        assert_eq!(sub_lines_text(None, None, None), "");
    }
}
