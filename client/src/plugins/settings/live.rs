//! How a changed setting reaches its consumer at runtime (#647).
//!
//! # The mechanism
//!
//! There is exactly one, and it is Bevy's own change detection — not a bespoke
//! event bus:
//!
//! 1. Settings live in **two** resources. [`ClientConfig`] is the author-facing
//!    `config.yaml` (graphics knobs, colours, window); [`GameOptions`] is the
//!    player-facing, persisted option set the options window edits.
//! 2. Anything *derived* from them (a resolved colour palette, a `Bloom`
//!    component, a window mode) is produced by an **apply system** named
//!    `apply_*`, gated on `resource_changed::<ClientConfig>` /
//!    `resource_changed::<GameOptions>`, and living in the module that owns the
//!    thing it writes.
//! 3. A derived value is therefore **never** computed in `Plugin::build`.
//!    `Plugin::build` runs exactly once, so a value derived there can never
//!    follow a later edit — that is precisely the shape every "the setting does
//!    nothing until you restart" bug in this tree has had.
//!
//! `resource_changed` is true on the frame a resource is inserted, so the same
//! system seeds the derived value at boot and refreshes it afterwards. One code
//! path, no separate initialisation, nothing to keep in sync.
//!
//! # Adding a setting
//!
//! Put the field in its settings struct, read it in an `apply_*` system in the
//! module that owns the consumer, register that system with
//! `.run_if(resource_changed::<..>)`, and add the group to [`SETTINGS_AUDIT`].
//! [`the_audit_covers_every_client_config_group`] fails the build if a new
//! group appears in [`ClientConfig`] without an audit verdict, so a setting
//! cannot be added and quietly wired to nothing.
//!
//! # Where a setting genuinely cannot be live
//!
//! Some values decide *plugin registration* (`dev_tools`, `network_settings.
//! enabled`, `scenes`, `dev_fast_login`) — they are read in `main()` or in a
//! `Plugin::build` before the schedule exists, so "restart" is not a wiring
//! defect but the truth about them. Those carry [`Liveness::Restart`] with the
//! reason spelled out, and the UI must say so rather than pretend.
//!
//! # A group is not always one lifetime (#730)
//!
//! `ClientConfig` groups are `config.yaml` sections, not liveness units: a
//! section can hold one field that decides plugin registration and another
//! that is read from `Res<ClientConfig>` in a system every frame. Collapsing
//! those into a single verdict makes the row wrong in one direction whichever
//! verdict wins, so such a group carries [`Liveness::Mixed`] and its note
//! names the restart half *and* the live half by field.

use bevy::prelude::*;

use crate::plugins::config::ClientConfig;
use crate::plugins::settings::options::GameOptions;

/// What actually happens when a settings group changes at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// An `apply_*` system picks the change up on the next frame.
    Live,
    /// Read once before the `App` exists (plugin registration) or by a
    /// one-shot scene setup; a change needs a restart. The `why` is not
    /// decoration — it is what the options UI has to tell the user.
    Restart,
    /// The group spans more than one lifetime: at least one field follows a
    /// change and at least one cannot (#730). The note must name both halves
    /// by field, because a UI that renders only the group verdict would tell
    /// the user something false about half of it.
    Mixed,
    /// Parsed and persisted, but nothing reads it. A defect with a ticket.
    DeadWire,
}

/// One `ClientConfig` group and what a runtime change to it does.
#[derive(Debug, Clone, Copy)]
pub struct SettingsGroup {
    /// The field name in [`ClientConfig`], exactly as spelled there.
    pub field: &'static str,
    pub liveness: Liveness,
    /// The `apply_*` system, the reason it cannot be live, or the issue that
    /// tracks the dead wire.
    pub note: &'static str,
}

/// The audit the issue asks for, as data rather than prose, so it cannot rot:
/// a group missing here fails [`the_audit_covers_every_client_config_group`].
pub const SETTINGS_AUDIT: &[SettingsGroup] = &[
    SettingsGroup {
        field: "network_settings",
        liveness: Liveness::Mixed,
        note: "restart: `enabled` gates GatewayPlugin registration \
               (net/plugin.rs:97) and `packet_dump` / `outbound_encryption` \
               decide an init_resource in NetworkCorePlugin::build \
               (net/plugin.rs:52-69) — none of the three can be undone in a \
               built App. live: `item_use_enabled` is read from \
               Res<ClientConfig> in the cast system \
               (hud/underbar/cast.rs:140), so it follows a change on the next \
               use, and `gateway_address` is read per connect attempt in \
               net::gateway::systems::init_gateway_service (:41), so an edit \
               lands on the next connect — nothing reconnects mid-session, \
               which is a missing feature and not a frozen value",
    },
    SettingsGroup {
        field: "window_settings",
        liveness: Liveness::Live,
        note: "config::window::apply_window_settings is the single writer of \
               mode, title and size, and it resolves one WindowIntent so \
               there is exactly one answer: the mode comes from \
               `window_settings.mode` on `window_settings.monitor` unless the \
               options window has flipped `video.window_mode_override` in \
               this session — that override is `#[serde(skip)]`, so it never \
               reaches user_settings.yaml and every restart returns to \
               config.yaml. It ran second before, from options_video, and \
               quietly overrode `mode` and `monitor` on every boot. Centring \
               and the scale-factor pin are boot-only in \
               config::window::setup_window (re-centring on an edit would \
               yank a moved window); `RESOLUTION=` stays an env override that \
               outranks the configured size",
    },
    SettingsGroup {
        field: "scenes",
        liveness: Liveness::Restart,
        note: "picks the boot scene in scenes::mod during Plugin::build; \
               changing it later is a scene switch, not a setting",
    },
    SettingsGroup {
        field: "diagnostics",
        liveness: Liveness::Restart,
        note: "the measurement tier split out of dev_tools: main.rs registers \
               RemotePlugin, BrpExtrasPlugin, RenderDiagnosticsPlugin and the \
               render-phase/asset counters from it. Registration again, so \
               restart by construction for the same reason as dev_tools — \
               which implies it, via ClientConfig::diagnostics_enabled",
    },
    SettingsGroup {
        field: "dev_tools",
        liveness: Liveness::Restart,
        note: "read at four registration sites and nowhere else: main.rs \
               (the egui inspectors; the BRP half moved to `diagnostics`, \
               which this implies), DevPlugin::build via \
               dev_tools_enabled (dev/mod.rs — a plain fn called from \
               build, not a run_if), EnvironmentPlugin::build \
               (environment/mod.rs — the inspectors AND the N/K/L/Shift+M \
               time-of-day hotkeys) and ScenePlugin::build \
               (scenes/mod.rs — the SceneState inspector). All four add \
               plugins or systems, and Bevy has neither plugin nor system \
               removal, so the whole flag is restart by construction — \
               checked at HEAD for #730, which expected a live half here \
               and there is none",
    },
    SettingsGroup {
        field: "dev_fast_login",
        liveness: Liveness::Restart,
        note: "consumed by the intro scene's one-shot login path; it has \
               already happened by the time anyone could change it",
    },
    SettingsGroup {
        field: "chat",
        liveness: Liveness::Live,
        note: "hud::chat::apply_chat_colors",
    },
    SettingsGroup {
        field: "nameplates",
        liveness: Liveness::Live,
        note: "hud::nameplates::apply_nameplate_colors",
    },
    SettingsGroup {
        field: "selection",
        liveness: Liveness::Live,
        note: "cursor::interactions::entity_select::apply_selection_colors \
               (also re-reads graphics.rim.mode, which it shares)",
    },
    SettingsGroup {
        field: "graphics",
        liveness: Liveness::Live,
        note: "terrain params via apply_terrain_render_params, bloom via \
               options_video::apply_bloom_option, rim mode via \
               apply_selection_colors, foliage via \
               map::foliage::apply_foliage_settings (#646). Brightness and the \
               unbacked quality rows have no feature behind them yet and say \
               so in the pane",
    },
    SettingsGroup {
        field: "hud",
        liveness: Liveness::Live,
        note: "toast_seconds, low_vitals_caution_percent and \
               region_banner_seconds are read from Res<ClientConfig> in the \
               systems that use them, so they follow a change immediately; \
               skill_window_native_grid and hud_scale are read while a window \
               is built (hud_scale via plugins::hud::scale, seeded and \
               refreshed by apply_hud_scale), so they follow on the next open \
               of that window",
    },
    SettingsGroup {
        field: "input",
        liveness: Liveness::Live,
        note: "camera::mouse_camera_roles reads config.input.mouse_scheme from \
               Res<ClientConfig> in the scroll/drag systems themselves, so a \
               change lands on the next frame with no apply system needed",
    },
    SettingsGroup {
        field: "fonts",
        liveness: Liveness::Restart,
        note: "deliberately not live, and this is the decision rather than an \
               excuse: assets::apply_pk2_face (assets/mod.rs:265) swaps the UI \
               face once and latches on a Local<bool>, because every spawned \
               TextFont holds its own cloned Handle<Font> — following a change \
               means walking every text entity in the world and re-resolving \
               its handle, for a single boot-time knob (`pk2_faces`) nobody \
               toggles mid-session. Revisit if a font picker is ever built",
    },
    SettingsGroup {
        field: "effects",
        liveness: Liveness::Live,
        note: "the item_use table is read from Res<ClientConfig> at the moment \
               an item-use ack arrives (hud/underbar/cast.rs::play_item_use_effect), \
               so an edited path takes effect on the very next use — which is \
               the point: the table ships empty because no archive data maps an \
               item to an .efp, and filling it in is a look-and-try loop that \
               would be miserable behind a restart",
    },
    SettingsGroup {
        field: "guild",
        liveness: Liveness::Restart,
        note: "restart: `position_grant` picks which of the two byte-identically \
               placed §Command buttons (ifguild.txt ids 105/106) the guild page \
               spawns, and that page is built once by \
               hud::community::ui::spawn_community_window on \
               OnEnter(SceneState::GameWorld) — the caption is baked into a \
               spawned Text at that moment, so a change lands on the next world \
               entry. Making it live would mean re-spawning the page for a knob \
               that selects a server era, which does not change mid-session",
    },
    SettingsGroup {
        field: "combat",
        liveness: Liveness::Live,
        note: "live: `knockdown_hold_seconds` is read from Res<ClientConfig> in \
               player::play_knockdowns at the moment a knockdown starts, and is \
               copied into that body's KnockedDown component — so an edit lands \
               on the next knockdown. Bodies already on the ground keep the \
               dwell they fell with, which is the only sane reading of a change \
               made mid-fall. Note the default is NEGATIVE, meaning 'derive from \
               characterdata KO_RecoverTime, else the prone clip's own length' \
               (player::knockdown_hold_secs); a value >= 0 overrides both, and \
               that override is what follows the config live",
    },
];

/// Look a group up by its [`ClientConfig`] field name.
pub fn verdict(field: &str) -> Option<SettingsGroup> {
    SETTINGS_AUDIT.iter().copied().find(|g| g.field == field)
}

/// Run condition: `ClientConfig` changed (true on the frame it is inserted, so
/// an apply system seeds and refreshes through one path).
pub fn config_changed(config: Res<ClientConfig>) -> bool {
    config.is_changed()
}

/// Run condition: [`GameOptions`] changed. Same contract as [`config_changed`].
pub fn options_changed(options: Res<GameOptions>) -> bool {
    options.is_changed()
}

#[cfg(test)]
mod test {
    use super::*;

    /// The field names of `struct ClientConfig`, read out of the source so the
    /// audit is checked against the real struct rather than against a copy of
    /// it somebody has to remember to update.
    fn client_config_fields() -> Vec<String> {
        let src = include_str!("../config/mod.rs");
        let start = src
            .find("pub struct ClientConfig {")
            .expect("ClientConfig is declared in config/mod.rs");
        let body = &src[start..];
        let end = body.find("\n}").expect("the struct is brace-closed");
        body[..end]
            .lines()
            .filter_map(|l| l.trim().strip_prefix("pub "))
            .filter_map(|l| l.split(':').next())
            .filter(|name| !name.is_empty() && !name.contains(' '))
            .map(str::to_string)
            .collect()
    }

    /// Every settings group carries a verdict. A new group added to
    /// `ClientConfig` without one fails here — which is the whole point: the
    /// dead wires this issue collects (#373 audio, #332 auto-potion, #605
    /// mouse swap, #379 camera) were all "parsed, persisted, read by nothing",
    /// and nothing in the build ever said so.
    #[test]
    fn the_audit_covers_every_client_config_group() {
        let fields = client_config_fields();
        assert!(
            fields.len() > 5,
            "the field scrape found only {fields:?} — it stopped matching the struct"
        );
        for field in &fields {
            assert!(
                verdict(field).is_some(),
                "ClientConfig.{field} has no row in SETTINGS_AUDIT: say whether \
                 it applies live, needs a restart (and why), or is a dead wire \
                 (with an issue)"
            );
        }
        for group in SETTINGS_AUDIT {
            assert!(
                fields.iter().any(|f| f == group.field),
                "SETTINGS_AUDIT names `{}`, which is not a ClientConfig field",
                group.field
            );
        }
    }

    /// A `Restart` or `DeadWire` verdict without a reason is not an audit, it
    /// is a shrug: the options UI has to be able to tell the user *why*.
    #[test]
    fn every_non_live_group_states_its_reason() {
        for group in SETTINGS_AUDIT {
            if group.liveness != Liveness::Live {
                assert!(
                    group.note.len() > 20,
                    "{} is {:?} but says only {:?}",
                    group.field,
                    group.liveness,
                    group.note
                );
            }
        }
    }

    /// A `Mixed` row exists only to say *which half is which*, so a note that
    /// does not name both halves is the collapsed verdict #730 was filed
    /// about, wearing a new label.
    #[test]
    fn every_mixed_group_names_a_restart_half_and_a_live_half() {
        for group in SETTINGS_AUDIT {
            if group.liveness == Liveness::Mixed {
                assert!(
                    group.note.contains("restart:") && group.note.contains("live:"),
                    "{} is Mixed but its note names no `restart:`/`live:` half: {:?}",
                    group.field,
                    group.note
                );
            }
        }
    }

    /// The `network_settings` split is a claim about two source sites, so it
    /// is checked against them: the restart half is read in a `Plugin::build`,
    /// the live half from `Res<ClientConfig>` in a system. If someone wires a
    /// reconnect (or moves `enabled` behind a run condition), this fails and
    /// the row gets re-judged instead of quietly going stale.
    #[test]
    fn the_network_settings_split_is_still_what_the_source_says() {
        let group = verdict("network_settings").expect("network_settings has a row");
        assert_eq!(group.liveness, Liveness::Mixed);

        let net_plugin = include_str!("../net/plugin.rs");
        assert!(
            build_bodies(net_plugin)
                .iter()
                .any(|b| b.contains("network_settings.enabled")),
            "the restart half moved: no Plugin::build in net/plugin.rs reads              network_settings.enabled any more"
        );

        let cast = include_str!("../hud/underbar/cast.rs");
        assert!(
            cast.contains("config.network_settings.item_use_enabled"),
            "the live half moved: cast.rs no longer reads item_use_enabled              from the config resource"
        );
        assert!(
            build_bodies(cast).is_empty(),
            "cast.rs grew a Plugin::build — re-check whether item_use_enabled              is still read per use"
        );
    }

    /// The counter-claim #730 makes about `dev_tools` — that some of what it
    /// gates is `run_if`-gated and could follow a change today — is false at
    /// HEAD: every read is a registration decision. This pins that, so the
    /// `Restart` verdict is a measurement and not a memory: the day a
    /// `run_if` reads the flag, this test fails and the row must be split.
    #[test]
    fn dev_tools_is_read_only_at_registration() {
        let sources = [
            ("main.rs", include_str!("../../main.rs")),
            ("dev/mod.rs", include_str!("../dev/mod.rs")),
            ("environment/mod.rs", include_str!("../environment/mod.rs")),
        ];
        for (name, text) in sources {
            let reads: Vec<&str> = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//") && l.contains("config.dev_tools"))
                .collect();
            assert!(
                !reads.is_empty(),
                "{name} no longer reads config.dev_tools — the audit note names it"
            );
            for line in reads {
                assert!(
                    !line.contains("run_if"),
                    "{name} now gates a run condition on dev_tools ({line:?}):                      that half follows a change, so the SETTINGS_AUDIT row must                      be split into a Mixed verdict (#730)"
                );
            }
        }
    }

    /// The rule the mechanism rests on, enforced instead of documented: no
    /// `Plugin::build` may derive a value from `ClientConfig`. `build` runs
    /// once, so anything derived there is frozen for the process lifetime —
    /// exactly the bug shape #647 is about.
    #[test]
    fn no_plugin_build_derives_a_value_from_the_config() {
        // Bevy cannot add or remove a plugin after the App is built, so a
        // config value that decides *plugin registration* is restart-only by
        // construction, not by neglect. Each exception is a registration
        // decision and is listed as `Restart` in SETTINGS_AUDIT.
        const ALLOWED: [(&str, &str); 4] = [
            (
                "net/plugin.rs",
                "network_settings.enabled adds GatewayPlugin",
            ),
            (
                "scenes/mod.rs",
                "scenes.startup picks the boot scene state; dev_tools adds the \
                 SceneState inspector",
            ),
            (
                "dev/mod.rs",
                "dev_tools adds the egui inspector plugins, the dev-window \
                 button and the bare-letter hotkeys",
            ),
            (
                "environment/mod.rs",
                "dev_tools adds the inspector windows and the time-of-day \
                 hotkeys",
            ),
        ];

        let mut offenders = Vec::new();
        let mut seen_exception = [false; ALLOWED.len()];
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let name = path.to_string_lossy().to_string();
                if let Some(i) = ALLOWED.iter().position(|(f, _)| name.ends_with(f)) {
                    seen_exception[i] = true;
                    continue;
                }
                for build in build_bodies(&text) {
                    if build.contains("resource::<ClientConfig>")
                        || build.contains("resource::<crate::plugins::config::ClientConfig>")
                    {
                        offenders.push(name.clone());
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "these `Plugin::build` bodies derive from ClientConfig, which freezes \
             the value for the whole process — move it into an `apply_*` system \
             gated on `resource_changed::<ClientConfig>`: {offenders:?}"
        );
        // A stale exception is how this guard rots into decoration.
        for (i, (file, why)) in ALLOWED.iter().enumerate() {
            assert!(
                seen_exception[i],
                "the exception for {file} ({why}) matched no file — drop it"
            );
        }
    }

    /// The bodies of every `fn build(&self, app: &mut App)` in one file, by
    /// brace counting (there is no syn dependency in this crate).
    fn build_bodies(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(at) = rest.find("fn build(&self, app: &mut App)") {
            let after = &rest[at..];
            let Some(open) = after.find('{') else { break };
            let mut depth = 0usize;
            let mut end = None;
            for (i, c) in after[open..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(open + i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end) = end else { break };
            out.push(after[open..=end].to_string());
            rest = &after[end..];
        }
        out
    }

    /// The mechanism rests on one Bevy behaviour: `resource_changed` is true
    /// on the frame a resource is *inserted*, not only after a later write.
    /// That is what lets a single `apply_*` system both seed a derived value
    /// at boot and refresh it afterwards. Bevy has changed change-detection
    /// details across releases (this tree moved 0.17 -> 0.19 recently), so the
    /// assumption is tested rather than trusted.
    #[test]
    fn resource_changed_fires_on_insertion_and_on_write_but_not_while_idle() {
        #[derive(Resource, Default)]
        struct Probe(u32);
        #[derive(Resource, Default)]
        struct Applied(u32);

        let mut app = App::new();
        app.init_resource::<Applied>().add_systems(
            Update,
            (|mut applied: ResMut<Applied>| applied.0 += 1).run_if(resource_changed::<Probe>),
        );

        app.insert_resource(Probe(0));
        app.update();
        assert_eq!(app.world().resource::<Applied>().0, 1, "seeds on insertion");

        app.update();
        assert_eq!(
            app.world().resource::<Applied>().0,
            1,
            "an idle frame must not re-apply"
        );

        app.world_mut().resource_mut::<Probe>().0 = 1;
        app.update();
        assert_eq!(app.world().resource::<Applied>().0, 2, "follows a write");
    }

    /// The scraper must actually see a `build` body, or the guard above passes
    /// vacuously — a green test that checks nothing is worse than no test.
    #[test]
    fn the_build_body_scraper_finds_a_real_body() {
        let bodies = build_bodies(
            "impl Plugin for X { fn build(&self, app: &mut App) { app.add_systems(a, b { c }); } }",
        );
        assert_eq!(bodies.len(), 1);
        assert!(bodies[0].contains("add_systems"));
        assert!(bodies[0].ends_with('}'));
    }
}
