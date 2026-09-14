//! Reusable in-game window chrome: the vanilla mframe_wnd_* framed window.
//!
//! Idea: every draggable in-game window (inventory, and future ones like
//! storage, skills, party) shares the same shell — the 8-piece mframe_wnd_
//! frame ring with its built-in title band, a com_bg_tile_d interior, a
//! title text, a close (X) button and drag-to-move via the title strip.
//! `spawn_game_window` builds that shell for a given content size and hands
//! back the root (for the caller's marker/visibility management), the content
//! container (for the caller's actual UI) and the close button (for the
//! caller's close behavior via an `Activate` observer).
//!
//! Frame geometry (measured off the decoded textures): all pieces are fully
//! opaque; corners/sides are 40px wide, the top strip is 68px tall and
//! carries the integrated title band (dark bar between gold trim lines at
//! y 6..28, double-line separator at y 29..31), the bottom strip is 48px
//! tall. Past the frame's detailed edge the art is not flat black but a
//! dithered dark maroon/olive, so the content tucks into the ring and only
//! the `FRAME_VIS_*` margins remain visible, with the bg tiling overlaying
//! the ring's flat inner region out to those edges.
//!
//! Three things are caller-supplied through [`GameWindowStyle`] because the
//! data varies per window: the interior tile letter (11 distinct letters
//! corpus-wide), whether there is a caption at all (`GDR_MAINPOPUP` declares
//! an empty one) and whether there is a close button (the bag and equipment
//! trees declare none).

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::Button;

use crate::assets::FontAssets;
use crate::plugins::ui_v2::style::ImageButtonStyle;

/// Art extents of the frame pieces.
const CHROME_SIDE: f32 = 40.0;
const CHROME_TOP: f32 = 68.0;
const CHROME_BOTTOM: f32 = 48.0;
/// The title band inside the top strip (window units, from the art).
const TITLE_Y: f32 = 6.0;
const TITLE_H: f32 = 22.0;
/// Visible frame margins (see the module doc).
pub const FRAME_VIS_SIDE: f32 = 8.0;
/// Top visible edge = the title band plus its separator lines.
pub const FRAME_VIS_TOP: f32 = 32.0;
pub const FRAME_VIS_BOTTOM: f32 = 12.0;
/// Padding between the visible frame edge and the content.
pub const CHROME_PAD: f32 = 4.0;
/// Content offset below the title band.
pub const CONTENT_TOP: f32 = FRAME_VIS_TOP + CHROME_PAD;

/// The 8-piece family this shell draws. It is deliberately **one** hardcoded
/// family and not an `mframe_{name}_` helper: a family name guarantees
/// nothing about the pieces behind it. `mframe_alc_` (4th-gen alchemy) ships
/// seven **4x4 stubs** and packs the entire 376x376 window plate into its
/// `right_up` slot, so a generic 9-slice helper would stretch a 4x4 across
/// three window edges and hide the real art in a corner (#476). Measure every
/// piece before adding a family here.
const CHROME_DIR: &str = "media://interface/frame/mframe_wnd_";
pub const BG_TILE_DDJ: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_d.ddj";

/// The per-window parts of the shell that the data does **not** hold constant.
///
/// Idea: `spawn_game_window` used to hardcode all three, so a caller had no
/// way to say otherwise — the bg tile letter is per-window data (11 distinct
/// letters corpus-wide), `GDR_MAINPOPUP` declares an empty caption, and the
/// bag/equipment trees declare no close button at all. The default is the
/// shell every current caller already draws, so this is a seam, not a
/// behaviour change.
///
/// **The letter is deliberately not a lookup table.** Only equipment's `_d`
/// is independently confirmed (unit doc §9-U5); every other window's letter
/// is UNKNOWN. A caller that moves off `_d` must cite the `DDJ=` line of its
/// own resinfo tree in a comment — inventing a letter here would be exactly
/// the kind of unbacked constant this shell exists to avoid.
pub struct GameWindowStyle {
    /// Interior tile asset. Callers with no located resinfo tree keep the
    /// default.
    pub bg_tile: &'static str,
    /// Whether to spawn the (X). `false` yields
    /// [`GameWindow::close_button`] `== None`, so an unwired page cannot hold
    /// a dangling observer target.
    pub close_button: bool,
}

impl Default for GameWindowStyle {
    fn default() -> Self {
        Self {
            bg_tile: BG_TILE_DDJ,
            close_button: true,
        }
    }
}
/// Native extent of the close art in all three states — the DDS headers of
/// `com_windowclose{,_focus,_press}.ddj` all read 16x16, so drawing it at 14
/// rescaled it.
const CLOSE_SIZE: f32 = 16.0;
/// Caption colour. `mframe_wnd_` blocks declare `FontColor="255,255,255,255"`
/// in 29 of their 31 corpus occurrences (`ifsystemwnd.txt:148` plus 28 in
/// `ginterface.txt`). The two exceptions are `GDR_COMMUNITY`
/// (`ginterface.txt:546`) and `GDR_QUESTINFO` (`:872`), both
/// `FontColor="255,239,153,255"`. resinfo `COLOR` is **A,R,G,B** — see the
/// parser at `assets/resinfo/interface_text/mod.rs:83-91`, and the corpus
/// proves it (`"84,252,122,0"` is a sane alpha-84 orange under ARGB but
/// invisible green under RGBA) — so those two are RGB(239,153,255), a pale
/// violet, *not* gold; gold is the rotated `"255,255,239,153"` (25 uses).
const TITLE_COLOR: Color = Color::srgb_u8(255, 255, 255);

const CLOSE_DDJ: &str = "media://interface/ifcommon/com_windowclose.ddj";
const CLOSE_FOCUS_DDJ: &str = "media://interface/ifcommon/com_windowclose_focus.ddj";
const CLOSE_PRESS_DDJ: &str = "media://interface/ifcommon/com_windowclose_press.ddj";

/// How one frame piece fills its rect.
///
/// The two side pieces carry a vertical motif whose rows repeat every 16 px
/// (byte-identical rows 16 apart in the decoded art), so stretching them to
/// an arbitrary window height distorts a period the artist authored. They
/// tile on Y instead — the same primitive the interior bg already uses. The
/// mid pieces stay stretched: their variation is low-amplitude dither with no
/// period to preserve, and the corners are fixed-size by construction.
fn edge_image_mode(piece: &str, s: f32) -> NodeImageMode {
    match piece {
        "left_side" | "right_side" => NodeImageMode::Tiled {
            tile_x: false,
            tile_y: true,
            stretch_value: s,
        },
        _ => NodeImageMode::Stretch,
    }
}

/// Outer window size (window units) wrapping a content area.
pub fn outer_size(content_size: (f32, f32)) -> (f32, f32) {
    (
        content_size.0 + 2.0 * (FRAME_VIS_SIDE + CHROME_PAD),
        CONTENT_TOP + content_size.1 + CHROME_PAD + FRAME_VIS_BOTTOM,
    )
}

/// Where a window's content sits inside its shell.
///
/// Idea: the shell used to *impose* this — outer size derived from the content
/// size through its own margins. That is right for windows whose resinfo row
/// only declares the content, and wrong for the ones that declare **both**
/// frames: `GDR_NPCWINDOW` is `386x451` with a nested `GDR_NW_NPCTALK` at
/// `11,48,364,391`, which the derived margins cannot express (they land on
/// `388x443`). So a caller with authored numbers passes them in and the shell
/// stops guessing; [`WindowGeometry::from_content`] keeps the old behaviour for
/// everyone else.
#[derive(Clone, Copy)]
pub struct WindowGeometry {
    /// Outer window extent in window units.
    pub outer: (f32, f32),
    /// Content origin inside the window, in window units.
    pub content_at: (f32, f32),
}

impl WindowGeometry {
    /// The shell's own margins around a content area.
    pub fn from_content(content_size: (f32, f32)) -> Self {
        Self {
            outer: outer_size(content_size),
            content_at: (FRAME_VIS_SIDE + CHROME_PAD, CONTENT_TOP),
        }
    }
}

/// A nested 9-slice frame drawn inside the shell, **under** the content — the
/// second frame windows like the NPC conversation declare in their own
/// resinfo file.
#[derive(Clone, Copy)]
pub struct InnerFrame {
    /// Asset path prefix of the 8 pieces, e.g. `media://…/npc_conversation_window_`.
    pub dir: &'static str,
    /// Frame rect in window units.
    pub rect: (f32, f32, f32, f32),
    /// Corner extent of the pieces (square corners).
    pub corner: f32,
}

/// The entities a spawned window shell hands back to its owner.
pub struct GameWindow {
    /// Whole-window node (right/top-anchored). Insert your marker, z-index
    /// and visibility management here.
    pub root: Entity,
    /// Empty container at the content origin — put the window's UI in here.
    pub content: Entity,
    /// The (X) button on the title band; attach an `Activate` observer.
    /// `None` when the shell was spawned with
    /// [`GameWindowStyle::close_button`] `false` — the bag and equipment
    /// trees declare no close button.
    pub close_button: Option<Entity>,
}

impl GameWindow {
    /// The (X) of a shell that was spawned with one.
    ///
    /// For the callers that ask for the default chrome and then wire their
    /// close behaviour: the `None` case is unreachable for them, and making
    /// each of them re-state that would bury the two windows where `None` is
    /// the point.
    pub fn expect_close_button(&self) -> Entity {
        self.close_button
            .expect("window was spawned without a close button")
    }
}

/// The window a title-bar drag moves, stored on the title bar.
#[derive(Component)]
struct ChromeDragTarget(Entity);

/// Drag bookkeeping on the title bar: the root's anchor at drag start.
#[derive(Component)]
struct ChromeDragStart {
    right: f32,
    top: f32,
}

/// Build the framed-window shell. The root is anchored by its right/top
/// corner (`anchor_right_top`, physical px) — the drag observers rely on
/// that anchoring.
#[allow(clippy::too_many_arguments)]
pub fn spawn_game_window(
    commands: &mut Commands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    camera: Entity,
    title: &str,
    content_size: (f32, f32),
    anchor_right_top: (f32, f32),
    scale: f32,
) -> GameWindow {
    spawn_game_window_styled(
        commands,
        asset_server,
        fonts,
        camera,
        title,
        content_size,
        anchor_right_top,
        scale,
        GameWindowStyle::default(),
    )
}

/// [`spawn_game_window`] with the per-window parts spelled out. See
/// [`GameWindowStyle`].
#[allow(clippy::too_many_arguments)]
pub fn spawn_game_window_styled(
    commands: &mut Commands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    camera: Entity,
    title: &str,
    content_size: (f32, f32),
    anchor_right_top: (f32, f32),
    scale: f32,
    style: GameWindowStyle,
) -> GameWindow {
    spawn_game_window_with(
        commands,
        asset_server,
        fonts,
        camera,
        title,
        WindowGeometry::from_content(content_size),
        None,
        anchor_right_top,
        scale,
        style,
    )
}

/// [`spawn_game_window_styled`] with the geometry supplied instead of derived,
/// plus the optional nested frame drawn under the content. This is the full
/// shell; the other two are the common defaults of it.
#[allow(clippy::too_many_arguments)]
pub fn spawn_game_window_with(
    commands: &mut Commands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    camera: Entity,
    title: &str,
    geometry: WindowGeometry,
    inner_frame: Option<InnerFrame>,
    anchor_right_top: (f32, f32),
    scale: f32,
    style: GameWindowStyle,
) -> GameWindow {
    let s = scale;
    let (window_w, window_h) = geometry.outer;
    let piece = |name: &str| asset_server.load::<Image>(format!("{CHROME_DIR}{name}.ddj"));

    let root = commands
        .spawn((
            Name::from(format!("{title} Window")),
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(anchor_right_top.0),
                top: Val::Px(anchor_right_top.1),
                width: Val::Px(window_w * s),
                height: Val::Px(window_h * s),
                ..default()
            },
            UiTargetCamera(camera),
        ))
        .id();

    let mut content = None;
    let mut close_button = None;
    commands.entity(root).with_children(|window| {
        // the 8-piece frame ring: fixed corners, stretched mids and sides,
        // drawn at the art's full extents (which reach under the content).
        // The later-drawn mids/sides overlap the earlier pieces by 1 unit —
        // butt-joined rects can land on fractional pixels at scaled/dragged
        // positions and open a 1px antialiasing seam at the joins; the art
        // is opaque and pattern-continuous there, so the overlap is invisible.
        let sw = CHROME_SIDE;
        let th = CHROME_TOP;
        let bh = CHROME_BOTTOM;
        let edges = [
            // (rect, piece)
            ((0.0, 0.0, sw, th), "left_up"),
            ((sw - 1.0, 0.0, window_w - 2.0 * sw + 2.0, th), "mid_up"),
            ((window_w - sw, 0.0, sw, th), "right_up"),
            ((0.0, th - 1.0, sw, window_h - th - bh + 2.0), "left_side"),
            (
                (window_w - sw, th - 1.0, sw, window_h - th - bh + 2.0),
                "right_side",
            ),
            ((0.0, window_h - bh, sw, bh), "left_down"),
            (
                (sw - 1.0, window_h - bh, window_w - 2.0 * sw + 2.0, bh),
                "mid_down",
            ),
            ((window_w - sw, window_h - bh, sw, bh), "right_down"),
        ];
        for (rect, name) in edges {
            window.spawn((
                abs_node(rect, s),
                ImageNode {
                    image: piece(name),
                    image_mode: edge_image_mode(name, s),
                    ..default()
                },
                Pickable::IGNORE,
            ));
        }

        // interior bg tiling on top of the ring's flat black, out to the
        // visible frame edges
        window.spawn((
            abs_node(
                (
                    FRAME_VIS_SIDE,
                    FRAME_VIS_TOP,
                    window_w - 2.0 * FRAME_VIS_SIDE,
                    window_h - FRAME_VIS_TOP - FRAME_VIS_BOTTOM,
                ),
                s,
            ),
            ImageNode {
                image: asset_server.load(style.bg_tile),
                image_mode: NodeImageMode::Tiled {
                    tile_x: true,
                    tile_y: true,
                    stretch_value: s,
                },
                ..default()
            },
            Pickable::IGNORE,
        ));

        // the title band is part of the top strip's art; this node carries
        // the text, the close button and the drag observers
        window
            .spawn((
                abs_node((0.0, TITLE_Y, window_w, TITLE_H), s),
                Name::from(format!("{title} Title Bar")),
                ChromeDragTarget(root),
            ))
            .observe(on_chrome_drag_start)
            .observe(on_chrome_drag)
            .with_children(|bar| {
                // an empty caption is real data, not a missing string:
                // `GDR_MAINPOPUP` declares one, so it gets no text node
                if !title.is_empty() {
                    bar.spawn((
                        Text::new(title.to_string()),
                        TextFont {
                            font: fonts.two.clone().into(),
                            font_size: FontSize::Px(8.5 * s),
                            ..default()
                        },
                        TextColor(TITLE_COLOR),
                        TextLayout::justify(Justify::Center),
                        Node {
                            position_type: PositionType::Absolute,
                            top: Val::Px(5.0 * s),
                            width: Val::Percent(100.0),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                }
                if style.close_button {
                    let close_style = ImageButtonStyle {
                        normal: asset_server.load(CLOSE_DDJ),
                        hover: asset_server.load(CLOSE_FOCUS_DDJ),
                        press: asset_server.load(CLOSE_PRESS_DDJ),
                        ..Default::default()
                    };
                    close_button = Some(
                        bar.spawn((
                            Button,
                            Hovered::default(),
                            close_button_node(s),
                            ImageNode {
                                image: close_style.normal.clone(),
                                image_mode: NodeImageMode::Stretch,
                                ..default()
                            },
                            close_style,
                        ))
                        .id(),
                    );
                }
            });

        // the nested frame, if the window declares one — under the content
        if let Some(frame) = inner_frame {
            let (fx, fy, fw, fh) = frame.rect;
            let c = frame.corner;
            let inner = |name: &str| asset_server.load::<Image>(format!("{}{name}.ddj", frame.dir));
            // same 1-unit overlap at the joins as the outer ring, for the same
            // antialiasing reason
            let ring = [
                ((fx, fy, c, c), "left_up"),
                ((fx + c - 1.0, fy, fw - 2.0 * c + 2.0, c), "mid_up"),
                ((fx + fw - c, fy, c, c), "right_up"),
                ((fx, fy + c - 1.0, c, fh - 2.0 * c + 2.0), "left_side"),
                (
                    (fx + fw - c, fy + c - 1.0, c, fh - 2.0 * c + 2.0),
                    "right_side",
                ),
                ((fx, fy + fh - c, c, c), "left_down"),
                (
                    (fx + c - 1.0, fy + fh - c, fw - 2.0 * c + 2.0, c),
                    "mid_down",
                ),
                ((fx + fw - c, fy + fh - c, c, c), "right_down"),
            ];
            for (rect, name) in ring {
                window.spawn((
                    abs_node(rect, s),
                    ImageNode {
                        image: inner(name),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            }
        }

        // empty content container for the caller's UI
        content = Some(
            window
                .spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(geometry.content_at.0 * s),
                        top: Val::Px(geometry.content_at.1 * s),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .id(),
        );
    });

    // Opt the window into the Escape chain, here rather than at each of the
    // two dozen call sites.
    //
    // The (X) *is* the closer (`hud::focus`), so a window that has one can be
    // closed by Escape and a window that declares none — the bag and equipment
    // trees — cannot. That is the correct rule and it is already expressed by
    // the data, so deriving it here means no caller can forget to opt in, and
    // Escape can never do something different from clicking the button.
    if let Some(close_button) = close_button {
        commands
            .entity(root)
            .insert(crate::plugins::hud::focus::HudWindow { close_button });
    }

    GameWindow {
        root,
        content: content.expect("content spawned"),
        close_button,
    }
}

/// `(x, y, w, h)` scaled uniformly — the resinfo-rect-to-pixels step shared by
/// all HUD windows.
pub fn scaled(rect: (f32, f32, f32, f32), s: f32) -> (f32, f32, f32, f32) {
    (rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s)
}

/// Absolutely positioned node from a `(x, y, w, h)` resinfo rect at scale `s`.
pub fn abs_node(rect: (f32, f32, f32, f32), s: f32) -> Node {
    let (x, y, w, h) = scaled(rect, s);
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(x),
        top: Val::Px(y),
        width: Val::Px(w),
        height: Val::Px(h),
        ..default()
    }
}

/// The close (X) button's box, right-anchored in the title band. Split out of
/// the spawn so the size can be asserted without running the app.
fn close_button_node(s: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        right: Val::Px(10.0 * s),
        top: Val::Px(4.0 * s),
        width: Val::Px(CLOSE_SIZE * s),
        height: Val::Px(CLOSE_SIZE * s),
        ..default()
    }
}

/// Start a window drag from the title bar: remember the root's anchor.
fn on_chrome_drag_start(
    drag: On<Pointer<DragStart>>,
    targets: Query<&ChromeDragTarget>,
    roots: Query<&Node>,
    mut commands: Commands,
) {
    let Ok(target) = targets.get(drag.entity) else {
        return;
    };
    let Ok(node) = roots.get(target.0) else {
        return;
    };
    let (Val::Px(right), Val::Px(top)) = (node.right, node.top) else {
        return;
    };
    commands
        .entity(drag.entity)
        .insert(ChromeDragStart { right, top });
}

/// Move the window with the pointer (the root is right/top-anchored).
fn on_chrome_drag(
    drag: On<Pointer<Drag>>,
    starts: Query<(&ChromeDragStart, &ChromeDragTarget)>,
    mut roots: Query<&mut Node>,
) {
    let Ok((start, target)) = starts.get(drag.entity) else {
        return;
    };
    let Ok(mut node) = roots.get_mut(target.0) else {
        return;
    };
    node.right = Val::Px(start.right - drag.event.distance.x);
    node.top = Val::Px(start.top + drag.event.distance.y);
}

#[cfg(test)]
mod test {
    use super::*;

    /// The close art is 16x16 in every state (DDS headers of
    /// `interface/ifcommon/com_windowclose{,_focus,_press}.ddj`), so the button
    /// must carry the art's own extent through the HUD scale rather than a
    /// hand-picked size — 14 downscaled it by 12.5% at every consumer.
    #[test]
    fn close_button_carries_its_art_extent_through_the_scale() {
        assert_eq!(CLOSE_SIZE, 16.0);
        let node = close_button_node(1.5);
        assert_eq!(node.width, Val::Px(24.0));
        assert_eq!(node.height, Val::Px(24.0));
    }

    /// `FontColor="255,255,255,255"` on 29 of the 31 `mframe_wnd_` blocks.
    #[test]
    fn title_caption_is_pure_white() {
        assert_eq!(TITLE_COLOR.to_srgba(), Srgba::WHITE);
    }

    /// `left_side`/`right_side` carry a vertical motif that repeats every
    /// 16 px, so stretching them to an arbitrary window height distorts a
    /// period the art authored. The mids keep `Stretch` — their variation is
    /// low-amplitude dither with no period — and the corners are fixed size.
    #[test]
    fn side_pieces_tile_over_their_sixteen_px_motif() {
        for piece in ["left_side", "right_side"] {
            assert!(
                matches!(
                    edge_image_mode(piece, 1.5),
                    NodeImageMode::Tiled {
                        tile_x: false,
                        tile_y: true,
                        stretch_value,
                    } if stretch_value == 1.5
                ),
                "{piece} must tile on Y at the HUD scale"
            );
        }
        for piece in [
            "mid_up",
            "mid_down",
            "left_up",
            "right_up",
            "left_down",
            "right_down",
        ] {
            assert!(
                matches!(edge_image_mode(piece, 1.5), NodeImageMode::Stretch),
                "{piece} must keep stretching"
            );
        }
    }

    /// Escape closes a window by pressing its own (X), and the shell is what
    /// wires that up — so a window with a close button is Escape-closable and
    /// one that declares none is not.
    ///
    /// Pinned at the source level because the alternative is two dozen call
    /// sites each remembering to opt in, which is exactly the per-window
    /// bookkeeping this shell exists to remove. The rule is derived from the
    /// data (`GameWindowStyle::close_button`), not chosen per window.
    #[test]
    fn the_shell_opts_its_window_into_the_escape_chain() {
        let src = include_str!("game_window.rs");
        let build = src
            .split("fn spawn_game_window_with")
            .nth(1)
            .expect("the full shell builder moved");
        assert!(
            build.contains("focus::HudWindow"),
            "the shell no longer marks its root as Escape-closable"
        );
        // ...and only when there is a button to press
        let marker_at = build.find("focus::HudWindow").expect("checked above");
        let guard_at = build
            .find("if let Some(close_button) = close_button")
            .expect("the close-button guard around the opt-in is gone");
        assert!(
            guard_at < marker_at,
            "the opt-in must stay inside the close-button guard: a window with \
             no (X) has nothing for Escape to activate"
        );
    }

    /// The style seam must not change what any existing caller draws: the
    /// default is still `com_bg_tile_d` plus an (X). Only equipment's `_d` is
    /// independently confirmed (unit doc §9-U5), so callers without a located
    /// resinfo tree keep it rather than guessing a letter.
    #[test]
    fn default_style_is_the_shell_every_current_caller_draws() {
        let style = GameWindowStyle::default();
        assert_eq!(style.bg_tile, BG_TILE_DDJ);
        assert!(style.bg_tile.ends_with("com_bg_tile_d.ddj"));
        assert!(style.close_button);
    }
}
