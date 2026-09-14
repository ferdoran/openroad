use crate::plugins::player::Player;
use crate::AppMode;
use crate::GameState;
use bevy::anti_alias::fxaa::Fxaa;
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{Layer, RenderLayers};
use bevy::camera::{ClearColorConfig, Hdr, ImageRenderTarget, RenderTarget};
use bevy::camera_controller::free_camera::{FreeCamera, FreeCameraPlugin};
use bevy::core_pipeline::prepass::DepthPrepass;
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::transform::TransformSystems;

/// Marker for the debug-mode fly camera, driven by Bevy's first-party [`FreeCamera`] controller.
#[derive(Component, Default)]
pub struct DebugCamera;
use crate::plugins::animation_sounds::LISTENER_EAR_GAP;
use crate::plugins::config::graphics::BloomSettings;
use crate::plugins::config::input::MouseScheme;
use crate::plugins::config::ClientConfig;
use crate::plugins::cursor::GameCursorCamera;
use crate::plugins::environment::reflections::sky_reflection_env_light;
use crate::plugins::map::terrain::{FOG_RANGE, REGION_SIZE, VISIBLE_RANGE};
use crate::plugins::settings::options::{GameOptions, SightMode};
use crate::plugins::world_origin::WorldOrigin;
use crate::scenes::world_scene::SpawnPoints;
use crate::scenes::{in_playable_world, SceneState};

/// Tags an entity as capable of panning and orbiting.
#[derive(Component)]
pub struct PlayerCamera;

/// Closest / farthest the follow camera may sit from the character (world units).
const CAMERA_MIN_DISTANCE: f32 = 40.0;
const CAMERA_MAX_DISTANCE: f32 = 400.0;
/// Distance the follow camera starts at.
const CAMERA_START_DISTANCE: f32 = 150.0;
/// World units the camera moves per mouse-wheel notch.
const CAMERA_ZOOM_SPEED: f32 = 20.0;
/// Radians of rotation per pixel of mouse movement while orbiting (right button held).
const CAMERA_ORBIT_SENSITIVITY: f32 = 0.005;
/// Height above the character's feet the camera aims at (bodies are ~18 units tall).
const CAMERA_TARGET_HEIGHT: f32 = 20.0;
/// Pitch (angle above the horizontal plane, radians) the camera starts at.
const CAMERA_START_PITCH: f32 = 0.5;
/// Pitch limits: just above horizontal up to nearly top-down (stays below PI/2 so
/// the up vector never degenerates in `look_at`).
const CAMERA_MIN_PITCH: f32 = 0.1;
const CAMERA_MAX_PITCH: f32 = 1.45;
/// How quickly the camera catches up to the character, as a lerp fraction/second.
const CAMERA_FOLLOW_SPEED: f32 = 8.0;
/// Fixed pitch of [`SightMode::Quarter`], radians above the horizontal plane.
///
/// **Ours, and the only number in the sight modes that is** (#379): the string
/// table says the quarter view's "height is fixed to this perspective" but no
/// file gives the angle. 0.9 rad ≈ 52° sits between the start pitch (0.5) and
/// the top-down clamp (1.45), which is the shallow-isometric look the mode's
/// own description promises ("provided for those inconvenienced by 3D motion").
const QUARTER_VIEW_PITCH: f32 = 0.9;

/// Orbit state of the third-person follow camera: zoom via the scroll wheel,
/// yaw via right-mouse drag.
#[derive(Resource)]
pub struct CameraRig {
    /// Current distance from the character, clamped to
    /// `[CAMERA_MIN_DISTANCE, CAMERA_MAX_DISTANCE]`.
    pub distance: f32,
    /// Yaw around the character (radians), controlled by dragging left/right
    /// with the right mouse button held. `0` sits the camera on the +Z side.
    pub yaw: f32,
    /// Pitch above the horizontal plane (radians), dragged up/down; clamped to
    /// `[CAMERA_MIN_PITCH, CAMERA_MAX_PITCH]`.
    pub pitch: f32,
}

impl Default for CameraRig {
    fn default() -> Self {
        Self {
            distance: CAMERA_START_DISTANCE,
            yaw: 0.0,
            pitch: CAMERA_START_PITCH,
        }
    }
}

#[derive(Default, Component)]
pub struct CinematicCamera;

#[derive(Default, Component)]
pub struct CinematicCamera2;

#[derive(Component)]
pub struct CameraPositionText;

pub enum CameraLayers {
    Main = 0,
    /// Offscreen player-head portrait for the mini-info HUD.
    Portrait = 1,
    /// Offscreen full-body player view for the inventory equipment panel.
    PaperDoll = 2,
    // 3-6 free
    // 7-10 reserved for UI Stuff
    Ui = 8,
    LoadingScreen = 9,
    Debug = 10,
    Cursor = 11,
    Undefined,
}

#[allow(dead_code)]
const CAMERA_ORIGIN: Vec3 = Vec3::new(577.938, 266.985, 467.280);
#[allow(dead_code)]
const CENTER_ORIGIN: Vec3 = Vec3::new(578.000, 259.430, 428.000);
#[allow(dead_code)]
const ROTATION_TO_CENTER: Vec3 = Vec3::new(0.190, 3.140, 0.000);
#[allow(dead_code)]
const DISTANCE: f32 = 40.0;
/// Near plane, from `Map/config.ifo` — a `JMXVCAMR1002` record whose 111 bytes
/// parse EOF-exact (`docs/formats/camr-jmxvcamr.md`). Replaces a magic 0.5.
const NEAR: f32 = 1.0;

/// Far plane: one region past the distance at which fog reaches full opacity,
/// so the projection can never clip geometry the fog has not already hidden.
///
/// Deliberately **not** `config.ifo`'s 5500. Our fog runs from `VISIBLE_RANGE *
/// REGION_SIZE` (3840) to `(VISIBLE_RANGE + FOG_RANGE) * REGION_SIZE` (5760),
/// and Bevy's linear fog is `alpha = (d - start) / (end - start)`, so at 5500
/// terrain is only ~86% opaque: a 5500 far plane would visibly cut partially
/// transparent geometry out of the outer fog ring and break the deliberate
/// hand-off to the horizon-matched `FOG_COLOR`. Deriving it from the streaming
/// constants means retuning those cannot reintroduce that clip — the invariant
/// is pinned by `the_far_plane_clears_the_fog_ceiling`.
///
/// The old 200000 was simply wrong: against a 0.5 near plane it gave a
/// 400,000:1 depth range. Tightening that to 7680:1 also buys depth precision
/// for the water SSR raymarch, which reads the view uniforms.
const FAR: f32 = (VISIBLE_RANGE + FOG_RANGE + 1) as f32 * REGION_SIZE;

/// Vertical FOV. `config.ifo` stores 45°, which is also Bevy's own
/// `PerspectiveProjection` default; the previous 1.0 rad (57.3°) was a magic
/// number. The record carries no aspect ratio, so "vertical" is inferred from
/// its left-handed D3D basis (`D3DXMatrixPerspectiveFovLH` takes a y-direction
/// FOV) rather than proven.
const FOV: f32 = std::f32::consts::FRAC_PI_4;

#[allow(dead_code)]
const VIEW_MATRIX: Mat4 = Mat4::from_cols_array(&[
    -1.0, -0.0, 0.002, 0.0, 0.0, 0.982, -0.189, 0.0, -0.002, -0.189, -0.982, 0.0, 578.681,
    -174.104, 508.389, 1.0,
]);
#[allow(dead_code)]
const PROJECTION_MATRIX: Mat4 = Mat4::from_cols_array(&[
    1.299, 0.0, 0.0, 0.0, 0.0, 1.732, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, -1.0, 0.0,
]);
#[allow(dead_code)]
const UNKNOWN_MATRIX: Mat4 = Mat4::from_cols_array(&[
    -1.0, 0.0, 0.002, 0.0, 0.0, 0.982, -0.189, 0.0, -0.002, -0.189, -0.982, 0.0, 0.0, 0.0, 40.0,
    1.0,
]);

const PROJECTION: Projection = Projection::Perspective(PerspectiveProjection {
    fov: FOV,
    near: NEAR,
    far: FAR,
    aspect_ratio: 1.0,
    near_clip_plane: Vec4::new(0.0, 0.0, -1.0, -NEAR),
});

impl CameraLayers {
    pub fn get_2d_layers() -> [Layer; 3] {
        [
            CameraLayers::Ui.into(),
            CameraLayers::LoadingScreen.into(),
            CameraLayers::Debug.into(),
        ]
    }
}

impl Into<Layer> for CameraLayers {
    fn into(self) -> Layer {
        self as Layer
    }
}

impl From<Layer> for CameraLayers {
    fn from(item: Layer) -> Self {
        match item {
            0 => CameraLayers::Main,
            1 => CameraLayers::Portrait,
            //2..7 => CameraLayers::xxx,
            8 => CameraLayers::Ui,
            9 => CameraLayers::LoadingScreen,
            10 => CameraLayers::Debug,
            11 => CameraLayers::Cursor,
            _ => CameraLayers::Undefined,
        }
    }
}

impl Default for PlayerCamera {
    fn default() -> Self {
        PlayerCamera {}
    }
}

/// Spawn the 2d UI camera every HUD and menu renders onto (`OnEnter(GameState::Loading)`).
///
/// Moved here from `plugins::ui` (#56-C), unchanged: this is a camera, and the
/// `Hdr` coupling below is the counterpart of [`attach_bloom`] in this same
/// module — keeping the two apart is what made the coupling easy to miss.
fn setup_ui_camera(mut commands: Commands, config: Res<ClientConfig>) {
    let ui_camera = commands
        .spawn((
            Camera2d,
            Camera {
                // renders after / on top of the main camera
                clear_color: ClearColorConfig::None,
                order: 1,
                ..default()
            },
            RenderLayers::from_layers(&CameraLayers::get_2d_layers()),
        ))
        .id();

    // Not optional when bloom is on, despite this camera having nothing to bloom:
    // bevy keys the shared main texture on `(target, usage, format, msaa)`, so a
    // camera whose format disagrees with the 3d camera's gets its *own* texture.
    // With `clear_color: None` this pass would then composite into an empty
    // texture and blit that over the window, erasing the 3d view. See
    // `attach_bloom`.
    if config.graphics.bloom.enabled {
        commands.entity(ui_camera).insert(Hdr);
    }
}

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app
            // TODO: ThirdPersonCameraPlugin removed - no Bevy 0.16 compatible version
            .add_plugins(FreeCameraPlugin)
            // The 2d UI camera, moved here with `setup_ui_camera` (#56-C).
            .add_systems(OnEnter(GameState::Loading), setup_ui_camera)
            .init_resource::<CameraRig>()
            // Ungated: every scene that spawns the player+fly camera pair
            // (world sandbox AND the testing scenes) needs Tab to actually
            // switch them — the old `SceneState::WorldSandbox` gate left the fly
            // camera frozen active in the testing scenes and also lost the
            // spawn-time `AppMode` race there. Scenes without both cameras
            // hit the `single_mut()` early-returns, so this is a no-op
            // everywhere else.
            .add_systems(Update, switch_camera)
            // Before anything renders: every window camera must agree on the
            // sample count (see `apply_window_camera_msaa`).
            //
            // Ordered: the MSAA pass keys on `RenderTarget::Window`, and
            // `apply_render_scale` is what stops a main-view camera being one.
            // Reversed, a scaled camera would silently miss `graphics.msaa`.
            .add_systems(
                Update,
                (apply_window_camera_msaa, apply_render_scale).chain(),
            )
            .add_systems(
                Update,
                // 4 Hz + write-on-change: rewriting the bevy_ui TextSpan
                // relayouts it, needlessly so every frame while standing still
                update_cam_position_text
                    .run_if(bevy::time::common_conditions::on_timer(
                        std::time::Duration::from_millis(250),
                    ))
                    .run_if(in_state(SceneState::WorldSandbox)),
            )
            // Ungated: every scene's cameras (world, intro, char-select) carry
            // the sky reflection probe and benefit from freezing it once baked.
            .add_systems(
                Update,
                crate::plugins::environment::reflections::freeze_baked_env_maps,
            )
            // Follow the character in PostUpdate, after all Update movement has
            // run and before transform propagation, so it samples a settled
            // player position every frame (avoids one-frame follow stutter).
            .add_systems(
                PostUpdate,
                follow_player_camera
                    .before(TransformSystems::Propagate)
                    .run_if(in_playable_world),
            );
    }
}

#[allow(dead_code)]
fn get_primary_window_size(windows: &Query<&mut Window>) -> Vec2 {
    let window = windows.single().unwrap();
    let window = Vec2::new(window.width() as f32, window.height() as f32);
    window
}

/// Spawn a camera like this
pub fn spawn_player_camera(
    app_mode: Res<State<AppMode>>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    existing_world_cameras: Query<Entity, Or<(With<PlayerCamera>, With<DebugCamera>)>>,
    origin: Res<WorldOrigin>,
    mut images: ResMut<Assets<Image>>,
    config: Res<ClientConfig>,
) {
    if !existing_world_cameras.is_empty() {
        return;
    }

    let is_debug_mode = *app_mode.get() == AppMode::DebugMode;
    let bloom = &config.graphics.bloom;

    let start = origin.to_render(SpawnPoints::jangan());

    // sky reflections for sheen/specular materials. Both cameras get one:
    // bevy's realtime filtering runs per-probe regardless of which camera is
    // active, so `freeze_baked_env_maps` bakes each once and then removes the
    // generator to stop the per-frame filtering cost.
    let sky_reflections = sky_reflection_env_light(&mut images);

    // spawn camera
    // TODO: ThirdPersonCamera removed - no Bevy 0.16 compatible version; re-add when available
    // DepthPrepass feeds the high-quality water's screen-space reflection raymarch (see
    // water_hq.wgsl). MSAA stays at the Bevy default (Sample4): forcing it off breaks
    // this project's main view render entirely (terrain/objects vanish, only the skybox
    // draws — verified by bisecting DepthPrepass vs Msaa::Off independently), and a
    // Sample2 experiment (2026-07-15, to remove the redundancy with the Fxaa post pass)
    // coincided with broken rendering + egui artifacts — reverted pending isolation.
    // The raymarcher's depth read compiles fine against a multisampled prepass texture
    // in this material-shader context (see the comment on `DEPTH_PREPASS` in
    // water_hq.wgsl).
    // Bloom (and the `Hdr` marker it requires) is layered on top of that Sample4 +
    // DepthPrepass combination; it changes the main texture's *format*, not its
    // sample count, so the caveat above is unaffected. See `attach_bloom`.
    let player_camera = commands
        .spawn((
            RenderLayers::layer(CameraLayers::Main.into()),
            Name::from("PlayerCamera"),
            Camera3d::default(),
            Camera {
                order: 0,
                is_active: !is_debug_mode,
                ..default()
            },
            PROJECTION,
            Transform::from_translation(start),
            Fxaa::default(),
            main_view_render_settings(&config),
            DepthPrepass,
            PlayerCamera::default(),
            GameCursorCamera::default(),
            // The ear of the game: animation sounds are emitted from their
            // entity and panned/attenuated against this listener
            // (`plugins::animation_sounds`).
            SpatialListener::new(LISTENER_EAR_GAP),
            sky_reflections.clone(),
        ))
        .id();
    attach_bloom(&mut commands, player_camera, bloom);

    let fly_camera = commands
        .spawn((
            RenderLayers::layer(CameraLayers::Main.into()),
            Name::from("FlyCamera"),
            Camera3d::default(),
            Camera {
                order: 0,
                is_active: is_debug_mode,
                ..default()
            },
            Transform::from_translation(Vec3::new(start.x - 2.0, start.y + 2.5, start.z + 5.0))
                .looking_at(start, Vec3::Y),
            PROJECTION,
            Fxaa::default(),
            main_view_render_settings(&config),
            DepthPrepass,
            DebugCamera,
            GameCursorCamera::default(),
            FreeCamera {
                walk_speed: 50.0,
                run_speed: 300.0,
                ..default()
            },
            sky_reflections,
        ))
        .id();
    attach_bloom(&mut commands, fly_camera, bloom);

    commands
        .spawn((
            RenderLayers::layer(CameraLayers::Debug.into()),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(12.0),
                bottom: Val::Px(12.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.025, 0.03, 0.72)),
            Name::from("Camera Position Panel"),
        ))
        .with_children(|children| {
            children
                .spawn((
                    RenderLayers::layer(CameraLayers::Debug.into()),
                    Text("Position: ".into()),
                    TextColor(Color::WHITE),
                    TextFont {
                        font_size: FontSize::Px(18.0),
                        font: asset_server
                            .load::<Font>(crate::assets::BUNDLED_FALLBACK_FACE)
                            .into(),
                        ..default()
                    },
                    Name::from("Camera Position Label"),
                ))
                .with_children(|position_text| {
                    position_text.spawn((
                        TextSpan(String::new()),
                        TextFont {
                            font_size: FontSize::Px(18.0),
                            font: asset_server
                                .load::<Font>(crate::assets::BUNDLED_FALLBACK_FACE)
                                .into(),
                            ..default()
                        },
                        TextColor(bevy::color::palettes::css::GOLD.into()),
                        CameraPositionText,
                    ));
                });
        });
}

/// Spawn the in-game scene's camera: only the third-person follow
/// [`PlayerCamera`], made active unconditionally (no fly camera, so
/// `switch_camera` — which is World-only — never toggles it off, regardless of
/// `AppMode`). Same render/reflection setup as the World `PlayerCamera`.
pub fn spawn_game_camera(
    mut commands: Commands,
    existing_world_cameras: Query<Entity, Or<(With<PlayerCamera>, With<DebugCamera>)>>,
    origin: Res<WorldOrigin>,
    mut images: ResMut<Assets<Image>>,
    config: Res<ClientConfig>,
) {
    if !existing_world_cameras.is_empty() {
        return;
    }

    let start = origin.to_render(SpawnPoints::jangan());
    let sky_reflections = sky_reflection_env_light(&mut images);

    let camera = commands
        .spawn((
            RenderLayers::layer(CameraLayers::Main.into()),
            Name::from("PlayerCamera"),
            Camera3d::default(),
            Camera {
                order: 0,
                is_active: true,
                ..default()
            },
            PROJECTION,
            Transform::from_translation(start),
            Fxaa::default(),
            main_view_render_settings(&config),
            DepthPrepass,
            PlayerCamera::default(),
            GameCursorCamera::default(),
            // The ear of the game: animation sounds are emitted from their
            // entity and panned/attenuated against this listener
            // (`plugins::animation_sounds`).
            SpatialListener::new(LISTENER_EAR_GAP),
            sky_reflections,
        ))
        .id();
    attach_bloom(&mut commands, camera, &config.graphics.bloom);
}

pub fn spawn_cinematic_camera<T>(
    query: Query<Entity, With<T>>,
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    config: Res<ClientConfig>,
) where
    T: Default + Component,
{
    if !query.is_empty() {
        return;
    }
    let camera = commands
        .spawn((
            RenderLayers::layer(CameraLayers::Main.into()),
            Name::from("Cinematic Camera"),
            T::default(),
            Camera3d::default(),
            Camera {
                order: 0,
                is_active: true,
                ..default()
            },
            PROJECTION,
            Fxaa::default(),
            main_view_render_settings(&config),
            // the intro scenes render harbor water too — the high-quality water's
            // reflection raymarch needs the depth prepass (see the PlayerCamera
            // comment above for the Msaa caveat)
            DepthPrepass,
            // metal on character-selection equipment reflects the sky probe
            sky_reflection_env_light(&mut images),
        ))
        .id();
    // character select is where the `+N` enhancement shine (up to 8.0) is on
    // display, so this camera wants bloom as much as the world ones do
    attach_bloom(&mut commands, camera, &config.graphics.bloom);
}

/// Point a set of cameras at `image`, and report which ones were taken.
///
/// The ids come back because [`apply_render_scale`] has to tag exactly the
/// cameras it took with [`RenderScaled`] — it must be able to put those, and
/// only those, back on the window later.
///
/// Only `RenderTarget::Window` is rewritten, so a camera already rendering
/// offscreen (the HUD portrait and paper-doll rigs) is never stolen.
///
/// `scale_factor` is the load-bearing argument. A camera's *logical* viewport is
/// its physical size divided by this, and that logical size is what
/// `world_to_viewport` / `viewport_to_world` speak in. Both callers exploit
/// that:
///
/// - [`apply_render_scale`] sizes the image at `window_physical * scale` and
///   passes `window_scale_factor * scale`, so the logical viewport stays
///   *identical to the window's* and every screen-space call site — the picking
///   ray (`cursor`), nameplates, hit counts, world anchors — keeps working in
///   window coordinates with no conversion.
/// - `screenshot` passes the window's own scale factor so the UI lays out at the
///   same logical size it would on screen; without it a retina capture renders
///   the HUD at half size.
pub(crate) fn retarget_window_cameras<'a>(
    targets: impl Iterator<Item = (Entity, Mut<'a, RenderTarget>)>,
    image: &Handle<Image>,
    scale_factor: f32,
) -> Vec<Entity> {
    let mut retargeted = Vec::new();
    for (entity, mut target) in targets {
        if matches!(*target, RenderTarget::Window(_)) {
            *target = RenderTarget::Image(ImageRenderTarget {
                handle: image.clone(),
                scale_factor,
            });
            retargeted.push(entity);
        }
    }
    retargeted
}

/// Physical size and `ImageRenderTarget::scale_factor` for the offscreen 3D
/// target at `scale`.
///
/// The scale cancels: logical viewport = `physical / scale_factor` =
/// `(window_physical * scale) / (window_scale_factor * scale)`, which is the
/// window's own logical size. That identity is the whole design — it is what
/// lets the 3D view shrink without any screen-space code knowing — and it is
/// pinned by `the_logical_viewport_survives_scaling`.
///
/// The physical size is rounded and floored at one texel: a zero-sized render
/// target is a wgpu validation error, and a window can legitimately report 0
/// while minimised.
pub fn render_scale_target(
    window_physical: UVec2,
    window_scale_factor: f32,
    scale: f32,
) -> (UVec2, f32) {
    let size = (window_physical.as_vec2() * scale)
        .round()
        .as_uvec2()
        .max(UVec2::ONE);
    (size, window_scale_factor * scale)
}

/// Tell `camera_system` that a camera's render target changed shape.
///
/// It recomputes `Camera::computed.target_info` only when the *target* signals a
/// change — a window resize/create message, or an `AssetEvent` on the image —
/// never when the `RenderTarget` **component** is simply reassigned
/// (`bevy_render::camera::camera_system`, the `is_changed` condition). Swapping a
/// camera from an image back to the window therefore leaves it sizing its depth
/// texture from the *image*, while the colour attachment is the window:
///
/// ```text
/// Attachments have differing sizes: the depth attachment's texture view has
/// extent (1892, 792, 1) but is followed by the color attachment at index 0's
/// texture view which has (3440, 1440, 1)
/// ```
///
/// which is a validation error, and bevy quits the application on it. Going *to*
/// an image happens to work only because `Assets::add` fires `AssetEvent::Added`
/// for the new handle, which is one of the conditions.
///
/// `Projection` change detection is another of those conditions, and it is the
/// one thing here we own, so touching it is how this code says "my target's
/// dimensions moved". Pinned by `restoring_to_native_must_notify_the_camera`.
fn retarget_notify(projection: &mut Mut<Projection>) {
    projection.set_changed();
}

/// Marks a camera whose window target [`apply_render_scale`] swapped for the
/// offscreen image, so returning to 1.0 knows exactly what to undo.
#[derive(Component)]
pub struct RenderScaled;

/// Marks the full-screen sprite that composites the scaled 3D image back under
/// the HUD.
#[derive(Component)]
pub struct RenderScaleBlit;

/// The live offscreen target, plus the inputs it was built for so a window
/// resize or a scale change can be detected without rebuilding every frame.
#[derive(Resource)]
pub struct RenderScaleState {
    image: Handle<Image>,
    blit: Entity,
    window_physical: UVec2,
    window_scale_factor: f32,
    scale: f32,
}

/// Render the 3D view at `graphics.render_scale` of the window and composite it
/// under a native-resolution HUD.
///
/// Idea: the frame is fill-bound, so pixels are the largest lever left — but
/// lowering the *window* resolution blurs text and icons, which is where it
/// shows. This scales only the 3D cameras: they render into an image, and one
/// full-screen sprite on the UI camera's layer stretches it back. bevy_ui draws
/// in its own pass after the 2d main pass, so the HUD is always on top of that
/// sprite without any z fighting for the ordering.
///
/// At scale 1.0 this retargets nothing and despawns anything it made, so the
/// default path is byte-identical to having no render scale at all.
///
/// Note what this *removes*: with the 3D cameras on their own image they no
/// longer share the window's main texture with the UI `Camera2d`, so the
/// `(target, usage, format, msaa)` agreement trap that [`attach_bloom`] and
/// [`apply_window_camera_msaa`] both exist to dodge simply does not apply
/// between them while a scale is active. The UI camera does have to start
/// clearing, though — it is now the only camera drawing to the window, and its
/// `ClearColorConfig::None` would otherwise leave the frame undefined wherever
/// the sprite does not land exactly.
#[allow(clippy::too_many_arguments)]
fn apply_render_scale(
    mut commands: Commands,
    settings: Option<Res<crate::plugins::dev::render_debug::RenderDebugSettings>>,
    config: Res<ClientConfig>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    state: Option<ResMut<RenderScaleState>>,
    mut world_cameras: Query<
        (Entity, &mut RenderTarget, &mut Projection),
        (With<Camera3d>, Without<Camera2d>),
    >,
    scaled: Query<Entity, With<RenderScaled>>,
    mut ui_camera: Query<&mut Camera, (With<Camera2d>, Without<Camera3d>)>,
    mut blit: Query<(&mut Sprite, &mut Transform), With<RenderScaleBlit>>,
) {
    // The live panel/BRP value wins once seeded; config is the fallback for the
    // frames before `seed_terrain_settings_from_config` has run, and for any
    // build without the resource at all.
    let requested = crate::plugins::config::graphics::RenderScale(
        settings
            .map(|s| s.render_scale)
            .unwrap_or(config.graphics.render_scale.0),
    );

    let Ok(window) = windows.single() else {
        return;
    };
    let window_physical = UVec2::new(window.physical_width(), window.physical_height());
    let window_scale_factor = window.scale_factor();

    // A minimised window reports 0 and cannot host a render target, so it is
    // treated as "no scaling" rather than clamped to a 1x1 image.
    if requested.is_native() || window_physical.min_element() == 0 {
        let Some(state) = state else {
            return;
        };
        // Only the cameras this system actually took: the HUD portrait and
        // paper-doll rigs are `Camera3d` too and were never on the window.
        for (entity, mut target, mut projection) in &mut world_cameras {
            if !scaled.contains(entity) {
                continue;
            }
            *target = RenderTarget::Window(bevy::window::WindowRef::Primary);
            retarget_notify(&mut projection);
            commands.entity(entity).remove::<RenderScaled>();
        }
        if let Ok(mut camera) = ui_camera.single_mut() {
            camera.clear_color = ClearColorConfig::None;
        }
        commands.entity(state.blit).despawn();
        commands.remove_resource::<RenderScaleState>();
        return;
    }

    let scale = requested.factor();
    let (size, target_scale_factor) =
        render_scale_target(window_physical, window_scale_factor, scale);
    let logical = window_physical.as_vec2() / window_scale_factor;

    if let Some(mut state) = state {
        if state.scale == scale
            && state.window_physical == window_physical
            && state.window_scale_factor == window_scale_factor
        {
            return;
        }
        // Resize in place: the cameras already hold this handle, so replacing
        // the image's contents keeps every target valid.
        if let Some(mut image) = images.get_mut(&state.image) {
            // `resize` reallocates the pixel buffer; the handle is unchanged, so
            // the cameras pointing at it stay valid across a window resize.
            image.resize(Extent3d {
                width: size.x,
                height: size.y,
                depth_or_array_layers: 1,
            });
        }
        for (_, mut target, mut projection) in &mut world_cameras {
            if let RenderTarget::Image(image_target) = &mut *target {
                image_target.scale_factor = target_scale_factor;
                retarget_notify(&mut projection);
            }
        }
        if let Ok((mut sprite, mut transform)) = blit.single_mut() {
            sprite.custom_size = Some(logical);
            transform.translation = Vec3::new(0.0, 0.0, BLIT_Z);
        }
        state.scale = scale;
        state.window_physical = window_physical;
        state.window_scale_factor = window_scale_factor;
        return;
    }

    let image = images.add(render_scale_image(size, config.graphics.bloom.enabled));
    let taken = retarget_window_cameras(
        world_cameras.iter_mut().map(|(e, target, _)| (e, target)),
        &image,
        target_scale_factor,
    );
    for (entity, _, mut projection) in &mut world_cameras {
        if taken.contains(&entity) {
            retarget_notify(&mut projection);
        }
    }
    if taken.is_empty() {
        // No 3D camera on the window yet (the scene is still loading). Retry
        // next frame rather than leaving a blit over an empty target.
        return;
    }
    for entity in &taken {
        commands.entity(*entity).insert(RenderScaled);
    }

    let blit = commands
        .spawn((
            Name::from("RenderScaleBlit"),
            Sprite {
                image: image.clone(),
                custom_size: Some(logical),
                ..default()
            },
            Transform::from_xyz(0.0, 0.0, BLIT_Z),
            RenderScaleBlit,
            RenderLayers::layer(CameraLayers::Ui.into()),
        ))
        .id();

    if let Ok(mut camera) = ui_camera.single_mut() {
        camera.clear_color = ClearColorConfig::Default;
    }

    info!(
        "render_scale {scale:.2}: 3D at {}x{}, HUD at {}x{} ({} camera(s) retargeted)",
        size.x,
        size.y,
        window_physical.x,
        window_physical.y,
        taken.len()
    );

    commands.insert_resource(RenderScaleState {
        image,
        blit,
        window_physical,
        window_scale_factor,
        scale,
    });
}

/// Far enough back that anything else drawn on the UI camera's 2d layers (the
/// loading screen, debug sprites) composites over the 3D view rather than
/// under it.
const BLIT_Z: f32 = -1000.0;

/// The offscreen colour target the scaled 3D view renders into.
///
/// The format has to match what the cameras expect to write, and that is the
/// third axis of the same `(target, usage, format, msaa)` coupling
/// [`attach_bloom`] documents: `Bloom` requires `Hdr`, and an `Hdr` camera's
/// emissive and `+N` shine values only survive on a float target. Writing them
/// into an 8-bit image would clip exactly the highlights bloom exists to catch,
/// so the format follows the bloom setting rather than being a fixed default.
fn render_scale_image(size: UVec2, hdr: bool) -> Image {
    let (format, clear) = if hdr {
        // 4 half-floats of opaque black.
        (
            TextureFormat::Rgba16Float,
            vec![0, 0, 0, 0, 0, 0, 0x00, 0x3C],
        )
    } else {
        (TextureFormat::Rgba8UnormSrgb, vec![0, 0, 0, 255])
    };
    let mut image = Image::new_fill(
        Extent3d {
            width: size.x.max(1),
            height: size.y.max(1),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &clear,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.usage =
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING;
    image
}

/// The two per-pixel render settings the main-view cameras take from config.
///
/// Both are bevy defaults that were never chosen here, and both cost per *lit
/// screen pixel* rather than per draw or per shadow-map texel — which is what
/// makes them levers on a fill-bound frame. Returned as one bundle so the four
/// spawn sites cannot drift apart, and so the reason they travel together is
/// written once. See `config::graphics::{ShadowFiltering, TonemappingConfig}`.
///
/// Unlike `Msaa` these need no cross-camera agreement — they do not participate
/// in the shared main texture's `(target, usage, format, msaa)` key — so they
/// are inserted at the spawn sites rather than driven by a system.
fn main_view_render_settings(
    config: &ClientConfig,
) -> (
    bevy::light::ShadowFilteringMethod,
    bevy::core_pipeline::tonemapping::Tonemapping,
) {
    (
        config.graphics.shadows.filtering.to_method(),
        config.graphics.tonemapping.to_tonemapping(),
    )
}

/// Keep every window-targeting camera on the configured sample count
/// (`graphics.msaa`).
///
/// This is a system rather than an insert at each spawn site on purpose. Bevy
/// keys the shared main texture on `(target, usage, format, msaa)`, so the
/// cameras drawing to the window must *all* agree or the disagreeing one gets
/// its own texture — and the UI `Camera2d`, which clears nothing, then blits an
/// empty texture over the 3D view. That failure has already happened once on
/// the `format` axis of the same key (see [`attach_bloom`] and
/// [`setup_ui_camera`]), and it happened because agreement was a thing to
/// remember at a spawn site. Driving it off `Added<Camera>` means a camera
/// added later cannot forget.
///
/// The offscreen portrait / paper-doll rigs render to their own images, which
/// are keyed separately, so they are deliberately left alone.
fn apply_window_camera_msaa(
    mut commands: Commands,
    config: Res<ClientConfig>,
    // RenderTarget is its own component in 0.19, not a field on Camera.
    cameras: Query<(Entity, &RenderTarget), Added<Camera>>,
) {
    let msaa = config.graphics.msaa.to_msaa();
    for (entity, target) in &cameras {
        if matches!(target, RenderTarget::Window(_)) {
            commands.entity(entity).insert(msaa);
        }
    }
}

/// Give a main-view camera the configured bloom, if enabled.
///
/// `Bloom` is `#[require(Hdr)]`, so this also flips the camera's main texture to
/// a float format. That is the whole point — the emissive/shine values above 1.0
/// only survive on an HDR target — but it comes with a trap: bevy keys the
/// shared main texture on `(target, usage, format, msaa)`, so *every* camera
/// drawing to the window must agree on the format. The UI `Camera2d` therefore
/// gets a matching `Hdr` in [`setup_ui_camera`]; without it the 2d pass
/// composites into a second, empty texture and blits it over the 3d view.
///
/// The offscreen portrait / paper-doll cameras render to their own `Rgba8`
/// images and must stay LDR, which is why this is opt-in per camera rather than
/// a blanket `Query<&Camera>` pass.
pub fn attach_bloom(commands: &mut Commands, camera: Entity, settings: &BloomSettings) {
    if settings.enabled {
        commands.entity(camera).insert(settings.to_bloom());
    }
}

/// Which mouse devices the camera is allowed to consume.
///
/// Idea: vanilla splits the two mouse devices between *changing the view* and
/// *using a shortcut* — never both on the camera. Which way round is the
/// player's choice, persisted as SROptionSet id 3101 (`isMouseShortcutSwapped`,
/// `textuisystem.txt` 917/918). This struct is what the camera reads so that
/// the option is an actual behaviour and not a value nobody consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseCameraRoles {
    /// Wheel notches zoom the camera.
    pub wheel_changes_view: bool,
    /// Holding the right button orbits the camera.
    pub right_button_changes_view: bool,
}

/// Resolve the mouse roles from the config knob + the persisted option.
///
/// `mouse_shortcut_swapped` is the id-3101 bool. Which of its two values maps
/// to which vanilla string is **ours to choose** — no default for id 3101
/// exists anywhere in `resinfo/` (`docs/re/ui/options-controls.md` §9) — so we
/// read the field's own name literally: *swapped* means the shortcut moved to
/// the wheel, i.e. 918 `UIIT_STT_USE_WHEEL_TO_USE_SKILL`, and the right button
/// changes the view instead. Unswapped is 917
/// `UIIT_STT_USE_WHEEL_TO_CHANGE_SIGHT`.
///
/// UNKNOWN, deliberately not guessed: openroad has no "use shortcut" mouse
/// action at all, so in either vanilla state the device that vanilla gives to
/// shortcuts currently does nothing. Wiring that half needs a mouse-quickslot
/// feature, which is not in this change.
pub fn mouse_camera_roles(scheme: MouseScheme, mouse_shortcut_swapped: bool) -> MouseCameraRoles {
    match scheme {
        // Non-original: both devices drive the camera (see `config::input`).
        MouseScheme::ZoomOrbit => MouseCameraRoles {
            wheel_changes_view: true,
            right_button_changes_view: true,
        },
        MouseScheme::Vanilla => MouseCameraRoles {
            wheel_changes_view: !mouse_shortcut_swapped,
            right_button_changes_view: mouse_shortcut_swapped,
        },
    }
}

/// Which orbit axes the player still controls in a given [`SightMode`].
///
/// Idea: the original defines its three view modes by *what they take away*,
/// not by three separate cameras — see the `_DESC1/2` strings quoted on
/// [`SightMode`]. So the mode is a constraint applied to the one
/// [`CameraRig`] we already have, and this struct is that constraint made
/// explicit so it can be tested without a window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SightAxes {
    /// The drag's horizontal component moves the camera. False in
    /// [`SightMode::ThirdPerson`], where yaw is pinned behind the character.
    pub yaw_is_free: bool,
    /// The drag's vertical component moves the camera. False in
    /// [`SightMode::Quarter`], whose "height is fixed to this perspective".
    pub pitch_is_free: bool,
    /// The pitch the mode holds the camera at, if it holds one.
    pub locked_pitch: Option<f32>,
}

/// Resolve a [`SightMode`] into the axes it leaves the player.
///
/// Zoom is deliberately free in all three: no mode's description mentions
/// distance, and taking the wheel away in two of three modes would be an
/// invention on top of the transcription.
pub fn sight_axes(mode: SightMode) -> SightAxes {
    match mode {
        SightMode::Free => SightAxes {
            yaw_is_free: true,
            pitch_is_free: true,
            locked_pitch: None,
        },
        SightMode::ThirdPerson => SightAxes {
            yaw_is_free: false,
            pitch_is_free: true,
            locked_pitch: None,
        },
        SightMode::Quarter => SightAxes {
            yaw_is_free: true,
            pitch_is_free: false,
            locked_pitch: Some(QUARTER_VIEW_PITCH),
        },
    }
}

/// The rig yaw that puts the camera directly behind `player`.
///
/// The rig's horizontal offset direction is `from_rotation_y(yaw) * +Z` and a
/// Bevy transform faces `-Z`, so "behind" is exactly the player's own Y euler
/// angle — no offset term, which is why this is a one-liner rather than a
/// constant someone would later have to justify.
fn yaw_behind(player: &Transform) -> f32 {
    player.rotation.to_euler(EulerRot::YXZ).0
}

/// Third-person camera that follows the character: scroll wheel zooms between
/// [`CAMERA_MIN_DISTANCE`] and [`CAMERA_MAX_DISTANCE`], and dragging with the
/// right mouse button held orbits it freely around the character (yaw + pitch).
/// The camera sits at the rig's yaw/pitch and smoothly catches up each frame.
///
/// Which of the two devices it may use comes from [`mouse_camera_roles`]; a
/// device the camera does not own still has its events drained here, so a
/// scheme change never replays a backlog.
#[allow(clippy::too_many_arguments)]
fn follow_player_camera(
    time: Res<Time>,
    mut scroll: MessageReader<MouseWheel>,
    mut motion: MessageReader<MouseMotion>,
    buttons: Res<ButtonInput<MouseButton>>,
    config: Res<ClientConfig>,
    options: Res<GameOptions>,
    mut rig: ResMut<CameraRig>,
    player_query: Query<&Transform, (With<Player>, Without<PlayerCamera>)>,
    mut camera_query: Query<&mut Transform, With<PlayerCamera>>,
    ui_hover: Query<
        &Hovered,
        Or<(
            With<crate::plugins::hud::chat::ui::ChatMessageList>,
            With<crate::plugins::hud::skill_window::ui::SkillWindowRoot>,
        )>,
    >,
) {
    let roles = mouse_camera_roles(
        config.input.mouse_scheme,
        options.keymap.mouse_shortcut_swapped,
    );

    // Wheeling over the chat list or the skill window scrolls that UI,
    // not the camera.
    let ui_hovered = ui_hover.iter().any(|hovered| hovered.get());
    for event in scroll.read() {
        if ui_hovered || !roles.wheel_changes_view {
            continue;
        }
        // Scrolling up (positive) pulls the camera in.
        rig.distance = (rig.distance - event.y * CAMERA_ZOOM_SPEED)
            .clamp(CAMERA_MIN_DISTANCE, CAMERA_MAX_DISTANCE);
    }

    // Orbit only while the right button is held (left click is move-to); when
    // it isn't, drop the accumulated motion so it doesn't jump on the next drag.
    // An axis the sight mode has taken away still consumes its half of the
    // drag rather than banking it, so switching back to Free does not snap.
    let axes = sight_axes(options.camera.sight);
    if roles.right_button_changes_view && buttons.pressed(MouseButton::Right) {
        let delta: Vec2 = motion.read().map(|m| m.delta).sum();
        if axes.yaw_is_free {
            rig.yaw -= delta.x * CAMERA_ORBIT_SENSITIVITY;
        }
        if axes.pitch_is_free {
            // Drag down (delta.y positive) tilts the camera overhead toward top-down.
            rig.pitch = (rig.pitch + delta.y * CAMERA_ORBIT_SENSITIVITY)
                .clamp(CAMERA_MIN_PITCH, CAMERA_MAX_PITCH);
        }
    } else {
        motion.clear();
    }

    if let Some(pitch) = axes.locked_pitch {
        rig.pitch = pitch;
    }

    let Ok(player) = player_query.single() else {
        return;
    };

    // Third person: the camera angle "is fixed behind the character", so yaw
    // is re-derived every frame from the character rather than dragged.
    if !axes.yaw_is_free {
        rig.yaw = yaw_behind(player);
    }
    let Ok(mut camera) = camera_query.single_mut() else {
        return;
    };

    let target = player.translation + Vec3::Y * CAMERA_TARGET_HEIGHT;
    // Offset direction on the orbit sphere: yaw around Y, pitch above the plane.
    let offset = Quat::from_rotation_y(rig.yaw) * Vec3::new(0.0, rig.pitch.sin(), rig.pitch.cos());
    let desired = target + offset * rig.distance;

    let t = (CAMERA_FOLLOW_SPEED * time.delta_secs()).min(1.0);
    camera.translation = camera.translation.lerp(desired, t);
    camera.look_at(target, Vec3::Y);
}

fn switch_camera(
    app_mode: Res<State<AppMode>>,
    mut player_cam_query: Query<&mut Camera, (With<PlayerCamera>, Without<DebugCamera>)>,
    mut fly_cam_query: Query<&mut Camera, (Without<PlayerCamera>, With<DebugCamera>)>,
) {
    let Ok(mut player_cam) = player_cam_query.single_mut() else {
        return;
    };
    let Ok(mut fly_cam) = fly_cam_query.single_mut() else {
        return;
    };

    let is_debug_mode = *app_mode.get() == AppMode::DebugMode;
    fly_cam.is_active = is_debug_mode;
    player_cam.is_active = !is_debug_mode;
}

pub fn despawn_cinematic_camera<T>(
    mut commands: Commands,
    cinematic_cam_query: Query<Entity, With<T>>,
) where
    T: Default + Component,
{
    cinematic_cam_query.iter().for_each(|cam| {
        commands.entity(cam).despawn();
    });
}

pub fn disable_camera<T>(mut cinematic_cam_query: Query<&mut Camera, With<T>>)
where
    T: Default + Component,
{
    cinematic_cam_query.iter_mut().for_each(|mut cam| {
        cam.is_active = false;
    });
}

pub fn enable_camera<T>(mut cinematic_cam_query: Query<&mut Camera, With<T>>)
where
    T: Default + Component,
{
    cinematic_cam_query.iter_mut().for_each(|mut cam| {
        cam.is_active = true;
    });
}

fn update_cam_position_text(
    cam_query: Query<(&GlobalTransform, &Camera), With<Camera3d>>,
    mut text_query: Query<&mut TextSpan, With<CameraPositionText>>,
    origin: Res<WorldOrigin>,
) {
    for (transform, camera) in cam_query.iter() {
        if !camera.is_active {
            continue;
        }
        let Ok(mut text) = text_query.single_mut() else {
            return;
        };
        let position = transform.translation();
        // region ids live in SRO space; the camera is in render space
        let region = origin.to_sro(position) / 1920.0;

        let formatted = format!("{position} - Region {} x {}", -region.x, region.z);
        if text.0 != formatted {
            text.0 = formatted;
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The three sight modes are defined by which orbit axis they remove — see
    /// the `_DESC` strings quoted on [`SightMode`]. If this table ever drifts,
    /// the pane's descriptions become false advertising.
    #[test]
    fn each_sight_mode_removes_the_axis_its_own_description_names() {
        // "Operates on multidirectional angle control and mouse movement"
        let free = sight_axes(SightMode::Free);
        assert!(free.yaw_is_free && free.pitch_is_free);
        assert_eq!(free.locked_pitch, None);

        // "Camera angle is fixed behind the character"
        let third = sight_axes(SightMode::ThirdPerson);
        assert!(
            !third.yaw_is_free,
            "third person pins yaw behind the character"
        );
        assert!(third.pitch_is_free, "only the yaw is named as fixed");

        // "The height is fixed to this perspective."
        let quarter = sight_axes(SightMode::Quarter);
        assert!(quarter.yaw_is_free, "only the height is named as fixed");
        assert!(!quarter.pitch_is_free);
        assert_eq!(quarter.locked_pitch, Some(QUARTER_VIEW_PITCH));
    }

    /// No mode's description mentions distance, so taking the wheel away would
    /// be an invention on top of a transcription (ADR-0009).
    #[test]
    fn no_sight_mode_takes_the_zoom_away() {
        for mode in SightMode::ALL {
            let axes = sight_axes(mode);
            assert!(
                axes.yaw_is_free || axes.pitch_is_free,
                "{mode:?} would leave the player no orbit control at all"
            );
        }
    }

    /// The quarter view's angle is the one number here that is ours, so it has
    /// to stay inside the rig's own clamp or the mode fights the drag limits.
    #[test]
    fn the_quarter_view_pitch_is_reachable_by_the_rig() {
        assert!(
            (CAMERA_MIN_PITCH..=CAMERA_MAX_PITCH).contains(&QUARTER_VIEW_PITCH),
            "the fixed quarter pitch {QUARTER_VIEW_PITCH} is outside the rig clamp"
        );
    }

    /// "Behind" has to be derived, not guessed: the rig's horizontal offset is
    /// `from_rotation_y(yaw) * +Z` and a Bevy transform faces `-Z`, so the
    /// camera must land on the side the character's back is on.
    #[test]
    fn yaw_behind_puts_the_camera_at_the_characters_back() {
        for angle in [0.0f32, 1.0, -2.0, std::f32::consts::PI] {
            let player = Transform::from_rotation(Quat::from_rotation_y(angle));
            let yaw = yaw_behind(&player);

            let horizontal = Quat::from_rotation_y(yaw) * Vec3::Z;
            let back = player.rotation * Vec3::Z;
            assert!(
                horizontal.dot(back) > 0.999,
                "at yaw {angle} the camera sat at {horizontal:?}, not behind ({back:?})"
            );
        }
    }

    /// A restart must not silently move anybody's camera: the shipped default
    /// is the free orbit openroad already had. (Which mode vanilla defaults to
    /// is UNKNOWN — no option tree in `resinfo/` carries a default value.)
    #[test]
    fn the_default_sight_mode_is_the_behaviour_we_already_shipped() {
        let options = GameOptions::default();
        assert_eq!(options.camera.sight, SightMode::Free);
        let axes = sight_axes(options.camera.sight);
        assert!(axes.yaw_is_free && axes.pitch_is_free && axes.locked_pitch.is_none());
    }

    /// #605: SROptionSet id 3101 was parsed, persisted and read by nobody.
    /// These are the two vanilla states of `textuisystem.txt` 917/918 — exactly
    /// one device changes the view, the other is reserved for shortcuts.
    #[test]
    fn vanilla_mouse_scheme_gives_the_camera_one_device() {
        // 917 `..._TO_CHANGE_SIGHT`: wheel changes view, right button shortcuts.
        let unswapped = mouse_camera_roles(MouseScheme::Vanilla, false);
        assert!(unswapped.wheel_changes_view);
        assert!(!unswapped.right_button_changes_view);

        // 918 `..._TO_USE_SKILL`: wheel uses the shortcut, right button views.
        let swapped = mouse_camera_roles(MouseScheme::Vanilla, true);
        assert!(!swapped.wheel_changes_view);
        assert!(swapped.right_button_changes_view);

        // The pair always swaps both devices at once, never neither/both.
        assert_ne!(unswapped, swapped);
    }

    /// Our own scheme is the named non-original one (ADR-0009): it puts both
    /// devices on the camera, which is neither vanilla state, and it must stay
    /// unaffected by id 3101.
    #[test]
    fn zoom_orbit_scheme_ignores_the_persisted_option() {
        for swapped in [false, true] {
            let roles = mouse_camera_roles(MouseScheme::ZoomOrbit, swapped);
            assert!(roles.wheel_changes_view);
            assert!(roles.right_button_changes_view);
        }
    }

    /// Bevy's linear fog is `alpha = (d - start) / (end - start)`, with
    /// `start = VISIBLE_RANGE * REGION_SIZE` and `end = (VISIBLE_RANGE +
    /// FOG_RANGE) * REGION_SIZE` (`map::terrain::rendering::fog`). Anything the
    /// far plane cuts before `end` is still partly transparent, so it pops out
    /// of view instead of finishing its fade into `FOG_COLOR`.
    ///
    /// `config.ifo`'s 5500 sits 260 units inside our 5760 fog end — only ~86%
    /// opaque — which is why #108 does not adopt it verbatim.
    #[test]
    fn the_far_plane_clears_the_fog_ceiling() {
        let fog_end = (VISIBLE_RANGE + FOG_RANGE) as f32 * REGION_SIZE;

        assert!(
            FAR >= fog_end,
            "far plane {FAR} clips geometry the fog has not hidden yet (fog ends at {fog_end})"
        );
    }

    /// The old 200000 far plane against a 0.5 near plane gave a 400,000:1 depth
    /// range, which is where the depth buffer loses precision. Keep the ratio
    /// in a sane band — this is the reason the fix is worth making at all.
    #[test]
    fn the_depth_range_stays_precise() {
        assert!(NEAR > 0.0);
        assert!(
            FAR / NEAR <= 20_000.0,
            "depth range {}:1 is too wide for f32 depth precision",
            FAR / NEAR
        );
    }

    /// `config.ifo` stores 45°, matching Bevy's own `PerspectiveProjection`
    /// default. Bevy's `fov` field is vertical radians.
    #[test]
    fn the_fov_matches_the_config_ifo_record() {
        assert!(
            (FOV.to_degrees() - 45.0).abs() < 1e-4,
            "{}",
            FOV.to_degrees()
        );
    }
}

#[cfg(test)]
mod render_scale_tests {
    use super::*;
    use crate::plugins::config::graphics::RenderScale;

    /// The invariant the whole design rests on: scaling the image *and* the
    /// target's scale factor by the same amount leaves the camera's logical
    /// viewport equal to the window's. That is why `world_to_viewport` and
    /// `viewport_to_world` — the picking ray, nameplates, hit counts, world
    /// anchors — need no per-call-site conversion. If this ever fails, every
    /// screen-space feature in the HUD silently misaligns.
    #[test]
    fn the_logical_viewport_survives_scaling() {
        let window_physical = UVec2::new(3440, 1440);
        for window_scale_factor in [1.0, 1.25, 2.0] {
            let native = window_physical.as_vec2() / window_scale_factor;
            for scale in [1.0, 0.9, 0.75, 0.5, 0.25] {
                let (size, target_scale_factor) =
                    render_scale_target(window_physical, window_scale_factor, scale);
                let logical = size.as_vec2() / target_scale_factor;
                assert!(
                    (logical - native).abs().max_element() < 1.0,
                    "scale {scale} at dpi {window_scale_factor}: logical {logical:?} \
                     drifted from the window's {native:?}"
                );
            }
        }
    }

    /// The point of the lever: fewer pixels. Pinned because a scale that does
    /// not actually shrink the target would still pass the viewport test above.
    #[test]
    fn scaling_down_actually_reduces_the_pixel_count() {
        let window = UVec2::new(3440, 1440);
        let full = render_scale_target(window, 1.0, 1.0).0.element_product();
        let half = render_scale_target(window, 1.0, 0.5).0.element_product();
        assert_eq!(full, 3440 * 1440);
        // 0.5 linear is 0.25 of the area.
        assert!(
            (half as f32 / full as f32 - 0.25).abs() < 0.01,
            "{half} of {full}"
        );
    }

    /// wgpu rejects a zero-sized texture outright, which would abort the run.
    /// A minimised window legitimately reports 0.
    #[test]
    fn a_degenerate_window_never_produces_a_zero_sized_target() {
        let (size, _) = render_scale_target(UVec2::ZERO, 1.0, 0.5);
        assert_eq!(size, UVec2::ONE);
        let (size, _) = render_scale_target(UVec2::new(1, 1), 1.0, 0.25);
        assert_eq!(size, UVec2::ONE);
    }

    /// 1.0 must be a true no-op: `apply_render_scale` reads `is_native` to
    /// decide whether to retarget anything at all, so the default config path
    /// has to be indistinguishable from having no render scale in the tree.
    #[test]
    fn the_default_scale_retargets_nothing() {
        assert!(RenderScale::default().is_native());
        assert_eq!(RenderScale::default().0, 1.0);
        assert!(RenderScale(1.0).is_native());
        assert!(!RenderScale(0.75).is_native());
    }

    /// Garbage in config must degrade to full resolution, not to a broken
    /// target: 0 and negatives would be a zero-sized image, >1 would render
    /// more pixels than the window has, and NaN propagates into the texture
    /// descriptor.
    #[test]
    fn out_of_range_scales_fall_back_to_full_resolution() {
        for bad in [0.0, -0.5, 1.5, f32::NAN, f32::INFINITY] {
            assert_eq!(
                RenderScale(bad).factor(),
                1.0,
                "render_scale {bad} should have fallen back to 1.0"
            );
            assert!(RenderScale(bad).is_native());
        }
        // Below the floor clamps rather than falling back, so a user asking for
        // "as low as it goes" still gets a scaled view.
        assert_eq!(RenderScale(0.1).factor(), RenderScale::MIN);
        assert!(!RenderScale(0.1).is_native());
    }

    /// The projection is how a target-shape change is signalled to
    /// `camera_system`, which otherwise never recomputes `target_info` on a
    /// `RenderTarget` reassignment and leaves the camera sizing its depth
    /// texture from the target it *used* to have. That mismatch is a wgpu
    /// validation error, and bevy quits the application on those, so this is
    /// the difference between the render-scale knob working and taking the
    /// client down on the way back to 1.0.
    #[test]
    fn retarget_notify_marks_the_projection_changed() {
        let mut world = World::new();
        let camera = world.spawn(Projection::default()).id();
        world.clear_trackers();

        let mut projection = world.get_mut::<Projection>(camera).unwrap();
        assert!(
            !projection.is_changed(),
            "clear_trackers should have reset the tick, or this test proves nothing"
        );

        retarget_notify(&mut projection);
        assert!(projection.is_changed());
    }

    /// Bloom requires `Hdr`, and an 8-bit target would clip exactly the
    /// emissive and `+N` shine values bloom exists to catch. The offscreen
    /// format is the third axis of the coupling `attach_bloom` documents.
    #[test]
    fn the_offscreen_format_follows_the_bloom_setting() {
        let ldr = render_scale_image(UVec2::new(64, 64), false);
        assert_eq!(ldr.texture_descriptor.format, TextureFormat::Rgba8UnormSrgb);
        let hdr = render_scale_image(UVec2::new(64, 64), true);
        assert_eq!(hdr.texture_descriptor.format, TextureFormat::Rgba16Float);

        for image in [&ldr, &hdr] {
            let usage = image.texture_descriptor.usage;
            assert!(usage.contains(TextureUsages::RENDER_ATTACHMENT));
            // The blit sprite samples it.
            assert!(usage.contains(TextureUsages::TEXTURE_BINDING));
        }
    }
}
