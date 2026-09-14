use bevy::asset::HandleTemplate;
use bevy::prelude::*;

/// Image handles for the four visual states of an image button.
///
/// The original's shared button ships `com_button`, `com_button_focus`,
/// `com_button_press` and `com_button_disable`; there is no `_hover` art, so
/// our `hover` slot is loaded from `_focus` at every call site. `disable` may
/// be left unset (`Handle::default()`) for call sites whose art has no
/// `_disable` frame — `update_image_button_visuals` then falls back
/// to `normal` and says so once in the log, rather than silently.
#[derive(Component, Default, Clone, FromTemplate)]
pub struct ImageButtonStyle {
    #[template(HandleTemplate<Image>)]
    pub normal: Handle<Image>,
    #[template(HandleTemplate<Image>)]
    pub hover: Handle<Image>,
    #[template(HandleTemplate<Image>)]
    pub press: Handle<Image>,
    #[template(HandleTemplate<Image>)]
    pub disable: Handle<Image>,
}

/// Sound played when the button is pressed.
#[derive(Component, Default, Clone, FromTemplate)]
pub struct ButtonSound(#[template(HandleTemplate<AudioSource>)] pub Handle<AudioSource>);

/// Marker for the `Text` overlay that mirrors a sibling password
/// [`bevy::text::EditableText`] as asterisks. bevy_text 0.19 does not support
/// password masking natively yet, so the real input renders its glyphs
/// transparently (caret stays visible) and this overlay shows the mask.
#[derive(Component, Default, Clone)]
pub struct PasswordEcho;

/// Color a faded-in text should end up with; used by the intro v2 fade
/// systems (the analogue of the old `TextTargetColor`).
#[derive(Component, Default, Clone)]
pub struct TargetColor(pub Srgba);
