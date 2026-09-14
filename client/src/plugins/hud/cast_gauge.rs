//! The cast/delay gauge — the bar that runs above the under-bar while a return
//! scroll (or another long action) resolves.
//!
//! Idea: this is `GDR_DELAY_GAUGE_BOARD:CIFDelayGaugeBoard`
//! (`ginterface.txt`, id 40, `Rect="416,606,192,112"`) hosting rows described by
//! `resinfo/ifdelayinfo.txt`. **Every rect here is transcribed, not invented** —
//! the archive specifies the whole widget, and the art measures out to match it
//! exactly:
//!
//! ```text
//! board   416,606,192,112     4-unit top inset + up to three 36-tall rows
//!   plate 0,4,192,36          com_casting_window.ddj is 192x36
//!   name  GDR_DI_NAME     0,7,167,12    CIFStatic,  HAlign=1 (centred)
//!   cancel GDR_DI_CANCEL  171,4,20,20   CIFButton,  com_casting_cancel.ddj is 20x20
//!   gauge GDR_DI_GAUGE_DELAY 6,27,184,8 CIFGauge,   com_casting_gauge_*.ddj are 184x8
//! ```
//!
//! The board's x is not arbitrary either: `(1024 - 192) / 2 == 416`, so it is
//! horizontally centred on the vanilla screen, just above the under-bar.
//!
//! **The control rects are board-relative, and the plate sits 4 units down.**
//! That inset is the load-bearing detail, and it is measured rather than
//! assumed. `112 == 4 + 3 * 36`, and decoding `com_casting_window.ddj` puts all
//! three controls exactly 4 below their authored y once the plate is placed at
//! board y=4:
//!
//! ```text
//! control              authored y   where the plate art puts it
//! GDR_DI_CANCEL            4        opaque socket at plate y 0..20, x 167..191
//! GDR_DI_GAUGE_DELAY      27        groove interior at plate y 23..31
//! GDR_DI_NAME              7        translucent band y 0..20, ending at x 167 —
//!                                   exactly where the cancel socket begins
//! ```
//!
//! Treating the plate as the row root instead (which is how this shipped in
//! round 3) hangs the cancel button 3 units below its socket and drops the fill
//! across the groove's lower bevel.
//!
//! `GDR_DI_GAUGE_DELAY` is `Style=0` with a rect equal to its art's pixel size,
//! which is exactly the [`hud::gauge`](crate::plugins::hud::gauge) recipe: fill
//! by **cropping** the art along X, never by resizing it.
//!
//! # The `_bright` art is [U]
//!
//! Three of the six fills ship a `com_casting_gauge_<kind>_bright.ddj` partner
//! at 192x16. **We do not draw it, because we do not know what it is for.** It
//! cannot belong in the gauge: the plate's printed groove is bevelled at plate
//! rows 22 and 32, i.e. a **9-row** channel, and a 16-row overlay spills past
//! both bevels and off the plate's bottom edge. Drawing it over the bar — which
//! is what round 4 did, having read its symmetric alpha falloff as a glow on the
//! fill — produces a second, thicker bar above the real one.
//!
//! Whatever it is (a completion flash across the row is the obvious guess), it
//! stays unwired until something identifies it. Rendering it faithfully would
//! also want an additive blend, which Bevy UI has no path to without a custom
//! material.
//!
//! # Cancelling
//!
//! `0x705B` **TELEPORT / TRANSITION CASTING CANCEL**, empty body — the opcode
//! this module's first version claimed did not exist. It is `[V]` from the
//! original client's own sender (`FUN_0081ee60`, twice), its `0xB05B` reply is
//! `u8 result` plus a `u16` error code when `result == 2`, and the button's
//! label string `UIIT_STT_TRANSITION_CANCEL` sits beside the ack's
//! `UIIT_MSG_TRANSITION_CANCEL_RESULT`. See
//! `docs/re/net/outbound/progression-teleport.md`.
//!
//! One caveat, stated rather than discovered the hard way: **vSRO never writes
//! `0xB05B`**, so a go-sro-derived server will most likely ignore the request.
//! The bar dismisses locally regardless, and [`on_cancel`] logs whether the
//! teleport lands anyway — which is the experiment that settles it.
//!
//! Not to be confused with `0x7074 ObjectActionRequest::Cancel` (byte `02`),
//! which this client already sends: that aborts the *object-action* loop
//! (attacks, skill casts) and has nothing to do with a `0x704C` item cast.
//!
//! The duration is likewise in the archive — see [`CAST_GAUGE_SECONDS`].

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::{Activate, Button};

use packets::agent::prelude::{EntityStateUpdate, TransitionCastingCancelRequest};
use packets::Packet;

use crate::assets::textdata::itemdata::ItemDataRow;
use crate::assets::FontAssets;
use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::game_window::abs_node;
use crate::plugins::hud::gauge::{gauge_crop_node, gauge_fill_width, GaugeArt};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::hud::underbar::cast::UseItemRequest;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::entities::NetworkId;
use crate::plugins::player::Player;
use crate::plugins::textdata::ClientItemData;
use crate::plugins::ui_v2::style::ImageButtonStyle;

/// `GDR_DELAY_GAUGE_BOARD` `Rect="416,606,192,112"` on the vanilla 1024x768
/// screen, held as the board's own size plus its distance from the bottom.
///
/// Anchored bottom-centre rather than at raw `left`/`top` window pixels: the
/// board is horizontally centred (`(1024 - 192) / 2 == 416`, its authored x) and
/// sits `768 - (606 + 112) == 50` above the bottom edge, directly over the
/// under-bar. Pinning it by `top` instead only reproduces that on a window of
/// exactly 1024x768 times [`hud_scale`]; on anything else the bar drifts away
/// from the under-bar it belongs to, which is how it shipped in round 3.
const BOARD: (f32, f32) = (192.0, 112.0);
const BOARD_BOTTOM: f32 = 50.0;

/// One row — the extent of `com_casting_window.ddj`.
const ROW: (f32, f32) = (192.0, 36.0);

/// Where the plate sits inside the board.
///
/// `112 == 4 + 3 * 36`: a 4-unit top inset above three 36-tall rows. **The
/// control rects below are board-relative, not plate-relative**, so the plate
/// has to be pushed down by this much or every control lands 4 units high — see
/// the module docs for the three-way art measurement that fixes it.
const PLATE_TOP: f32 = 4.0;

// The three controls of `ifdelayinfo.txt`'s `Create` section, verbatim. The file
// is one flat section with no sub-container and no `SubSection`, so these rects
// have exactly one parent: the board.
const NAME_RECT: (f32, f32, f32, f32) = (0.0, 7.0, 167.0, 12.0);
const CANCEL_RECT: (f32, f32, f32, f32) = (171.0, 4.0, 20.0, 20.0);
const GAUGE_RECT: (f32, f32, f32, f32) = (6.0, 27.0, 184.0, 8.0);

const ART: &str = "media://interface/ifcommon/";
const PLATE_DDJ: &str = "media://interface/ifcommon/com_casting_window.ddj";

/// `GDR_DI_NAME` `FontColor="255,255,255,255"` — resinfo COLOR is A,R,G,B, so
/// opaque white.
const NAME_COLOR: Color = Color::srgb(1.0, 1.0, 1.0);

/// Fallback cast length, in seconds, for an item whose row does not give one.
///
/// **The real duration is data, not a knob** — see [`item_cast_seconds`]. This
/// only covers a cast we cannot attribute to an item at all.
///
/// It stays 15.4 because that is the shortest *measured* return: a bar that runs
/// out early and waits is a better failure than one that is still crawling when
/// the teleport fires.
pub const CAST_GAUGE_SECONDS: f32 = 15.4;

/// `0x30BF` state kind that announces a cast. Five occurrences in the capture,
/// value `1` every time, each ~30-100 ms before the matching `0xB04C` ack.
///
/// `[S]`: the census in `packets::agent::ingame` names kinds 0/1/4/8; this one
/// is new, and only its *set* transition has ever been observed — there is no
/// `→ 0` and no cast-complete push anywhere in the dump.
const STATE_KIND_CASTING: u8 = 0x0B;

/// itemdata column carrying an item's cast time in **milliseconds**.
///
/// `[V]`, from two independent directions that agree exactly:
///
/// ```text
/// ITEM_ETC_SCROLL_RETURN_01   ref   61   col 119 = 30000   measured 30.27 / 30.12 s
/// ITEM_ETC_SCROLL_RETURN_02   ref 2198   col 119 = 15000   measured 15.40 / 15.32 / 15.15 s
/// ITEM_ETC_SCROLL_RETURN_03          -   col 119 =  5000
/// ITEM_ETC_SCROLL_RETURN_THIEFDEN_01 -   col 119 = 300000
/// ITEM_*_RETURN_SCROLL_HIGH_SPEED    -   col 119 =  1000
/// ```
///
/// The five captured casts split into a 15 s cluster and a 30 s cluster, the
/// two clusters used different bag slots, and the two scrolls' itemdata rows
/// carry exactly 15000 and 30000. The `HIGH_SPEED` rows reading 1000 ms settle
/// the unit independently of the capture.
///
/// **Scoped on purpose.** This is a per-class param slot, not a global column:
/// 9,599 rows carry `-1` and potions carry 0/5/820, all meaningless as times.
/// [`item_cast_seconds`] reads it only for the classes that announce a cast.
const ITEMDATA_CAST_MS_COL: usize = 119;

/// Which fill art a cast draws.
///
/// The archive ships six, one per kind of long action, which is how the
/// original distinguishes them — it picks a different bar rather than tinting
/// one. Three of them also ship a `_bright` partner, which we deliberately do
/// not draw; see the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastKind {
    Return,
    Skill,
    RecallGuild,
    Collection,
    Health,
    Spool,
}

impl CastKind {
    /// The `com_casting_gauge_*` stem for this kind.
    fn stem(self) -> &'static str {
        match self {
            CastKind::Return => "return",
            CastKind::Skill => "skill",
            CastKind::RecallGuild => "recallguild",
            CastKind::Collection => "collection",
            CastKind::Health => "health",
            CastKind::Spool => "spool",
        }
    }

    /// Whether a `_bright` partner exists for this kind. Three of the six ship
    /// one.
    ///
    /// Kept although nothing draws it: it is a *fact about the archive*, and the
    /// test below pins which kinds have one so the next person to identify what
    /// `_bright` is for does not have to re-derive the set. [`bright_art`] is
    /// the path it would load.
    ///
    /// [`bright_art`]: CastKind::bright_art
    fn has_bright(self) -> bool {
        matches!(
            self,
            CastKind::Return | CastKind::Skill | CastKind::RecallGuild
        )
    }

    fn fill_art(self) -> String {
        format!("{ART}com_casting_gauge_{}.ddj", self.stem())
    }

    /// The `_bright` partner's path, for the kinds that have one.
    ///
    /// **Nothing calls this at render time** — see the module docs on why the
    /// overlay is `[U]` and unwired.
    #[cfg_attr(not(test), expect(dead_code, reason = "[U]: see the module docs"))]
    fn bright_art(self) -> Option<String> {
        self.has_bright()
            .then(|| format!("{ART}com_casting_gauge_{}_bright.ddj", self.stem()))
    }
}

/// How long an item's cast runs, from its own itemdata row.
///
/// `None` when the row is missing or its [`ITEMDATA_CAST_MS_COL`] is not a
/// positive millisecond count — which is every item that does not cast, since
/// that column means something different (or nothing) for other classes.
pub fn item_cast_seconds(row: Option<&ItemDataRow>) -> Option<f32> {
    let ms: i64 = row?.0.get(ITEMDATA_CAST_MS_COL)?.trim().parse().ok()?;
    (ms > 0).then(|| ms as f32 / 1000.0)
}

/// The cast currently running, if any.
///
/// A single cast rather than the board's three rows: the board is authored for
/// up to three, but nothing in this client can start a second one, and an
/// empty-row layout nobody can reach would be untested code.
#[derive(Resource, Default)]
pub struct CastGauge {
    pub active: Option<ActiveCast>,
    /// The cast length of the item we just asked to use, waiting for the
    /// server's `0x30BF` to turn it into a running bar.
    ///
    /// Armed on the **outgoing** `0x704C` rather than the incoming ack: the ack
    /// arrives 30-100 ms *after* the cast-start push, and by the time it lands
    /// the slot it names may already be empty (one captured return exhausted its
    /// stack, `remaining == 0`), so the item is no longer there to look up. On
    /// the request side we still hold it.
    pub pending_seconds: Option<f32>,
}

pub struct ActiveCast {
    pub kind: CastKind,
    pub label: String,
    pub elapsed: f32,
    pub duration: f32,
}

impl CastGauge {
    /// Begin a cast, replacing any that was running.
    pub fn start(&mut self, kind: CastKind, label: impl Into<String>, duration: f32) {
        self.active = Some(ActiveCast {
            kind,
            label: label.into(),
            elapsed: 0.0,
            duration: duration.max(0.01),
        });
    }

    /// Fraction complete, 0..=1.
    pub fn fraction(&self) -> f32 {
        self.active
            .as_ref()
            .map(|cast| (cast.elapsed / cast.duration).clamp(0.0, 1.0))
            .unwrap_or(0.0)
    }
}

#[derive(Component)]
struct CastGaugeRoot;
/// The crop node whose width is the fill. **The only node the fill drives, and
/// the only bar in the tree** — see the module docs on the `_bright` overlay.
#[derive(Component)]
struct CastGaugeFill;
#[derive(Component)]
struct CastGaugeLabel;
#[derive(Component)]
struct CastGaugeCancel;

pub struct CastGaugePlugin;

impl Plugin for CastGaugePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CastGauge>().add_systems(
            Update,
            (
                arm_cast_duration,
                start_cast_on_state_update,
                advance_cast,
                sync_cast_gauge,
            )
                .chain()
                .run_if(super::hud_scenes),
        );
    }
}

/// Remember how long the item we are about to use casts for.
///
/// Reads the same [`UseItemRequest`] the under-bar dispatches on, so the length
/// is known before the server's cast-start push comes back. An item with no cast
/// time in its row arms nothing, and the bar falls back to the config value.
fn arm_cast_duration(
    mut requests: MessageReader<UseItemRequest>,
    item_data: Res<ClientItemData>,
    mut gauge: ResMut<CastGauge>,
) {
    for request in requests.read() {
        gauge.pending_seconds = item_cast_seconds(item_data.get(&(request.ref_id as i32)));
    }
}

/// Start the bar when the server says a cast began.
fn start_cast_on_state_update(
    mut reader: MessageReader<EntityStateUpdate>,
    players: Query<&NetworkId, With<Player>>,
    config: Option<Res<crate::plugins::config::ClientConfig>>,
    mut gauge: ResMut<CastGauge>,
) {
    let fallback = config
        .as_deref()
        .map_or(CAST_GAUGE_SECONDS, |c| c.hud.cast_gauge_seconds);
    for msg in reader.read() {
        if msg.kind != STATE_KIND_CASTING || msg.value == 0 {
            continue;
        }
        // Only our own cast gets a bar — the packet is broadcast per entity.
        if !players.iter().any(|NetworkId(uid)| *uid == msg.unique_id) {
            continue;
        }
        let armed = gauge.pending_seconds.take();
        let seconds = armed.unwrap_or(fallback);
        // Return is the only kind this state has ever been seen for; a second
        // kind would need its own trigger, not a guess here.
        info!(
            "cast: started, {seconds:.1}s from {} (0x30BF kind {STATE_KIND_CASTING:#04x})",
            if armed.is_some() {
                "itemdata"
            } else {
                "the config fallback"
            }
        );
        gauge.start(CastKind::Return, "Return", seconds);
    }
}

/// Run the clock, and clear the bar one frame after it completes.
///
/// The clear is deliberately late by a frame. Clearing at `elapsed >= duration`
/// meant `fraction()` was never once observed at 1.0 by the sync system below —
/// the last width it could ever draw was the frame before completion, so the bar
/// visibly vanished a sliver short of full.
fn advance_cast(time: Res<Time>, mut gauge: ResMut<CastGauge>) {
    let Some(cast) = gauge.active.as_mut() else {
        return;
    };
    if cast.elapsed >= cast.duration {
        gauge.active = None;
        return;
    }
    cast.elapsed = (cast.elapsed + time.delta_secs()).min(cast.duration);
}

/// Rebuild the window when the cast starts or ends; drive the fill each frame
/// while it runs.
///
/// Rebuild-on-change like the storage/store windows, because the plate's art
/// and the fill's art both depend on the cast kind — a persistent hidden tree
/// would have to repaint both anyway.
fn sync_cast_gauge(
    gauge: Res<CastGauge>,
    existing: Query<Entity, With<CastGaugeRoot>>,
    mut fills: Query<&mut Node, With<CastGaugeFill>>,
    fonts: Res<FontAssets>,
    asset_server: Res<AssetServer>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
    mut drawn: Local<Option<CastKind>>,
) {
    let wanted = gauge.active.as_ref().map(|cast| cast.kind);
    if *drawn == wanted {
        // Same cast still running: only the crop moves.
        let s = hud_scale();
        let width = gauge_fill_width(gauge.fraction(), GAUGE_RECT.2 * s);
        // Guarded like every other gauge consumer (`magic_state_board`,
        // `target_window`, ...): an unconditional write marks `Node` changed and
        // forces a UI relayout on every frame of the cast.
        for mut node in fills.iter_mut() {
            if node.width != width {
                node.width = width;
            }
        }
        return;
    }
    for entity in existing.iter() {
        commands.entity(entity).despawn();
    }
    // The sentinel is set only once the tree is actually going up. Setting it
    // before this `else` meant a frame with no `Camera2d` — which is reachable
    // on the frame a cast starts — marked the kind as drawn, spawned nothing,
    // and then took the early-return above forever after: no bar for that cast.
    let (Some(cast), Ok(camera)) = (gauge.active.as_ref(), cam_query.single()) else {
        *drawn = None;
        return;
    };
    *drawn = wanted;
    let s = hud_scale();

    commands
        .spawn((
            CastGaugeRoot,
            Name::from("Cast Gauge"),
            // A full-width, bottom-anchored strip that centres the board, the
            // same way the under-bar this sits above centres itself. Anchoring
            // the board itself by `right`/`top` in window pixels only lands it
            // over the under-bar when the window happens to be 1024x768 scaled.
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                bottom: Val::Px(BOARD_BOTTOM * s),
                width: Val::Percent(100.0),
                height: Val::Px(BOARD.1 * s),
                justify_content: JustifyContent::Center,
                ..default()
            },
            GlobalZIndex(52),
            UiTargetCamera(camera),
            Pickable::IGNORE,
        ))
        .with_children(|strip| {
            strip
                .spawn((
                    Node {
                        width: Val::Px(BOARD.0 * s),
                        height: Val::Px(BOARD.1 * s),
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|board| build_row(board, cast, &fonts, &asset_server, s));
        });
}

/// One `com_casting_window` row: the plate, then the three authored controls.
///
/// The plate is a **child** of the board at [`PLATE_TOP`], not the row root —
/// the controls' rects are board-relative, so making the plate the root shifts
/// every one of them 4 units up out of its printed socket.
fn build_row(
    board: &mut ChildSpawnerCommands,
    cast: &ActiveCast,
    fonts: &FontAssets,
    asset_server: &AssetServer,
    s: f32,
) {
    board.spawn((
        abs_node((0.0, PLATE_TOP, ROW.0, ROW.1), s),
        ImageNode {
            image: asset_server.load(PLATE_DDJ),
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Pickable::IGNORE,
    ));

    board.spawn((
        CastGaugeLabel,
        Text::new(cast.label.clone()),
        TextFont {
            font: fonts.two.clone().into(),
            font_size: bevy::text::FontSize::Px(8.0 * s),
            ..default()
        },
        TextColor(NAME_COLOR),
        TextLayout::justify(Justify::Center),
        abs_node(NAME_RECT, s),
        Pickable::IGNORE,
    ));

    // The gauge, per `hud::gauge`: track (authored rect, clipped) -> crop -> art
    // at its native extent. The track's clip is what stops the crop drawing past
    // the bar's ends.
    let mut track_node = abs_node(GAUGE_RECT, s);
    track_node.overflow = bevy::ui::Overflow::clip();
    board
        .spawn((track_node, Pickable::IGNORE))
        .with_children(|track| {
            track
                .spawn((
                    CastGaugeFill,
                    gauge_crop_node(gauge_fill_width(0.0, GAUGE_RECT.2 * s), GAUGE_RECT.3 * s),
                    Pickable::IGNORE,
                ))
                .with_children(|crop| {
                    crop.spawn((
                        GaugeArt,
                        ImageNode {
                            image: asset_server.load(cast.kind.fill_art()),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(0.0),
                            top: Val::Px(0.0),
                            width: Val::Px(GAUGE_RECT.2 * s),
                            height: Val::Px(GAUGE_RECT.3 * s),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                });
        });

    let cancel_style = ImageButtonStyle {
        normal: asset_server.load(format!("{ART}com_casting_cancel.ddj")),
        hover: asset_server.load(format!("{ART}com_casting_cancel_focus.ddj")),
        press: asset_server.load(format!("{ART}com_casting_cancel_press.ddj")),
        ..Default::default()
    };
    board
        .spawn((
            CastGaugeCancel,
            Button,
            Hovered::default(),
            abs_node(CANCEL_RECT, s),
            ImageNode {
                image: cancel_style.normal.clone(),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            cancel_style,
        ))
        .observe(on_cancel);
}

/// Cancel the cast: ask the server to abort it, and dismiss the bar either way.
///
/// Sends `0x705B` **TELEPORT / TRANSITION CASTING CANCEL** (empty body), which
/// is `[V]` from the original client's own sender — see the module docs. The
/// local dismiss is unconditional because the request may well go unanswered:
/// **vSRO never writes the `0xB05B` reply**, so on a go-sro-derived server this
/// is expected to be ignored and the teleport to land anyway.
///
/// That is the experiment, and the log line is how it is read: if `0x34B5` still
/// arrives after this, the opcode is not honoured here and the button is
/// cosmetic; if the teleport does not come, it works.
fn on_cancel(
    _: On<Activate>,
    mut gauge: ResMut<CastGauge>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    if gauge.active.take().is_none() {
        return;
    }
    let Ok(connection) = conn.single() else {
        info!("cast: gauge dismissed locally (offline — would send 0x705B)");
        return;
    };
    let request = TransitionCastingCancelRequest;
    if let Err(e) = connection.get_sender().send(Packet::from(request).into()) {
        error!("cast: failed to send the cancel: {}", e.0);
        return;
    }
    info!(
        "cast: cancel requested (0x705B) and gauge dismissed — a teleport (0x34B5) \
         arriving anyway means the server did not honour it"
    );
}

#[cfg(test)]
mod test {
    use super::*;

    /// Every rect is a transcription of `ifdelayinfo.txt` and the art measures
    /// out to match, so pin both halves together — a drift in either is a
    /// mis-read of the archive rather than a design change.
    #[test]
    fn the_rects_match_the_art_they_draw() {
        // com_casting_gauge_*.ddj measure 184x8
        assert_eq!((GAUGE_RECT.2, GAUGE_RECT.3), (184.0, 8.0));
        // com_casting_cancel.ddj measures 20x20
        assert_eq!((CANCEL_RECT.2, CANCEL_RECT.3), (20.0, 20.0));
        // com_casting_window.ddj measures 192x36
        assert_eq!(ROW, (192.0, 36.0));
        // every control sits inside the row
        for (x, y, w, h) in [NAME_RECT, CANCEL_RECT, GAUGE_RECT] {
            assert!(x + w <= ROW.0, "{x}+{w} overflows the row width");
            assert!(y + h <= ROW.1, "{y}+{h} overflows the row height");
        }
    }

    /// `GDR_DELAY_GAUGE_BOARD` is horizontally centred on the vanilla screen:
    /// `(1024 - 192) / 2 == 416` is its authored x. We centre with a flex strip
    /// rather than that literal margin, so what is pinned here is the *fact*
    /// that centring reproduces the authored x, plus the bottom offset the
    /// strip is anchored by.
    #[test]
    fn the_board_is_centred_above_the_underbar() {
        assert_eq!((1024.0 - BOARD.0) / 2.0, 416.0, "authored x");
        assert_eq!(768.0 - (606.0 + BOARD.1), BOARD_BOTTOM, "authored y");
    }

    /// The board is exactly a 4-unit inset above three rows, which is what makes
    /// the plate's own offset [`PLATE_TOP`] rather than 0 — and with it lands
    /// every control in its printed socket.
    #[test]
    fn the_board_holds_three_rows_under_a_four_unit_inset() {
        assert_eq!(BOARD.1, PLATE_TOP + 3.0 * ROW.1);
        assert_eq!(BOARD.0, ROW.0);
    }

    /// The two controls whose sockets are printed into `com_casting_window.ddj`
    /// must land inside them once the plate is offset by [`PLATE_TOP`].
    ///
    /// The socket bounds are measured from the art, not authored anywhere: the
    /// cancel panel is the fully-opaque block at plate `x 167..191, y 0..20` in
    /// the otherwise translucent upper band, and the gauge groove is the channel
    /// between the bright bevel rows at plate y 22 and y 32. Both were 4 units
    /// out while the plate was the row root.
    #[test]
    fn the_controls_land_in_the_sockets_printed_on_the_plate() {
        // plate-relative = authored - PLATE_TOP
        let cancel_top = CANCEL_RECT.1 - PLATE_TOP;
        assert_eq!(cancel_top, 0.0);
        assert!(
            cancel_top + CANCEL_RECT.3 <= 21.0,
            "cancel overhangs its 21-row socket"
        );
        assert!(
            CANCEL_RECT.0 >= 167.0 && CANCEL_RECT.0 + CANCEL_RECT.2 <= 192.0,
            "cancel escapes its socket horizontally"
        );

        let gauge_top = GAUGE_RECT.1 - PLATE_TOP;
        assert_eq!(gauge_top, 23.0, "the groove interior starts at plate y 23");
        assert!(
            gauge_top + GAUGE_RECT.3 <= 32.0,
            "the fill crosses the groove's lower bevel at plate y 32"
        );
    }

    /// Only three of the six fills ship a `_bright` partner. Nothing draws it —
    /// see the module docs — but which kinds *have* one is a fact about the
    /// archive worth keeping, so the next person to identify it need not
    /// re-derive the set.
    #[test]
    fn only_the_kinds_with_bright_art_ask_for_it() {
        for kind in [CastKind::Return, CastKind::Skill, CastKind::RecallGuild] {
            assert!(kind.bright_art().is_some(), "{kind:?}");
        }
        for kind in [CastKind::Collection, CastKind::Health, CastKind::Spool] {
            assert!(kind.bright_art().is_none(), "{kind:?}");
        }
    }

    /// The fill runs 0..1 and stops there — a cast that overruns its estimated
    /// duration must not draw past the end of the track.
    #[test]
    fn the_fill_is_clamped() {
        let mut gauge = CastGauge::default();
        assert_eq!(gauge.fraction(), 0.0, "no cast, no fill");
        gauge.start(CastKind::Return, "Return", 10.0);
        assert_eq!(gauge.fraction(), 0.0);
        gauge.active.as_mut().unwrap().elapsed = 5.0;
        assert!((gauge.fraction() - 0.5).abs() < f32::EPSILON);
        gauge.active.as_mut().unwrap().elapsed = 99.0;
        assert_eq!(
            gauge.fraction(),
            1.0,
            "an overrun must not exceed the track"
        );
    }

    /// A zero or negative duration must not divide by zero.
    #[test]
    fn a_degenerate_duration_still_produces_a_fraction() {
        let mut gauge = CastGauge::default();
        gauge.start(CastKind::Return, "Return", 0.0);
        assert!(gauge.fraction().is_finite());
    }

    /// The cast length comes from the item's own row.
    ///
    /// The two return scrolls in the capture are pinned by value because they
    /// are the evidence: 30000 ms and 15000 ms against measured casts of
    /// 30.27/30.12 s and 15.40/15.32/15.15 s.
    #[test]
    fn the_cast_length_comes_from_the_item_row() {
        let row = |cast_ms: &str| {
            let mut cols = vec![String::new(); ITEMDATA_CAST_MS_COL + 1];
            cols[ITEMDATA_CAST_MS_COL] = cast_ms.to_string();
            ItemDataRow(cols)
        };
        // ITEM_ETC_SCROLL_RETURN_01 (ref 61) and _02 (ref 2198)
        assert_eq!(item_cast_seconds(Some(&row("30000"))), Some(30.0));
        assert_eq!(item_cast_seconds(Some(&row("15000"))), Some(15.0));
        // ITEM_*_RETURN_SCROLL_HIGH_SPEED — the row that settles the unit
        assert_eq!(item_cast_seconds(Some(&row("1000"))), Some(1.0));

        // Everything that does not cast arms nothing, so the bar falls back:
        // the column means something else entirely for those classes.
        assert_eq!(item_cast_seconds(Some(&row("-1"))), None, "9,599 rows");
        assert_eq!(item_cast_seconds(Some(&row("0"))), None);
        assert_eq!(item_cast_seconds(Some(&row(""))), None);
        assert_eq!(item_cast_seconds(None), None, "no itemdata row at all");
        // A row too short to reach the column must not panic.
        assert_eq!(item_cast_seconds(Some(&ItemDataRow(vec![]))), None);
    }

    /// **The rounds-3-and-4 bug, pinned.** The crop must be empty at fraction 0.
    ///
    /// Round 3 drew the `_bright` art full-width over the bar, so the gauge read
    /// full from its first frame; round 4 cropped that overlay along with the
    /// fill, which turned it into a second, thicker bar. There is now exactly
    /// one bar, and its width is the fraction.
    #[test]
    fn nothing_is_drawn_at_full_width_while_the_bar_is_empty() {
        let track = GAUGE_RECT.2;
        assert_eq!(gauge_fill_width(0.0, track), Val::Px(0.0));
        assert_eq!(gauge_fill_width(1.0, track), Val::Px(track));
        assert_eq!(gauge_fill_width(0.5, track), Val::Px((track / 2.0).round()));
    }

    /// The clock must let the bar be *seen* at 1.0 before it clears.
    ///
    /// It used to drop `active` the moment `elapsed >= duration`, so the sync
    /// system never once observed a full bar — it vanished a sliver short.
    #[test]
    fn a_completed_cast_reads_full_for_one_frame_before_it_clears() {
        let mut app = App::new();
        app.init_resource::<CastGauge>()
            .init_resource::<Time>()
            .add_systems(Update, advance_cast);
        app.world_mut()
            .resource_mut::<CastGauge>()
            .start(CastKind::Return, "Return", 0.5);
        // Overrun the duration in one step.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs(1));
        app.update();
        let gauge = app.world().resource::<CastGauge>();
        assert!(gauge.active.is_some(), "the full frame must survive");
        assert_eq!(gauge.fraction(), 1.0);

        app.update();
        assert!(
            app.world().resource::<CastGauge>().active.is_none(),
            "and then it clears"
        );
    }
}
