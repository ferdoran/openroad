//! The vanilla **small popup** shell: an `mframe_wnd_` framed 214x245 window
//! with a caption and a close (X), and nothing else.
//!
//! Idea: this shell is not the System window's — the System window is merely
//! its first user. `resinfo/ifcompositeitemwnd.txt` is a **verbatim copy** of
//! `ifsystemwnd.txt`'s shell: same root rect, same art, and the same child ids
//! (6 for the inner frame, 5 for the background tile) carrying byte-identical
//! rects. Four further popups in the corpus (`ui-newitemslot-popup`,
//! `ui-eventgate-window`, `ui-change-player-model-window`, `ui-gacha-window`)
//! are the same shell with different content. So the shell lives here once and
//! callers supply only their content — the same fix as `hud::game_window`'s
//! shared `CONTENT_TOP`, made *before* four sites hand-derive it rather than
//! after (#310/#313/#314).
//!
//! What this module deliberately does **not** do is build any interior. The
//! composite window's content is code-composed in the original and no
//! descriptor holds it, so an interior specified today would be an invented
//! value (#542).

use bevy::ecs::relationship::RelatedSpawnerCommands;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::Button;

use crate::plugins::ui_v2::style::ImageButtonStyle;

/// Window geometry, verbatim from `resinfo/ginterface.txt:306`
/// (`GDR_SYSTEM:CIFSystemWnd`, ID 98 at :305): `Rect=RECT,"300,200,214,245"`,
/// authored against the vanilla 1024x768 canvas — and byte-identical on
/// `GDR_COMPOSITE_ITEM` (`:1541-1550`).
///
/// Only the **extent** is applied. 214x245 has two independent witnesses inside
/// the window itself — 17 + 180 + 17 ([`INNER_FRAME_RECT`]) and 31 + 152 + 31
/// (the System window's button column) — but the `300,200` origin has none:
/// the byte-identical rect on a second, unrelated window is what shows it is a
/// copied template block rather than a placement decision. Applying it as raw
/// pixels would regress the placement too: this tree has no design-space
/// mapping (`plugins::ui` pins the UI scale factor to 1.0) and ships 1920x1080,
/// where a 1024x768-space origin that is near-centre lands in the upper-left
/// quadrant. No other window root here uses its ginterface origin either. So
/// the popup stays centred until the rescale rule is settled
/// (`docs/re/ui/wndpos-persistence.md` U4).
pub(crate) const WINDOW_RECT: (f32, f32, f32, f32) = (300.0, 200.0, 214.0, 245.0);
pub(crate) const WINDOW_W: f32 = WINDOW_RECT.2;
pub(crate) const WINDOW_H: f32 = WINDOW_RECT.3;

/// The shell's inner frame, id **6** in both trees (`ifsystemwnd.txt:110`,
/// `ifcompositeitemwnd.txt:6`), `int_window_` art. Recorded, not yet drawn:
/// the System window draws its buttons straight on the background, and
/// nothing in the corpus says what the composite window puts inside it.
#[allow(dead_code)]
pub(crate) const INNER_FRAME_RECT: (f32, f32, f32, f32) = (17.0, 45.0, 180.0, 185.0);
/// The shell's background tile rect, id **5** in both trees
/// (`ifsystemwnd.txt:120`, `ifcompositeitemwnd.txt:25`), `com_bg_tile_b`.
#[allow(dead_code)]
pub(crate) const INNER_BG_RECT: (f32, f32, f32, f32) = (33.0, 82.0, 148.0, 110.0);

/// Caption offset inside the `mframe_wnd_mid_up` art: its dark title band runs
/// rows 6..28 with a bright double-line separator at 29..32, so a 13px line sits
/// at y=11. This is the shared chrome's own value (`hud::game_window`'s
/// `TITLE_Y` 6 + its 5px inset), which `docs/re/ui/hud-game-window-chrome.md` §6
/// verifies byte-exact; at the previous 18 the caption crossed the separator.
const TITLE_TOP: f32 = 11.0;
const TITLE_FONT_SIZE: f32 = 13.0;
/// Title colour: `ginterface.txt:302` `FontColor=COLOR,"255,255,255,255"`.
/// resinfo colours are ARGB (see `hud::chat::ui`), so this is pure white.
const TITLE_COLOR: Color = Color::WHITE;

// The window frame is a 9-slice built from 8 `mframe_wnd_*` border pieces around
// a tiled centre (`com_bg_tile_b`). Border track sizes are the texture
// dimensions: left/right columns 40px, top row 68px, bottom row 48px.
const FRAME_DIR: &str = "media://interface/frame/";
const BG_TILE_DDJ: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_b.ddj";
const FRAME_L: f32 = 40.0;
const FRAME_R: f32 = 40.0;
const FRAME_T: f32 = 68.0;
const FRAME_B: f32 = 48.0;

const CLOSE_DDJ: &str = "media://interface/ifcommon/com_windowclose.ddj";
const CLOSE_FOCUS_DDJ: &str = "media://interface/ifcommon/com_windowclose_focus.ddj";
const CLOSE_PRESS_DDJ: &str = "media://interface/ifcommon/com_windowclose_press.ddj";
/// Native extent of the close art in all three states (16x16 DDS headers).
const CLOSE_SIZE: f32 = 16.0;
const CLOSE_INSET: f32 = 14.0;

/// The entities a spawned popup hands back to its owner.
pub(crate) struct SmallPopup {
    /// Whole-window node. Insert your marker here and add your content as
    /// children — the shell places nothing inside the frame.
    pub root: Entity,
    /// The (X) on the title band; attach your own `Activate` observer, because
    /// how a window closes is the window's business, not the shell's.
    pub close_button: Entity,
}

/// Build the shell: centred root, 9-slice frame, caption, close button.
pub(crate) fn spawn_small_popup(
    commands: &mut Commands,
    asset_server: &AssetServer,
    camera: Entity,
    title: &str,
    font: &Handle<Font>,
) -> SmallPopup {
    let close_style = ImageButtonStyle {
        normal: asset_server.load(CLOSE_DDJ),
        hover: asset_server.load(CLOSE_FOCUS_DDJ),
        press: asset_server.load(CLOSE_PRESS_DDJ),
        ..Default::default()
    };
    let mut close_button = Entity::PLACEHOLDER;

    let root = commands
        .spawn((
            Name::from(format!("{title} Window")),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(WINDOW_W),
                height: Val::Px(WINDOW_H),
                margin: UiRect::all(Val::Auto), // centers on screen; see WINDOW_RECT
                ..default()
            },
            GlobalZIndex(150),
            UiTargetCamera(camera),
        ))
        .with_children(|w| {
            // Frame first so it sits behind the caption and the content.
            spawn_frame(w, asset_server);
            w.spawn((
                Text::new(title.to_string()),
                TextFont {
                    font: font.clone().into(),
                    font_size: FontSize::Px(TITLE_FONT_SIZE),
                    ..default()
                },
                TextColor(TITLE_COLOR),
                TextLayout::justify(Justify::Center),
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(TITLE_TOP),
                    width: Val::Percent(100.0),
                    ..default()
                },
                Pickable::IGNORE,
            ));
            close_button = w
                .spawn((
                    Button,
                    Hovered::default(),
                    Node {
                        position_type: PositionType::Absolute,
                        right: Val::Px(CLOSE_INSET),
                        top: Val::Px(CLOSE_INSET),
                        width: Val::Px(CLOSE_SIZE),
                        height: Val::Px(CLOSE_SIZE),
                        ..default()
                    },
                    ImageNode::new(close_style.normal.clone()),
                    close_style,
                ))
                .id();
        })
        .id();

    SmallPopup { root, close_button }
}

/// The 9-slice window frame: 8 `mframe_wnd_*` border pieces laid out in a 3x3
/// CSS grid (corners fixed to their texture size, edges stretched along one
/// axis) around a stretched `com_bg_tile_b` centre. `pub(crate)`: the options
/// window reuses the same vanilla frame at its own size.
pub(crate) fn spawn_frame(
    parent: &mut RelatedSpawnerCommands<ChildOf>,
    asset_server: &AssetServer,
) {
    let piece = |name: &str| asset_server.load::<Image>(format!("{FRAME_DIR}{name}.ddj"));
    parent
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                display: Display::Grid,
                grid_template_columns: vec![
                    RepeatedGridTrack::px(1, FRAME_L),
                    RepeatedGridTrack::flex(1, 1.0),
                    RepeatedGridTrack::px(1, FRAME_R),
                ],
                grid_template_rows: vec![
                    RepeatedGridTrack::px(1, FRAME_T),
                    RepeatedGridTrack::flex(1, 1.0),
                    RepeatedGridTrack::px(1, FRAME_B),
                ],
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|g| {
            // Row-major: TL, T, TR, L, centre, R, BL, B, BR.
            slice(g, piece("mframe_wnd_left_up"));
            slice(g, piece("mframe_wnd_mid_up"));
            slice(g, piece("mframe_wnd_right_up"));
            slice(g, piece("mframe_wnd_left_side"));
            slice(g, asset_server.load::<Image>(BG_TILE_DDJ));
            slice(g, piece("mframe_wnd_right_side"));
            slice(g, piece("mframe_wnd_left_down"));
            slice(g, piece("mframe_wnd_mid_down"));
            slice(g, piece("mframe_wnd_right_down"));
        });
}

/// One grid cell of the frame: an image stretched to fill its cell.
fn slice(grid: &mut RelatedSpawnerCommands<ChildOf>, image: Handle<Image>) {
    grid.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        ImageNode {
            image,
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Pickable::IGNORE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shell's arithmetic closes on its own numbers, in both directions —
    /// which is what makes 214x245 sourced rather than a magic pair.
    /// `17 + 180 + 17 = 214` across, `45 + 185 + 15 = 245` down.
    #[test]
    fn the_shell_extent_is_witnessed_by_its_own_children() {
        let (fx, fy, fw, fh) = INNER_FRAME_RECT;
        assert_eq!(fx + fw + fx, WINDOW_W);
        assert_eq!(fy + fh + 15.0, WINDOW_H);
        // the background tile sits inside the inner frame
        let (bx, by, bw, bh) = INNER_BG_RECT;
        assert!(bx >= fx && bx + bw <= fx + fw);
        assert!(by >= fy && by + bh <= fy + fh);
    }

    /// The caption must sit inside `mframe_wnd_mid_up`'s dark band (rows 6..28),
    /// not across the bright separator at 29..32 that a 13px line at the old
    /// y=18 reached into. Moved here with the shell: every popup on this
    /// template inherits the answer instead of re-deriving it.
    #[test]
    fn title_sits_inside_the_frame_caption_band() {
        assert_eq!(TITLE_TOP, 11.0);
        let line_bottom = TITLE_TOP + TITLE_FONT_SIZE * 1.2; // default line box
        assert!(
            line_bottom < 29.0,
            "caption bottom {line_bottom} must clear the separator at 29"
        );
        // resinfo COLOR is ARGB, so 255,255,255,255 is plain white
        assert_eq!(TITLE_COLOR, Color::WHITE);
    }

    /// `ifcompositeitemwnd.txt` is a verbatim copy of `ifsystemwnd.txt`'s
    /// shell: same root rect, same child ids, byte-identical child rects. That
    /// is the whole finding of #542 — there is no second window to build, only
    /// a template with a second user.
    #[test]
    fn the_composite_window_is_this_shell_with_no_content_of_its_own() {
        // ginterface.txt:306 (GDR_SYSTEM) and :1541 (GDR_COMPOSITE_ITEM)
        assert_eq!(WINDOW_RECT, (300.0, 200.0, 214.0, 245.0));
        // ifsystemwnd.txt:110 / ifcompositeitemwnd.txt:6 — both id 6
        assert_eq!(INNER_FRAME_RECT, (17.0, 45.0, 180.0, 185.0));
        // ifsystemwnd.txt:120 / ifcompositeitemwnd.txt:25 — both id 5
        assert_eq!(INNER_BG_RECT, (33.0, 82.0, 148.0, 110.0));
    }
}
