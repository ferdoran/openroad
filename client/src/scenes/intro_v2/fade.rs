use std::time::Duration;

use bevy::color::ColorToComponents;
use bevy::prelude::*;
use bevy_tweening::{Delay, EaseMethod, Lens, RepeatStrategy, Tween, TweenAnim};

use crate::plugins::ui_v2::style::TargetColor;
use crate::util::tweening_ext::color_lens::ColorLens;
use crate::util::tweening_ext::visibility_toggle_lens::VisibilityLens;

use super::IntroV2State;

/// Marker for the fullscreen fade-to-black overlay.
#[derive(Component, Default, Clone)]
pub struct FadeScreenV2;

/// Tween lens fading a node's [`BackgroundColor`].
pub struct BackgroundColorLens {
    pub start: Vec4,
    pub end: Vec4,
}

impl Lens<BackgroundColor> for BackgroundColorLens {
    fn lerp(&mut self, mut target: Mut<BackgroundColor>, ratio: f32) {
        let color = self.start + (self.end - self.start) * ratio;
        target.0 = Srgba::from_vec4(color).into();
    }
}

/// While this resource exists the screen is fading to black; when the timer
/// finishes the given sub-state is entered (at the fade's darkest point).
#[derive(Resource)]
pub struct FadeToBlackTimer {
    timer: Timer,
    next: Option<IntroV2State>,
}

impl FadeToBlackTimer {
    pub fn to(next: IntroV2State) -> Self {
        Self {
            timer: Timer::from_seconds(1.0, TimerMode::Once),
            next: Some(next),
        }
    }
}

/// Fade to black and back over 2 seconds, switching to the next state at the
/// darkest point.
#[derive(Message)]
pub struct FadeToBlack;

pub fn fade_screen() -> impl Scene {
    bsn! {
        FadeScreenV2
        Name("Fade Screen V2")
        Node {
            width: percent(100),
            height: percent(100),
        }
        GlobalZIndex(100)
        BackgroundColor(Color::NONE)
        Pickable::IGNORE
    }
}

pub fn on_fade_to_black(
    query: Query<Entity, With<FadeScreenV2>>,
    mut reader: MessageReader<FadeToBlack>,
    mut commands: Commands,
) {
    if reader.read().next().is_none() {
        return;
    }

    let Ok(fade_entity) = query.single() else {
        return;
    };

    // 1s to black + 1s back; the 1s FadeToBlackTimer switches the sub-state
    // exactly at the fully-black peak.
    let tween = Tween::new::<BackgroundColor, _>(
        EaseMethod::EaseFunction(EaseFunction::Linear),
        Duration::from_secs(1),
        BackgroundColorLens {
            start: Color::NONE.to_srgba().to_vec4(),
            end: Color::BLACK.to_srgba().to_vec4(),
        },
    )
    .with_repeat_strategy(RepeatStrategy::MirroredRepeat)
    .with_repeat_count(2);

    commands.entity(fade_entity).insert(TweenAnim::new(tween));
}

/// Makes the screen root marked with `M` visible and fades all of its
/// image/text descendants in (250ms delay so a simultaneous
/// [`hide_screen`] of the previous screen finishes first, then 250ms fade,
/// matching the old intro's timing).
pub fn show_screen<M: Component>(
    roots: Query<Entity, With<M>>,
    mut visibilities: Query<&mut Visibility>,
    children_query: Query<&Children>,
    images: Query<(), With<ImageNode>>,
    text_targets: Query<&TargetColor>,
    mut commands: Commands,
) {
    for root in roots.iter() {
        if let Ok(mut visibility) = visibilities.get_mut(root) {
            *visibility = Visibility::Visible;
        }

        for entity in std::iter::once(root).chain(children_query.iter_descendants(root)) {
            if images.contains(entity) {
                let tween =
                    Delay::new(Duration::from_millis(250)).then(Tween::new::<ImageNode, _>(
                        EaseMethod::EaseFunction(EaseFunction::SineInOut),
                        Duration::from_millis(250),
                        ColorLens::fade_opaque(),
                    ));
                commands.entity(entity).insert(TweenAnim::new(tween));
            }
            if let Ok(target) = text_targets.get(entity) {
                let tween =
                    Delay::new(Duration::from_millis(250)).then(Tween::new::<TextColor, _>(
                        EaseMethod::EaseFunction(EaseFunction::SineInOut),
                        Duration::from_millis(250),
                        ColorLens {
                            start: Color::NONE.to_srgba().to_vec4(),
                            end: target.0.to_vec4(),
                        },
                    ));
                commands.entity(entity).insert(TweenAnim::new(tween));
            }
        }
    }
}

/// Fades all image/text descendants of the screen root marked with `M` out
/// over 250ms and then hides the root.
pub fn hide_screen<M: Component>(
    roots: Query<Entity, With<M>>,
    children_query: Query<&Children>,
    images: Query<(), With<ImageNode>>,
    text_targets: Query<&TargetColor>,
    mut commands: Commands,
) {
    for root in roots.iter() {
        for entity in std::iter::once(root).chain(children_query.iter_descendants(root)) {
            if images.contains(entity) {
                let tween = Tween::new::<ImageNode, _>(
                    EaseMethod::EaseFunction(EaseFunction::SineInOut),
                    Duration::from_millis(250),
                    ColorLens::fade_transparent(),
                );
                commands.entity(entity).insert(TweenAnim::new(tween));
            }
            if let Ok(target) = text_targets.get(entity) {
                let tween = Tween::new::<TextColor, _>(
                    EaseMethod::EaseFunction(EaseFunction::SineInOut),
                    Duration::from_millis(250),
                    ColorLens {
                        start: target.0.to_vec4(),
                        end: Color::NONE.to_srgba().to_vec4(),
                    },
                );
                commands.entity(entity).insert(TweenAnim::new(tween));
            }
        }

        let hide = Tween::new::<Visibility, _>(
            EaseMethod::EaseFunction(EaseFunction::SineInOut),
            Duration::from_millis(250),
            VisibilityLens::hidden(),
        );
        commands.entity(root).insert(TweenAnim::new(hide));
    }
}

pub fn on_fade_timer_finished(
    time: Res<Time>,
    mut fade_timer: ResMut<FadeToBlackTimer>,
    mut next_state: ResMut<NextState<IntroV2State>>,
    mut commands: Commands,
) {
    if fade_timer.timer.tick(time.delta()).just_finished() {
        if let Some(next) = fade_timer.next {
            next_state.set(next);
        }
        commands.remove_resource::<FadeToBlackTimer>();
    }
}
