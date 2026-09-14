use bevy::prelude::*;
use bevy_asset_loader::prelude::{
    ConfigureLoadingState, LoadingState, LoadingStateAppExt, LoadingStateConfig,
};
use bevy_inspector_egui::quick::StateInspectorPlugin;
use iyes_progress::ProgressPlugin;
use std::env;

use crate::assets::FontAssets;
use crate::plugins::camera::CameraPlugin;
use crate::plugins::config::ClientConfig;
use crate::plugins::dungeon::DungeonPlugin;
use crate::plugins::map::MapPlugin;
use crate::plugins::world_origin::WorldOrigin;
use crate::scenes::testing::alchemy_ui::AlchemyUiPreviewPlugin;
use crate::scenes::testing::animation_testing::AnimationTestingScenePlugin;
use crate::scenes::testing::autopotion_ui::AutoPotionUiPreviewPlugin;
use crate::scenes::testing::char_select_ui::CharSelectUiPreviewPlugin;
use crate::scenes::testing::character_info_ui::CharacterInfoUiPreviewPlugin;
use crate::scenes::testing::chat_ui::ChatUiPreviewPlugin;
use crate::scenes::testing::choice_confirm_ui::ChoiceConfirmUiPreviewPlugin;
use crate::scenes::testing::dungeons::DungeonsScenePlugin;
use crate::scenes::testing::equipments::EquipmentsScenePlugin;
use crate::scenes::testing::inventory_ui::InventoryUiPreviewPlugin;
use crate::scenes::testing::mini_info_ui::MiniInfoUiPreviewPlugin;
use crate::scenes::testing::minimap_ui::MinimapUiPreviewPlugin;
use crate::scenes::testing::new_asset_loading::NewAssetLoadingScenePlugin;
use crate::scenes::testing::npc_dialog_ui::NpcDialogUiPreviewPlugin;
use crate::scenes::testing::particles::ParticleTestingScenePlugin;
use crate::scenes::testing::party_ui::PartyUiPreviewPlugin;
use crate::scenes::testing::quest_reward_ui::QuestRewardUiPreviewPlugin;
use crate::scenes::testing::skills::SkillsScenePlugin;
use crate::scenes::testing::stall_ui::StallUiPreviewPlugin;
use crate::scenes::testing::underbar_ui::UnderbarUiPreviewPlugin;
use crate::scenes::testing::world_map_ui::WorldMapUiPreviewPlugin;

pub mod game_scene;
pub mod intro_v2;
pub mod loading_screen;
mod testing;
pub mod world_scene;

#[derive(States, Reflect, Default, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum SceneState {
    #[default]
    Loading,
    IntroV2,
    /// The offline **dev sandbox** (`make run world`): a terrain region, a
    /// hardcoded player and a fly cam, with no HUD of its own. The
    /// fidelity-bearing scene is [`SceneState::GameWorld`].
    ///
    /// Named `WorldSandbox` rather than `World` on purpose (#372, audit
    /// finding F7): the old name read like the production world scene, and
    /// the original client has no per-scene UI composition to grade it
    /// against — it is a debug scene, and the type should say so. The
    /// `world` startup token stays as it is, so `make run world` is unchanged.
    WorldSandbox,
    /// The in-game scene entered from character selection: holds the actual
    /// selected character and owns the post-join networking. Distinct from the
    /// [`SceneState::WorldSandbox`] dev scene (fly cam + hardcoded player) and
    /// from the `GameState::Game` runtime phase.
    GameWorld,
    AnimationTesting,
    UiTesting,
    AssetLoadTesting,
    Equipments,
    ParticleTesting,
    /// Offline skill-system test scene: spawn a character + training dummy
    /// and exercise the skill window / underbar / local cast pipeline.
    Skills,
    /// Offline dungeon test scene: spawn a character inside a dungeon
    /// interior (default: Donwhang cave) with an egui window to switch
    /// between all dungeoninfo.txt dungeons.
    Dungeons,
}

pub struct SceneManagerPlugin;

impl Plugin for SceneManagerPlugin {
    fn build(&self, app: &mut App) {
        let config = app.world().resource::<ClientConfig>();
        let scene_from_config = config.scenes.startup.clone();
        let dev_fast_login_enabled = config.dev_fast_login.enabled;
        let scene_from_env = env::var("SCENE").ok();
        let start_scene = detect_start_scene(&scene_from_config, scene_from_env);
        // Footgun guard: dev_fast_login only runs inside the intro_v2 scene, so
        // with any other startup scene `dev_fast_login.enabled: true` is a silent no-op and
        // the client never reaches GameWorld. Warn instead of failing silently.
        if dev_fast_login_enabled && start_scene != SceneState::IntroV2 {
            warn!(
                "dev_fast_login.enabled=true but startup scene resolves to {:?}; dev_fast_login only runs in intro_v2. \
                 Set scenes.startup: intro_v2 (or SCENE=intro_v2) to log in and reach GameWorld.",
                start_scene
            );
        }
        app.register_type::<SceneState>()
            .init_state::<SceneState>()
            .init_resource::<WorldOrigin>()
            .add_loading_state(LoadingState::new(SceneState::Loading))
            // FontAssets is used across every scene (intro, HUD, testing), so
            // the scene manager owns its loading, not any single scene plugin.
            .configure_loading_state(
                LoadingStateConfig::new(SceneState::Loading).load_collection::<FontAssets>(),
            )
            .add_plugins((
                CameraPlugin,
                loading_screen::LoadingScenePlugin,
                MapPlugin,
                DungeonPlugin,
                intro_v2::IntroV2ScenePlugin,
                world_scene::WorldScenePlugin,
                game_scene::GameScenePlugin,
                NewAssetLoadingScenePlugin,
                AnimationTestingScenePlugin,
                (
                    CharSelectUiPreviewPlugin,
                    MiniInfoUiPreviewPlugin,
                    MinimapUiPreviewPlugin,
                    ChatUiPreviewPlugin,
                    InventoryUiPreviewPlugin,
                    UnderbarUiPreviewPlugin,
                    AlchemyUiPreviewPlugin,
                    CharacterInfoUiPreviewPlugin,
                    PartyUiPreviewPlugin,
                    NpcDialogUiPreviewPlugin,
                    QuestRewardUiPreviewPlugin,
                    ChoiceConfirmUiPreviewPlugin,
                    WorldMapUiPreviewPlugin,
                    AutoPotionUiPreviewPlugin,
                    StallUiPreviewPlugin,
                ),
                EquipmentsScenePlugin,
                ParticleTestingScenePlugin,
                SkillsScenePlugin,
                DungeonsScenePlugin,
                ProgressPlugin::<SceneState>::new()
                    // Note: To set the scene you want to start with, change it here
                    .with_state_transition(SceneState::Loading, start_scene),
            ));

        // Same `dev_tools` gate the other inspectors use: `DevWindowsVisible`
        // defaults to `true`, so a `run_if` on it alone left this egui window
        // on screen in a plain play session.
        if app
            .world()
            .get_resource::<crate::plugins::config::ClientConfig>()
            .is_some_and(|config| config.dev_tools)
        {
            app.add_plugins(
                StateInspectorPlugin::<SceneState>::default()
                    .run_if(crate::plugins::dev::dev_windows_visible),
            );
        }
    }
}

#[allow(dead_code)]
fn log_scene_state(scene_state: Res<State<SceneState>>) {
    info!("scene state: {:?}", scene_state);
}

/// Run condition for gameplay systems (player movement/animation, follow
/// camera) that must run in the [`SceneState::WorldSandbox`] dev scene, the
/// `GameWorld` in-game scene, and the offline `Skills` test scene.
pub fn in_playable_world(state: Res<State<SceneState>>) -> bool {
    matches!(
        **state,
        SceneState::WorldSandbox
            | SceneState::GameWorld
            | SceneState::Skills
            | SceneState::Dungeons
    )
}

fn detect_start_scene(scene_from_config: &String, scene_from_env: Option<String>) -> SceneState {
    let scene = match scene_from_env {
        None => scene_from_config.to_lowercase(),
        Some(s) => s.to_lowercase(),
    };
    match scene.as_str() {
        "world" => SceneState::WorldSandbox,
        "game" => SceneState::GameWorld,
        "animations" => SceneState::AnimationTesting,
        "ui_testing" => SceneState::UiTesting,
        "asset_loading" => SceneState::AssetLoadTesting,
        "equipments" => SceneState::Equipments,
        "particles" => SceneState::ParticleTesting,
        "skills" => SceneState::Skills,
        "dungeons" => SceneState::Dungeons,
        // Default + unknown route to the v2 login flow; the legacy v1 intro
        // (and its "characters" scene) was removed in #192.
        "intro" | "intro_v2" | _ => SceneState::IntroV2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #372 / audit F7: the dev sandbox is called `WorldSandbox` in the type
    /// system, but its startup token stays `world` — renaming the variant must
    /// not change how `make run world` or `SCENE=world` resolve.
    #[test]
    fn world_token_still_selects_the_sandbox() {
        assert_eq!(
            detect_start_scene(&"world".to_string(), None),
            SceneState::WorldSandbox
        );
        assert_eq!(
            detect_start_scene(&"intro".to_string(), Some("WORLD".to_string())),
            SceneState::WorldSandbox
        );
        assert_eq!(
            detect_start_scene(&"game".to_string(), None),
            SceneState::GameWorld
        );
    }

    /// The sandbox and the production scene are still two distinct states —
    /// this rename does not fold them (that is the other half of F7).
    #[test]
    fn sandbox_and_game_world_stay_distinct() {
        assert_ne!(SceneState::WorldSandbox, SceneState::GameWorld);
    }
}
