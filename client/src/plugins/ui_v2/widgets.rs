use bevy::input_focus::tab_navigation::TabIndex;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::{EditableText, FontSourceTemplate, TextCursorStyle};
use bevy::ui_widgets::Button;

use super::style::{ImageButtonStyle, PasswordEcho, TargetColor};

/// A game-styled image button built on the headless widget button: emits
/// `Activate` on click, visual state handled by `update_image_button_visuals`.
pub fn image_button(style: ImageButtonStyle, width: f32, height: f32) -> impl Scene {
    let ImageButtonStyle {
        normal,
        hover,
        press,
        disable,
    } = style;
    let initial = normal.clone();
    bsn! {
        Button
        Hovered
        Node {
            width: px(width),
            height: px(height),
            justify_content: JustifyContent::Center,
            align_self: AlignSelf::Center,
        }
        ImageNode { image: {initial} }
        ImageButtonStyle { normal: {normal}, hover: {hover}, press: {press}, disable: {disable} }
    }
}

/// Centered label, typically spawned as a child of an [`image_button`].
#[allow(dead_code)]
pub fn button_label(text: &str, font: Handle<Font>) -> impl Scene {
    label(text, font, 16.0)
}

/// Label that starts fully transparent and carries a [`TargetColor`], so the
/// intro's fade systems can fade it in (mirrors the old intro's `text()`).
pub fn label(text: &str, font: Handle<Font>, font_size: f32) -> impl Scene {
    let text = text.to_string();
    bsn! {
        Text({text})
        TextFont { font: FontSourceTemplate::Handle({font}), font_size: {FontSize::Px(font_size)} }
        TextColor(Color::NONE)
        TargetColor(Srgba::WHITE)
        TextLayout::justify(Justify::Center)
        Node {
            justify_content: JustifyContent::Center,
            align_self: AlignSelf::Center,
        }
        Pickable::IGNORE
    }
}

/// Top padding that vertically centers the 12px input text (line box ≈14.4px)
/// in the 20px input fields; `EditableText` has no vertical alignment API.
const INPUT_PAD_TOP: f32 = 3.0;

/// Single-line text input. Click to focus, typing/caret/selection handled by
/// bevy's `EditableTextInputPlugin`.
pub fn text_input(font: Handle<Font>, tab: i32) -> impl Scene {
    bsn! {
        EditableText { visible_lines: {Some(1.0)}, allow_newlines: false }
        Node {
            width: percent(100),
            height: percent(100),
            padding: {UiRect::top(Val::Px(INPUT_PAD_TOP))},
        }
        TextFont { font: FontSourceTemplate::Handle({font}), font_size: {FontSize::Px(12.0)} }
        TextColor(Color::WHITE)
        TargetColor(Srgba::WHITE)
        TextCursorStyle { color: Color::WHITE }
        TabIndex({tab})
    }
}

/// Single-line password input: the editable text renders transparent glyphs
/// (caret stays visible) while a sibling overlay shows asterisks. `M` is a
/// marker component placed on the inner `EditableText` entity so callers can
/// query the value.
pub fn password_input<M: Component + Default + Clone + Unpin>(
    font: Handle<Font>,
    tab: i32,
) -> impl Scene {
    let echo_font = font.clone();
    bsn! {
        Node { width: percent(100), height: percent(100) }
        Children [
            (
                M
                EditableText { visible_lines: {Some(1.0)}, allow_newlines: false }
                Node {
                    width: percent(100),
                    height: percent(100),
                    padding: {UiRect::top(Val::Px(INPUT_PAD_TOP))},
                }
                TextFont { font: FontSourceTemplate::Handle({font}), font_size: {FontSize::Px(12.0)} }
                TextColor(Color::NONE)
                TextCursorStyle { color: Color::WHITE }
                TabIndex({tab})
            ),
            (
                PasswordEcho
                Text("")
                Node {
                    position_type: PositionType::Absolute,
                    left: px(0),
                    top: px(0),
                    width: percent(100),
                    height: percent(100),
                    padding: {UiRect::top(Val::Px(INPUT_PAD_TOP))},
                }
                TextFont { font: FontSourceTemplate::Handle({echo_font}), font_size: {FontSize::Px(12.0)} }
                TextColor(Color::WHITE)
                // fade with the rest of the screen; the inner EditableText
                // must NOT get a TargetColor (its glyphs stay transparent so
                // the plaintext password is never shown)
                TargetColor(Srgba::WHITE)
                Pickable::IGNORE
            ),
        ]
    }
}
