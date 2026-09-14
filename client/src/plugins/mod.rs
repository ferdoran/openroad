pub mod animation_culling;
pub mod animation_sounds;
pub mod assets;
pub mod camera;
pub mod combat;
pub mod diagnostics;
pub mod environment;
pub mod gm;
pub mod hud;
pub mod input_watchdog;
pub mod map;
pub mod nav;
pub mod options_audio;
pub mod options_camera;
pub mod options_game;
pub mod options_input_tab;
pub mod options_video;
pub mod options_window;
pub mod player;
pub mod screenshot;
pub mod settings;
pub mod skills;
pub mod skybox;
pub mod small_popup;
pub mod system_window;
pub mod texani;
pub mod ui_v2;
pub mod world_origin;
pub mod zone_ambience;
pub mod zone_bgm;

pub mod config;
pub mod cos;
pub mod cursor;
pub mod dev;
pub mod dungeon;
pub mod dynamic_resource_loader;
pub mod effects;
pub mod net;
pub mod textdata;

#[cfg(test)]
mod tests {
    /// This file's source up to (not including) the test module, so the
    /// literals below cannot match themselves.
    fn registry() -> &'static str {
        include_str!("mod.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("split always yields a first part")
    }

    /// **Locked decision (`CLAUDE.md`): extensibility rides static Cargo
    /// features — no dynamic DLL/WASM plugins.**
    ///
    /// The tree carried the remains of the opposite design for a long time:
    /// `plugins/loader.rs` with `list_plugins()` / `load_plugin(PathBuf)`
    /// stubs whose comment proposed "bevy-modding or `include!`", plus a
    /// `sro_v188` version-plugin that only logged its own name at `Startup`.
    /// Both were registered in `main.rs`, which made a rejected architecture
    /// look like a live one. #56-C removed them; this pins the decision so a
    /// future "plugin loader" has to argue with `CLAUDE.md` first.
    ///
    /// Note what this does NOT forbid: `dynamic_resource_loader` (SRO resource
    /// mirroring, used by 16 modules) and the `bevy_asset_loader` asset
    /// loaders are unrelated to code loading — which is why the predicate is
    /// the two removed type names, not the word "loader".
    /// #56-C, the last part: **exactly one UI module.** `plugins::ui` used to
    /// sit next to `plugins::ui_v2` and read like a v1 of it. Measured, it was
    /// four unrelated things — a hand-rolled text input and a button system
    /// (dead, removed), the 2d UI camera and window settings (live), and the
    /// shared context-menu widget (live). The dead halves went, and the two
    /// live halves moved to the modules that own their domain
    /// (`plugins::camera`, `plugins::options_video`, `plugins::hud`), so the
    /// duality is gone rather than migrated.
    ///
    /// The predicate is the module declaration, and `ui_v2` deliberately stays.
    #[test]
    fn only_one_ui_module_remains() {
        assert!(
            !registry().contains("pub mod ui;"),
            "plugins::ui is back: it was not a v1 of ui_v2 but a bag of \
             unrelated things, and its live halves belong to their domains"
        );
        assert!(registry().contains("pub mod ui_v2;"));
    }

    #[test]
    fn no_dynamic_plugin_loader_is_registered() {
        let main_rs = include_str!("../main.rs");
        for forbidden in ["PluginLoader", "SroV188Plugin"] {
            assert!(
                !main_rs.contains(forbidden),
                "`{forbidden}` in main.rs: CLAUDE.md locks extensibility to \
                 static Cargo features, not dynamic plugin loading"
            );
        }
        for forbidden in ["pub mod loader;", "pub mod sro_v188;"] {
            assert!(
                !registry().contains(forbidden),
                "`{forbidden}`: the dynamic-plugin scaffolding #56-C removed"
            );
        }
    }
}
