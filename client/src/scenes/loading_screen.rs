use bevy::app::{App, Plugin};
use bevy::asset::{AssetServer, Handle, UntypedHandle};
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::default;
use bevy::prelude::*;
use bevy::ui::{PositionType, Val};
use iyes_progress::ProgressTracker;
use rand::Rng;

use crate::plugins::camera::CameraLayers;
use crate::scenes::SceneState;
use crate::GameState;

pub struct LoadingScenePlugin;

impl Plugin for LoadingScenePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(LoadingScreen::new())
            .add_systems(OnEnter(SceneState::Loading), setup_loading_screen)
            .add_systems(
                OnExit(SceneState::Loading),
                (
                    teardown_loading_screen,
                    adjust_game_state.run_if(in_state(GameState::Loading)),
                ),
            )
            .add_systems(
                Update,
                loading_progress.run_if(in_state(SceneState::Loading)),
            )
            .add_systems(Update, loaded_system.run_if(in_state(GameState::Loaded)));
    }
}

/// Design space of the loading chrome. The rects below are authored in
/// **1600x1200** (`ginterface.txt:268` gives `GDR_LOADING` `Rect="0,0,1600,1200"`,
/// and the caption's `top=1025 + 35` already overruns a 768-tall canvas), while
/// every background art measures **1024x768** and is stretched to fill. Mixing
/// the two spaces is the trap this screen is built around: transcribing these
/// rects into a 1024x768 canvas mislays the gauge (y=985) and the caption
/// (y=1025) off-screen.
const DESIGN: (f32, f32) = (1600.0, 1200.0);

/// `GDR_LOADINGFRAME:CIFStatic` id 23, `Rect="241,973,1121,64"`,
/// `loading_form.ddj` (art 720x40, stretched) — `pscharacterselect.txt:345`.
const FRAME_RECT: (f32, f32, f32, f32) = (241.0, 973.0, 1121.0, 64.0);
/// `GDR_LOADINGG:CIFGauge` id 24, `Rect="268,985,1064,20"`,
/// `gauge_loading.ddj` — `pscharacterselect.txt:326`.
const GAUGE_RECT: (f32, f32, f32, f32) = (268.0, 985.0, 1064.0, 20.0);
/// `GDR_LOADING_STA:CIFStatic` id 27, `Rect="268,1025,252,35"`,
/// `nowloading.ddj` — `pscharacterselect.txt:307`. The caption is **baked art**
/// (144x20, stretched into the 252x35 rect), not a string: there is no
/// "Now Loading" key anywhere in `textuisystem.txt`.
const CAPTION_RECT: (f32, f32, f32, f32) = (268.0, 1025.0, 252.0, 35.0);

/// The gauge art is a 4x12 **cross-section**: all four columns are byte-
/// identical while the twelve rows form a vertical gold gradient, so the fill
/// runs left to right and the art is stretched along X. Its native height is
/// 12 and the authored rect is 20 — the rect wins, deliberately.
const GAUGE_DDJ: &str = "media://interface/loading/gauge_loading.ddj";
const FRAME_DDJ: &str = "media://interface/loading/loading_form.ddj";
const CAPTION_DDJ: &str = "media://interface/loading/nowloading.ddj";

/// Design-space rect -> percentage node, so the 1600x1200 layout scales with
/// the window instead of assuming a resolution.
///
/// Public because three surfaces draw this chrome (the intro loading screen,
/// the world-entry overlay in `game_scene`, the board->creation cut in
/// `intro_v2::region_select`) and every one of them that re-derived the
/// geometry got it wrong — see [`spawn_loading_chrome`].
pub fn design_node(rect: (f32, f32, f32, f32)) -> Node {
    let (x, y, w, h) = rect;
    Node {
        position_type: PositionType::Absolute,
        left: Val::Percent(100.0 * x / DESIGN.0),
        top: Val::Percent(100.0 * y / DESIGN.1),
        width: Val::Percent(100.0 * w / DESIGN.0),
        height: Val::Percent(100.0 * h / DESIGN.1),
        ..default()
    }
}

/// Spawns the authored loading chrome — frame, optional gauge, caption art —
/// into `parent`.
///
/// Idea: the chrome is the same three [`design_node`] rects on every loading
/// surface, so it lives once here rather than being transcribed per site.
/// `parent` must be a node that spans the whole window, because each rect is a
/// percentage of the 1600x1200 design space; a letterboxed or pixel-sized
/// container would slide the caption off the frame it sits on.
///
/// `with_gauge` models the *absence* of a progress source rather than a second
/// widget that happens to lack one: the board->creation cut is a fixed-duration
/// cut with nothing to report, so it passes `false`. The boot screen fills the
/// gauge from `iyes_progress` ([`loading_progress`]) and the world-entry overlay
/// from its own readiness gates (`game_scene::dismiss_loading_overlay_when_ready`).
///
/// The caption is deliberately [`CAPTION_DDJ`] art and never a string — there
/// is no "Now Loading" key anywhere in `textuisystem.txt`, so a text
/// substitution can only be an invention (`docs/re/ui/scene-loading.md` §8.4).
pub fn spawn_loading_chrome(
    parent: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    with_gauge: bool,
) {
    parent.spawn((
        design_node(FRAME_RECT),
        ImageNode {
            image: asset_server.load(FRAME_DDJ),
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Name::from("Loading Frame"),
        Pickable::IGNORE,
    ));
    if with_gauge {
        // the gauge fill: a zero-width node inside the authored rect whose
        // width tracks progress (the art stretches along X)
        parent
            .spawn((
                design_node(GAUGE_RECT),
                Name::from("Loading Gauge"),
                Pickable::IGNORE,
            ))
            .with_children(|gauge| {
                gauge.spawn((
                    LoadingBar {
                        loading_progress: LoadingProgress(0.0),
                        loading_bar_bundle: ImageNode {
                            image: asset_server.load(GAUGE_DDJ),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                    },
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(0.0),
                        top: Val::Px(0.0),
                        width: Val::Percent(0.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    Name::from("Progress Bar"),
                    Pickable::IGNORE,
                ));
            });
    }
    parent.spawn((
        design_node(CAPTION_RECT),
        ImageNode {
            image: asset_server.load(CAPTION_DDJ),
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Name::from("Now Loading"),
        Pickable::IGNORE,
    ));
}

fn setup_loading_screen(mut commands: Commands, asset_server: Res<AssetServer>) {
    const BASE_PATH: &str = "media://interface/loading";
    // `GDR_LOADING` (id 27, ginterface.txt:259-277) carries `DDJ=""` — the
    // background art is chosen in code in the original too, so the rotation
    // below is an openroad choice, not a drift.
    const LOADING_BGS: [&str; 5] = [
        "loading_default.ddj",
        "loading_china_1.ddj",
        "loading_hotan.ddj",
        "loading_europe_1.ddj",
        "loading_europe_2.ddj",
    ];

    let mut rng = rand::rng();
    let random_bg_index = rng.random_range(0..LOADING_BGS.len());

    let background_handle: Handle<Image> =
        asset_server.load(format!("{}/{}", BASE_PATH, LOADING_BGS[random_bg_index]));

    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            LoadingBarComp,
            Name::from("Loading Screen"),
            RenderLayers::layer(CameraLayers::LoadingScreen.into()),
        ))
        .with_children(|screen| {
            // 1024x768 art stretched over the whole 1600x1200 layout
            screen.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                ImageNode {
                    image: background_handle,
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
            ));
            spawn_loading_chrome(screen, &asset_server, true);
        });
    info!("initialized loading screen");
}

#[derive(Resource)]
pub struct LoadingScreen {
    pub loading_assets: Vec<UntypedHandle>,
    is_active: bool,
}

#[derive(Bundle)]
pub(crate) struct LoadingBar {
    loading_progress: LoadingProgress,
    loading_bar_bundle: ImageNode,
}

#[derive(Component)]
pub(crate) struct LoadingProgress(f32);

#[derive(Component)]
pub struct LoadingBarComp;

#[allow(dead_code)]
impl LoadingScreen {
    pub fn new() -> LoadingScreen {
        LoadingScreen {
            loading_assets: Vec::new(),
            is_active: false,
        }
    }
    pub fn create(handles: Vec<UntypedHandle>) -> LoadingScreen {
        LoadingScreen {
            loading_assets: handles,
            is_active: false,
        }
    }
    pub fn extend(&mut self, handles: Vec<UntypedHandle>) {
        self.loading_assets.extend(handles);
    }
    pub fn begin(&mut self) {
        self.is_active = true;
    }
    pub fn end(&mut self) {
        self.is_active = false;
    }
    pub fn is_finished(&self) -> bool {
        self.is_active && self.loading_assets.is_empty()
    }
}

fn loaded_system(
    current_state: Res<State<GameState>>,
    mut next_state: ResMut<NextState<GameState>>,
    loading_screen: Res<LoadingScreen>,
) {
    if loading_screen.is_active {
        return;
    }
    if *current_state.get() != GameState::Loaded {
        return;
    }

    info!("all assets are loaded");
    // Pretend we prepare new assets, such as texture atlases

    next_state.set(GameState::Game);
}

fn teardown_loading_screen(mut commands: Commands, query: Query<Entity, With<LoadingBarComp>>) {
    query.iter().for_each(|e| {
        commands.entity(e).despawn();
    });
}

fn loading_progress(
    tracker: Option<Res<ProgressTracker<SceneState>>>,
    mut query: Query<(&mut Node, &mut LoadingProgress), With<LoadingProgress>>,
) {
    if let Some(tracker) = tracker {
        let progress = tracker.get_global_progress();
        if progress.total > 0 {
            let loading_progress = progress.done as f32 / progress.total as f32;
            for (mut node, mut bar) in query.iter_mut() {
                bar.0 = loading_progress;
                trace!("loading progress {}/{}", progress.done, progress.total);
                // the fill is a fraction of the authored gauge rect, so it
                // scales with the window like the rest of the chrome
                node.width = Val::Percent(100.0 * loading_progress);
            }
        }
    }
}

fn adjust_game_state(mut game_state: ResMut<NextState<GameState>>) {
    game_state.set(GameState::Loaded);
}

#[cfg(test)]
mod test {
    use super::*;

    /// The chrome is authored in 1600x1200 while the background art is
    /// 1024x768. The caption is the proof: 1025 + 35 = 1060 is already past a
    /// 768-tall canvas, so transcribing these rects into art space would put
    /// the gauge and the caption off-screen.
    #[test]
    fn the_layout_space_is_1600x1200_not_the_arts_1024x768() {
        assert_eq!(DESIGN, (1600.0, 1200.0));
        assert!(CAPTION_RECT.1 + CAPTION_RECT.3 > 768.0);
        assert!(GAUGE_RECT.1 > 768.0);
        // every authored rect still fits the real canvas
        for rect in [FRAME_RECT, GAUGE_RECT, CAPTION_RECT] {
            assert!(rect.0 + rect.2 <= DESIGN.0);
            assert!(rect.1 + rect.3 <= DESIGN.1);
        }
    }

    /// The three rects are the authored ones, and they nest the way the tree
    /// draws them: the gauge and the caption sit inside/below the frame.
    #[test]
    fn the_chrome_rects_are_the_authored_ones() {
        assert_eq!(FRAME_RECT, (241.0, 973.0, 1121.0, 64.0));
        assert_eq!(GAUGE_RECT, (268.0, 985.0, 1064.0, 20.0));
        assert_eq!(CAPTION_RECT, (268.0, 1025.0, 252.0, 35.0));
        // gauge inside the frame, sharing the caption's x
        assert!(GAUGE_RECT.0 >= FRAME_RECT.0);
        assert!(GAUGE_RECT.0 + GAUGE_RECT.2 <= FRAME_RECT.0 + FRAME_RECT.2);
        assert_eq!(GAUGE_RECT.0, CAPTION_RECT.0);
        // the old hand-tuned 684x14 fill was neither the rect nor the art
        assert_ne!((GAUGE_RECT.2, GAUGE_RECT.3), (684.0, 14.0));
    }

    /// Boots just enough of an app to spawn the chrome under a full-window
    /// root, the way all three loading surfaces do.
    fn chrome_app(with_gauge: bool) -> (App, Entity) {
        let mut app = App::new();
        // `asset_server.load()` spawns an IO task, so the pools must exist
        // before the first load or bevy_tasks panics with "The IoTaskPool has
        // not been initialized yet" — the neighbouring AssetPlugin tests never
        // hit it because they only `add()` assets, never load a path.
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<Image>();
        let root = app
            .world_mut()
            .run_system_cached_with(
                |with_gauge: In<bool>, mut commands: Commands, assets: Res<AssetServer>| {
                    commands
                        .spawn(Node {
                            width: Val::Percent(100.0),
                            height: Val::Percent(100.0),
                            ..default()
                        })
                        .with_children(|parent| {
                            spawn_loading_chrome(parent, &assets, *with_gauge);
                        })
                        .id()
                },
                with_gauge,
            )
            .expect("spawn_loading_chrome failed");
        (app, root)
    }

    fn descendants(app: &App, root: Entity) -> Vec<Entity> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(entity) = stack.pop() {
            if let Some(children) = app.world().get::<Children>(entity) {
                stack.extend(children.iter());
                out.extend(children.iter());
            }
        }
        out
    }

    /// #628: `game_scene` and `region_select` each re-derived this chrome
    /// because the rects were private here, and the creation cut mixed design
    /// spaces — a percentage frame with a `Val::Px(252)` caption, which slides
    /// off the frame at every window size but exactly design scale. The whole
    /// point of the shared helper is that nothing it emits is in pixels.
    #[test]
    fn the_shared_chrome_is_percentage_only() {
        let (app, root) = chrome_app(true);
        let nodes = descendants(&app, root);
        assert!(!nodes.is_empty());
        for entity in nodes {
            let node = app.world().get::<Node>(entity).expect("chrome node");
            // the gauge fill is the one legitimate Val::Px: a zero offset
            // inside its own percentage-sized rect
            for val in [node.width, node.height] {
                assert!(
                    !matches!(val, Val::Px(_)),
                    "{entity} sizes the chrome in pixels: {val:?}"
                );
            }
        }
    }

    /// The caption is baked art, never a string — there is no "Now Loading"
    /// key in `textuisystem.txt`, so any text here is an invention (#628).
    #[test]
    fn the_caption_is_art_and_the_gauge_is_optional() {
        let (app, root) = chrome_app(false);
        let nodes = descendants(&app, root);
        // frame + caption, no gauge and no gauge fill
        assert_eq!(nodes.len(), 2, "the gaugeless chrome is frame + caption");
        for entity in &nodes {
            assert!(
                app.world().get::<ImageNode>(*entity).is_some(),
                "chrome part {entity} is not art"
            );
            assert!(app.world().get::<Text>(*entity).is_none());
        }

        let (app, root) = chrome_app(true);
        // frame + gauge + fill + caption
        assert_eq!(descendants(&app, root).len(), 4);
    }

    /// Design rects become percentages of the window, not pixels.
    #[test]
    fn design_rects_map_to_percentages() {
        let node = design_node(GAUGE_RECT);
        assert_eq!(node.left, Val::Percent(100.0 * 268.0 / 1600.0));
        assert_eq!(node.top, Val::Percent(100.0 * 985.0 / 1200.0));
        assert_eq!(node.width, Val::Percent(100.0 * 1064.0 / 1600.0));
    }
}
