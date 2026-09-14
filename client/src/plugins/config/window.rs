use crate::plugins::config::ClientConfig;
use crate::plugins::settings::options::GameOptions;
use bevy::prelude::*;
use bevy::window::{Monitor, PresentMode, PrimaryWindow, VideoModeSelection, WindowMode};
use serde_derive::Deserialize;
use std::env;

/// The YAML spelling of the window mode, as its own enum rather than bevy's
/// [`WindowMode`].
///
/// Idea (#539): bevy's variants carry payloads —
/// `BorderlessFullscreen(MonitorSelection)`, `Fullscreen(MonitorSelection,
/// VideoModeSelection)` — and config-rs 0.14 cannot build a newtype variant
/// from a bare YAML string: it hits an `unreachable!()`
/// (`config-0.14.1/src/de.rs:337`) and the client dies before its window
/// exists. Only `Windowed` survived, which is why the tracked `config.yaml`
/// worked and `config.example.yaml` (`mode: BorderlessFullscreen`) panicked
/// every fresh setup. Unit variants here keep the documented one-word values
/// working and decouple the config file from bevy's enum shape.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowModeConfig {
    #[default]
    Windowed,
    /// Bevy 0.19 dropped `SizedFullscreen`; borderless is its closest
    /// equivalent, and the alias keeps an existing user `config.yaml` booting.
    #[serde(alias = "SizedFullscreen")]
    BorderlessFullscreen,
    Fullscreen,
}

impl WindowModeConfig {
    /// The bevy mode for this setting on `monitor` (the same monitor the
    /// window is centered on). Exclusive fullscreen keeps the monitor's
    /// current video mode — picking a specific one needs the monitor's mode
    /// list, which does not exist at config-load time.
    pub fn to_window_mode(self, monitor: MonitorSelection) -> WindowMode {
        match self {
            WindowModeConfig::Windowed => WindowMode::Windowed,
            WindowModeConfig::BorderlessFullscreen => WindowMode::BorderlessFullscreen(monitor),
            WindowModeConfig::Fullscreen => {
                WindowMode::Fullscreen(monitor, VideoModeSelection::Current)
            }
        }
    }
}

/// How finished frames are handed to the display.
///
/// Idea: this is the VSync switch, and it is also one half of the pipeline's
/// shape. `immediate` presents as soon as a frame is ready (tearing, lowest
/// latency, and the only mode that reports a true throughput number, which is
/// why every reading in `docs/perf-baselines.md` was taken on it); `fifo` waits
/// for the display's refresh, which is what most players mean by VSync.
///
/// **The configured value is a request, not a fact.** Bevy falls back silently
/// when the surface does not support a mode — `Immediate` degrades to `Fifo`
/// with no log line at all (`bevy_render::view::window`, the `present_mode`
/// fallback table). So a capture cannot assume it got what it asked for, and
/// the only way to know which mode is live is to compare behaviour: at 75 Hz,
/// `fifo` caps the frame at 13.33 ms and `immediate` does not.
///
/// Live-reconfigurable: bevy rebuilds the surface when this changes, so it can
/// be swept without a restart (unlike [`WindowSettings::max_frame_latency`]).
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PresentModeConfig {
    /// Present immediately; may tear. The default, and what this shipped with.
    #[default]
    Immediate,
    /// Wait for vblank. Ordinary VSync.
    Fifo,
    /// Like `fifo`, but presents late frames immediately rather than holding
    /// them to the next refresh — less stutter, occasional tearing.
    FifoRelaxed,
    /// Replace the queued frame instead of blocking. VSync without the
    /// frame-rate quantisation, where supported.
    Mailbox,
    /// `fifo_relaxed` if available, else `fifo`.
    AutoVsync,
    /// `immediate`, else `mailbox`, else `fifo`.
    AutoNoVsync,
}

impl PresentModeConfig {
    pub fn to_present_mode(self) -> PresentMode {
        match self {
            PresentModeConfig::Immediate => PresentMode::Immediate,
            PresentModeConfig::Fifo => PresentMode::Fifo,
            PresentModeConfig::FifoRelaxed => PresentMode::FifoRelaxed,
            PresentModeConfig::Mailbox => PresentMode::Mailbox,
            PresentModeConfig::AutoVsync => PresentMode::AutoVsync,
            PresentModeConfig::AutoNoVsync => PresentMode::AutoNoVsync,
        }
    }
}

#[derive(Deserialize)]
pub struct WindowSettings {
    pub width: f32,
    pub height: f32,
    pub mode: WindowModeConfig,
    pub title: String,
    pub monitor: MonitorSelection,
    /// See [`PresentModeConfig`]. Defaulted so an existing `config.yaml` that
    /// predates this field keeps the behaviour it had.
    #[serde(default)]
    pub present_mode: PresentModeConfig,
    /// Swapchain depth: how many frames the GPU may be working on before the
    /// renderer has to wait for one to finish presenting.
    ///
    /// Idea: this is the other half of the pipeline's shape, and the reason it
    /// is exposed at all. Measured 2026-09-05 at the wgpu default of 2, nothing
    /// in the frame was saturated — the main thread waited 12.53 ms, the render
    /// thread waited 10.80 ms in the swapchain acquire, and the GPU was busy
    /// only ~13.9 ms of a 21.85 ms frame. A chain that shallow cannot start the
    /// next frame's GPU work until the last one has been presented, so acquire
    /// and GPU execution serialise instead of overlapping.
    ///
    /// `None` (the default) leaves wgpu's own default of 2, i.e. exactly the
    /// behaviour every reading in `docs/perf-baselines.md` was taken on.
    ///
    /// **Restart-only.** Bevy reads this when it creates the surface and never
    /// again, so changing it in a running client does nothing at all — that is
    /// a property of the engine, not a bug here.
    #[serde(default)]
    pub max_frame_latency: Option<u32>,
}

impl WindowSettings {
    /// The validated swapchain depth. `Some(0)` is not a shallower swapchain,
    /// it is an invalid surface configuration, so it degrades to the default
    /// with a warning rather than reaching wgpu.
    pub fn frame_latency(&self) -> Option<std::num::NonZeroU32> {
        match self.max_frame_latency {
            None => None,
            Some(0) => {
                bevy::log::warn_once!(
                    "window_settings.max_frame_latency: 0 is not a valid swapchain depth; \
                     using the driver default"
                );
                None
            }
            Some(n) => std::num::NonZeroU32::new(n),
        }
    }
}

/// What the window should look like right now, as a value rather than a
/// sequence of writes.
///
/// Idea: the window used to be written by four systems in two plugins, each
/// computing its own answer, so the last one to run decided — and the one that
/// ran last read the *player options*, which is how `config.yaml`'s `mode:`
/// came to do nothing. Resolving to a single comparable value instead makes
/// "who wins" a property of [`window_intent`] alone, and lets the apply skip
/// writing at all when nothing it owns actually changed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WindowIntent {
    pub mode: WindowMode,
    pub title: String,
    /// `None` in either fullscreen: the monitor owns the resolution there and
    /// writing one fights the compositor.
    pub size: Option<(f32, f32)>,
}

/// The window mode `config.yaml` asks for, unless this session has been told
/// otherwise.
///
/// `config.yaml` is openroad's single authority for the window and the only
/// one that survives a restart; `session` is the in-game Video toggle, which
/// is deliberately not persisted (see
/// [`GameOptions::video`](crate::plugins::settings::options::VideoOptions::window_mode_override)).
/// The monitor comes from the config in every branch — a fullscreen mode
/// pinned to `MonitorSelection::Current` would quietly discard a configured
/// `monitor: Primary`.
pub(crate) fn resolved_window_mode(settings: &WindowSettings, session: Option<bool>) -> WindowMode {
    match session {
        None => settings.mode.to_window_mode(settings.monitor),
        Some(true) => WindowMode::Windowed,
        // "not windowed" keeps the config's own flavour of fullscreen, so a
        // user who asked for exclusive fullscreen does not silently get
        // borderless back when they toggle away and back.
        Some(false) => match settings.mode {
            WindowModeConfig::Fullscreen => {
                WindowMode::Fullscreen(settings.monitor, VideoModeSelection::Current)
            }
            _ => WindowMode::BorderlessFullscreen(settings.monitor),
        },
    }
}

/// The full intent, kept a pure function so it can be tested without an `App`
/// and without touching the process environment.
pub(crate) fn window_intent(
    settings: &WindowSettings,
    session: Option<bool>,
    env_resolution: Option<&str>,
) -> WindowIntent {
    let mode = resolved_window_mode(settings, session);
    // `RESOLUTION=` is a developer escape hatch, not a setting: it outranks
    // the configured size and is never written back.
    let size = if mode == WindowMode::Windowed {
        Some(
            env_resolution
                .and_then(parse_resolution)
                .unwrap_or((settings.width, settings.height)),
        )
    } else {
        None
    };
    WindowIntent {
        mode,
        title: settings.title.clone(),
        size,
    }
}

/// `WxH`, or `None` for anything else. A malformed value is ignored with a
/// warning rather than panicking the boot: it is a hand-typed env var.
fn parse_resolution(value: &str) -> Option<(f32, f32)> {
    let (w, h) = value.split_once('x')?;
    match (w.trim().parse::<f32>(), h.trim().parse::<f32>()) {
        (Ok(w), Ok(h)) if w > 0.0 && h > 0.0 => Some((w, h)),
        _ => {
            warn!("ignoring malformed RESOLUTION={value:?}, expected e.g. 1600x900");
            None
        }
    }
}

/// Boot-only window decisions, as opposed to the settings
/// [`apply_window_settings`] keeps following.
///
/// Centring is a boot decision: re-centring on every edit would yank a window
/// the user has since moved. It is also a *windowed* decision, for the same
/// reason [`WindowIntent::size`] is `None` in fullscreen — the monitor owns the
/// geometry there and writing one fights the compositor. Centring a fullscreen
/// window asks winit to place a window of the configured size on the monitor,
/// so a 1920x1080 config on a 1920x1200 panel lands offset down and right with
/// part of the view off-screen, instead of covering it.
///
/// The scale-factor pin is a boot decision about the renderer rather than a
/// setting — the whole HUD is transcribed at the original's raw `resinfo` pixel
/// rects and scaled by its own `hud.hud_scale` knob, so letting the OS factor
/// multiply every rect on top would resize the entire UI on a HiDPI display.
/// Only the *override* is set: winit re-reports the real OS factor every frame,
/// so a plain `set_scale_factor` would not stick.
/// Log what the display is actually doing, once, at startup.
///
/// Idea: every frame-time number is read against the refresh rate, and this
/// machine has already produced two wrong answers for its own display —
/// `Win32_VideoController` reported a stale 1920x1200 resolution, and
/// `Screen.Bounds` reported the DPI-scaled 2752x1152 as if it were physical
/// (see `docs/perf-baselines.md`). Bevy has the value winit actually got from
/// the compositor, so print that rather than asking the OS a third time.
///
/// `refresh_rate_millihertz` is `None` on some platforms/drivers; say so
/// explicitly instead of substituting a plausible number.
///
/// Runs in `Update` behind a latch rather than at startup: bevy_winit spawns
/// the monitor entities from its own event loop, which is after `PostStartup`,
/// so a startup system sees an empty query and silently logs nothing.
pub(crate) fn log_monitors(monitors: Query<&Monitor>, mut logged: Local<bool>) {
    if *logged || monitors.is_empty() {
        return;
    }
    *logged = true;
    for monitor in &monitors {
        let refresh = match monitor.refresh_rate_millihertz {
            Some(mhz) => format!(
                "{:.3} Hz ({:.3} ms/refresh)",
                mhz as f64 / 1000.0,
                1e6 / mhz as f64
            ),
            None => String::from("refresh rate not reported"),
        };
        info!(
            "monitor {:?}: {}x{} physical, scale {:.2}, {refresh}, {} video modes",
            monitor.name.as_deref().unwrap_or("<unnamed>"),
            monitor.physical_width,
            monitor.physical_height,
            monitor.scale_factor,
            monitor.video_modes.len(),
        );
    }
}

pub(crate) fn setup_window(
    config: Res<ClientConfig>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if let Ok(mut window) = windows.single_mut() {
        // `None`: this is Startup, before the in-game Video toggle can have
        // been touched, so the boot mode is whatever config.yaml asked for.
        if resolved_window_mode(&config.window_settings, None) == WindowMode::Windowed {
            window.position.center(config.window_settings.monitor);
        }
        window.resolution.set_scale_factor_override(Some(1.0));
    }
}

/// The **only** system that writes the live window's mode, title and size
/// ([`crate::plugins::settings::live`]). Winit applies all three without a
/// restart, so this group is `Live` in the audit rather than an apology in the
/// options UI.
///
/// It reads both settings resources because both can move the intent, and it
/// re-applies only when the intent actually differs from what it last wrote.
/// That guard is load-bearing, not an optimisation: the run condition fires
/// for *any* change to either resource — an audio slider, a camera radio, a
/// dragged HUD window, a foliage row that takes `ResMut<ClientConfig>` — and
/// without it each of those would re-assert the mode and stomp a window the
/// user had resized or alt-entered in the meantime.
pub(crate) fn apply_window_settings(
    config: Res<ClientConfig>,
    options: Res<GameOptions>,
    mut applied: Local<Option<WindowIntent>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    let intent = window_intent(
        &config.window_settings,
        options.video.window_mode_override,
        env::var("RESOLUTION").ok().as_deref(),
    );
    if applied.as_ref() == Some(&intent) {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        // No window yet — leave `applied` unset so the next change retries.
        return;
    };

    if window.mode != intent.mode {
        window.mode = intent.mode;
    }
    if window.title != intent.title {
        window.title.clone_from(&intent.title);
    }
    if let Some((width, height)) = intent.size {
        if window.resolution.width() != width || window.resolution.height() != height {
            window.resolution.set(width, height);
        }
    }
    *applied = Some(intent);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::config::tests::example_config;

    /// An app with just enough world to run the apply: the two settings
    /// resources and a primary window. No plugins — the system under test
    /// needs none, and the established pattern in this tree is to add only
    /// what a test actually uses (`plugins::options_input_tab`, `plugins::nav`).
    ///
    /// The window is deliberately spawned in the *wrong* mode, so a passing
    /// assertion can only come from the apply having run.
    fn app_with_window(config: ClientConfig, options: GameOptions) -> App {
        let mut app = App::new();
        app.insert_resource(config)
            .insert_resource(options)
            .add_systems(Update, apply_window_settings);
        app.world_mut().spawn((
            Window {
                mode: WindowMode::BorderlessFullscreen(MonitorSelection::Current),
                ..default()
            },
            PrimaryWindow,
        ));
        app
    }

    fn window_of(app: &mut App) -> Window {
        let mut query = app
            .world_mut()
            .query_filtered::<&Window, With<PrimaryWindow>>();
        query.iter(app.world()).next().expect("a window").clone()
    }

    /// Runs `setup_window` alone, against a window whose position starts
    /// unset, so the assertion can only come from the boot system.
    fn boot_window(config: ClientConfig) -> Window {
        let mut app = App::new();
        app.insert_resource(config)
            .add_systems(Startup, setup_window);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.update();
        let mut query = app
            .world_mut()
            .query_filtered::<&Window, With<PrimaryWindow>>();
        query.iter(app.world()).next().expect("a window").clone()
    }

    /// Centring is windowed-only. Asking winit to centre a *fullscreen* window
    /// positions one of the configured size on the monitor instead of covering
    /// it, so a 1920x1080 config on a 1920x1200 panel lands offset down and
    /// right with part of the view off-screen — which is what this reproduced
    /// before the mode check.
    #[test]
    fn fullscreen_boot_leaves_the_position_to_the_compositor() {
        let mut config = example_config();
        config.window_settings.mode = WindowModeConfig::BorderlessFullscreen;
        let window = boot_window(config);
        assert!(
            !matches!(window.position, WindowPosition::Centered(_)),
            "fullscreen must not request a centred position, got {:?}",
            window.position
        );
    }

    /// The other half: windowed boot still centres, which is the behaviour the
    /// mode check is narrowing rather than removing.
    #[test]
    fn windowed_boot_still_centres() {
        let mut config = example_config();
        config.window_settings.mode = WindowModeConfig::Windowed;
        let window = boot_window(config);
        assert!(
            matches!(window.position, WindowPosition::Centered(_)),
            "windowed boot should centre, got {:?}",
            window.position
        );
    }

    /// The regression this whole change exists for: `mode: Windowed` in
    /// `config.yaml` used to be overwritten on frame 1 by the options layer,
    /// whose `window_mode` defaulted to "not windowed" — so a *fresh install
    /// with no `user_settings.yaml` at all* still booted borderless
    /// fullscreen, and the config key looked dead.
    #[test]
    fn config_windowed_survives_boot_without_a_session_override() {
        let mut config = example_config();
        config.window_settings.mode = WindowModeConfig::Windowed;
        config.window_settings.monitor = MonitorSelection::Primary;
        config.window_settings.width = 1920.0;
        config.window_settings.height = 1080.0;

        let mut app = app_with_window(config, GameOptions::default());
        app.update();

        let window = window_of(&mut app);
        assert_eq!(window.mode, WindowMode::Windowed);
        assert_eq!(window.resolution.width(), 1920.0);
        assert_eq!(window.resolution.height(), 1080.0);
    }

    /// The session override is still allowed to win while the client runs —
    /// "config always wins" is about what survives a restart, and what makes
    /// that true is `#[serde(skip)]` on the field, not this system refusing
    /// to honour it.
    #[test]
    fn a_session_override_beats_the_config() {
        let mut config = example_config();
        config.window_settings.mode = WindowModeConfig::BorderlessFullscreen;

        let mut options = GameOptions::default();
        options.video.window_mode_override = Some(true);

        let mut app = app_with_window(config, options);
        app.update();

        assert_eq!(window_of(&mut app).mode, WindowMode::Windowed);
    }

    /// `apply_window_mode` hardcoded `MonitorSelection::Current`, so a
    /// configured `monitor:` was discarded every time the mode was applied.
    #[test]
    fn the_fullscreen_override_uses_the_configured_monitor() {
        let settings = WindowSettings {
            width: 1024.0,
            height: 768.0,
            mode: WindowModeConfig::BorderlessFullscreen,
            title: "t".into(),
            monitor: MonitorSelection::Primary,
            present_mode: PresentModeConfig::default(),
            max_frame_latency: None,
        };

        assert_eq!(
            resolved_window_mode(&settings, Some(false)),
            WindowMode::BorderlessFullscreen(MonitorSelection::Primary)
        );
        // Toggling away from windowed keeps the config's own flavour of
        // fullscreen rather than flattening exclusive to borderless.
        let exclusive = WindowSettings {
            mode: WindowModeConfig::Fullscreen,
            ..settings
        };
        assert_eq!(
            resolved_window_mode(&exclusive, Some(false)),
            WindowMode::Fullscreen(MonitorSelection::Primary, VideoModeSelection::Current)
        );
    }

    /// The run condition fires on *any* change to either settings resource,
    /// so without the intent guard an audio slider or a dragged HUD window
    /// would re-assert the mode and undo a window the user had alt-entered or
    /// resized by hand.
    #[test]
    fn an_unrelated_option_change_does_not_re_force_the_window() {
        let mut config = example_config();
        config.window_settings.mode = WindowModeConfig::Windowed;
        config.window_settings.width = 1280.0;
        config.window_settings.height = 720.0;

        let mut app = app_with_window(config, GameOptions::default());
        app.update();

        // Stand-in for the user resizing the window / hitting alt-enter.
        {
            let mut query = app
                .world_mut()
                .query_filtered::<&mut Window, With<PrimaryWindow>>();
            let mut window = query.single_mut(app.world_mut()).expect("a window");
            window.mode = WindowMode::BorderlessFullscreen(MonitorSelection::Current);
            window.resolution.set(800.0, 600.0);
        }

        app.world_mut()
            .resource_mut::<GameOptions>()
            .audio
            .bgm_volume = 42;
        app.update();

        let window = window_of(&mut app);
        assert_eq!(
            window.mode,
            WindowMode::BorderlessFullscreen(MonitorSelection::Current),
            "an audio change must not touch the window mode"
        );
        assert_eq!(window.resolution.width(), 800.0);
    }

    /// The other half of the guard: narrowing the trigger must not cost the
    /// `Liveness::Live` verdict the audit gives this group.
    #[test]
    fn a_window_settings_edit_still_moves_the_window() {
        let mut config = example_config();
        config.window_settings.mode = WindowModeConfig::Windowed;

        let mut app = app_with_window(config, GameOptions::default());
        app.update();

        app.world_mut()
            .resource_mut::<ClientConfig>()
            .window_settings
            .mode = WindowModeConfig::BorderlessFullscreen;
        app.update();

        assert_eq!(
            window_of(&mut app).mode,
            WindowMode::BorderlessFullscreen(MonitorSelection::Primary),
            "config.example.yaml's monitor is Primary"
        );
    }

    /// `RESOLUTION=` outranks the configured size, and a malformed value is
    /// ignored rather than panicking the boot — it used to `unwrap()`.
    #[test]
    fn the_env_resolution_outranks_the_config_and_tolerates_junk() {
        let settings = WindowSettings {
            width: 1920.0,
            height: 1080.0,
            mode: WindowModeConfig::Windowed,
            title: "t".into(),
            monitor: MonitorSelection::Primary,
            present_mode: PresentModeConfig::default(),
            max_frame_latency: None,
        };

        assert_eq!(
            window_intent(&settings, None, Some("1600x900")).size,
            Some((1600.0, 900.0))
        );
        assert_eq!(
            window_intent(&settings, None, Some("nonsense")).size,
            Some((1920.0, 1080.0))
        );
        // In either fullscreen the monitor owns the resolution.
        assert_eq!(
            window_intent(&settings, Some(false), Some("1600x900")).size,
            None
        );
    }

    /// Every documented spelling has to reach wgpu, because the fallback that
    /// makes this hard to verify at runtime is silent: a typo'd mode would not
    /// error, it would deserialize-fail or quietly behave as something else.
    #[test]
    fn every_present_mode_spelling_maps_to_its_wgpu_mode() {
        for (yaml, expected) in [
            ("immediate", PresentMode::Immediate),
            ("fifo", PresentMode::Fifo),
            ("fifo_relaxed", PresentMode::FifoRelaxed),
            ("mailbox", PresentMode::Mailbox),
            ("auto_vsync", PresentMode::AutoVsync),
            ("auto_no_vsync", PresentMode::AutoNoVsync),
        ] {
            let parsed: PresentModeConfig =
                serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("{yaml:?}: {e}"));
            assert_eq!(parsed.to_present_mode(), expected, "{yaml}");
        }
    }

    /// The default must be the behaviour this shipped with, or every frame-time
    /// reading in docs/perf-baselines.md silently stops being comparable.
    #[test]
    fn the_defaults_are_the_behaviour_this_shipped_with() {
        assert_eq!(
            PresentModeConfig::default().to_present_mode(),
            PresentMode::Immediate
        );
        let settings = settings_with_latency(None);
        assert_eq!(
            settings.frame_latency(),
            None,
            "unset means the wgpu default"
        );
    }

    /// 0 is not a shallower swapchain, it is an invalid surface configuration.
    /// It has to degrade to the driver default rather than reach wgpu.
    #[test]
    fn a_zero_frame_latency_degrades_instead_of_reaching_wgpu() {
        assert_eq!(settings_with_latency(Some(0)).frame_latency(), None);
        assert_eq!(
            settings_with_latency(Some(3)).frame_latency(),
            std::num::NonZeroU32::new(3)
        );
    }

    fn settings_with_latency(max_frame_latency: Option<u32>) -> WindowSettings {
        WindowSettings {
            width: 1920.0,
            height: 1080.0,
            mode: WindowModeConfig::Windowed,
            title: String::from("test"),
            monitor: MonitorSelection::Primary,
            present_mode: PresentModeConfig::default(),
            max_frame_latency,
        }
    }
}
