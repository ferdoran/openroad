use bevy::app::{App, Plugin};
use bevy::asset::{Asset, AssetServer, Handle, HandleTemplate, UntypedHandle};
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::default;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy::ui::{Overflow, PositionType, Val};
use iyes_progress::ProgressTracker;
use rand::Rng;

use crate::plugins::camera::CameraLayers;
use crate::scenes::SceneState;
use crate::GameState;

pub struct LoadingScenePlugin;

impl Plugin for LoadingScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(UiMaterialPlugin::<LoadingBackdropMaterial>::default())
            .insert_resource(LoadingScreen::new())
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
            .add_systems(Update, loaded_system.run_if(in_state(GameState::Loaded)))
            // ungated on purpose: the same boxes carry the world-entry
            // overlay, the creation cut and the char-select join overlay,
            // which live in other scenes. Query-only, so it is safe there and
            // in the headless harnesses. `attach_backdrop_material` rides along
            // for the same reason: the backdrop is spawned on all four surfaces
            // and only this plugin owns its material store.
            .add_systems(Update, (fit_design_surfaces, attach_backdrop_material));
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
pub const FRAME_RECT: (f32, f32, f32, f32) = (241.0, 973.0, 1121.0, 64.0);
/// `GDR_LOADINGG:CIFGauge` id 24, `Rect="268,985,1064,20"`,
/// `gauge_loading.ddj` — `pscharacterselect.txt:326`.
pub const GAUGE_RECT: (f32, f32, f32, f32) = (268.0, 985.0, 1064.0, 20.0);
/// `GDR_LOADING_STA:CIFStatic` id 27, `Rect="268,1025,252,35"`,
/// `nowloading.ddj` — `pscharacterselect.txt:307`. The caption is **baked art**
/// (144x20, stretched into the 252x35 rect), not a string: there is no
/// "Now Loading" key anywhere in `textuisystem.txt`.
pub const CAPTION_RECT: (f32, f32, f32, f32) = (268.0, 1025.0, 252.0, 35.0);

/// The gauge art is a 4x12 **cross-section**: all four columns are byte-
/// identical while the twelve rows form a vertical gold gradient, so the fill
/// runs left to right and the art is stretched along X. Its native height is
/// 12 and the authored rect is 20 — the rect wins, deliberately.
const GAUGE_DDJ: &str = "media://interface/loading/gauge_loading.ddj";
const FRAME_DDJ: &str = "media://interface/loading/loading_form.ddj";
const CAPTION_DDJ: &str = "media://interface/loading/nowloading.ddj";

/// Aspect of everything on this screen. The design canvas is 1600x1200 and
/// **all 40** loading backgrounds in `Media/interface/loading/` are 1024x768 —
/// both 4:3. The authored chrome only *looks* right at that ratio, and the data
/// says so rather than taste: the caption rect `268,1025,252,35` has the
/// on-screen aspect `5.4 * (W/H)`, which equals `144/20 = 7.2` — the exact
/// aspect of `nowloading.ddj` — precisely when `W/H = 4/3`.
///
/// DEVIATION, deliberate: the original gives no answer for a non-4:3 window.
/// Stretching art and chrome to the window would squash the painting and the
/// "now loading" strip on every 16:9 screen, so the ratio is kept instead:
/// painting *and* chrome sit in the largest centred 4:3 box that fits
/// (*contain*), which reproduces the authored proportions at any window size.
///
/// Letting the painting *cover* the window instead looks zoomed in: at 21:9
/// cover eats ~44% of the picture, and being uncropped in one axis does not
/// make that the whole painting. Hence the third box: the leftover area is not
/// black bars but the *same* picture blown up to cover, blurred and dimmed
/// ([`BACKDROP_BLUR`], [`BACKDROP_BRIGHTNESS`]), so the painting ends in its
/// own colours. Nothing of the art is lost and nothing is stretched.
pub const DESIGN_ASPECT: f32 = DESIGN.0 / DESIGN.1;

/// How a loading surface maps the 4:3 design space onto the window.
/// [`fit_design_surfaces`] writes the resulting pixel rect every time the
/// window changes; the children stay in percentages of it.
/// A struct with one flag rather than a two-variant enum because the
/// `bsn!` scenes have to spell it too (`DesignFit { cover: true }`), and the
/// macro's enum patches need generated `default_*` constructors the field
/// form does not.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct DesignFit {
    /// `true`: fill the window and crop the overflow — the blurred backdrop.
    /// `false`: largest 4:3 box that fits, centred — the painting and the
    /// authored chrome.
    pub cover: bool,
}

impl DesignFit {
    /// Fill the window, keep 4:3, crop what does not fit.
    pub const COVER: Self = Self { cover: true };
    /// Largest centred 4:3 box inside the window.
    pub const CONTAIN: Self = Self { cover: false };
}

/// Blur radius of the backdrop, as a fraction of the cover box's height.
///
/// Chosen by eye against the real art at 16:9 and 21:9: below ~0.05 the
/// backdrop still reads as a second, wrongly-cropped picture competing with
/// the painting; above ~0.12 it is an even smear that no longer echoes the
/// composition. The value is a fraction, not pixels, so the effect is the same
/// on a 1280 and a 3440 window.
///
/// `assets/shaders/loading_backdrop.wgsl` explains how a radius this wide is
/// gathered in 25 taps without ghosting the paintings' hard silhouettes — and
/// why that method does not depend on the mip chain, which 18 of the 40 vanilla
/// backgrounds do not have.
pub const BACKDROP_BLUR: f32 = 0.09;

/// Brightness of the backdrop, as a **linear** factor (UI shaders work in
/// linear space, the art is sRGB): `0.18` linear is `0.18^(1/2.2) ~= 0.45` of
/// the original as the eye sees it. That is the level at which the painting
/// clearly reads as the foreground while the leftover area still carries its
/// colours instead of going black.
pub const BACKDROP_BRIGHTNESS: f32 = 0.18;

/// The blur-and-dim material of the backdrop
/// (`assets/shaders/loading_backdrop.wgsl` states the idea and the method).
///
/// `settings` is one `Vec4` rather than two `f32` uniforms because a uniform
/// buffer binding is 16-byte aligned anyway — two scalars would occupy the
/// same 16 bytes and need two bindings.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct LoadingBackdropMaterial {
    /// `x` = blur radius in UV, `y` = linear brightness, `zw` unused.
    #[uniform(0)]
    pub settings: Vec4,
    #[texture(1)]
    #[sampler(2)]
    pub art: Handle<Image>,
}

impl UiMaterial for LoadingBackdropMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/loading_backdrop.wgsl".into()
    }
}

/// Marks a node as the blurred backdrop for `art`; [`attach_backdrop_material`]
/// turns it into a [`MaterialNode`].
///
/// Why the indirection instead of spawning the `MaterialNode` directly: the
/// fourth loading surface is a `bsn!` scene
/// (`intro_v2::character_select::join_loading_overlay`) which gets no
/// `Assets<LoadingBackdropMaterial>` to `add()` a material to. A marker that
/// one system resolves keeps all four surfaces on the same code instead of
/// giving that one its own backdrop.
/// A tuple struct, and `Default` + `FromTemplate` are what `bsn!` needs to
/// spell this component in the fourth surface: the macro's pseudo-specialised `FromTemplate` path wants
/// both, and an asset handle only gets one through `HandleTemplate` — the same
/// pattern `ui_v2::style::ButtonSound` uses for its handle field.
#[derive(Component, Clone, Debug, Default, FromTemplate)]
pub struct LoadingBackdrop(#[template(HandleTemplate<Image>)] pub Handle<Image>);

/// Give every [`LoadingBackdrop`] its material, once.
///
/// `Assets<LoadingBackdropMaterial>` is inserted by the `UiMaterialPlugin` this
/// plugin adds, so it exists wherever this system is registered — unlike a
/// scene resource it cannot go missing.
pub fn attach_backdrop_material(
    mut commands: Commands,
    mut materials: ResMut<Assets<LoadingBackdropMaterial>>,
    pending: Query<(Entity, &LoadingBackdrop), Without<MaterialNode<LoadingBackdropMaterial>>>,
) {
    for (entity, backdrop) in pending.iter() {
        let material = materials.add(LoadingBackdropMaterial {
            settings: Vec4::new(BACKDROP_BLUR, BACKDROP_BRIGHTNESS, 0.0, 0.0),
            art: backdrop.0.clone(),
        });
        commands.entity(entity).insert(MaterialNode(material));
    }
}

/// The node a [`DesignFit`] entity starts with: absolutely positioned, sized
/// by [`fit_design_surfaces`]. `Val::Px(0.0)` here is a placeholder, not a
/// layout — the system overwrites all four fields on its first run.
pub fn design_fit_node() -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(0.0),
        top: Val::Px(0.0),
        width: Val::Px(0.0),
        height: Val::Px(0.0),
        ..default()
    }
}

/// The pixel rect a [`DesignFit`] box gets in a `win_w` x `win_h` window,
/// as `(left, top, width, height)`. Centred on both axes; a cover box gets a
/// negative offset on the axis it overflows.
///
/// Pure, because that is where the property worth pinning lives: the ratio of
/// the returned box is [`DESIGN_ASPECT`] whatever the window does, so every
/// percentage rect inside it keeps the aspect it was authored with.
pub fn design_fit_rect(win_w: f32, win_h: f32, fit: DesignFit) -> (f32, f32, f32, f32) {
    let window_is_wider = win_w / win_h > DESIGN_ASPECT;
    // cover: the *other* axis overflows; contain: it is the one that fits
    let match_width = if fit.cover {
        window_is_wider
    } else {
        !window_is_wider
    };
    let (w, h) = if match_width {
        (win_w, win_w / DESIGN_ASPECT)
    } else {
        (win_h * DESIGN_ASPECT, win_h)
    };
    ((win_w - w) / 2.0, (win_h - h) / 2.0, w, h)
}

/// Size every [`DesignFit`] box against the current window.
///
/// A plain `Query`-only system, so it is safe in every scene and in the
/// headless harnesses (no scene resource, nothing to fail parameter
/// validation on) — with no window and no tagged entity it does nothing.
/// Writes are guarded so an idle frame marks no node changed.
pub fn fit_design_surfaces(
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut fitted: Query<(&DesignFit, &mut Node)>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let (win_w, win_h) = (window.resolution.width(), window.resolution.height());
    if win_w <= 0.0 || win_h <= 0.0 {
        return;
    }
    for (fit, mut node) in fitted.iter_mut() {
        let (left, top, w, h) = design_fit_rect(win_w, win_h, *fit);
        // One comparison before the first `DerefMut`: touching any field of a
        // `Mut<Node>` marks the whole component changed, so a per-field guard
        // would still repaint the surface every frame.
        if (node.left, node.top, node.width, node.height)
            != (Val::Px(left), Val::Px(top), Val::Px(w), Val::Px(h))
        {
            node.left = Val::Px(left);
            node.top = Val::Px(top);
            node.width = Val::Px(w);
            node.height = Val::Px(h);
        }
    }
}

/// Design-space rect -> `(left, top, width, height)` in **percent** of the
/// window, so the 1600x1200 layout scales instead of assuming a resolution.
///
/// Split out of [`design_node`] because a fourth surface needs the numbers
/// without the node: the char-select join overlay is a `bsn!` scene
/// (`intro_v2::character_select::join_loading_overlay`), which cannot call a
/// `ChildSpawnerCommands` helper. Sharing the conversion keeps the two
/// renderers from drifting even though they cannot share the spawn code.
pub fn design_pct(rect: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let (x, y, w, h) = rect;
    (
        100.0 * x / DESIGN.0,
        100.0 * y / DESIGN.1,
        100.0 * w / DESIGN.0,
        100.0 * h / DESIGN.1,
    )
}

/// Design-space rect -> percentage node, so the 1600x1200 layout scales with
/// the window instead of assuming a resolution.
///
/// Public because three surfaces draw this chrome (the intro loading screen,
/// the world-entry overlay in `game_scene`, the board->creation cut in
/// `intro_v2::region_select`) and every one of them that re-derived the
/// geometry got it wrong — see [`spawn_loading_chrome`].
pub fn design_node(rect: (f32, f32, f32, f32)) -> Node {
    let (left, top, width, height) = design_pct(rect);
    Node {
        position_type: PositionType::Absolute,
        left: Val::Percent(left),
        top: Val::Percent(top),
        width: Val::Percent(width),
        height: Val::Percent(height),
        ..default()
    }
}

/// Spawns a whole loading surface into `parent`: the blurred backdrop in a
/// [`DesignFit::COVER`] box, then the painting and the authored chrome in a
/// [`DesignFit::CONTAIN`] box.
///
/// Idea: the three boxes keep the screen from stretching with the window and
/// from looking zoomed in — see [`DESIGN_ASPECT`] for the numbers and the stated
/// deviation. `parent` must be a full-window node with
/// `Overflow::clip()`, because the cover box deliberately overflows it on the
/// axis that does not fit.
pub fn spawn_loading_surface(
    parent: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    background: Handle<Image>,
    with_gauge: bool,
) {
    parent.spawn((
        DesignFit::COVER,
        design_fit_node(),
        LoadingBackdrop(background.clone()),
        Name::from("Loading Backdrop"),
        Pickable::IGNORE,
    ));
    parent.spawn((
        DesignFit::CONTAIN,
        design_fit_node(),
        ImageNode {
            image: background,
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Name::from("Loading Background"),
        Pickable::IGNORE,
    ));
    parent
        .spawn((
            DesignFit::CONTAIN,
            design_fit_node(),
            Name::from("Loading Chrome"),
            Pickable::IGNORE,
        ))
        .with_children(|chrome| {
            spawn_loading_chrome(chrome, asset_server, with_gauge);
        });
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
                // the backdrop's cover box overflows this node on the axis
                // that does not fit 4:3; without the clip it would paint
                // outside the window
                overflow: Overflow::clip(),
                ..default()
            },
            LoadingBarComp,
            Name::from("Loading Screen"),
            RenderLayers::layer(CameraLayers::LoadingScreen.into()),
        ))
        .with_children(|screen| {
            // blurred fill in a 4:3 cover box, the 1024x768 painting and the
            // chrome in the 4:3 contain box on top of it
            spawn_loading_surface(screen, &asset_server, background_handle, true);
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

/// Sets every gauge fill in `bars` to `fraction` (clamped to `0..=1`).
///
/// Why this lives here and not at the caller: the fill is *this* module's
/// widget — a zero-width child of the authored `GDR_LOADINGG` rect whose width
/// is a percentage of it (see [`spawn_loading_chrome`]) — and the in-scene
/// loading screen already drives it that way from `iyes_progress`
/// ([`loading_progress`]). The region board has its own progress source (the
/// creation screen's asset load)
/// but must not grow a second answer to "how does a gauge get filled".
pub(crate) fn set_gauge_fraction(
    bars: &mut Query<(&mut Node, &mut LoadingProgress)>,
    fraction: f32,
) {
    let fraction = fraction.clamp(0.0, 1.0);
    for (mut node, mut bar) in bars.iter_mut() {
        bar.0 = fraction;
        node.width = Val::Percent(100.0 * fraction);
    }
}

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

    /// The chrome must keep its authored aspect at any window size.
    ///
    /// What this pins: the caption rect `268,1025,252,35` in the
    /// 1600x1200 canvas has the on-screen aspect `5.4 * (box_w/box_h)`, and
    /// `nowloading.ddj` is 144x20 = 7.2 — so the authored geometry is correct
    /// exactly at 4:3 and at no other ratio. Inside a [`DesignFit`] box the
    /// caption therefore keeps the art's aspect at *any* window size.
    ///
    /// The RED control is the second half of the test: the same arithmetic
    /// against the full window misses 7.2 by 33% at 16:9 and by 79% at 21:9.
    /// Without it a green here would only be saying that 4:3 is 4:3.
    #[test]
    fn the_caption_keeps_the_arts_aspect_in_a_design_box_and_not_in_the_window() {
        const ART_ASPECT: f32 = 144.0 / 20.0; // nowloading.ddj, DDS header
        let caption_aspect = |box_w: f32, box_h: f32| {
            let (_, _, w, h) = design_pct(CAPTION_RECT);
            (w / 100.0 * box_w) / (h / 100.0 * box_h)
        };

        for (win_w, win_h) in [
            (1024.0, 768.0),
            (1280.0, 720.0),
            (1920.0, 1080.0),
            (3440.0, 1440.0),
            (1080.0, 1920.0),
        ] {
            for fit in [DesignFit::COVER, DesignFit::CONTAIN] {
                let (left, top, w, h) = design_fit_rect(win_w, win_h, fit);
                // the box is 4:3 and centred, whatever the window is
                assert!(
                    (w / h - DESIGN_ASPECT).abs() < 1e-3,
                    "{fit:?} {win_w}x{win_h}"
                );
                assert!((left - (win_w - w) / 2.0).abs() < 1e-3);
                assert!((top - (win_h - h) / 2.0).abs() < 1e-3);
                // and the caption drawn in it has the art's proportions
                assert!(
                    (caption_aspect(w, h) - ART_ASPECT).abs() < 1e-2,
                    "{fit:?} at {win_w}x{win_h}: caption aspect {}",
                    caption_aspect(w, h)
                );
            }
            // cover fills the window, contain fits inside it
            let (_, _, cover_w, cover_h) = design_fit_rect(win_w, win_h, DesignFit::COVER);
            assert!(cover_w >= win_w - 1e-3 && cover_h >= win_h - 1e-3);
            let (_, _, fit_w, fit_h) = design_fit_rect(win_w, win_h, DesignFit::CONTAIN);
            assert!(fit_w <= win_w + 1e-3 && fit_h <= win_h + 1e-3);
        }

        // RED control: full-window placement is only right at 4:3
        assert!((caption_aspect(1024.0, 768.0) - ART_ASPECT).abs() < 1e-2);
        assert!(caption_aspect(1920.0, 1080.0) > ART_ASPECT * 1.3);
        assert!(caption_aspect(3440.0, 1440.0) > ART_ASPECT * 1.7);
    }

    /// A covered loading screen looks zoomed in.
    ///
    /// The number that makes cover the wrong answer, and the reason the third
    /// box exists: at 3440x1440 a 4:3 cover box throws away 44% of the
    /// painting, at 16:9 a quarter of it. Contain throws away nothing — it
    /// leaves 44%/25% of the *window* over instead, which is what the blurred
    /// backdrop fills.
    ///
    /// The RED control is the cover column: if someone re-points the painting
    /// at `DesignFit::COVER`, `visible_fraction` is no longer 1.0 and the
    /// first assertion fails.
    #[test]
    fn cover_crops_the_painting_and_contain_does_not() {
        // how much of the 4:3 art survives in a box fitted this way
        let visible_fraction = |win_w: f32, win_h: f32, fit: DesignFit| {
            let (_, _, w, h) = design_fit_rect(win_w, win_h, fit);
            // the visible part is the intersection of box and window
            (w.min(win_w) / w) * (h.min(win_h) / h)
        };

        for (win_w, win_h, cover_crop) in [
            (1600.0, 900.0, 0.25),
            (1920.0, 1080.0, 0.25),
            (3440.0, 1440.0, 0.442),
            // portrait: 4:3 has to grow even further to cover it
            (1080.0, 1920.0, 0.578),
        ] {
            // the painting is drawn whole, whatever the window
            assert!(
                (visible_fraction(win_w, win_h, DesignFit::CONTAIN) - 1.0).abs() < 1e-3,
                "contain crops at {win_w}x{win_h}"
            );
            // what cover costs
            let lost = 1.0 - visible_fraction(win_w, win_h, DesignFit::COVER);
            assert!(
                (lost - cover_crop).abs() < 5e-3,
                "cover at {win_w}x{win_h} crops {lost}, expected {cover_crop}"
            );
            // and that is exactly the share of the window the backdrop fills
            let (_, _, w, h) = design_fit_rect(win_w, win_h, DesignFit::CONTAIN);
            let filled = (w * h) / (win_w * win_h);
            assert!((1.0 - filled - cover_crop).abs() < 5e-3);
        }

        // at 4:3 there is nothing to fill and nothing to crop: the two boxes
        // coincide, so the backdrop is invisible rather than a second look
        assert_eq!(
            design_fit_rect(1024.0, 768.0, DesignFit::COVER),
            design_fit_rect(1024.0, 768.0, DesignFit::CONTAIN)
        );
    }

    /// The surface is backdrop + painting + chrome, the backdrop covers, the
    /// painting is contained, and both show the *same* picture — the backdrop
    /// echoing the painting is the whole idea (`BACKDROP_BLUR`).
    #[test]
    fn the_surface_puts_the_painting_in_the_contain_box_over_a_covering_backdrop() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<Image>()
        .init_asset::<LoadingBackdropMaterial>();

        let root = app
            .world_mut()
            .run_system_cached(|mut commands: Commands, assets: Res<AssetServer>| {
                let art: Handle<Image> = assets.load("media://interface/loading/x.ddj");
                commands
                    .spawn(design_fit_node())
                    .with_children(|parent| {
                        spawn_loading_surface(parent, &assets, art, true);
                    })
                    .id()
            })
            .expect("spawn_loading_surface failed");

        let children: Vec<Entity> = app
            .world()
            .get::<Children>(root)
            .expect("no surface")
            .iter()
            .collect();
        assert_eq!(children.len(), 3, "backdrop + painting + chrome");
        let (backdrop, painting, chrome) = (children[0], children[1], children[2]);

        // the backdrop is the only box that covers, and it is not an ImageNode
        // (its material blurs and dims the art instead)
        assert_eq!(
            app.world().get::<DesignFit>(backdrop),
            Some(&DesignFit::COVER)
        );
        assert!(app.world().get::<ImageNode>(backdrop).is_none());
        assert_eq!(
            app.world().get::<DesignFit>(painting),
            Some(&DesignFit::CONTAIN)
        );
        assert_eq!(
            app.world().get::<DesignFit>(chrome),
            Some(&DesignFit::CONTAIN)
        );

        // same picture in both, or the backdrop stops echoing the painting
        let art_of_backdrop = app
            .world()
            .get::<LoadingBackdrop>(backdrop)
            .expect("marker")
            .0
            .clone();
        let art_of_painting = app
            .world()
            .get::<ImageNode>(painting)
            .expect("painting")
            .image
            .clone();
        assert_eq!(art_of_backdrop, art_of_painting);

        // the chrome is drawn after the painting, i.e. on top of it
        assert!(app.world().get::<Children>(chrome).is_some());

        // and the marker becomes a material exactly once
        app.world_mut()
            .run_system_cached(attach_backdrop_material)
            .expect("attach_backdrop_material failed");
        let material = app
            .world()
            .get::<MaterialNode<LoadingBackdropMaterial>>(backdrop)
            .expect("backdrop has no material")
            .0
            .clone();
        let materials = app.world().resource::<Assets<LoadingBackdropMaterial>>();
        assert_eq!(materials.len(), 1);
        let material = materials.get(&material).expect("material asset");
        assert_eq!(material.settings.x, BACKDROP_BLUR);
        assert_eq!(material.settings.y, BACKDROP_BRIGHTNESS);
        assert_eq!(material.art, art_of_painting);

        // a second run must not add a second material for the same node
        app.world_mut()
            .run_system_cached(attach_backdrop_material)
            .expect("attach_backdrop_material failed");
        assert_eq!(
            app.world()
                .resource::<Assets<LoadingBackdropMaterial>>()
                .len(),
            1
        );
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
