use std::f32::consts::PI;
use std::time::Duration;

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::{InteractionDisabled, Overflow, Pressed};
use bevy::ui_widgets::Activate;
use bevy_tweening::TweenAnim;

use packets::agent::prelude::*;
use packets::Packet;

use crate::assets::bmt::sheen::ShineColor;
use crate::assets::textdata::leveldata::max_hp_or_mp;
use crate::assets::FontAssets;
use crate::net::connection::SilkroadConnection;
use crate::plugins::camera::CinematicCamera2;
use crate::plugins::dynamic_resource_loader::{
    AttachmentShine, MirroredResource, PendingItemAttachment, PreferredAnimationGroup,
    UnloadedResource,
};
use crate::plugins::hud::modal_dialog::{modal_plate, modal_scrim};
use crate::plugins::map::terrain::Terrain;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::settings::options::GameOptions;
use crate::plugins::textdata::{
    ClientCharacterData, ClientItemData, ClientLevelData, ClientUiStrings,
};
use crate::plugins::ui_v2::style::{ButtonSound, ImageButtonStyle};
use crate::plugins::ui_v2::widgets::{image_button, label};
use crate::plugins::world_origin::{set_world_origin, WorldOrigin};
use crate::scenes::SceneState;
use crate::util::mesh::needs_winding_reversal;
use crate::util::tweening_ext::keyframe_tween;

use super::assets::IntroV2Assets;
use super::chrome::InfoTextV2Update;
use super::fade::{FadeToBlack, FadeToBlackTimer};
use super::login_form::main_button_style;
use super::scene_data::ActiveCharSelectSceneV2;
use super::{IntroV2State, IntroV2Ui};
use crate::plugins::hud::scale::hud_scale;

/// Root marker of the Create/Cancel button row.
#[derive(Component, Default, Clone)]
pub struct CharSelectControls;

/// Root marker of the Start/Delete/Cancel button row shown while a character
/// is selected.
#[derive(Component, Default, Clone)]
pub struct SelectedCharControls;

/// Marker on the Start button, used to re-enable it when a join fails.
#[derive(Component, Default, Clone)]
pub struct StartButton;

/// Root marker of the character info box.
#[derive(Component, Default, Clone)]
pub struct CharSelectInfoBox;

/// Root marker of the delete confirmation modal.
#[derive(Component, Default, Clone)]
pub struct DeleteConfirmModal;

/// Root marker of the fullscreen loading overlay shown while joining the
/// world.
#[derive(Component, Default, Clone)]
pub struct JoinLoadingOverlay;

/// A 3d character preview spawned from the character list response.
#[derive(Component, Default, Copy, Clone)]
pub struct SelectableCharacterV2;

/// The lobby data of a character preview, kept on the root entity for the
/// info box and the delete/join requests.
#[derive(Component, Clone)]
pub struct CharacterInfoV2(pub LobbyCharacter);

/// The clicked character preview. Its presence drives the whole selection
/// UI (info box, Start/Delete/Cancel row) via run conditions, mirroring the
/// captcha's resource-presence pattern.
#[derive(Resource)]
pub struct SelectedCharacterV2(pub Entity);

/// The shared material a hover-highlighted mesh had before it was swapped
/// for a tinted clone; restored when the pointer leaves.
#[derive(Component)]
pub struct OriginalMaterial(pub Handle<StandardMaterial>);

/// Set while a world join is in flight. Doubles as the signal for the
/// CharacterList exit systems to keep the agent connection alive, because the
/// world scene keeps talking to the same agent server.
#[derive(Resource)]
pub struct PendingWorldJoin {
    #[allow(dead_code)]
    pub character_name: String,
}

/// The full lobby data of the character being joined, carried across the scene
/// transition so the game scene can assemble the *actual* selected character
/// (model + equipment, named after it) instead of a hardcoded default. Consumed
/// and removed by the game scene once the player is spawned.
#[derive(Resource, Clone)]
pub struct JoiningCharacter(pub LobbyCharacter);

/// Camera flight time when zooming onto or away from a character.
const CAMERA_ZOOM_DURATION: Duration = Duration::from_millis(700);

// Zoom framing, in world units of an (unscaled) ~19-unit-tall character
// model: the camera stops this far in front of the clicked character at
// roughly head height, looking at the upper body — the vanilla close-up
// (torso + head filling the frame at fov 1.0).
const CAMERA_ZOOM_DISTANCE: f32 = 14.0;
const CAMERA_ZOOM_HEIGHT: f32 = 15.0;
const CHAR_FOCUS_HEIGHT: f32 = 14.0;

/// Additive emissive tint of the hover highlight.
const HOVER_EMISSIVE: LinearRgba = LinearRgba::rgb(0.15, 0.12, 0.05);

pub fn control_buttons(assets: &IntroV2Assets, fonts: &FontAssets) -> impl Scene {
    let font = fonts.nine.clone();
    let cancel_font = fonts.nine.clone();
    let button_sound = assets.sound_button_sound_a.clone();
    let cancel_sound = button_sound.clone();

    bsn! {
        CharSelectControls
        Name("Character Select Controls V2")
        Node {
            position_type: PositionType::Absolute,
            flex_direction: FlexDirection::Row,
            bottom: percent(7.5),
            right: percent(1),
            column_gap: px(15),
        }
        Children [
            (
                image_button(main_button_style(assets), MAIN_BUTTON_W, MAIN_BUTTON_H)
                ImageNode { color: Color::NONE }
                ButtonSound({button_sound})
                Children [ (label("Create", font, 16.0)) ]
                on(|_activate: On<Activate>,
                    fade_timer: Option<Res<FadeToBlackTimer>>,
                    mut next_state: ResMut<NextState<IntroV2State>>,
                    mut commands: Commands| {
                    // Ignore the click while the Cancel fade is in flight: the
                    // fade overlay doesn't block picking, and the queued
                    // LoginForm switch would then fire from inside
                    // CharacterCreate, skipping the CharacterList exit chain
                    // that restores camera/origin/connection.
                    if fade_timer.is_some() {
                        return;
                    }
                    // Keep the agent connection across the sub-state switch so
                    // Create/CheckName can use it (see disconnect_from_agent_server).
                    commands.insert_resource(super::character_create::EnteringCharacterCreate);
                    next_state.set(IntroV2State::RegionSelect);
                })
            ),
            (
                image_button(main_button_style(assets), MAIN_BUTTON_W, MAIN_BUTTON_H)
                ImageNode { color: Color::NONE }
                ButtonSound({cancel_sound})
                Children [ (label("Cancel", cancel_font, 16.0)) ]
                on(|_activate: On<Activate>,
                    mut fade_writer: MessageWriter<FadeToBlack>,
                    mut commands: Commands| {
                    fade_writer.write(FadeToBlack);
                    commands.insert_resource(FadeToBlackTimer::to(IntroV2State::LoginForm));
                })
            ),
        ]
    }
}

/// The Start/Delete/Cancel row replacing [`control_buttons`] while a
/// character is selected. Spawned outside the `show_screen` fade flow, so
/// buttons and labels start fully visible.
pub(crate) fn selected_control_buttons(assets: &IntroV2Assets, fonts: &FontAssets) -> impl Scene {
    let start_font = fonts.nine.clone();
    let delete_font = fonts.nine.clone();
    let cancel_font = fonts.nine.clone();
    let start_sound = assets.sound_button_sound_a.clone();
    let delete_sound = start_sound.clone();
    let cancel_sound = start_sound.clone();

    bsn! {
        SelectedCharControls
        Name("Selected Character Controls V2")
        Node {
            position_type: PositionType::Absolute,
            flex_direction: FlexDirection::Row,
            bottom: percent(7.5),
            right: percent(1),
            column_gap: px(15),
        }
        Children [
            (
                image_button(main_button_style(assets), MAIN_BUTTON_W, MAIN_BUTTON_H)
                StartButton
                ButtonSound({start_sound})
                Children [ (label("Start", start_font, 16.0) TextColor(Color::WHITE)) ]
                on(on_start_activate)
            ),
            (
                image_button(main_button_style(assets), MAIN_BUTTON_W, MAIN_BUTTON_H)
                ButtonSound({delete_sound})
                Children [ (label("Delete", delete_font, 16.0) TextColor(Color::WHITE)) ]
                on(on_delete_activate)
            ),
            (
                image_button(main_button_style(assets), MAIN_BUTTON_W, MAIN_BUTTON_H)
                ButtonSound({cancel_sound})
                Children [ (label("Cancel", cancel_font, 16.0) TextColor(Color::WHITE)) ]
                on(on_selection_cancel_activate)
            ),
        ]
    }
}

/// Control row for a character inside the deletion-pending window: the
/// original swaps Start/Delete for **Restore + Cancel**
/// (`UIO_CTL_CHARACTER_DELETE_CONDITION`, `resinfo/pscharacterselect.txt`), so
/// a pending character can only be recovered or deselected — not started, and
/// not deleted again.
pub(crate) fn deleting_control_buttons(
    assets: &IntroV2Assets,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
) -> impl Scene {
    let restore_font = fonts.nine.clone();
    let cancel_font = fonts.nine.clone();
    let restore_sound = assets.sound_button_sound_a.clone();
    let cancel_sound = restore_sound.clone();
    // Captions come from the user's own PK2 (`UIO_STT_CHAR_RECOVERY` =
    // "Restore"), not from invented wording.
    let restore_label = ui_strings
        .get_or("UIO_STT_CHAR_RECOVERY", "Restore")
        .to_string();

    bsn! {
        SelectedCharControls
        Name("Deleting Character Controls V2")
        Node {
            position_type: PositionType::Absolute,
            flex_direction: FlexDirection::Row,
            bottom: percent(7.5),
            right: percent(1),
            column_gap: px(15),
        }
        Children [
            (
                image_button(main_button_style(assets), 91.0, 41.0)
                ButtonSound({restore_sound})
                Children [ (label(&restore_label, restore_font, 16.0) TextColor(Color::WHITE)) ]
                on(on_restore_activate)
            ),
            (
                image_button(main_button_style(assets), 91.0, 41.0)
                ButtonSound({cancel_sound})
                Children [ (label("Cancel", cancel_font, 16.0) TextColor(Color::WHITE)) ]
                on(on_selection_cancel_activate)
            ),
        ]
    }
}

fn spawn_default_controls(
    commands: &mut Commands,
    assets: &IntroV2Assets,
    fonts: &FontAssets,
    camera: Entity,
) {
    commands
        .spawn_scene(control_buttons(assets, fonts))
        .insert((UiTargetCamera(camera), IntroV2Ui));
}

pub fn spawn_control_buttons(
    mut commands: Commands,
    assets: Res<IntroV2Assets>,
    fonts: Res<FontAssets>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found");
        return;
    };

    spawn_default_controls(&mut commands, &assets, &fonts, camera);
}

pub fn request_character_list(mut query: Query<&SilkroadConnection, With<AgentConnection>>) {
    let Ok(conn) = query.single_mut() else {
        return;
    };
    send_character_list_request(conn);
}

fn send_character_list_request(conn: &SilkroadConnection) {
    let frame = Packet::from(CharacterSelectionActionRequest {
        action: CharacterSelectionAction::List,
        name: None,
        create: None,
    })
    .into();

    if let Err(e) = conn.get_sender().send(frame) {
        error!("failed to send frame: {}", e.0);
    }
}

/// A fresh entry into character selection never has a join in flight; a
/// stale [`PendingWorldJoin`] (e.g. from a previous session that never made
/// it into the world) would block the Start button forever.
pub fn reset_join_state(mut commands: Commands) {
    commands.remove_resource::<PendingWorldJoin>();
}

/// The mesh raycast backend runs with `require_markers`, so the char-select
/// camera must opt in to mesh picking explicitly.
pub fn enable_mesh_picking_camera(
    cam_query: Query<Entity, (With<CinematicCamera2>, Without<MeshPickingCamera>)>,
    mut commands: Commands,
) {
    for entity in cam_query.iter() {
        commands.entity(entity).insert(MeshPickingCamera);
    }
}

/// Anchor the floating world origin on the char-select location so the camera,
/// the lined-up characters and the surrounding terrain live at small
/// render-space coordinates (see `world_origin`). First system of the
/// `OnEnter(CharacterList)` chain — everything below places entities.
pub fn set_origin_to_char_select(
    char_select_scene: Res<ActiveCharSelectSceneV2>,
    mut origin: ResMut<WorldOrigin>,
    mut terrain: Query<&mut Transform, With<Terrain>>,
) {
    let anchor = char_select_scene.0.cam_base() * Vec3::new(-1.0, 1.0, 1.0);
    set_world_origin(anchor, &mut origin, &mut terrain);
}

pub fn start_camera_animation(
    mut cam_query: Query<(Entity, &mut Transform), With<CinematicCamera2>>,
    mut commands: Commands,
    char_select_scene: Res<ActiveCharSelectSceneV2>,
    origin: Res<WorldOrigin>,
) {
    let Ok((entity, mut transform)) = cam_query.single_mut() else {
        return;
    };

    let base = (char_select_scene.0.cam_base() + char_select_scene.0.cam_offset())
        * Vec3::new(-1.0, 1.0, 1.0);

    transform.translation = origin.to_render(base);
    commands.entity(entity).insert(TweenAnim::new(
        char_select_scene.0.get_init_camera_anim(origin.0),
    ));
}

/// Spawns a 3d preview per character from the list response. Port of the
/// old `on_char_selection_action_response`.
pub fn on_char_selection_action_response(
    char_select_scene: Res<ActiveCharSelectSceneV2>,
    char_data: Res<ClientCharacterData>,
    item_data: Res<ClientItemData>,
    asset_server: Res<AssetServer>,
    mut reader: MessageReader<CharacterSelectionActionResponse>,
    mut commands: Commands,
    origin: Res<WorldOrigin>,
) {
    for res in reader.read() {
        if let Some(err) = res.error_code {
            error!("[CharacterSelectionAction] Error : {}", err);
        }

        // Characters are lined up between the start and end offsets on the
        // X axis (see the old character_scene.rs for the reference values).
        let begin = char_select_scene.0.char_start_offset() * Vec3::new(-1.0, 1.0, 1.0);
        let end = char_select_scene.0.char_end_offset() * Vec3::new(-1.0, 1.0, 1.0);
        let dir = end - begin;

        let start =
            origin.to_render(char_select_scene.0.cam_base() * Vec3::new(-1.0, 1.0, 1.0)) + begin;
        if let Some(characters) = &res.characters {
            for (i, char) in characters.characters.iter().enumerate() {
                // Passive RE evidence (EP-RE #239): log every lobby char's
                // server-sent scale byte + stats. Growing the scale corpus is
                // what pins the still-UNKNOWN height/volume nibble packing
                // (docs/net-char-select-0x7007.md); the hex byte matters, the
                // decimal is for quick reading.
                info!(
                    "lobby char '{}': ref {}, scale 0x{:02X} ({}), level {}, str {}, int {}, hp {}, mp {}",
                    char.name,
                    char.ref_obj_id,
                    char.scale,
                    char.scale,
                    char.level,
                    char.str,
                    char.int,
                    char.hp,
                    char.mp,
                );
                let ref_char_id = char.ref_obj_id as i32;
                let Some(char_data) = char_data.get(&ref_char_id) else {
                    warn!(
                        "lobby char '{}': no characterdata row for ref {ref_char_id} (missing/skipped shard?); not rendering this character",
                        char.name
                    );
                    continue;
                };
                let offset = (dir / (characters.count as f32 + 1.0)) * (i as f32 + 1.0);
                let path = char_data.resource_path();
                // Characters are mirrored on X (scale.x = -1) like every other
                // SRO resource so they render with the correct handedness (weapon
                // in the right hand). The mirror makes the placement determinant
                // negative, so the shared winding rule reverses their meshes.
                let scale = 1.0 + (char.scale as f32 / 255.0);
                let transform = Transform::from_translation(start + offset)
                    .with_scale(Vec3::new(-scale, scale, scale))
                    .with_rotation(Quat::from_rotation_y(PI));
                let mut char_commands = commands.spawn((
                    transform,
                    Visibility::default(),
                    SelectableCharacterV2,
                    Hovered::default(),
                    CharacterInfoV2(char.clone()),
                    Name::from(char.name.as_str()),
                ));
                if let Some(path) = path {
                    char_commands.insert(UnloadedResource(asset_server.load(path)));
                }
                if needs_winding_reversal(&transform.to_matrix()) {
                    char_commands.insert(MirroredResource);
                }
                // picking events on the (asynchronously spawned) leaf meshes
                // bubble up the hierarchy to this root
                char_commands.observe(on_character_clicked);
                let char_entity = char_commands.id();

                attach_equipment(
                    &mut commands,
                    &asset_server,
                    &item_data,
                    char_entity,
                    char.char_items
                        .iter()
                        .chain(char.avatar_items.iter())
                        .map(|item| (item.id, Some(item.plus))),
                );
            }
        }
    }
}

/// Attaches each equipped item's 3d resource under `char_entity`, mirroring the
/// vanilla equip layer: the weapon selects the character's animation stance,
/// items with no 3d resource are skipped, and a `+N` item carries its shine
/// tier. Shared by the char-list previews and the creation screen; `plus` is
/// `None` for freshly-picked starter gear (no enhancement yet).
pub(super) fn attach_equipment(
    commands: &mut Commands,
    asset_server: &AssetServer,
    item_data: &ClientItemData,
    char_entity: Entity,
    items: impl Iterator<Item = (u32, Option<u8>)>,
) {
    for (id, plus) in items {
        let Some(item_row) = item_data.get(&(id as i32)) else {
            warn!("no itemdata entry for equipped item: {}", id);
            continue;
        };
        // the equipped weapon decides which animation set the character plays
        // (e.g. the spear or bow stance)
        if let Some(group) = item_row.animation_group() {
            commands
                .entity(char_entity)
                .insert(PreferredAnimationGroup(group.to_string()));
        }
        // items without a 3d resource (e.g. pure stat items) cannot be rendered
        let Some(item_path) = item_row.resource_path() else {
            continue;
        };
        let mut attachment = commands.spawn((
            PendingItemAttachment(asset_server.load(item_path)),
            ChildOf(char_entity),
            Name::from(format!("item {}", item_row.code_name())),
        ));
        if let Some(color) = plus.and_then(ShineColor::for_opt_level) {
            attachment.insert(AttachmentShine::Tier(color));
        }
    }
}

/// The preview meshes spawn asynchronously once their resources load, so
/// each frame any untagged mesh below a character root is made pickable
/// (the raycast backend only tests `Pickable` entities).
pub fn tag_pickable_meshes(
    roots: Query<Entity, With<SelectableCharacterV2>>,
    children: Query<&Children>,
    untagged: Query<(), (With<Mesh3d>, Without<Pickable>)>,
    mut commands: Commands,
) {
    for root in roots.iter() {
        for entity in children.iter_descendants(root) {
            if untagged.contains(entity) {
                // RayCastBackfaces: the previews are mirrored (scale.x = -1)
                // with pre-reversed winding, so face orientation in mesh
                // space is unreliable for the raycast's backface culling —
                // accept hits from both sides.
                commands
                    .entity(entity)
                    .insert((Pickable::default(), RayCastBackfaces));
            }
        }
    }
}

/// Brightens a hovered character by swapping every mesh's material for an
/// emissive-tinted clone (and restoring the original on pointer-out). The
/// shared handles must never be mutated in place: they are loaded by asset
/// path, so a tint would bleed into every other user of the material.
/// Meshes that finish loading mid-hover stay untinted until the next hover
/// change, which is acceptable.
pub fn update_hover_highlight(
    hovered_roots: Query<(Entity, &Hovered), (With<SelectableCharacterV2>, Changed<Hovered>)>,
    children: Query<&Children>,
    mesh_materials: Query<&MeshMaterial3d<StandardMaterial>>,
    originals: Query<&OriginalMaterial>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    for (root, hovered) in hovered_roots.iter() {
        if hovered.0 {
            for entity in children.iter_descendants(root) {
                let Ok(material) = mesh_materials.get(entity) else {
                    continue;
                };
                if originals.contains(entity) {
                    continue;
                }
                let Some(mut tinted) = materials.get(&material.0).cloned() else {
                    continue;
                };
                tinted.emissive = tinted.emissive + HOVER_EMISSIVE;
                let tinted_handle = materials.add(tinted);
                commands.entity(entity).insert((
                    MeshMaterial3d(tinted_handle),
                    OriginalMaterial(material.0.clone()),
                ));
            }
        } else {
            restore_materials(root, &children, &originals, &mut commands);
        }
    }
}

fn restore_materials(
    root: Entity,
    children: &Query<&Children>,
    originals: &Query<&OriginalMaterial>,
    commands: &mut Commands,
) {
    for entity in children.iter_descendants(root) {
        if let Ok(original) = originals.get(entity) {
            commands
                .entity(entity)
                .insert(MeshMaterial3d(original.0.clone()))
                .remove::<OriginalMaterial>();
        }
    }
}

/// Selecting a character freezes the hover state (the pointer sits still, so
/// no `Changed<Hovered>` fires), so any active highlight is cleared here.
pub fn clear_highlights(tinted: Query<(Entity, &OriginalMaterial)>, mut commands: Commands) {
    for (entity, original) in tinted.iter() {
        commands
            .entity(entity)
            .insert(MeshMaterial3d(original.0.clone()))
            .remove::<OriginalMaterial>();
    }
}

/// `Pointer<Click>` observer attached to each character root: marks the
/// character selected and flies the camera onto it.
pub fn on_character_clicked(
    click: On<Pointer<Click>>,
    selected: Option<Res<SelectedCharacterV2>>,
    transforms: Query<&Transform, With<SelectableCharacterV2>>,
    cam_query: Query<
        (Entity, &Transform),
        (With<CinematicCamera2>, Without<SelectableCharacterV2>),
    >,
    mut commands: Commands,
) {
    if selected.is_some() || click.event.button != PointerButton::Primary {
        return;
    }

    // the event bubbled up from a leaf mesh; `entity` is the propagation
    // target the observer sits on, i.e. the character root
    let root = click.entity;
    let Ok(char_transform) = transforms.get(root) else {
        return;
    };
    let Ok((cam_entity, cam_transform)) = cam_query.single() else {
        return;
    };

    // Like the vanilla client, the zoom targets the clicked character
    // itself, not an authored scene pose (the YAML select keyframes are
    // untuned and frame nothing): the camera stops in front of the
    // character — approaching from wherever the camera currently is — at
    // head height, looking at the upper body.
    let char_pos = char_transform.translation;
    let char_scale = char_transform.scale.y;
    let mut toward_cam = cam_transform.translation - char_pos;
    toward_cam.y = 0.0;
    let toward_cam = toward_cam.normalize_or(Vec3::Z);

    let target_pos =
        char_pos + toward_cam * CAMERA_ZOOM_DISTANCE + Vec3::Y * (CAMERA_ZOOM_HEIGHT * char_scale);
    let focus = char_pos + Vec3::Y * (CHAR_FOCUS_HEIGHT * char_scale);
    let target_rot = Transform::from_translation(target_pos)
        .looking_at(focus, Vec3::Y)
        .rotation;

    commands
        .entity(cam_entity)
        .insert(TweenAnim::new(keyframe_tween(
            cam_transform.translation,
            target_pos,
            cam_transform.rotation,
            target_rot,
            CAMERA_ZOOM_DURATION,
        )));
    commands.insert_resource(SelectedCharacterV2(root));
}

fn zoom_out_camera(
    cam_query: &Query<(Entity, &Transform), With<CinematicCamera2>>,
    char_select_scene: &ActiveCharSelectSceneV2,
    origin: &WorldOrigin,
    commands: &mut Commands,
) {
    let Ok((cam_entity, cam_transform)) = cam_query.single() else {
        return;
    };
    // Flying to the init animation's final keyframe (instead of remembering
    // the pre-zoom transform) keeps the target deterministic even when the
    // character was clicked mid-animation.
    let Some((end_pos, end_rot)) = char_select_scene.0.init_camera_end_pose(origin.0) else {
        return;
    };

    commands
        .entity(cam_entity)
        .insert(TweenAnim::new(keyframe_tween(
            cam_transform.translation,
            end_pos,
            cam_transform.rotation,
            end_rot,
            CAMERA_ZOOM_DURATION,
        )));
}

// Info box layout: transplanted from the vanilla UI definition
// Media.pk2/resinfo/pscharacterselect.txt (Section "Info"). Its
// GDR_STA_CHARINFO window is info.ddj at the native 228x140, and every child
// element carries a Rect=RECT,"x,y,w,h" entry in window space — those
// coordinates are used verbatim below, uniformly scaled by hud_scale().

/// Fill fraction of a `current / max` gauge pair, clamped to `[0, 1]`.
///
/// `max == 0` means the maximum could not be derived (an unknown level/stat
/// combination), and a full bar is the honest fallback there: the lobby never
/// sends a maximum, so a 0-width bar would be a claim we cannot back.
fn gauge_fill(current: u32, max: u32) -> f32 {
    if max == 0 {
        return 1.0;
    }
    (current as f32 / max as f32).clamp(0.0, 1.0)
}

pub(crate) fn info_box(
    info: &LobbyCharacter,
    level_data: &ClientLevelData,
    assets: &IntroV2Assets,
    fonts: &FontAssets,
) -> impl Scene {
    let window = assets.info_window.clone();
    let hp_image = assets.hp_bar.clone();
    let mp_image = assets.mp_bar.clone();
    let name_font = fonts.nine.clone();
    let level_cap_font = fonts.nine.clone();
    let level_font = fonts.nine.clone();
    let exp_cap_font = fonts.nine.clone();
    let exp_font = fonts.nine.clone();
    let sp_cap_font = fonts.nine.clone();
    let sp_font = fonts.nine.clone();

    let name = info.name.clone();
    let level_text = format!("{}", info.level);
    // exp is shown as the percentage of the current level's requirement
    // (leveldata.txt column 1), like the original client
    let exp_text = match level_data.max_exp(info.level) {
        Some(max) if max > 0 => {
            format!("{:.2}%", info.exp_offset as f64 / max as f64 * 100.0)
        }
        _ => "-".to_string(),
    };
    let sp_text = format!("{}", info.stat_points);

    // The lobby packet carries current HP/MP but no maximum, so the bars used
    // to be pinned full. The maximum is reproducible offline from the same
    // `1.02^(level-1) * stat * 10` curve the server uses — capture-verified
    // against all 20 of our 0x303D bodies (`docs/stat-derivation-server-spec.md`
    // §5) — and the lobby's `str`/`int` are the base primaries with no equip or
    // buff modifiers, which is exactly what that curve wants.
    //
    // APPROX: the curve itself is `[S]` (community-derived; only its
    // `1.02^(lvl-1)` shape is `[V]` from the binary), and whether v1.188's
    // char-select ever draws a partial bar at all is still `[U]` — one original
    // screenshot settles it. If it turns out the original always shows a full
    // bar, delete the two fractions, not the formula.
    let hp_fill = gauge_fill(info.hp, max_hp_or_mp(info.level, info.str));
    let mp_fill = gauge_fill(info.mp, max_hp_or_mp(info.level, info.int));

    // gold of the vanilla caption texts (FontColor "255,255,208,81", ARGB)
    let caption_color = Color::srgb_u8(255, 208, 81);

    let s = hud_scale();
    // vanilla FontIndex 2 is a small UI font; 11px fits the 13px-high cells
    let font_size = 11.0 * s;
    let row_h = 13.0 * s;

    let window_w = 228.0 * s;
    let window_h = 140.0 * s;
    // GDR_STA_NAME "32,29,177,13"
    let name_l = 32.0 * s;
    let name_t = 29.0 * s;
    let name_w = 177.0 * s;
    // GDR_GAU_HP "46,50,136,8" / GDR_GAU_MP "46,62,136,8"
    let bar_l = 46.0 * s;
    let bar_w = 136.0 * s;
    let bar_h = 8.0 * s;
    let hp_t = 50.0 * s;
    let mp_t = 62.0 * s;
    // EXP row "38,80,28,13" + "76,80,43,13", SP "126,80,20,13" + "155,80,49,13".
    // The stat caption cell is widened a little beyond the vanilla 20px: it
    // was sized for "SP" but reads "Stat" here.
    let stats_t = 80.0 * s;
    let exp_cap_l = 38.0 * s;
    let exp_cap_w = 28.0 * s;
    let exp_val_l = 76.0 * s;
    let exp_val_w = 43.0 * s;
    let sp_cap_l = 122.0 * s;
    let sp_cap_w = 28.0 * s;
    let sp_val_l = 155.0 * s;
    let sp_val_w = 49.0 * s;
    // LEVEL row "79,102,36,13" + "126,102,25,13"
    let level_t = 102.0 * s;
    let level_cap_l = 79.0 * s;
    let level_cap_w = 36.0 * s;
    let level_val_l = 126.0 * s;
    let level_val_w = 25.0 * s;

    bsn! {
        CharSelectInfoBox
        Name("Character Info Box V2")
        ImageNode { image: {window}, image_mode: NodeImageMode::Stretch }
        Node {
            position_type: PositionType::Absolute,
            right: percent(3),
            top: percent(30),
            width: px(window_w),
            height: px(window_h),
        }
        Children [
            (
                // the character name goes into the window's sunken slot
                label(&name, name_font, font_size)
                TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, left: px(name_l), top: px(name_t), width: px(name_w), height: px(row_h) }
            ),
            // HP/MP gauges. A vanilla `CIFGauge` with `Style=0` CROPS its art
            // along X at 1:1 texel scale, it does not stretch it to the fill
            // width (`docs/re/ui/hp-mp-gauge-widget.md`), so the percentage
            // goes on a clip wrapper and the image keeps its full authored
            // width. Same three-node recipe as `hud/character_info/ui.rs`,
            // which is the one site in the tree that already had it right.
            // Here the two models happen to render identically — `hp.ddj` and
            // `mp.ddj` are 136x8 with no X structure — so this is for
            // consistency, not for the pixels.
            (
                Node {
                    position_type: PositionType::Absolute,
                    left: px(bar_l),
                    top: px(hp_t),
                    width: px(bar_w),
                    height: px(bar_h),
                    overflow: {Overflow::clip()},
                }
                Children [
                    (
                        Node {
                            width: percent(hp_fill * 100.0),
                            height: percent(100),
                            overflow: {Overflow::clip()},
                        }
                        Pickable::IGNORE
                        Children [
                            (
                                ImageNode { image: {hp_image}, image_mode: NodeImageMode::Stretch }
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: px(0.0),
                                    top: px(0.0),
                                    width: px(bar_w),
                                    height: px(bar_h),
                                }
                                Pickable::IGNORE
                            ),
                        ]
                    ),
                ]
            ),
            (
                Node {
                    position_type: PositionType::Absolute,
                    left: px(bar_l),
                    top: px(mp_t),
                    width: px(bar_w),
                    height: px(bar_h),
                    overflow: {Overflow::clip()},
                }
                Children [
                    (
                        Node {
                            width: percent(mp_fill * 100.0),
                            height: percent(100),
                            overflow: {Overflow::clip()},
                        }
                        Pickable::IGNORE
                        Children [
                            (
                                ImageNode { image: {mp_image}, image_mode: NodeImageMode::Stretch }
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: px(0.0),
                                    top: px(0.0),
                                    width: px(bar_w),
                                    height: px(bar_h),
                                }
                                Pickable::IGNORE
                            ),
                        ]
                    ),
                ]
            ),
            // EXP and SP share one row, gold caption + white value each
            (
                label("EXP", exp_cap_font, font_size)
                TextColor({caption_color})
                Node { position_type: PositionType::Absolute, left: px(exp_cap_l), top: px(stats_t), width: px(exp_cap_w), height: px(row_h) }
            ),
            (
                label(&exp_text, exp_font, font_size)
                TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, left: px(exp_val_l), top: px(stats_t), width: px(exp_val_w), height: px(row_h) }
            ),
            (
                label("Stat", sp_cap_font, font_size)
                TextColor({caption_color})
                Node { position_type: PositionType::Absolute, left: px(sp_cap_l), top: px(stats_t), width: px(sp_cap_w), height: px(row_h) }
            ),
            (
                label(&sp_text, sp_font, font_size)
                TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, left: px(sp_val_l), top: px(stats_t), width: px(sp_val_w), height: px(row_h) }
            ),
            // LEVEL sits centered in the row below
            (
                label("Level", level_cap_font, font_size)
                TextColor({caption_color})
                Node { position_type: PositionType::Absolute, left: px(level_cap_l), top: px(level_t), width: px(level_cap_w), height: px(row_h) }
            ),
            (
                label(&level_text, level_font, font_size)
                TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, left: px(level_val_l), top: px(level_t), width: px(level_val_w), height: px(row_h) }
            ),
        ]
    }
}

/// Swaps the underbar to Start/Delete/Cancel and shows the info box when a
/// character gets selected.
pub fn spawn_selection_ui(
    selected: Res<SelectedCharacterV2>,
    info_query: Query<&CharacterInfoV2>,
    controls: Query<Entity, With<CharSelectControls>>,
    level_data: Res<ClientLevelData>,
    assets: Res<IntroV2Assets>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    let Ok(info) = info_query.get(selected.0) else {
        return;
    };
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found");
        return;
    };

    for entity in controls.iter() {
        commands.entity(entity).despawn();
    }
    commands
        .spawn_scene(info_box(&info.0, &level_data, &assets, &fonts))
        .insert((UiTargetCamera(camera), IntroV2Ui));
    // A character awaiting deletion gets Restore + Cancel instead of
    // Start/Delete/Cancel (#202).
    if info.0.is_deleting {
        commands
            .spawn_scene(deleting_control_buttons(&assets, &fonts, &ui_strings))
            .insert((UiTargetCamera(camera), IntroV2Ui));
    } else {
        commands
            .spawn_scene(selected_control_buttons(&assets, &fonts))
            .insert((UiTargetCamera(camera), IntroV2Ui));
    }
}

/// Tears the selection UI down and brings the Create/Cancel row back once
/// the selection resource disappears (Cancel or successful deletion). The
/// respawned row starts transparent, so `show_screen::<CharSelectControls>`
/// is chained after this system to fade it in.
pub fn on_deselect(
    selection_ui: Query<
        Entity,
        Or<(
            With<CharSelectInfoBox>,
            With<SelectedCharControls>,
            With<DeleteConfirmModal>,
        )>,
    >,
    existing_controls: Query<(), With<CharSelectControls>>,
    assets: Res<IntroV2Assets>,
    fonts: Res<FontAssets>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    for entity in selection_ui.iter() {
        commands.entity(entity).despawn();
    }

    // `resource_removed` can fire once spuriously when the state is
    // re-entered (the resource was dropped while this system wasn't
    // running), so never stack a second row onto an existing one.
    if !existing_controls.is_empty() {
        return;
    }
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found");
        return;
    };
    spawn_default_controls(&mut commands, &assets, &fonts, camera);
}

/// `Activate` observer of the Cancel button in the selection underbar: flies
/// the camera back out and drops the selection ([`on_deselect`] restores the
/// UI).
pub fn on_selection_cancel_activate(
    _activate: On<Activate>,
    pending: Option<Res<PendingWorldJoin>>,
    cam_query: Query<(Entity, &Transform), With<CinematicCamera2>>,
    char_select_scene: Res<ActiveCharSelectSceneV2>,
    origin: Res<WorldOrigin>,
    mut commands: Commands,
) {
    if pending.is_some() {
        return;
    }

    zoom_out_camera(&cam_query, &char_select_scene, &origin, &mut commands);
    commands.remove_resource::<SelectedCharacterV2>();
}

/// The shared `warning_button.ddj` three-state style. Both confirm modals in
/// the intro (delete on char-select, create on char-create) sit on the same
/// 76x32 art, so they share one style rather than each minting their own.
pub(super) fn warning_button_style(assets: &IntroV2Assets) -> ImageButtonStyle {
    ImageButtonStyle {
        normal: assets.warning_button.clone(),
        hover: assets.warning_button_focus.clone(),
        press: assets.warning_button_press.clone(),
        ..Default::default()
    }
}

// Native size of the warning_delete.ddj window frame and its
// warning_button.ddj buttons.
const DELETE_MODAL_WIDTH: f32 = 344.0;
const DELETE_MODAL_HEIGHT: f32 = 192.0;
/// `GDR_BTN_WACCEPT` `90,145,76,32` / `GDR_BTN_WCANCEL` `178,145,76,32`.
const WARNING_BUTTON_Y: f32 = 145.0;
const WARNING_BUTTON_WIDTH: f32 = 76.0;
const WARNING_BUTTON_HEIGHT: f32 = 32.0;

/// Deletion confirmation modal, following the captcha modal's template: a
/// dimming fullscreen scrim (which deliberately blocks clicks on the
/// characters and the underbar) with the centered warning window.
/// Button geometry from `resinfo/pscharacterselect.txt`: every `GDR_BTN_*` on
/// this screen is `0,0,92,41` (we had 91 wide).
const MAIN_BUTTON_W: f32 = 92.0;
const MAIN_BUTTON_H: f32 = 41.0;

fn delete_modal(
    character_name: &str,
    assets: &IntroV2Assets,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
) -> impl Scene {
    let window = assets.warning_delete_window.clone();
    let text_font = fonts.nine.clone();
    let confirm_font = fonts.nine.clone();
    let cancel_font = fonts.nine.clone();
    let button_sound = assets.sound_button_sound_a.clone();
    let cancel_sound = button_sound.clone();

    // The original's copy carries the 7-day reservation warning, which our
    // hand-written line dropped entirely (UIO_STT_CHAR_DEL_CONFIRM).
    let warning = ui_strings
        .get("UIO_STT_CHAR_DEL_CONFIRM")
        .unwrap_or("The character will be deleted.")
        .replace("\\n", "\n");
    let confirmation_text = format!("{character_name}\n\n{warning}");
    // The accept button reads "Delete" in the original, not "Confirm".
    let accept_label = ui_strings
        .get("UIO_SELCHAR_CTL_DELETE")
        .unwrap_or("Delete")
        .to_string();
    let cancel_label = ui_strings
        .get("UIO_COMMON_CTL_CANCEL")
        .unwrap_or("Cancel")
        .to_string();

    bsn! {
        modal_scrim()
        DeleteConfirmModal
        Name("Delete Confirmation Modal V2")
        Children [
            (
                modal_plate(window, DELETE_MODAL_WIDTH, DELETE_MODAL_HEIGHT)
                Children [
                    (
                        // label() starts transparent for the fade systems,
                        // which never run on this modal, so force it white.
                        label(&confirmation_text, text_font, 12.0)
                        TextColor(Color::WHITE)
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(30),
                            top: px(48),
                            width: px(284),
                            height: px(60),
                        }
                    ),
                    (
                        image_button(warning_button_style(assets), WARNING_BUTTON_WIDTH, WARNING_BUTTON_HEIGHT)
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(90),
                            top: px(WARNING_BUTTON_Y),
                            width: px(WARNING_BUTTON_WIDTH),
                            height: px(WARNING_BUTTON_HEIGHT),
                        }
                        ButtonSound({button_sound})
                        Children [ (label(&accept_label, confirm_font, 12.0) TextColor(Color::WHITE)) ]
                        on(on_delete_confirm_activate)
                    ),
                    (
                        image_button(warning_button_style(assets), WARNING_BUTTON_WIDTH, WARNING_BUTTON_HEIGHT)
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(178),
                            top: px(WARNING_BUTTON_Y),
                            width: px(WARNING_BUTTON_WIDTH),
                            height: px(WARNING_BUTTON_HEIGHT),
                        }
                        ButtonSound({cancel_sound})
                        Children [ (label(&cancel_label, cancel_font, 12.0) TextColor(Color::WHITE)) ]
                        on(|_activate: On<Activate>,
                            modal_query: Query<Entity, With<DeleteConfirmModal>>,
                            mut commands: Commands| {
                            for modal in modal_query.iter() {
                                commands.entity(modal).despawn();
                            }
                        })
                    ),
                ]
            ),
        ]
    }
}

/// `Activate` observer of the Delete button: opens the confirmation modal.
pub fn on_delete_activate(
    _activate: On<Activate>,
    selected: Option<Res<SelectedCharacterV2>>,
    info_query: Query<&CharacterInfoV2>,
    existing_modal: Query<(), With<DeleteConfirmModal>>,
    assets: Res<IntroV2Assets>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    if !existing_modal.is_empty() {
        return;
    }
    let Some(selected) = selected else {
        return;
    };
    let Ok(info) = info_query.get(selected.0) else {
        return;
    };
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found");
        return;
    };

    commands
        .spawn_scene(delete_modal(&info.0.name, &assets, &fonts, &ui_strings))
        .insert((UiTargetCamera(camera), IntroV2Ui));
}

/// `Activate` observer of the modal's Confirm button: sends the deletion
/// request and closes the modal; [`on_character_delete_response`] handles
/// the outcome.
pub fn on_delete_confirm_activate(
    _activate: On<Activate>,
    selected: Option<Res<SelectedCharacterV2>>,
    info_query: Query<&CharacterInfoV2>,
    conn_query: Query<&SilkroadConnection, With<AgentConnection>>,
    modal_query: Query<Entity, With<DeleteConfirmModal>>,
    mut commands: Commands,
) {
    for modal in modal_query.iter() {
        commands.entity(modal).despawn();
    }

    let Some(selected) = selected else {
        return;
    };
    let Ok(info) = info_query.get(selected.0) else {
        return;
    };
    let Ok(conn) = conn_query.single() else {
        return;
    };

    let frame = Packet::from(CharacterSelectionActionRequest {
        action: CharacterSelectionAction::Delete,
        name: Some(info.0.name.clone()),
        create: None,
    })
    .into();

    if let Err(e) = conn.get_sender().send(frame) {
        error!("failed to send frame: {}", e.0);
    }
}

/// `Activate` observer of the Restore button shown for a deletion-pending
/// character: sends action 5 with the character's name (same wire shape as
/// Delete). [`on_character_delete_response`] handles the outcome.
///
/// Restore undoes a destructive action rather than performing one, so it sends
/// directly. The original does also carry a confirm dialog for it
/// (`UIO_STT_CHAR_RECOVERY_CONFIRM`) — see the PR for why that is left out here.
pub fn on_restore_activate(
    _activate: On<Activate>,
    selected: Option<Res<SelectedCharacterV2>>,
    info_query: Query<&CharacterInfoV2>,
    conn_query: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    let Some(selected) = selected else {
        return;
    };
    let Ok(info) = info_query.get(selected.0) else {
        return;
    };
    let Ok(conn) = conn_query.single() else {
        return;
    };

    let frame = Packet::from(CharacterSelectionActionRequest {
        action: CharacterSelectionAction::Restore,
        name: Some(info.0.name.clone()),
        create: None,
    })
    .into();

    if let Err(e) = conn.get_sender().send(frame) {
        error!("failed to send frame: {}", e.0);
    }
}

/// Handles the server's answer to a deletion or restore request: on success the
/// camera flies back out and the lobby is rebuilt from a fresh list request (a
/// deleted character then shows its deletion countdown state; a restored one is
/// back to normal).
pub fn on_character_delete_response(
    mut reader: MessageReader<CharacterSelectionActionResponse>,
    mut info_text_writer: MessageWriter<InfoTextV2Update>,
    characters: Query<Entity, With<SelectableCharacterV2>>,
    conn_query: Query<&SilkroadConnection, With<AgentConnection>>,
    cam_query: Query<(Entity, &Transform), With<CinematicCamera2>>,
    char_select_scene: Res<ActiveCharSelectSceneV2>,
    assets: Res<IntroV2Assets>,
    origin: Res<WorldOrigin>,
    options: Res<GameOptions>,
    mut commands: Commands,
) {
    for res in reader.read() {
        let action = match res.action {
            CharacterSelectionAction::Delete => "deletion",
            CharacterSelectionAction::Restore => "restore",
            _ => continue,
        };

        if res.result == 1 {
            zoom_out_camera(&cam_query, &char_select_scene, &origin, &mut commands);
            commands.remove_resource::<SelectedCharacterV2>();
            for entity in characters.iter() {
                commands.entity(entity).despawn();
            }
            if let Ok(conn) = conn_query.single() {
                send_character_list_request(conn);
            }
        } else {
            let code = res.error_code.unwrap_or_default();
            info_text_writer.write(InfoTextV2Update(format!(
                "Character {action} failed (error {code})."
            )));
            if let Some(playback) = options.audio.fx_playback() {
                commands.spawn((AudioPlayer::new(assets.sound_error.clone()), playback));
            }
        }
    }
}

/// Fullscreen overlay shown while the world join is in flight, reusing the
/// vanilla loading artwork.
fn join_loading_overlay(assets: &IntroV2Assets, fonts: &FontAssets) -> impl Scene {
    let background = assets.loading_background.clone();
    let font = fonts.nine.clone();

    bsn! {
        JoinLoadingOverlay
        Name("Join Loading Overlay V2")
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
        }
        BackgroundColor(Color::BLACK)
        // above the fade screen (100) so nothing of the scene shines through
        GlobalZIndex(200)
        Pickable::IGNORE
        Children [
            (
                ImageNode { image: {background} }
                Node { max_width: percent(100), max_height: percent(100) }
                Pickable::IGNORE
            ),
            (
                label("Joining world...", font, 16.0)
                TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, bottom: percent(10) }
            ),
        ]
    }
}

/// `Activate` observer of the Start button: sends the join request and
/// covers the screen with the loading overlay right away — deliberately no
/// fade animation.
pub fn on_start_activate(
    activate: On<Activate>,
    pending: Option<Res<PendingWorldJoin>>,
    selected: Option<Res<SelectedCharacterV2>>,
    info_query: Query<&CharacterInfoV2>,
    conn_query: Query<&SilkroadConnection, With<AgentConnection>>,
    assets: Res<IntroV2Assets>,
    fonts: Res<FontAssets>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    if pending.is_some() {
        return;
    }
    let Some(selected) = selected else {
        return;
    };
    let Ok(info) = info_query.get(selected.0) else {
        return;
    };
    let Ok(conn) = conn_query.single() else {
        return;
    };
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found");
        return;
    };

    // double-activation guard; also clear Pressed because the release
    // observer skips disabled buttons (same pattern as the Connect button)
    commands
        .entity(activate.entity)
        .insert(InteractionDisabled)
        .remove::<Pressed>();

    let frame = Packet::from(CharacterJoinRequest {
        character_name: info.0.name.clone(),
    })
    .into();
    if let Err(e) = conn.get_sender().send(frame) {
        error!("failed to send frame: {}", e.0);
        commands
            .entity(activate.entity)
            .remove::<InteractionDisabled>();
        return;
    }

    commands.insert_resource(PendingWorldJoin {
        character_name: info.0.name.clone(),
    });
    // Carry the picked character into the game scene, which assembles the
    // real model + equipment from it (see game_scene::spawn_selected_player).
    commands.insert_resource(JoiningCharacter(info.0.clone()));
    commands
        .spawn_scene(join_loading_overlay(&assets, &fonts))
        .insert((UiTargetCamera(camera), IntroV2Ui));
}

/// Handles the join response: enters the world scene on success, otherwise
/// tears the loading overlay down again and re-enables the Start button.
pub fn on_character_join_response(
    mut reader: MessageReader<CharacterJoinResponse>,
    mut info_text_writer: MessageWriter<InfoTextV2Update>,
    mut next_scene: ResMut<NextState<SceneState>>,
    overlay_query: Query<Entity, With<JoinLoadingOverlay>>,
    start_buttons: Query<Entity, (With<StartButton>, With<InteractionDisabled>)>,
    assets: Res<IntroV2Assets>,
    options: Res<GameOptions>,
    mut commands: Commands,
) {
    for res in reader.read() {
        if res.result == 1 {
            // Enter the dedicated in-game scene, which assembles the selected
            // character (carried via JoiningCharacter) and shows its own
            // loading screen until terrain + player are ready.
            next_scene.set(SceneState::GameWorld);
        } else {
            let code = res.error.unwrap_or_default();
            error!("[CharacterJoin] failed with error {}", code);
            info_text_writer.write(InfoTextV2Update(format!(
                "Failed to join the world (error {}).",
                code
            )));
            for entity in overlay_query.iter() {
                commands.entity(entity).despawn();
            }
            for entity in start_buttons.iter() {
                commands.entity(entity).remove::<InteractionDisabled>();
            }
            commands.remove_resource::<PendingWorldJoin>();
            if let Some(playback) = options.audio.fx_playback() {
                commands.spawn((AudioPlayer::new(assets.sound_error.clone()), playback));
            }
        }
    }
}

pub fn despawn_controls(query: Query<Entity, With<CharSelectControls>>, mut commands: Commands) {
    for entity in query.iter() {
        commands.entity(entity).despawn();
    }
}

/// Despawns the selection UI (info box, Start/Delete/Cancel row, modal) and
/// drops the selection when the character list state is left.
pub fn despawn_selection_ui(
    query: Query<
        Entity,
        Or<(
            With<CharSelectInfoBox>,
            With<SelectedCharControls>,
            With<DeleteConfirmModal>,
        )>,
    >,
    mut commands: Commands,
) {
    for entity in query.iter() {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<SelectedCharacterV2>();
}

pub fn despawn_characters(
    query: Query<Entity, With<SelectableCharacterV2>>,
    mut commands: Commands,
) {
    for entity in query.iter() {
        commands.entity(entity).despawn();
    }
}

pub fn disconnect_from_agent_server(
    query: Query<Entity, With<AgentConnection>>,
    joining: Option<Res<PendingWorldJoin>>,
    entering_create: Option<Res<super::character_create::EnteringCharacterCreate>>,
    mut commands: Commands,
) {
    // A join in flight means this exit leads into the world scene, which keeps
    // talking to the same agent server. Entering the creation sub-state likewise
    // keeps the connection alive (Create/CheckName go over it).
    if joining.is_some() || entering_create.is_some() {
        return;
    }

    let Ok(entity) = query.single() else {
        return;
    };

    commands.entity(entity).despawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `GDR_BTN_*` on this screen is `0,0,92,41` in
    /// `resinfo/pscharacterselect.txt` — we rendered them 91 wide (#273).
    #[test]
    fn main_buttons_match_the_resinfo_rect() {
        assert_eq!((MAIN_BUTTON_W, MAIN_BUTTON_H), (92.0, 41.0));
    }

    /// The info box's gauges used to be pinned at 100%. The fill is
    /// `current / (1.02^(level-1) * stat * 10)`, clamped, with a full bar when
    /// the maximum cannot be derived.
    #[test]
    fn info_box_gauges_fill_from_the_derived_maximum() {
        // the canonical starting character: level 1, STR/INT 20 -> max 200
        assert_eq!(max_hp_or_mp(1, 20), 200);
        assert_eq!(gauge_fill(200, max_hp_or_mp(1, 20)), 1.0);
        assert_eq!(gauge_fill(100, max_hp_or_mp(1, 20)), 0.5);
        // the captured level-17 sheet: STR 36 -> 494, INT 84 -> 1153
        assert_eq!(max_hp_or_mp(17, 36), 494);
        assert_eq!(max_hp_or_mp(17, 84), 1153);
        assert_eq!(gauge_fill(247, 494), 0.5);
    }

    /// No panic and no empty bar when the maximum is unknown, and a server
    /// value above the derived maximum cannot overflow the track.
    #[test]
    fn gauge_fill_is_clamped_and_falls_back_to_full() {
        assert_eq!(gauge_fill(0, 0), 1.0, "unknown max -> full bar");
        assert_eq!(gauge_fill(500, 0), 1.0);
        assert_eq!(gauge_fill(0, 200), 0.0, "a dead character reads empty");
        assert_eq!(gauge_fill(999, 200), 1.0, "over-max clamps to full");
        assert!((0.0..=1.0).contains(&gauge_fill(u32::MAX, 1)));
        // level 0 must not produce a negative exponent
        assert_eq!(max_hp_or_mp(0, 20), 200);
    }

    /// The delete-warning modal's two buttons are `GDR_BTN_WACCEPT`
    /// `90,145,76,32` and `GDR_BTN_WCANCEL` `178,145,76,32`. We had y=136.
    #[test]
    fn delete_modal_buttons_match_the_resinfo_rects() {
        assert_eq!(WARNING_BUTTON_Y, 145.0);
        assert_eq!((WARNING_BUTTON_WIDTH, WARNING_BUTTON_HEIGHT), (76.0, 32.0));
    }

    /// `UIO_STT_CHAR_DEL_CONFIRM` stores its line breaks as the two characters
    /// `\` `n`, so the raw string has to be unescaped or the whole 7-day
    /// warning renders as one run-on line.
    #[test]
    fn the_deletion_warning_unescapes_its_line_breaks() {
        let raw = "The character will be deleted.\\nIf you delete the character, \
                   it will be reserved for 7 days.";
        let shown = raw.replace("\\n", "\n");
        assert!(shown.contains('\n'), "literal \\n must become a real break");
        assert!(!shown.contains("\\n"), "no escapes may survive");
        assert_eq!(shown.lines().count(), 2);
    }
}
