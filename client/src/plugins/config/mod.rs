//! `config.yaml` and its typed sections.
//!
//! Convention (#581): **config enums stay flat unit variants** with an
//! explicit mapping function to whatever payload-carrying enum the engine
//! wants — [`window::WindowModeConfig`] is the pattern. config-rs 0.14 can
//! only build a payload-carrying variant from a YAML table and *panics* on a
//! bare string, so a payload variant exposed here turns a config typo into an
//! `unreachable!()`. [`deserialize_guarded`] catches that panic for the cases
//! the convention cannot reach (borrowed engine enums), but the convention is
//! what keeps it from happening at all.

use bevy::app::App;
use bevy::prelude::*;
use config::Config;
use serde::Deserialize;
use std::panic::AssertUnwindSafe;

use crate::plugins::config::chat::ChatSettings;
use crate::plugins::config::dev_fast_login::DevFastLoginSettings;
use crate::plugins::config::fonts::FontSettings;
use crate::plugins::config::graphics::GraphicsSettings;
use crate::plugins::config::guild::GuildSettings;
use crate::plugins::config::hud::HudSettings;
use crate::plugins::config::input::InputSettings;
use crate::plugins::config::nameplates::NameplateSettings;
use crate::plugins::config::network::NetworkSettings;
use crate::plugins::config::scene::SceneSettings;
use crate::plugins::config::selection::SelectionSettings;
use crate::plugins::config::window::WindowSettings;

pub mod chat;
pub mod combat;
pub mod dev_fast_login;
pub mod division;
pub mod effects;
pub mod fonts;
pub mod graphics;
pub mod guild;
pub mod hud;
pub mod input;
pub mod nameplates;
pub mod network;
mod scene;
pub mod selection;
pub mod window;

pub struct ConfigPlugin;

impl Plugin for ConfigPlugin {
    fn build(&self, app: &mut App) {
        // Nothing is derived here on purpose: a value computed in `build` is
        // frozen for the process lifetime, which is the bug shape #647 is
        // about. See `plugins::settings::live` for the mechanism.
        // `setup_window` is on `Startup`, not on a state's `OnEnter`: it no
        // longer depends on any game state, `WindowPlugin::build` has already
        // spawned the primary window by then, and when the *initial*
        // `OnEnter` runs relative to `Startup` is a bevy_state implementation
        // detail that has moved between versions.
        app.add_systems(Startup, (log_network_config, window::setup_window))
            // Update, not Startup: bevy_winit spawns the monitor entities
            // from its own event loop, so a startup system sees nothing.
            .add_systems(Update, window::log_monitors)
            .add_systems(
                PreUpdate,
                // Both resources feed the window intent, so a change to
                // either has to be able to move it; `apply_window_settings`
                // itself no-ops when the resolved intent is unchanged.
                window::apply_window_settings.run_if(
                    crate::plugins::settings::live::config_changed
                        .or_else(crate::plugins::settings::live::options_changed),
                ),
            );
    }
}

fn log_network_config(config: Res<ClientConfig>) {
    info!("initialized network config: {:?}", config.network_settings);
}

#[derive(Resource, Deserialize)]
pub struct ClientConfig {
    pub network_settings: NetworkSettings,
    pub window_settings: WindowSettings,
    pub scenes: SceneSettings,
    /// Adds the egui world/resource inspectors and the debug-draw overlays.
    /// Off by default: reflecting the whole ECS world into egui every frame
    /// costs double-digit FPS once terrain is streamed in. Implies
    /// [`Self::diagnostics`].
    #[serde(default)]
    pub dev_tools: bool,
    /// The measurement tier on its own: the Bevy Remote Protocol server
    /// (`make perf`) plus `RenderDiagnosticsPlugin`'s per-pass timings.
    ///
    /// Split out from `dev_tools` because that switch also drags in the world
    /// inspector and the nav debug draws, which between them cost double-digit
    /// FPS — so measuring through it measured a build nobody plays, and the
    /// shipping configuration was unobservable. This tier is cheap enough to
    /// leave on while taking a baseline; see `docs/perf-baselines.md`.
    ///
    /// Restart-only, for the same reason as `dev_tools`: it decides plugin
    /// registration, and Bevy cannot add a plugin after startup.
    #[serde(default)]
    pub diagnostics: bool,
    /// Optional developer fast-login + first-character join on the intro_v2
    /// scene. Deliberately **not** called `autologin`: v1.188 ships a feature by
    /// that name and it is a login queue, not this
    /// (`docs/re/ui/scene-intro-autologin.md`). The `autologin` alias keeps
    /// existing user `config.yaml` files working.
    #[serde(default, alias = "autologin")]
    pub dev_fast_login: DevFastLoginSettings,
    /// Chat window customization (per-channel text colors).
    #[serde(default)]
    pub chat: ChatSettings,
    /// Floating name-label customization (per-kind text colors).
    #[serde(default)]
    pub nameplates: NameplateSettings,
    /// Hover/selection highlight customization (rim colors, strengths).
    #[serde(default)]
    pub selection: SelectionSettings,
    /// Render quality knobs (bloom).
    #[serde(default)]
    pub graphics: GraphicsSettings,
    /// HUD behaviour knobs (skill-window layout source).
    #[serde(default)]
    pub hud: HudSettings,
    /// Input behaviour knobs (mouse scheme).
    #[serde(default)]
    pub input: InputSettings,
    /// Where the UI face comes from (PK2 vs the bundled OFL fallback).
    #[serde(default)]
    pub fonts: FontSettings,
    /// Which particle effect an item plays when it is used.
    #[serde(default)]
    pub effects: effects::EffectSettings,
    /// Guild-window knobs (which of the two co-anchored command buttons).
    #[serde(default)]
    pub guild: GuildSettings,
    /// Combat presentation (knockdown dwell).
    #[serde(default)]
    pub combat: combat::CombatSettings,
}

impl ClientConfig {
    /// Whether to register the measurement tier (BRP, render-pass timings,
    /// per-phase draw counts).
    ///
    /// `dev_tools` implies it: that switch used to *be* the only way to reach
    /// BRP, so a config asking for the full toolbox must keep getting it.
    pub fn diagnostics_enabled(&self) -> bool {
        self.diagnostics || self.dev_tools
    }

    /// Called from `main()` before the `App` is built: plugin *registration*
    /// (dev tools, networking) is decided by config values, so the resource
    /// must exist in the world before any `Plugin::build` runs.
    pub fn load() -> Self {
        // A readable message, not a bare `unwrap`: this runs before the window
        // exists, so this string is the only thing a user with a broken
        // `config.yaml` ever sees (#539).
        Self::from_file("config").unwrap_or_else(|err| {
            panic!(
                "failed to load config.yaml: {err}\nsee config.example.yaml for the expected shape"
            )
        })
    }

    /// Load and deserialize one config file (name without extension, as
    /// `config::File::with_name` wants it). Split out of [`load`](Self::load)
    /// so a test can point the *same* loader at `config.example.yaml` — the
    /// file a fresh setup copies — and prove it still matches this struct
    /// (#539). `pub(crate)` so tests outside this module can build a config
    /// from the shipped example instead of from a struct literal that would
    /// drift away from what users actually run.
    pub(crate) fn from_file(name: &str) -> Result<Self, config::ConfigError> {
        let config = Config::builder()
            .add_source(config::File::with_name(name))
            .build()?;
        deserialize_guarded(config)
    }
}

/// Deserialize a loaded [`Config`], turning config-rs's enum-shape
/// `unreachable!()` into an error that names the offending key (#581).
///
/// Idea: config 0.14 can only build a *payload-carrying* enum variant from a
/// YAML table. A bare string reaches `VariantAccess::newtype_variant_seed`
/// with a `String` and hits `unreachable!()` (`config-0.14.1/src/de.rs:337`),
/// so the process dies with "internal error: entered unreachable code" naming
/// nothing — it reads like a corrupt install rather than a one-line config
/// typo (#539, #566).
///
/// Our own config enums dodge it by convention — flat unit variants plus an
/// explicit mapping function, see [`window::WindowModeConfig`] — but the
/// convention cannot cover the foreign enums the config borrows, and one is
/// live today: `window_settings.monitor` is bevy's `MonitorSelection`, whose
/// `Index(usize)` variant carries a payload. So the load is fenced instead:
/// catch the panic, then replay the same data through serde_yaml, which
/// reports the very same mismatch as an ordinary error *and* names the key
/// config-rs never named.
///
/// The replay only runs on the failure path, so a good config still loads
/// through config-rs alone.
fn deserialize_guarded<T: serde::de::DeserializeOwned>(
    config: Config,
) -> Result<T, config::ConfigError> {
    let replay = config.clone();
    match std::panic::catch_unwind(AssertUnwindSafe(move || config.try_deserialize::<T>())) {
        Ok(result) => result,
        Err(_) => Err(shape_mismatch_error::<T>(replay)),
    }
}

/// The readable error for a load that panicked inside config-rs: the key path
/// and serde's own description of the mismatch.
///
/// The key path comes from serde_yaml's *string* parser, which tracks it —
/// deserializing a `serde_yaml::Value` directly does not, and reports the bare
/// "invalid type: unit variant, expected newtype variant" naming nothing. So
/// the value is re-serialized and re-parsed. Its `at line L column C` suffix
/// therefore points into that normalized text, not into the user's file; the
/// key path is the part that transfers.
fn shape_mismatch_error<T: serde::de::DeserializeOwned>(config: Config) -> config::ConfigError {
    // `deserialize_any` into a value model never asks for an enum, so this
    // step cannot hit the same panic.
    let value = match config.try_deserialize::<serde_yaml::Value>() {
        Ok(value) => value,
        Err(err) => return err,
    };
    let text = match serde_yaml::to_string(&value) {
        Ok(text) => text,
        Err(err) => return config::ConfigError::Message(err.to_string()),
    };
    match serde_yaml::from_str::<T>(&text) {
        Err(err) => config::ConfigError::Message(format!(
            "{err} — config-rs cannot build an enum variant that carries a payload \
             from a bare string; give the key a `variant: value` table instead (#581)"
        )),
        // Only reachable if the two deserializers disagree; still better than
        // re-raising a panic that names nothing.
        Ok(_) => config::ConfigError::Message(
            "config-rs panicked while deserializing this config, but the same data is \
             accepted by serde_yaml — please report this with your config.yaml (#581)"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renaming `autologin:` to `dev_fast_login:` must not silently disable a
    /// user's existing `config.yaml` — a missing block deserializes to
    /// `enabled: false`, which would look like the feature "just stopped".
    /// The `serde(alias)` is what prevents that, so it is tested.
    #[test]
    fn dev_fast_login_still_accepts_the_old_autologin_key() {
        #[derive(Deserialize, Debug)]
        struct Wrapper {
            #[serde(default, alias = "autologin")]
            dev_fast_login: DevFastLoginSettings,
        }

        let old: Wrapper =
            serde_yaml::from_str("autologin:\n  enabled: true\n  username: u\n  password: p\n")
                .expect("the legacy key parses");
        assert!(old.dev_fast_login.enabled);
        assert_eq!(old.dev_fast_login.username, "u");
        // No captcha answer means "let the modal handle it", never a guess.
        assert_eq!(old.dev_fast_login.captcha_answer, None);

        let new: Wrapper =
            serde_yaml::from_str("dev_fast_login:\n  enabled: true\n  captcha_answer: \"1\"\n")
                .expect("the new key parses");
        assert!(new.dev_fast_login.enabled);
        assert_eq!(new.dev_fast_login.captcha_answer.as_deref(), Some("1"));
    }

    /// A real [`ClientConfig`] built from the shipped `config.example.yaml`,
    /// for tests elsewhere in this module tree that need one to mutate. Built
    /// from the file rather than from a struct literal on purpose: a literal
    /// drifts silently from what users actually run, which is the whole point
    /// of `example_config_deserializes_into_client_config`.
    pub(crate) fn example_config() -> ClientConfig {
        ClientConfig::from_file(&example_config_name()).expect("config.example.yaml loads")
    }

    /// The path `config::File::with_name` wants for the example file: the
    /// repo-root `config.example.yaml` without its extension.
    fn example_config_name() -> String {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../config.example.yaml")
            .with_extension("")
            .to_str()
            .expect("the example path is utf-8")
            .to_string()
    }

    /// `dev_tools` is the superset switch, so turning it on must not silently
    /// cost the BRP server it used to imply — the split that separated the
    /// measurement tier from the inspectors is only safe if this holds.
    #[test]
    fn dev_tools_implies_the_diagnostics_tier() {
        let mut config = ClientConfig::from_file(&example_config_name())
            .expect("config.example.yaml matches ClientConfig");
        assert!(
            !config.diagnostics_enabled(),
            "both off in the shipped file"
        );

        config.diagnostics = true;
        assert!(config.diagnostics_enabled());

        config.diagnostics = false;
        config.dev_tools = true;
        assert!(config.diagnostics_enabled());
    }

    /// The whole `config.example.yaml` must deserialize into [`ClientConfig`]
    /// through the *same* loader `main()` uses (#539).
    ///
    /// Until this test existed only individual blocks were covered, and the
    /// file drifted where nothing looked: `mode: BorderlessFullscreen` became
    /// unrepresentable when bevy's `WindowMode` variants took payloads, so
    /// config-rs 0.14 hit its `unreachable!()` (de.rs:337) and every fresh
    /// setup — `cp config.example.yaml config.yaml`, the documented path since
    /// #526 untracked `config.yaml` — panicked before opening a window.
    #[test]
    fn example_config_deserializes_into_client_config() {
        let config = ClientConfig::from_file(&example_config_name())
            .expect("config.example.yaml matches ClientConfig");

        assert_eq!(
            config.window_settings.mode,
            super::window::WindowModeConfig::BorderlessFullscreen
        );
        assert_eq!(config.window_settings.width, 1920.0);
        assert_eq!(config.window_settings.title, "OpenRoad Client");
        assert!(config.network_settings.enabled);
        assert!(!config.dev_tools);
        assert!(!config.dev_fast_login.enabled);
        // An empty `captcha_answer:` is YAML null, i.e. "let the modal handle
        // the challenge" — never a guessed code.
        assert_eq!(config.dev_fast_login.captcha_answer, None);
    }

    /// Every documented `mode:` spelling has to survive the round trip into
    /// bevy's payload-carrying `WindowMode`, including the `SizedFullscreen`
    /// of older user configs (bevy 0.19 dropped that variant).
    #[test]
    fn every_documented_window_mode_maps_to_a_bevy_mode() {
        use super::window::WindowModeConfig;
        use bevy::window::{MonitorSelection, VideoModeSelection, WindowMode};

        let parse = |mode: &str| -> WindowModeConfig {
            serde_yaml::from_str(mode).unwrap_or_else(|e| panic!("`{mode}` must parse: {e}"))
        };
        assert_eq!(parse("Windowed"), WindowModeConfig::Windowed);
        assert_eq!(
            parse("BorderlessFullscreen"),
            WindowModeConfig::BorderlessFullscreen
        );
        assert_eq!(parse("Fullscreen"), WindowModeConfig::Fullscreen);
        assert_eq!(
            parse("SizedFullscreen"),
            WindowModeConfig::BorderlessFullscreen,
            "an existing config.yaml from before bevy 0.19 must still boot"
        );

        let monitor = MonitorSelection::Primary;
        assert_eq!(
            WindowModeConfig::Windowed.to_window_mode(monitor),
            WindowMode::Windowed
        );
        assert_eq!(
            WindowModeConfig::BorderlessFullscreen.to_window_mode(monitor),
            WindowMode::BorderlessFullscreen(monitor)
        );
        assert_eq!(
            WindowModeConfig::Fullscreen.to_window_mode(monitor),
            WindowMode::Fullscreen(monitor, VideoModeSelection::Current)
        );
    }

    /// A config that does not match the struct must come back as an `Err` the
    /// caller can print, not as a panic from inside config-rs (#539).
    #[test]
    fn a_broken_config_is_an_error_not_a_panic() {
        let dir = std::env::temp_dir().join(format!(
            "openroad-cfg-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("config.yaml");
        std::fs::write(
            &file,
            "network_settings: 3
",
        )
        .expect("write");

        // `expect_err` would need `ClientConfig: Debug`, which the resource has
        // no other reason to carry.
        let Err(err) = ClientConfig::from_file(dir.join("config").to_str().expect("utf-8")) else {
            panic!("a config that does not match the struct must not deserialize");
        };
        assert!(
            !format!("{err}").is_empty(),
            "the error carries a message for the user"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// #581: naming an enum variant as a bare string where the Rust side
    /// expects a payload-carrying variant must come back as an `Err` that says
    /// *which* key is wrong. Before the guard this panicked inside config-rs
    /// (`unreachable!()`, `config-0.14.1/src/de.rs:337`) with a message that
    /// named nothing — the trap #539 only removed the trigger for.
    ///
    /// It runs against the real loader and a real key: `window_settings.monitor`
    /// is bevy's `MonitorSelection`, whose `Index(usize)` variant carries a
    /// payload, so `monitor: Index` is a mistake a user can actually make.
    #[test]
    fn an_enum_shape_mismatch_is_an_error_naming_the_key() {
        // Start from the example config so every *other* key is valid and the
        // load reaches the one bad value.
        let mut config: serde_yaml::Value = serde_yaml::from_str(
            &std::fs::read_to_string(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config.example.yaml"),
            )
            .expect("config.example.yaml is readable"),
        )
        .expect("config.example.yaml is yaml");
        config["window_settings"]["monitor"] = serde_yaml::Value::String("Index".to_string());

        let dir = std::env::temp_dir().join(format!(
            "openroad-cfg-shape-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(
            dir.join("config.yaml"),
            serde_yaml::to_string(&config).expect("serialize"),
        )
        .expect("write");

        let Err(err) = ClientConfig::from_file(dir.join("config").to_str().expect("utf-8")) else {
            panic!("`monitor: Index` must not deserialize");
        };
        let message = format!("{err}");
        assert!(
            message.contains("window_settings.monitor"),
            "the error has to name the offending key, got: {message}"
        );
        // Pins the *path*, not just the outcome: this sentence exists only in
        // `shape_mismatch_error`, which is reached only after `catch_unwind`
        // caught config-rs's `unreachable!()`. Without it the test would also
        // pass on the ordinary type-mismatch path that #539 already covered,
        // and the trap this issue is about would stay untested.
        assert!(
            message.contains("cannot build an enum variant that carries a payload"),
            "the error has to come from the caught enum-shape panic, got: {message}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The `graphics:` block in `config.example.yaml` is what users copy into
    /// their own `config.yaml`, so it has to keep matching [`GraphicsSettings`]
    /// — a typo there only shows up as a startup panic otherwise.
    #[test]
    fn example_graphics_config_deserializes() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../config.example.yaml")
            .with_extension("");
        let graphics = Config::builder()
            .add_source(config::File::with_name(path.to_str().unwrap()))
            .build()
            .expect("config.example.yaml is readable")
            .get::<GraphicsSettings>("graphics")
            .expect("the graphics block matches GraphicsSettings");

        assert!(graphics.bloom.enabled);
        assert_eq!(graphics.bloom.intensity, 0.15);
        // Down from 4.0 and paired with an intensity: `pow(streak, 4)` on a
        // mid-tone texture kept ~6% of the streak's energy, and nothing scaled
        // it back, which is why the +N glow was invisible.
        assert_eq!(graphics.sheen.shine_pow, 2.0);
        assert_eq!(graphics.sheen.intensity, 3.0);
        assert!(graphics.rim.enabled);
        assert_eq!(graphics.rim.color, "5AFFFFFF");
        assert_eq!(graphics.rim.power, 3.0);
        assert_eq!(graphics.rim.mode, super::graphics::RimMode::Relative);
        assert_eq!(graphics.rim.strength, 1.0);
        assert_eq!(
            graphics.terrain.lighting,
            super::graphics::TerrainLightingConfig::Dynamic
        );
        assert!(!graphics.terrain.lightmap_flip_v);
        assert!(graphics.shadows.enabled);
        assert_eq!(graphics.shadows.cascades, 2);
        assert_eq!(graphics.shadows.distance, 120.0);
        // `mode: "off"` must stay quoted in the example — bare `off` is a YAML
        // boolean and would fail the enum. This assertion is what proves the
        // quoting works end to end.
        assert_eq!(graphics.foliage.mode, super::graphics::FoliageMode::Off);
        assert_eq!(graphics.foliage.view_distance, 300.0);
        assert_eq!(graphics.foliage.density, 1.0);
        assert!(!graphics.foliage.cast_shadows);
        assert_eq!(graphics.foliage.pack.density, 8.0);
        assert_eq!(graphics.foliage.pack.scale, 1.0);
        assert_eq!(graphics.foliage.tint.tile_blend_pack, 1.0);
        assert_eq!(graphics.foliage.tint.tile_blend_native, 0.25);
        assert_eq!(graphics.foliage.tint.baked_light, 0.5);
    }

    /// Same guard for the `hud:` block.
    #[test]
    fn example_hud_config_deserializes() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../config.example.yaml")
            .with_extension("");
        let hud = Config::builder()
            .add_source(config::File::with_name(path.to_str().unwrap()))
            .build()
            .expect("config.example.yaml is readable")
            .get::<HudSettings>("hud")
            .expect("the hud block matches HudSettings");

        assert!(hud.skill_window_native_grid);
        assert_eq!(hud.hud_scale, crate::plugins::hud::scale::DEFAULT_HUD_SCALE);
    }
}
