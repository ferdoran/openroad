//! In-game HUD elements (SceneState::GameWorld), built like the intro_v2 UI:
//! layouts hand-transcribed from the vanilla resinfo definitions, rendered as
//! BSN scenes on the persistent 2d UI camera.
//!
//! # Registration
//!
//! This file is a *registry*, not a wiring sheet. Every window owns a `Plugin`
//! next to its own code and `HudPlugin` only names it, so adding a window
//! touches one line here and nothing else. The previous shape — one 500-line
//! `HudPlugin::build` where each window appended to five or six shared blocks
//! (`init_resource`, the `OnEnter`/`OnExit` tuples, an `Update` tuple, the
//! `PostUpdate` tuple) — made this file a serialization point: *every* HUD
//! branch collided with *every* other HUD branch, so N concurrent windows cost
//! O(N^2) rebases that carried no semantic content (#558).
//!
//! Two consequences worth knowing: Bevy's 20-system tuple limit is now per
//! window rather than per HUD, so the "nested: the flat tuple exceeds Bevy's
//! 20-system limit" workarounds are gone; and system order inside a schedule
//! was already unordered, so splitting the shared tuples changed no behaviour
//! (the `.chain()` groups that *did* express order are intra-window and moved
//! together).

pub mod academy_appraisal;
pub mod action;
pub mod alchemy;
pub mod appearance_change;
pub mod arena;
pub mod autopotion;
pub mod cast_gauge;
pub mod character_info;
pub mod chat;
pub mod chat_bubble;
pub mod collection;
pub mod community;
pub mod context_menu;
pub mod cooldown;
pub mod cos;
pub mod cos_command;
pub mod cos_status;
pub mod death;
pub mod exchange;
pub mod flipbook;
pub mod focus;
pub mod free_pvp;
pub mod game_guide;
pub mod game_window;
pub mod gauge;
pub mod guild_storage;
pub mod hitcount;
pub mod inventory;
pub mod magic_state_board;
pub mod main_popup;
pub mod minimap;
pub mod modal_dialog;
pub mod nameplates;
pub mod npc_dialog;
pub mod party;
pub mod party_matching;
pub mod pet_mini_info;
pub mod petition;
pub mod player_mini_info;
pub mod quest_reward;
pub mod quest_reward_confirm;
pub mod quick_party;
pub mod region_banner;
pub mod scale;
pub mod skill_window;
pub mod stall;
pub mod stall_network;
pub mod storage;
pub mod store;
pub mod system_message;
pub mod target_menu;
pub mod target_window;
pub mod toast;
pub mod underbar;
pub mod window_positions;
pub mod world_anchor;
pub mod world_map;

use bevy::prelude::*;

use crate::scenes::SceneState;

/// Scenes where the shared HUD Update systems run: the in-game scene, the
/// offline widget previews, and the offline Skills test scene (underbar,
/// target window, nameplates, hitcounts, drag ghost).
fn hud_scenes(state: Res<State<SceneState>>) -> bool {
    matches!(
        **state,
        SceneState::GameWorld | SceneState::UiTesting | SceneState::Skills
    )
}

/// Scenes where the map widgets (minimap, world-map window) run: the shared
/// HUD scenes plus the offline Dungeons scene — which gets the maps for
/// verifying dungeon floor tiles, but none of the rest of the HUD.
fn map_scenes(state: Res<State<SceneState>>) -> bool {
    matches!(
        **state,
        SceneState::GameWorld | SceneState::UiTesting | SceneState::Skills | SceneState::Dungeons
    )
}

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        // Alphabetical, one line per window, grouped only because Bevy's
        // `add_plugins` tuple tops out at 15 elements. A 16th in any group is
        // not a nice error — it is an unreadable `Plugins<_> is not satisfied`
        // on this call — so a group that fills up shifts its last entry into
        // the next one rather than growing.
        app.add_plugins((
            (
                academy_appraisal::AcademyAppraisalPlugin,
                action::ActionPlugin,
                alchemy::AlchemyPlugin,
                appearance_change::AppearanceChangePlugin,
                arena::ArenaPlugin,
                autopotion::AutoPotionPlugin,
                character_info::CharacterInfoPlugin,
                chat::ChatPlugin,
                chat_bubble::ChatBubblePlugin,
                collection::CollectionPlugin,
                community::CommunityPlugin,
                cooldown::CooldownPlugin,
                cos::CosPlugin,
                cos_command::CosCommandPlugin,
                cos_status::CosStatusPlugin,
            ),
            (
                death::DeathPlugin,
                exchange::ExchangePlugin,
                free_pvp::FreePvpPlugin,
                game_guide::GameGuidePlugin,
                guild_storage::GuildStoragePlugin,
                hitcount::HitcountPlugin,
                inventory::InventoryPlugin,
                magic_state_board::MagicStateBoardPlugin,
                minimap::MinimapPlugin,
                nameplates::NameplatesPlugin,
                npc_dialog::NpcDialogPlugin,
                petition::PetitionPlugin,
                player_mini_info::PlayerMiniInfoPlugin,
                quest_reward::QuestRewardPlugin,
            ),
            (
                region_banner::RegionBannerPlugin,
                scale::HudScalePlugin,
                skill_window::SkillWindowPlugin,
                stall::StallPlugin,
                stall_network::StallNetworkPlugin,
                storage::StoragePlugin,
                store::StorePlugin,
                system_message::SystemMessagePlugin,
                target_menu::TargetMenuPlugin,
                target_window::TargetWindowPlugin,
                toast::ToastPlugin,
                underbar::UnderbarPlugin,
                window_positions::WindowPositionsPlugin,
                world_map::WorldMapPlugin,
            ),
            (
                // The party surfaces. Their own group because groups 2 and 3
                // have one free slot each between them and this feature brings
                // three plugins — a group that fills up shifts, it does not
                // grow. The comment sits INSIDE the tuple on purpose: the two
                // tests below parse this list textually, one plugin per line,
                // and rustfmt collapses a short tuple onto one line unless
                // something in it forbids that.
                party::PartyWindowPlugin,
                party_matching::PartyMatchingPlugin,
                quick_party::QuickPartyPlugin,
                // Window focus + the Escape chain. Not a window itself: it is
                // what decides which window Escape closes.
                focus::HudFocusPlugin,
                // The MainPopup group. Also not a window: it is what makes the
                // five pages of the original's single frame behave as one.
                main_popup::MainPopupPlugin,
                cast_gauge::CastGaugePlugin,
            ),
        ));
    }
}

#[cfg(test)]
mod test {
    /// The registry text, up to (not including) this test module — so the
    /// literals the tests search for do not match themselves.
    fn registry() -> &'static str {
        include_str!("mod.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("split always yields a first part")
    }

    /// The registry may only *name* plugins. If a window's wiring creeps back
    /// into this file, the shared-block collisions that #558 measured come
    /// back with it, so the invariant is pinned rather than trusted.
    #[test]
    fn the_registry_only_names_plugins() {
        for forbidden in [".add_systems(", ".init_resource::<", ".add_message::<"] {
            assert!(
                !registry().contains(forbidden),
                "`{forbidden}` belongs in the window's own Plugin, not in the HUD registry"
            );
        }
    }

    /// Every HUD submodule is either registered here or is one of the shared
    /// helpers that register nothing. A new window whose `pub mod` line lands
    /// without its plugin line therefore fails here, instead of compiling
    /// cleanly and silently doing nothing at runtime.
    /// Bevy implements `Plugins` for tuples up to **15** elements, so a
    /// 16th entry in any group is not a compile error in this file — it makes
    /// the whole nested tuple stop implementing the trait, and the error lands
    /// at `add_plugins` with every type name elided.
    ///
    /// That is exactly how this registry broke: it is a shared hotspot (#558),
    /// three lanes appended a plugin each in one lap, every PR was green on
    /// its own, and the *combination* pushed one group to 16. Counting the
    /// groups here turns that into a test failure naming the group, instead of
    /// a trait error naming nothing.
    #[test]
    fn no_plugin_group_exceeds_the_tuple_arity_bevy_implements() {
        const MAX_PLUGIN_TUPLE: usize = 15;
        let text = registry();
        let start = text
            .find("app.add_plugins((")
            .expect("the registry call moved");
        let call = &text[start..];
        let end = call
            .find("\n        ));")
            .expect("the registry call is unterminated");
        let mut sizes = Vec::new();
        for group in call[..end].split("            (\n").skip(1) {
            sizes.push(
                group
                    .lines()
                    .take_while(|line| !line.trim_start().starts_with(')'))
                    .filter(|line| line.trim_end().ends_with("Plugin,"))
                    .count(),
            );
        }
        assert!(!sizes.is_empty(), "no plugin groups found: parser drifted");
        for (index, size) in sizes.iter().enumerate() {
            assert!(
                *size <= MAX_PLUGIN_TUPLE,
                "plugin group {index} holds {size} plugins; Bevy implements \
                 `Plugins` only up to {MAX_PLUGIN_TUPLE}. Open a new group \
                 rather than growing this one."
            );
        }
        // and every plugin really is inside a group (nothing lost in the split)
        assert_eq!(
            sizes.iter().sum::<usize>(),
            call[..end].matches("Plugin,").count(),
            "a plugin sits outside the groups"
        );
    }

    #[test]
    fn every_hud_module_is_registered_or_an_excused_helper() {
        // `game_window` is the shared window chrome (rects, spawn helpers) —
        // pure functions, no systems and no state. `modal_dialog` is the same
        // shape for the `msgbox2_window_` dialog shell (#664): a scrim, a
        // plate and the plate's insets, spawned by its callers' scenes.
        // `world_anchor` is the shared world->viewport projection for the
        // pooled overlays (#661) — likewise pure functions, no systems and no
        // state; giving it a plugin would hand a helper a schedule it does not
        // need.
        // `gauge` is the shared CIFGauge crop recipe (#630) — node builders and
        // one BSN scene, driven by each window's own fill system.
        // `pet_mini_info` is the one child panel that is not a window at all:
        // `ifplayerminiinfo.txt` declares it *inside* the mini-info panel
        // (`53,55,154,40`, #303), so its two systems belong to that panel's
        // plugin and giving it its own would spawn it independently of the
        // parent it lives in.
        // `quest_reward_confirm` is the second half of ONE flow (#663): the
        // picker raises it, Cancel returns to the picker with the selection
        // intact, and the two share `QuestRewardState`. Its systems and its
        // resource are registered — by `QuestRewardPlugin`, next to the
        // picker's own, because splitting one flow across two plugins would
        // make the ordering between them implicit. Excused here as a
        // sibling-registered module, not as a stateless helper.
        // `context_menu` is the shared `ub_new_wnd_` popup (#56-C moved it here
        // from `plugins::ui`, whose remaining halves went to their domains):
        // spawn/close helpers plus its components, no systems and no state —
        // its callers (`target_menu`, and the under-bar flyout that shares the
        // construction) drive it from their own plugins.
        // `flipbook` is the resinfo `Style=512` sprite-sheet description — one
        // struct and two pure functions, no systems and no state. It is shared
        // because the low-vitals overlays are authored TWICE at different sheet
        // sizes (the mini-info's 512x64 pair and the quick-party board's
        // 256x64 pair), so the tile arithmetic cannot be a constant in either.
        const HELPERS: [&str; 8] = [
            "game_window",
            "modal_dialog",
            "world_anchor",
            "gauge",
            "flipbook",
            "pet_mini_info",
            "quest_reward_confirm",
            "context_menu",
        ];

        let registry = registry();
        let registered: Vec<&str> = registry
            .lines()
            .map(str::trim)
            .filter(|l| l.ends_with("Plugin,"))
            .filter_map(|l| l.split("::").next())
            .collect();
        for module in registry
            .lines()
            .filter_map(|l| l.strip_prefix("pub mod "))
            .map(|l| l.trim_end_matches(';'))
        {
            assert!(
                HELPERS.contains(&module) || registered.contains(&module),
                "hud module `{module}` is neither registered in HudPlugin nor an excused helper"
            );
        }
    }
}
