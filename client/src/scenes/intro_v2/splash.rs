use bevy::prelude::*;

use super::assets::IntroV2Assets;
use super::IntroV2State;

/// Root marker of the splash screen (big logo, click to continue).
#[derive(Component, Default, Clone)]
pub struct SplashRoot;

/// `GDR_STA_BIGLOGO`'s rect in `pstitle.txt` §`Title` (`:857`) is
/// `360,448,896,300` in the original's 1600x1200 design space — i.e. the art at
/// its native 896x300, centred: the rect's midpoint is `(808, 598)` against the
/// screen centre `(800, 600)`. See `docs/re/ui/scene-intro-splash.md` §3 and
/// `docs/re/ui/scene-intro-login-form.md:19`.
const BIG_LOGO_WIDTH: f32 = 896.0;
const BIG_LOGO_HEIGHT: f32 = 300.0;

pub fn splash_logo(assets: &IntroV2Assets) -> impl Scene {
    let logo = assets.logo_big.clone();
    bsn! {
        SplashRoot
        Name("Splash Logo V2")
        Node {
            flex_direction: FlexDirection::Row,
            position_type: PositionType::Absolute,
            width: {px(BIG_LOGO_WIDTH)},
            height: {px(BIG_LOGO_HEIGHT)},
            margin: {UiRect::all(Val::Auto)},
        }
        Pickable::IGNORE
        Children [
            (
                ImageNode { image: {logo} }
                Pickable::IGNORE
            ),
        ]
    }
}

/// How the original advanced splash -> login is **UNKNOWN** — no string or
/// control for it was found in the data (`docs/re/ui/scene-intro-splash.md` §9).
/// Advancing on a left click is therefore an openroad affordance, not a
/// reproduction.
pub fn on_splash_click(
    mut next_state: ResMut<NextState<IntroV2State>>,
    buttons: Res<ButtonInput<MouseButton>>,
) {
    if buttons.just_pressed(MouseButton::Left) {
        next_state.set(IntroV2State::LoginForm);
    }
}
