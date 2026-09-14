//! One-shot screenshot capture: `SCREENSHOT=<path>` renders, grabs a frame, writes
//! a PNG and exits.
//!
//! Idea (#564): UI work on this project is judged against the original client's
//! own art, but an autonomous session cannot use `screencapture` — it needs a
//! composited, unlocked desktop and returns an all-black frame otherwise. So the
//! capture is taken inside our own render graph instead of asking the OS.
//!
//! The first implementation captured the *swapchain* (`Screenshot::primary_window`)
//! on the theory that a readback of the surface texture bypasses the compositor.
//! Measured on macOS with the display locked (#709) it does not: every pixel came
//! back `Rgb([0,0,0])`, including for a pure-UI control scene that cannot
//! legitimately be black. The swapchain is the one render target whose contents
//! depend on the window server, so a window that is never composited yields a
//! cleared surface.
//!
//! Therefore the capture frame is rendered **offscreen**: we allocate an image the
//! size of the window's framebuffer, point every window-targeted camera at it for
//! the capture frame, and read *that* back (`Screenshot::image`). A camera drawing
//! into a GPU image never touches the window server, so the result is identical on
//! a locked display, over SSH, or on a visible desktop — and it is deterministic,
//! which is what a scripted acceptance test needs. Cameras that already render to
//! their own images (HUD portrait / paper-doll RTTs) are left alone.
//!
//! The capture is asynchronous — the readback lands one or more frames after the
//! request — so this is a small state machine rather than a single system: wait for
//! the scene to settle, retarget the cameras, give the render world a couple of
//! frames to allocate the image and draw into it, spawn one [`Screenshot`], and
//! exit from the observer that receives the image. A watchdog covers the case where
//! the image never arrives, so a scripted run fails instead of hanging forever.
//!
//! The cameras are never pointed back at the window: the process exits as soon as
//! the PNG is written (or the watchdog fires), so restoring them would only add a
//! state to the machine that nothing can observe.
//!
//! Exit codes are the contract for that script:
//!
//! | code | meaning |
//! |------|---------|
//! | 0    | PNG written |
//! | 3    | frame captured but could not be encoded/written |
//! | 4    | capture never came back within the watchdog |
//! | 101  | the client panicked (rust's default) — i.e. *not* a screenshot failure |
//!
//! **A screenshot of this client is SRO asset data**: it is the user's own PK2
//! art, rendered. It must never be committed. Bare filenames are therefore
//! resolved under the gitignored [`SCREENSHOT_DIR`], the same treatment
//! `packet_dump/` gets for runtime output.

use crate::plugins::camera::retarget_window_cameras;
use bevy::app::AppExit;
use bevy::asset::RenderAssetUsages;
use bevy::camera::{Camera, RenderTarget};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::window::PrimaryWindow;
use std::env;
use std::path::PathBuf;
use std::time::Duration;

/// Gitignored output directory for bare `SCREENSHOT=` filenames. See the module
/// comment: screenshots are the user's PK2 art and are never committed.
pub const SCREENSHOT_DIR: &str = "screenshots";

/// Seconds to let the app run before asking for the frame. The scenes stream
/// terrain and textures out of the PK2s for several seconds after startup, so
/// capturing on frame 0 reliably photographs an empty loading screen. Override
/// with `SCREENSHOT_AFTER`.
const DEFAULT_DELAY_SECS: f32 = 10.0;

/// How long to wait for the GPU readback after the request before giving up.
/// The readback normally lands within a frame or two; this only exists so an
/// unattended run cannot hang.
const CAPTURE_TIMEOUT_SECS: f32 = 30.0;

/// Frames between retargeting the cameras and asking for the readback. The image
/// asset has to reach the render world and be turned into a `GpuImage` (one
/// extract + prepare cycle) before a camera can draw into it, and
/// `prepare_screenshots` skips a target whose `GpuImage` does not exist yet with
/// `Unknown image for screenshot`. Three frames is that cycle plus slack; the cost
/// of overshooting is three frames, the cost of undershooting is a lost capture.
const WARMUP_FRAMES: u32 = 3;

/// Pixel format of the offscreen capture target. 8-bit sRGB is what the window
/// surface presents and what `try_into_dynamic` can decode; the HDR cameras
/// tone-map into their output attachment, so nothing is lost that the window
/// would have shown.
const CAPTURE_FORMAT: TextureFormat = TextureFormat::Rgba8UnormSrgb;

/// Size used when there is no primary window to measure (headless runs). 1280x720
/// is the resolution the #709 measurements were taken at, so a capture without a
/// window is directly comparable to one with it.
const FALLBACK_SIZE: UVec2 = UVec2::new(1280, 720);

#[derive(Resource, Debug, Clone, PartialEq)]
pub struct ScreenshotRequest {
    pub path: PathBuf,
    pub delay: Duration,
}

impl ScreenshotRequest {
    /// Build the request from the raw env values, or `None` when `SCREENSHOT`
    /// is unset/empty. Split out from the plugin so the parsing is testable
    /// without an `App`.
    ///
    /// `SCREENSHOT=1` (or `true`) is treated as "somewhere sensible" rather
    /// than a file literally named `1`, because that is what a shell user
    /// typing a flag means.
    fn from_env_values(screenshot: Option<&str>, after: Option<&str>) -> Option<Self> {
        let raw = screenshot?.trim();
        if raw.is_empty() {
            return None;
        }

        let mut path = if matches!(raw, "1" | "true" | "yes") {
            PathBuf::from(SCREENSHOT_DIR).join("screenshot.png")
        } else {
            PathBuf::from(raw)
        };
        // A bare filename goes to the gitignored dir; an explicit path is the
        // caller's business.
        if path.parent() == Some(std::path::Path::new("")) {
            path = PathBuf::from(SCREENSHOT_DIR).join(path);
        }
        if path.extension().is_none() {
            path.set_extension("png");
        }

        let delay = after
            .and_then(|value| match value.trim().parse::<f32>() {
                Ok(secs) if secs >= 0.0 => Some(secs),
                _ => {
                    warn!("SCREENSHOT_AFTER={value:?} is not a non-negative number; using default");
                    None
                }
            })
            .unwrap_or(DEFAULT_DELAY_SECS);

        Some(Self {
            path,
            delay: Duration::from_secs_f32(delay),
        })
    }

    fn from_env() -> Option<Self> {
        let screenshot = env::var("SCREENSHOT").ok();
        let after = env::var("SCREENSHOT_AFTER").ok();
        Self::from_env_values(screenshot.as_deref(), after.as_deref())
    }
}

/// Where the state machine is (see the module comment).
#[derive(Default, Debug, PartialEq)]
enum Phase {
    /// Letting the scene stream in; nothing has been touched yet.
    #[default]
    Settling,
    /// The cameras now draw into `target`; waiting for the render world to catch up.
    Warmup {
        frames_left: u32,
        target: Handle<Image>,
    },
    /// The readback has been requested; waiting for the image (watchdog armed).
    Awaiting,
}

/// Where the state machine is, and how long it has been there.
#[derive(Resource)]
struct ScreenshotClock {
    elapsed: Duration,
    phase: Phase,
}

/// The offscreen frame the capture is rendered into.
///
/// `RENDER_WORLD` only — nothing on the CPU side reads these pixels; the readback
/// is done by `bevy_render`'s screenshot pass, which needs `COPY_SRC`, and the
/// cameras need `RENDER_ATTACHMENT`. `TEXTURE_BINDING` is what the screenshot
/// pipeline samples the texture through.
fn offscreen_target(size: UVec2) -> Image {
    let mut image = Image::new_fill(
        Extent3d {
            width: size.x.max(1),
            height: size.y.max(1),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        CAPTURE_FORMAT,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.usage =
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC | TextureUsages::TEXTURE_BINDING;
    image
}

pub struct ScreenshotPlugin;

impl Plugin for ScreenshotPlugin {
    fn build(&self, app: &mut App) {
        let Some(request) = ScreenshotRequest::from_env() else {
            return;
        };
        info!(
            "screenshot: will capture to {} after {:.1}s",
            request.path.display(),
            request.delay.as_secs_f32()
        );
        app.insert_resource(request)
            .insert_resource(ScreenshotClock {
                elapsed: Duration::ZERO,
                phase: Phase::Settling,
            })
            .add_systems(Update, drive_screenshot);
    }
}

/// Drive the capture: settle, retarget offscreen, warm up, ask, watch.
#[allow(clippy::too_many_arguments)]
fn drive_screenshot(
    mut commands: Commands,
    time: Res<Time>,
    request: Res<ScreenshotRequest>,
    mut clock: ResMut<ScreenshotClock>,
    mut images: ResMut<Assets<Image>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut cameras: Query<(Entity, &mut RenderTarget), With<Camera>>,
    mut exit: MessageWriter<AppExit>,
) {
    clock.elapsed += time.delta();
    let elapsed = clock.elapsed;

    // `mem::take` so the phase can be matched by value while `clock` stays usable.
    let next = match std::mem::take(&mut clock.phase) {
        Phase::Settling if elapsed < request.delay => Phase::Settling,
        Phase::Settling => {
            let window = windows.single().ok();
            let measured = window
                .map(|window| UVec2::new(window.physical_width(), window.physical_height()))
                .filter(|size| size.x > 0 && size.y > 0);
            if measured.is_none() {
                // Said out loud because the fallback and a real 1280x720 window
                // produce byte-identical dimensions in the log below, and a reader
                // has to be able to tell "measured the window" from "guessed".
                warn!(
                    "screenshot: no primary window to measure — capturing at the \
                     fallback {}x{}",
                    FALLBACK_SIZE.x, FALLBACK_SIZE.y
                );
            }
            let size = measured.unwrap_or(FALLBACK_SIZE);
            let scale_factor = window.map(|window| window.scale_factor()).unwrap_or(1.0);
            let target = images.add(offscreen_target(size));
            let retargeted =
                retarget_window_cameras(cameras.iter_mut(), &target, scale_factor).len();
            if retargeted == 0 {
                warn!(
                    "screenshot: no camera renders to the window — the capture will be \
                     whatever the offscreen target was cleared to"
                );
            }
            info!(
                "screenshot: capturing an offscreen {}x{} frame ({retargeted} camera(s) \
                 retargeted from the window)",
                size.x, size.y
            );
            clock.elapsed = Duration::ZERO;
            Phase::Warmup {
                frames_left: WARMUP_FRAMES,
                target,
            }
        }
        Phase::Warmup {
            frames_left,
            target,
        } if frames_left > 0 => Phase::Warmup {
            frames_left: frames_left - 1,
            target,
        },
        Phase::Warmup { target, .. } => {
            commands.spawn(Screenshot::image(target)).observe(save);
            clock.elapsed = Duration::ZERO;
            Phase::Awaiting
        }
        Phase::Awaiting => {
            if elapsed.as_secs_f32() > CAPTURE_TIMEOUT_SECS {
                error!(
                    "screenshot: no frame came back within {CAPTURE_TIMEOUT_SECS:.0}s — \
                     the render target produced no readback"
                );
                exit.write(AppExit::from_code(4));
            }
            Phase::Awaiting
        }
    };
    clock.phase = next;
}

/// Encode and write the captured frame, then exit.
///
/// Bevy ships `save_to_disk` for this, but it cannot also end the run, and
/// relying on two observers for the same event would make the exit depend on
/// observer ordering. Doing both here keeps "written" and "exited 0" the same
/// event.
fn save(
    captured: On<ScreenshotCaptured>,
    request: Res<ScreenshotRequest>,
    mut exit: MessageWriter<AppExit>,
) {
    let path = &request.path;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                error!("screenshot: cannot create {}: {err}", parent.display());
                exit.write(AppExit::from_code(3));
                return;
            }
        }
    }

    // The alpha channel carries brightness when HDR is on (bloom puts the
    // cameras on an HDR target), so RGB8 is what actually looks like the frame.
    let frame = match captured.image.clone().try_into_dynamic() {
        Ok(image) => image.to_rgb8(),
        Err(err) => {
            error!("screenshot: unsupported frame format: {err}");
            exit.write(AppExit::from_code(3));
            return;
        }
    };

    // A uniformly flat frame is the signature of a render target that never
    // drew anything, which is a very different failure from "no file". Say so
    // rather than leaving a reviewer to open a black PNG and guess.
    let mut pixels = frame.pixels();
    if let Some(first) = pixels.next().copied() {
        if pixels.all(|pixel| *pixel == first) {
            warn!(
                "screenshot: every pixel of the captured frame is {first:?} — \
                 the frame rendered nothing"
            );
        }
    }

    match frame.save_with_format(path, image::ImageFormat::Png) {
        Ok(()) => {
            info!(
                "screenshot: wrote {} ({}x{})",
                path.display(),
                frame.width(),
                frame.height()
            );
            exit.write(AppExit::Success);
        }
        Err(err) => {
            error!("screenshot: cannot write {}: {err}", path.display());
            exit.write(AppExit::from_code(3));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::Camera3d;
    use bevy::camera::ImageRenderTarget;
    use bevy::window::WindowRef;

    /// #709: the capture must come from an offscreen image, not the swapchain, so
    /// the target we allocate has to be usable as one — a colour attachment the
    /// screenshot pass may copy out of. A missing `COPY_SRC`/`RENDER_ATTACHMENT`
    /// here is a black PNG at runtime and nothing at compile time.
    #[test]
    fn offscreen_target_is_a_readable_render_attachment() {
        let image = offscreen_target(UVec2::new(1280, 720));
        let usage = image.texture_descriptor.usage;
        assert!(usage.contains(TextureUsages::RENDER_ATTACHMENT));
        assert!(usage.contains(TextureUsages::COPY_SRC));
        assert!(usage.contains(TextureUsages::TEXTURE_BINDING));
        assert_eq!(image.texture_descriptor.format, CAPTURE_FORMAT);
        assert_eq!(image.texture_descriptor.size.width, 1280);
        assert_eq!(image.texture_descriptor.size.height, 720);

        // A degenerate window size must not produce a zero-sized texture (wgpu
        // rejects those outright, which would abort the whole run).
        let zero = offscreen_target(UVec2::ZERO);
        assert_eq!(zero.texture_descriptor.size.width, 1);
        assert_eq!(zero.texture_descriptor.size.height, 1);
    }

    /// The retarget must take the window cameras and *only* those: the HUD portrait
    /// and paper-doll cameras already render to their own images, and moving them
    /// would both blank the HUD in the captured frame and put two writers on the
    /// one target being read back.
    #[test]
    fn retarget_takes_window_cameras_and_leaves_rtt_cameras_alone() {
        let mut world = World::new();
        let capture = Handle::<Image>::default();
        let portrait_image: Handle<Image> =
            bevy::asset::uuid_handle!("00000709-0000-0000-0000-000000000709");

        let window_cam = world
            .spawn((
                Camera3d::default(),
                Camera::default(),
                RenderTarget::Window(WindowRef::Primary),
            ))
            .id();
        let portrait_cam = world
            .spawn((
                Camera3d::default(),
                Camera::default(),
                RenderTarget::Image(ImageRenderTarget::from(portrait_image.clone())),
            ))
            .id();

        let mut query = world.query_filtered::<(Entity, &mut RenderTarget), With<Camera>>();
        let retargeted = retarget_window_cameras(query.iter_mut(&mut world), &capture, 2.0);
        assert_eq!(retargeted, vec![window_cam]);

        let window_target = world.get::<RenderTarget>(window_cam).unwrap();
        match window_target {
            RenderTarget::Image(target) => {
                assert_eq!(target.handle, capture);
                // the window's scale factor has to survive, or the UI lays out at
                // half size on a retina capture
                assert_eq!(target.scale_factor, 2.0);
            }
            other => panic!("window camera was not retargeted offscreen: {other:?}"),
        }

        let portrait_target = world.get::<RenderTarget>(portrait_cam).unwrap();
        assert_eq!(
            portrait_target.as_image(),
            Some(&portrait_image),
            "an offscreen RTT camera must keep its own target"
        );
    }

    /// The plugin must be inert unless it is asked for: an unset (or empty)
    /// `SCREENSHOT` may never make a normal run exit early.
    #[test]
    fn screenshot_request_needs_the_env_var() {
        assert_eq!(ScreenshotRequest::from_env_values(None, None), None);
        assert_eq!(ScreenshotRequest::from_env_values(Some(""), None), None);
        assert_eq!(ScreenshotRequest::from_env_values(Some("  "), None), None);
    }

    /// A bare filename must land in the gitignored dir — a screenshot is the
    /// user's own PK2 art and must not be droppable into the repo root by
    /// accident. An explicit path stays untouched.
    #[test]
    fn screenshot_bare_filename_goes_to_the_gitignored_dir() {
        let bare = ScreenshotRequest::from_env_values(Some("shot.png"), None).unwrap();
        assert_eq!(bare.path, PathBuf::from("screenshots/shot.png"));

        let flag = ScreenshotRequest::from_env_values(Some("1"), None).unwrap();
        assert_eq!(flag.path, PathBuf::from("screenshots/screenshot.png"));

        let explicit = ScreenshotRequest::from_env_values(Some("/tmp/out.png"), None).unwrap();
        assert_eq!(explicit.path, PathBuf::from("/tmp/out.png"));

        // No extension is still a PNG, since that is the only format written.
        let no_ext = ScreenshotRequest::from_env_values(Some("/tmp/out"), None).unwrap();
        assert_eq!(no_ext.path, PathBuf::from("/tmp/out.png"));
    }

    /// `SCREENSHOT_AFTER` is the knob that decides whether the frame shows the
    /// loading screen or the scene, so a typo in it must fall back to the
    /// documented default rather than capturing at t=0.
    #[test]
    fn screenshot_after_falls_back_on_garbage() {
        let default = ScreenshotRequest::from_env_values(Some("a.png"), None).unwrap();
        assert_eq!(default.delay, Duration::from_secs_f32(DEFAULT_DELAY_SECS));

        let parsed = ScreenshotRequest::from_env_values(Some("a.png"), Some("2.5")).unwrap();
        assert_eq!(parsed.delay, Duration::from_secs_f32(2.5));

        let zero = ScreenshotRequest::from_env_values(Some("a.png"), Some("0")).unwrap();
        assert_eq!(zero.delay, Duration::ZERO);

        for bad in ["", "soon", "-1"] {
            let request = ScreenshotRequest::from_env_values(Some("a.png"), Some(bad)).unwrap();
            assert_eq!(
                request.delay,
                Duration::from_secs_f32(DEFAULT_DELAY_SECS),
                "SCREENSHOT_AFTER={bad:?} must fall back to the default"
            );
        }
    }
}
