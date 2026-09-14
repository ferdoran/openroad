//! The Guild page — page 10 of the Community window (`GDR_COMMUNITY_GUILD`).
//!
//! Idea: the whole page is transcribed from the user's own
//! `resinfo/ifguild.txt` — §Create (the page chrome, which declares frame
//! #25), §GuildInfo (the header block, `ifguild.txt:65-353`), and #25's own
//! three children: the roster list (§MemberView/§SortBtn), the notice strip
//! (§NotifySubBox) and the command column (§Command). Every rect const carries
//! the `ifguild.txt:NNN` line it came from, so no number here was chosen.
//!
//! What is still missing is inside those sections rather than a section of its
//! own, and each piece is missing for a stated reason: the **GP gauge fill**
//! stays at zero (the required-GP threshold is server-authoritative — see
//! below), the roster's **Grade column** stays blank (the `0x3101` record
//! carries no grade), and **four of the five command verbs** have no wire
//! (only Join sends `0x70F3`; the rest would each need an invented value, per
//! §Command's own note).
//!
//! Rects are page-local: the six community pages all share
//! `ifcommunity.txt`'s `13,61,451,320`, and `community/ui.rs` already spawns
//! that container — exactly the frame `letter.rs` draws into.
//!
//! Two things the original leaves us to decide, both stated rather than
//! silently done (`docs/re/ui/hud-guild-window.md` §3.10 / §9-U4):
//!
//! * **The two art overflows.** `gil_windo01.ddj` is 588x108 at x=6 (right
//!   edge 594) and `gil_bar01.ddj` is 376x28 at x=120 (right edge 496), both
//!   past the 451-wide page. Both were re-authored for the 588-px 4th-gen
//!   pane and the shipped classic tree still points at them. Whether the
//!   original clips or overdraws is `[U]` (§9-U4 — it needs a decompile or a
//!   screenshot), so we **clip at the page's right edge**: the art is drawn at
//!   its native size inside a clipping node, which keeps the bitmap 1:1 (no
//!   squash) and keeps the page from painting over the Community frame.
//! * **The GP gauge fill.** The record carries `guild_points`, but nothing in
//!   the client's data carries the GP a level *requires* — that threshold is
//!   server-authoritative and arrives only with the `ifguildlevelup` flow. So
//!   the gauge track is drawn and the fill stays at zero, and the percent
//!   readout shows the empty marker, rather than us inventing a maximum.
//!   [`GuildGpFill`] is the one node a later ticket writes to.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::FontAssets;
use crate::plugins::cursor::interactions::entity_select::SelectedEntity;
use crate::plugins::hud::game_window::abs_node;
use crate::plugins::hud::gauge::{gauge_art_node, gauge_crop_node, gauge_fill_width};
use crate::plugins::hud::inventory::ui::format_thousands;
use crate::plugins::net::entities::NetworkId;
use crate::plugins::net::guild::{GuildAction, GuildRoster};
use crate::plugins::textdata::ClientUiStrings;

// --- §Create — page chrome, `ifguild.txt:4-63` (3 controls) -----------------

/// `GDR_GUILD_INFO_WND:CIFStatic` (`ifguild.txt:6`, Rect `6,4,0,0`) — art-sized
/// from `gil_windo01.ddj`, measured **588x108**. `6 + 588 = 594 > 451`, so it
/// overflows the page by 143 px; see the module doc for the clip decision.
const INFO_WND_POS: (f32, f32) = (6.0, 4.0);
const INFO_WND_ART: (f32, f32) = (588.0, 108.0);
const INFO_WND_DDJ: &str = "media://interface/guild/gil_windo01.ddj";
/// `GDR_GUILD_FRAME:CIFFrame` (`ifguild.txt:25`, Rect `6,103,440,211`).
const FRAME_RECT: (f32, f32, f32, f32) = (6.0, 103.0, 440.0, 211.0);
const FRAME_PIECE: f32 = 16.0;
const FRAME_DIR: &str = "media://interface/frame/frameg01_wnd_";
/// `GDR_GUILD_BG:CIFStatic` (`ifguild.txt:44`, Rect `27,105,403,58`) — the
/// tile behind the notice strip.
const BG_RECT: (f32, f32, f32, f32) = (27.0, 105.0, 403.0, 58.0);
const BG_TILE_DDJ: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_b.ddj";

/// The page rect every community page shares (`ifcommunity.txt`), used as the
/// clip boundary for the two overflowing statics.
const PAGE_SIZE: (f32, f32) = (451.0, 320.0);

// --- §GuildInfo — header block, `ifguild.txt:65-353` (15 controls) ----------

/// `GDR_GUILD_INFO_GUILD_MARK` (`:219`, Rect `144,16,0,0`) — art-sized
/// `gil_mark01.ddj`, measured 16x16. The emblem is server art in vanilla; we
/// draw the default plate until an emblem source exists.
const MARK_POS: (f32, f32) = (144.0, 16.0);
const MARK_SIZE: (f32, f32) = (16.0, 16.0);
const MARK_DDJ: &str = "media://interface/guild/gil_mark01.ddj";
/// `GDR_GUILD_INFO_GUILD_NAME` (`:200`, Rect `166,18,80,14`, HAlign 0).
const NAME_RECT: (f32, f32, f32, f32) = (166.0, 18.0, 80.0, 14.0);
/// `GDR_GUILD_INFO_STA_GUILD_LEVEL` (`:295`, Rect `381,18,34,15`, HAlign 0,
/// FontColor `255,255,217,83`, Text `UIO_CHARINFO_STT_LEVEL`).
const LEVEL_LABEL_RECT: (f32, f32, f32, f32) = (381.0, 18.0, 34.0, 15.0);
/// `GDR_GUILD_INFO_GUILD_LEVEL` (`:181`, Rect `419,18,17,15`, HAlign 0).
const LEVEL_RECT: (f32, f32, f32, f32) = (419.0, 18.0, 17.0, 15.0);
/// `GDR_GUILD_INFO_STA_GUILD_LEADER` (`:276`, Rect `28,51,84,14`, HAlign 0,
/// FontColor `255,239,218,164`, Text `UIIT_STT_GUILD_LEADER`).
const LEADER_LABEL_RECT: (f32, f32, f32, f32) = (28.0, 51.0, 84.0, 14.0);
/// `GDR_GUILD_INFO_GUILD_LEADER_RACE` (`:162`, Rect `90,49,16,16`) — the file
/// names `com_kindred_china16.ddj`, which is the *placeholder*: the original
/// swaps the art for the master's race at runtime. We draw the same
/// placeholder, because the record's `model_id` -> race mapping is a separate
/// piece of work (the roster row ticket needs it too).
const LEADER_RACE_RECT: (f32, f32, f32, f32) = (90.0, 49.0, 16.0, 16.0);
const LEADER_RACE_DDJ: &str = "media://interface/ifcommon/com_kindred_china16.ddj";
/// `GDR_GUILD_INFO_GUILD_LEADER` (`:143`, Rect `112,51,110,14`, HAlign 0).
const LEADER_RECT: (f32, f32, f32, f32) = (112.0, 51.0, 110.0, 14.0);
/// `GDR_GUILD_INFO_STA_GUILDSMAN_NUM` (`:257`, Rect `261,51,67,14`, **HAlign
/// 2**, FontColor `255,239,218,164`, Text `UIIT_STT_GUILDSMAN_NUM`).
const MEMBER_LABEL_RECT: (f32, f32, f32, f32) = (261.0, 51.0, 67.0, 14.0);
/// `GDR_GUILD_INFO_GUILD_MEMBER_NUM` (`:124`, Rect `339,51,67,14`, **HAlign 2**).
const MEMBER_NUM_RECT: (f32, f32, f32, f32) = (339.0, 51.0, 67.0, 14.0);
/// `GDR_GUILD_INFO_BAR_BOARD` (`:333`, Rect `120,67,0,0`) — art-sized
/// `gil_bar01.ddj`, measured **376x28**. `120 + 376 = 496 > 451`: overflows by
/// 45 px, clipped like `INFO_WND` above (module doc).
const BAR_BOARD_POS: (f32, f32) = (120.0, 67.0);
const BAR_BOARD_ART: (f32, f32) = (376.0, 28.0);
const BAR_BOARD_DDJ: &str = "media://interface/guild/gil_bar01.ddj";
/// `GDR_GUILD_INFO_STA_GUILD_POINT_GP` (`:238`, Rect `28,75,89,14`, HAlign 0,
/// FontColor `255,239,218,164`, Text `UIIT_STT_GUILD_POINT`).
const GP_LABEL_RECT: (f32, f32, f32, f32) = (28.0, 75.0, 89.0, 14.0);
/// `GDR_GUILD_INFO_GP_GAUGE:CIFGauge` (`:314`, Rect `128,76,0,0`) — art-sized
/// `gil_point.ddj`, measured 144x8 (R5G6B5, no alpha channel).
const GP_GAUGE_POS: (f32, f32) = (128.0, 76.0);
const GP_GAUGE_ART: (f32, f32) = (144.0, 8.0);
const GP_GAUGE_DDJ: &str = "media://interface/guild/gil_point.ddj";
/// `GDR_GUILD_INFO_GUILD_POINT_PERCENT` (`:105`, Rect `168,75,64,14`, HAlign 1).
const GP_PERCENT_RECT: (f32, f32, f32, f32) = (168.0, 75.0, 64.0, 14.0);
/// `GDR_GUILD_INFO_GUILD_POINT_GP` (`:86`, Rect `276,75,129,14`, HAlign 1).
const GP_VALUE_RECT: (f32, f32, f32, f32) = (276.0, 75.0, 129.0, 14.0);
/// `GDR_GUILD_INFO_GUILD_POINT_BTN:CIFButton` (`:67`, Rect `410,72,0,0`) —
/// art-sized `com_donation_button.ddj`, measured 16x16. Drawn as art only: the
/// GP donation flow is `ifguildpointup.txt`, a separate child of #25, and a
/// button that opens nothing would be a dead wire.
const GP_BUTTON_POS: (f32, f32) = (410.0, 72.0);
const GP_BUTTON_SIZE: (f32, f32) = (16.0, 16.0);
const GP_BUTTON_DDJ: &str = "media://interface/ifcommon/com_donation_button.ddj";

/// `FontColor=255,255,255,255` — the seven runtime statics.
const VALUE_COLOR: Color = Color::srgb(1.0, 1.0, 1.0);
/// `FontColor=255,239,218,164` — leader / member-count / GP labels.
const LABEL_COLOR: Color = Color::srgb(239.0 / 255.0, 218.0 / 255.0, 164.0 / 255.0);
/// `FontColor=255,255,217,83` — the "Level" label alone (`:300`).
const LEVEL_LABEL_COLOR: Color = Color::srgb(1.0, 217.0 / 255.0, 83.0 / 255.0);

/// What a runtime static shows while the player is guildless. The page is
/// *shown*, not hidden — the original has no `Visible` key and page identity
/// is the id alone, so an empty guild page is the guildless state.
const EMPTY: &str = "-";

/// The seven `Text=""` statics of §GuildInfo, i.e. the ones the record fills.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum GuildInfoField {
    Name,
    Level,
    Leader,
    MemberCount,
    GpPercent,
    GpValue,
}

/// The GP gauge's crop node — the only node the fill width is written to
/// (`hud/gauge.rs`).
#[derive(Component)]
pub struct GuildGpFill;

/// The text a field shows for the given record (`None` = guildless).
fn field_text(field: GuildInfoField, data: Option<&packets::agent::guild::GuildData>) -> String {
    let Some(data) = data else {
        return EMPTY.to_string();
    };
    match field {
        GuildInfoField::Name => data.name.clone(),
        GuildInfoField::Level => data.level.to_string(),
        // The record carries no "this is the master" field other than the
        // per-member flag, so the master is the member that claims it.
        GuildInfoField::Leader => data
            .members
            .iter()
            .find(|m| m.is_master)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| EMPTY.to_string()),
        // Count only — the per-level member *cap* is server-side (it never
        // reaches the client), so no "n/max" is rendered.
        GuildInfoField::MemberCount => data.member_count.to_string(),
        // See the module doc: no GP threshold exists client-side.
        GuildInfoField::GpPercent => EMPTY.to_string(),
        GuildInfoField::GpValue => format_thousands(u64::from(data.guild_points)),
    }
}

// --- §MemberView — the roster pane, `ifguild.txt:415-476` (3) ---------------

/// `GDR_GUILD_MEMBER_VIEW_BLACKSQUARE:CIFStretchWnd` (`:445`, Rect
/// `14,138,333,166`) — the list plate; `com_blacksquare_` is six 4 px trim
/// pieces around a flat black interior, the same family `letter.rs` uses.
const MEMBER_PLATE_RECT: (f32, f32, f32, f32) = (14.0, 138.0, 333.0, 166.0);
const BLACKSQUARE_PIECE: f32 = 4.0;
const BLACKSQUARE_DIR: &str = "media://interface/ifcommon/com_blacksquare_";
/// `GDR_GUILD_USERVIEW_SCROLLMGR:CIFScrollManager` (`:426`, Rect
/// `17,163,328,139`) — the row area. The manager itself (its pooled rows and
/// its `CIFVerticalScroll`) is the shared widget of #56 and is **not** built
/// here; this row draws the fixed visible rows the manager's own height
/// implies.
const MEMBER_LIST_RECT: (f32, f32, f32, f32) = (17.0, 163.0, 328.0, 139.0);
/// `GDR_GUILD_MEMBER_VIEW_BG:CIFStatic` (`:465`, Rect `328,163,102,135`) — the
/// right gutter tile behind the command column.
const MEMBER_VIEW_BG_RECT: (f32, f32, f32, f32) = (328.0, 163.0, 102.0, 135.0);

/// Row pitch and height. **Both are sourced, not chosen**
/// (`hud-guild-window.md` §3.5): the pitch-23/height-24 law is proven inside
/// this very file set (`ifguildmasterelection.txt` / `ifguildmasterleave.txt`
/// each lay out 7 rows at y 64/87/110/… inside a 164-tall manager) and
/// `guild.2dt`'s 6 `CNIFGuildUserSlot` entries repeat it for this window.
const ROW_PITCH: f32 = 23.0;
const ROW_HEIGHT: f32 = 24.0;
/// `139 = 6·23 + 1` ⇒ six visible rows. `[S]` in the doc, and the 4th-gen
/// tree's explicit six slots agree.
const VISIBLE_ROWS: usize = 6;

// --- §SortBtn — the column header strip, `ifguild.txt:595-731` (7) ----------

/// `GDR_GUILD_SORT_STATIC1` (`:720`, Rect `17,141,0,0`) — art-sized
/// `gil_shape01.ddj`, measured 24x24, the strip's left cap.
const SORT_CAP_LEFT: (f32, f32, f32, f32) = (17.0, 141.0, 24.0, 24.0);
const SORT_CAP_LEFT_DDJ: &str = "media://interface/guild/gil_shape01.ddj";
/// `GDR_GUILD_CONDITION_BUTTON` (`:606`, Rect `18,142,0,0`) — art-sized
/// `stl_condition.ddj`, 20x20, the online/offline filter. It sits **inside**
/// the left cap (x 18 vs 17, y 142 vs 141), which is why it is drawn after it.
/// Presentational: sorting/filtering the roster is not on the wire and not in
/// this row's scope.
const SORT_CONDITION: (f32, f32, f32, f32) = (18.0, 142.0, 20.0, 20.0);
const SORT_CONDITION_DDJ: &str = "media://interface/stall/stl_condition.ddj";
/// `GDR_GUILD_SORT_STATIC2` (`:625`, Rect `329,141,0,0`) — art-sized
/// `gil_shape.ddj` 16x24, the right cap.
const SORT_CAP_RIGHT: (f32, f32, f32, f32) = (329.0, 141.0, 16.0, 24.0);
const SORT_CAP_RIGHT_DDJ: &str = "media://interface/guild/gil_shape.ddj";

/// The four sort buttons, in file order 1..4 (`:701,:682,:663,:644`). The art
/// widths are the measured DDJ extents (`w=h=0` ⇒ art-sized), and the x-run
/// **overlaps its neighbours on purpose**: 39+132 = 171 vs 170 (−1),
/// 170+44 = 214 vs 211 (−3), 211+52 = 263 vs 261 (−2). Those seams are how
/// vanilla butts the header plates together, so they are reproduced verbatim
/// rather than "corrected" to a clean tiling.
const SORT_BUTTONS: [(f32, f32, &str, &str, &str); 4] = [
    (
        39.0,
        132.0,
        "media://interface/guild/gil_subj_button02.ddj",
        "UIIT_STT_GUILDSMAN",
        "Member",
    ),
    (
        170.0,
        44.0,
        "media://interface/guild/gil_subj_button03.ddj",
        "UIIT_STT_LEVEL",
        "Level",
    ),
    (
        211.0,
        52.0,
        "media://interface/guild/gil_subj_button04.ddj",
        "UIIT_STT_GRADE",
        "Grade",
    ),
    (
        261.0,
        68.0,
        "media://interface/guild/gil_subj_button05.ddj",
        "UIIT_STT_GP_SUBSCRIPION",
        "Donate GP",
    ),
];
const SORT_BUTTON_Y: f32 = 141.0;
const SORT_BUTTON_H: f32 = 24.0;

// --- The row prototype, `ifguildmemberslot.txt` (6), slot-local ------------

/// `GDR_GMS_ONOFF` (`:110`, Rect `5,6,0,0`) — art-sized `gil_contact_off.ddj`,
/// 12x16; `_on` is the same size. This is the one cell whose **art** carries
/// state.
const SLOT_ONOFF: (f32, f32, f32, f32) = (5.0, 6.0, 12.0, 16.0);
const SLOT_ONOFF_OFF_DDJ: &str = "media://interface/guild/gil_contact_off.ddj";
const SLOT_ONOFF_ON_DDJ: &str = "media://interface/guild/gil_contact_on.ddj";
/// `GDR_GMS_RACE_MARK` (`:91`, Rect `36,5,16,16`) — the same
/// `com_kindred_china16.ddj` placeholder the header's leader mark uses.
const SLOT_RACE_MARK: (f32, f32, f32, f32) = (36.0, 5.0, 16.0, 16.0);
/// `GDR_GMS_NAME` (`:72`, `62,7,84,14`, HAlign 1).
const SLOT_NAME: (f32, f32, f32, f32) = (62.0, 7.0, 84.0, 14.0);
/// `GDR_GMS_LEVEL` (`:53`, `160,7,26,14`, HAlign 1).
const SLOT_LEVEL: (f32, f32, f32, f32) = (160.0, 7.0, 26.0, 14.0);
/// `GDR_GMS_GRADE` (`:34`, `199,7,40,14`, HAlign 1).
const SLOT_GRADE: (f32, f32, f32, f32) = (199.0, 7.0, 40.0, 14.0);
/// `GDR_GMS_DONATEDGP` (`:15`, `254,7,48,14`, HAlign 1).
const SLOT_DONATED_GP: (f32, f32, f32, f32) = (254.0, 7.0, 48.0, 14.0);

/// One roster cell, addressed by row index and column.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub struct GuildRosterCell {
    pub row: usize,
    pub column: RosterColumn,
}

/// The four text columns of a member row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RosterColumn {
    Name,
    Level,
    /// `UIIT_STT_GRADE`. **Deliberately blank**: the `0x3101` roster record
    /// (`packets::agent::guild::GuildMember`) carries no grade/rank field —
    /// it has the character level, the guild points, the permission bits and
    /// the grant-name `nickname`, none of which *is* a grade. Filling this
    /// from `nickname` or from the permission bits would be inventing the
    /// column's meaning, so it renders empty until a capture names it.
    Grade,
    DonatedGp,
}

/// The online/offline dot of a row — the only cell whose art changes.
#[derive(Component, Clone, Copy)]
pub struct GuildRosterOnline(pub usize);

/// The per-row cells for a member, or the empty state past the roster's end.
fn cell_text(column: RosterColumn, member: Option<&packets::agent::guild::GuildMember>) -> String {
    let Some(member) = member else {
        return String::new();
    };
    match column {
        RosterColumn::Name => member.name.clone(),
        RosterColumn::Level => member.level.to_string(),
        RosterColumn::Grade => String::new(),
        RosterColumn::DonatedGp => format_thousands(u64::from(member.guild_points)),
    }
}

// --- §NotifySubBox — the notice strip, `ifguild.txt:354-414` (3) ------------

/// `GDR_GUILD_NOTIFY_SUBJECT_STATIC` (`:365`, `24,117,53,14`, FontColor
/// `255,239,218,164`, Text `UIIT_STT_GUILD_COMMON_KNOW`).
const NOTICE_LABEL_RECT: (f32, f32, f32, f32) = (24.0, 117.0, 53.0, 14.0);
/// `GDR_GUILD_NOTIFY_SUBJECT:CIFSelectableArea` (`:403`, `15,110,428,28`) —
/// the click target that opens the read pane. `CIFSelectableArea` occurs
/// **twice** in the whole 247-file resinfo corpus (here and in
/// `ifapprenticeship.txt`), so it is a real class, not a typo: an invisible
/// hit area over the strip, which is why it draws no art.
const NOTICE_SUBJECT_RECT: (f32, f32, f32, f32) = (15.0, 110.0, 428.0, 28.0);
/// Where the subject text itself is drawn inside that area. **Ours**: the
/// `CIFSelectableArea` carries no text rect of its own, so the subject is laid
/// out just right of the label, on the label's own baseline.
const NOTICE_SUBJECT_TEXT_RECT: (f32, f32, f32, f32) = (82.0, 117.0, 330.0, 14.0);
/// `GDR_GUILD_NOTIFY_EDIT_BTN:CIFButton` (`:384`, `417,111,0,0`) — art-sized
/// `stl_edit_button.ddj`, measured 24x24. Presentational here: the notice
/// **write** modal (`ifguildnotifywrite.txt` -> `0x70F9`) is a separate child
/// of #97, so a live button would open nothing.
const NOTICE_EDIT_BTN: (f32, f32, f32, f32) = (417.0, 111.0, 24.0, 24.0);
const NOTICE_EDIT_BTN_DDJ: &str = "media://interface/stall/stl_edit_button.ddj";

// --- §NotifyContents — the read pane, `ifguild.txt:732-753` + its child -----

/// `GDR_GUILD_NOTIFY_CONTENTS:CIFGuildNotifyContents` (`:743`, `6,138,440,177`)
/// — the host. It is **co-anchored at y=138** with `§MemberView`'s plate
/// (`14,138,333,166`) and `§GrantPower`'s (`14,138,423,143`): the shared rect
/// *is* the client's "if", so exactly one of the three may be visible. This
/// host is spawned hidden and shown by clicking the subject strip; it is wider
/// and taller than the roster plate it covers (asserted in the tests).
const NOTICE_PANE_RECT: (f32, f32, f32, f32) = (6.0, 138.0, 440.0, 177.0);

/// `ifguildnotifycontents.txt`, host-relative (6 controls).
/// `GDR_GUILD_NOTIFY_CON_BLACKSQUARE:CIFStretchWnd` (`:110`, `15,15,410,125`).
const NOTICE_PLATE_RECT: (f32, f32, f32, f32) = (15.0, 15.0, 410.0, 125.0);
/// `_BG_02:CIFStatic` (`:72`, `19,19,402,117`) — `com_bg_tile_e.ddj`.
const NOTICE_BG02_RECT: (f32, f32, f32, f32) = (19.0, 19.0, 402.0, 117.0);
const NOTICE_BG02_DDJ: &str = "media://interface/ifcommon/bg_tile/com_bg_tile_e.ddj";
/// `_TEXT:CIFTextBox` (`:15`, `27,27,371,101`) — the notice body.
const NOTICE_TEXT_RECT: (f32, f32, f32, f32) = (27.0, 27.0, 371.0, 101.0);
/// `_SCROLL:CIFVerticalScroll` (`:34`, `406,32,16,75`) — declared with an
/// empty `DDJ=`, i.e. the engine's own scrollbar art. The shared scroll widget
/// is #56's, so the rect is transcribed and reserved; nothing is drawn into it
/// rather than inventing a bar.
const NOTICE_SCROLL_RECT: (f32, f32, f32, f32) = (406.0, 32.0, 16.0, 75.0);
/// `_BG_01:CIFNormalTile` (`:91`, `16,140,408,21`) — `com_bg_tile_b.ddj`.
const NOTICE_BG01_RECT: (f32, f32, f32, f32) = (16.0, 140.0, 408.0, 21.0);
/// `_BUTTON:CIFButton` (`:53`, `182,145,0,0`) — art-sized `com_button.ddj`,
/// measured 76x24, FontColor `255,255,245,218`, Text `UIIS_CTL_CONFIRM`. It
/// closes the pane, which is the only thing a read pane's OK can do.
const NOTICE_OK_RECT: (f32, f32, f32, f32) = (182.0, 145.0, 76.0, 24.0);
const NOTICE_OK_DDJ: &str = "media://interface/ifcommon/com_button.ddj";
/// `FontColor=255,255,245,218` — the read pane's Confirm caption.
const NOTICE_OK_COLOR: Color = Color::srgb(1.0, 1.0, 218.0 / 255.0);

// --- §Command — the action column, `ifguild.txt:477-594` (6 in 5 slots) -----

/// Every command button is `com_mid_button.ddj` (measured **88x24**) at
/// `353,y,0,0`, HAlign 1 / VAlign 1, FontColor `255,255,245,218`.
/// `353 + 88 = 441 <= 451`, so the column fits the page as authored.
const COMMAND_X: f32 = 353.0;
const COMMAND_BTN_SIZE: (f32, f32) = (88.0, 24.0);
const COMMAND_BTN_DDJ: &str = "media://interface/ifcommon/com_mid_button.ddj";
/// `FontColor=255,255,245,218` (`ifguild.txt:579` and its five siblings).
const COMMAND_TEXT_COLOR: Color = Color::srgb(1.0, 1.0, 218.0 / 255.0);

/// The five slot positions, top to bottom, on a **27** pitch
/// (`:583,:564,:545,:526,:507`).
const COMMAND_YS: [f32; 5] = [142.0, 169.0, 196.0, 223.0, 250.0];

/// Slot index of "Join" — the only command with a wire path today.
const COMMAND_SLOT_INVITE: usize = 0;

/// The four unconditional commands, in id order 101..104.
const COMMAND_KEYS: [(&str, &str); 4] = [
    ("UIIT_STT_GUILD_JOIN", "Join"),
    ("UIIT_CTL_AUTHORITY_GRANT", "Grant authority"),
    ("UIIT_STT_GUILD_EXPULSION", "Withdraw"),
    ("UIIT_STT_GUILD_EXIT", "Leave"),
];

/// The fifth slot's two candidates. Ids **105 and 106 share
/// `353,250,0,0` byte-for-byte** (`:507` / `:488`), so exactly one is ever
/// drawn — the data expresses the choice as a geometric collision and states
/// no rule. Id 106 additionally carries `Style=64` where every sibling carries
/// `0`, and what that bit means is `[U]` (`hud-guild-window.md` §9-U5). Hence
/// the config flag rather than a guess: default id 105, the era-specific
/// id 106 behind `guild.position_grant`.
const COMMAND_SLOT5_DEFAULT: (&str, &str) = ("UIIT_STT_GUILD_NAME_GRANT", "Grant name");
const COMMAND_SLOT5_POSITION_GRANT: (&str, &str) =
    ("UIIT_CTL_GUILD_POSITION_GRANT", "Position allocating");

/// The five captions actually drawn, for the given flag.
fn command_keys(position_grant: bool) -> [(&'static str, &'static str); 5] {
    let slot5 = if position_grant {
        COMMAND_SLOT5_POSITION_GRANT
    } else {
        COMMAND_SLOT5_DEFAULT
    };
    [
        COMMAND_KEYS[0],
        COMMAND_KEYS[1],
        COMMAND_KEYS[2],
        COMMAND_KEYS[3],
        slot5,
    ]
}

/// The one command with a fully sourced request *and* an addressable target:
/// invite (0x70F3). It sends the click-selected entity's spawn id, which is how
/// the original addresses it too — the guild window has no target picker of its
/// own, the world selection is the target.
///
/// The other four stay presentational. Not for lack of an opcode: expel
/// (0x70F4) is addressed **by name** and needs a roster row selection this page
/// does not have yet, and leave/disband/promote each carry one `u32` whose
/// meaning the decompile does not give (`docs/net-guild-lifecycle.md`), so
/// sending one would mean inventing its value.
fn on_guild_invite(
    _: On<Activate>,
    selected: Res<SelectedEntity>,
    ids: Query<&NetworkId>,
    mut actions: MessageWriter<GuildAction>,
) {
    let Some(entity) = selected.0 else {
        info!("guild: invite clicked with nothing selected");
        return;
    };
    match ids.get(entity) {
        Ok(id) => {
            actions.write(GuildAction::Invite(id.0));
        }
        // A selected world entity always carries a NetworkId; sending a zero
        // uid instead would be a silent, server-visible mistake.
        Err(_) => warn!("guild: invite on {entity:?}, which has no NetworkId"),
    }
}

/// Is the notice read pane open? The three y=138 panes are mutually exclusive
/// by construction (they share the rect), so this is a boolean today and
/// becomes a three-way when §GrantPower lands.
#[derive(Resource, Default, Debug, PartialEq, Eq)]
pub struct GuildNoticeOpen(pub bool);

/// The read pane's host node.
#[derive(Component)]
pub struct GuildNoticePane;

/// The strip's subject text (the notice **title**) and the pane's body text.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum GuildNoticeField {
    /// `GuildData::notice` — the title half of `GuildNoticeEditRequest`.
    Subject,
    /// `GuildData::message` — the body half.
    Body,
}

fn notice_text(field: GuildNoticeField, data: Option<&packets::agent::guild::GuildData>) -> String {
    let Some(data) = data else {
        return String::new();
    };
    match field {
        GuildNoticeField::Subject => data.notice.clone(),
        GuildNoticeField::Body => data.message.clone(),
    }
}

/// Clicking the subject strip opens the read pane.
fn on_notice_subject(_: On<Activate>, mut open: ResMut<GuildNoticeOpen>) {
    open.0 = true;
}

/// The pane's Confirm closes it — a read pane has nothing else to confirm.
fn on_notice_confirm(_: On<Activate>, mut open: ResMut<GuildNoticeOpen>) {
    open.0 = false;
}

/// Mirror [`GuildNoticeOpen`] onto the pane, and push the record's notice
/// strings onto the two texts.
pub fn update_guild_notice(
    roster: Res<GuildRoster>,
    open: Res<GuildNoticeOpen>,
    mut panes: Query<&mut Node, With<GuildNoticePane>>,
    mut fields: Query<(&GuildNoticeField, &mut Text)>,
) {
    if open.is_changed() {
        for mut node in panes.iter_mut() {
            node.display = if open.0 { Display::Flex } else { Display::None };
        }
    }
    if !roster.is_changed() {
        return;
    }
    for (field, mut text) in fields.iter_mut() {
        let next = notice_text(*field, roster.data.as_ref());
        if text.0 != next {
            text.0 = next;
        }
    }
}

/// Fill the Guild page container with §Create + §GuildInfo + the roster.
pub fn spawn_guild_page(
    page: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
    position_grant: bool,
    s: f32,
) {
    let text_font = |size: f32| TextFont {
        font: fonts.two.clone().into(),
        font_size: FontSize::Px(size * s),
        ..default()
    };
    let image = |rect: (f32, f32, f32, f32), path: String| {
        (
            abs_node(rect, s),
            ImageNode {
                image: asset_server.load(path),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        )
    };

    // --- §Create -----------------------------------------------------------

    // GDR_GUILD_INFO_WND — clipped at the page's right edge (module doc).
    let (ix, iy) = INFO_WND_POS;
    page.spawn((
        Node {
            overflow: Overflow::clip(),
            ..abs_node(
                (ix, iy, clipped_width(ix, INFO_WND_ART.0), INFO_WND_ART.1),
                s,
            )
        },
        Pickable::IGNORE,
    ))
    .with_children(|clip| {
        clip.spawn(image(
            (0.0, 0.0, INFO_WND_ART.0, INFO_WND_ART.1),
            INFO_WND_DDJ.to_string(),
        ));
    });

    // GDR_GUILD_BG — the tiled plate under the notice strip.
    page.spawn((
        abs_node(BG_RECT, s),
        ImageNode {
            image: asset_server.load(BG_TILE_DDJ),
            image_mode: NodeImageMode::Tiled {
                tile_x: true,
                tile_y: true,
                stretch_value: s,
            },
            ..default()
        },
        Pickable::IGNORE,
    ));

    // GDR_GUILD_FRAME — the frameg01_wnd_ ring around the body.
    let (fx, fy, fw, fh) = FRAME_RECT;
    for ((x, y, w, h), piece) in ring(fw, fh, FRAME_PIECE) {
        page.spawn(image(
            (fx + x, fy + y, w, h),
            format!("{FRAME_DIR}{piece}.ddj"),
        ));
    }

    // --- §GuildInfo --------------------------------------------------------

    // GDR_GUILD_INFO_BAR_BOARD — clipped exactly like INFO_WND.
    let (bx, by) = BAR_BOARD_POS;
    page.spawn((
        Node {
            overflow: Overflow::clip(),
            ..abs_node(
                (bx, by, clipped_width(bx, BAR_BOARD_ART.0), BAR_BOARD_ART.1),
                s,
            )
        },
        Pickable::IGNORE,
    ))
    .with_children(|clip| {
        clip.spawn(image(
            (0.0, 0.0, BAR_BOARD_ART.0, BAR_BOARD_ART.1),
            BAR_BOARD_DDJ.to_string(),
        ));
    });

    // The three plain art statics.
    page.spawn(image(
        (MARK_POS.0, MARK_POS.1, MARK_SIZE.0, MARK_SIZE.1),
        MARK_DDJ.to_string(),
    ));
    page.spawn(image(LEADER_RACE_RECT, LEADER_RACE_DDJ.to_string()));
    page.spawn(image(
        (
            GP_BUTTON_POS.0,
            GP_BUTTON_POS.1,
            GP_BUTTON_SIZE.0,
            GP_BUTTON_SIZE.1,
        ),
        GP_BUTTON_DDJ.to_string(),
    ));

    // GDR_GUILD_INFO_GP_GAUGE — the shared three-node gauge recipe
    // (`hud/gauge.rs`): track (authored rect) / crop (the fill) / art (native).
    let (gx, gy) = GP_GAUGE_POS;
    page.spawn((
        abs_node((gx, gy, GP_GAUGE_ART.0, GP_GAUGE_ART.1), s),
        Pickable::IGNORE,
    ))
    .with_children(|track| {
        track
            .spawn((
                GuildGpFill,
                gauge_crop_node(
                    gauge_fill_width(0.0, GP_GAUGE_ART.0 * s),
                    GP_GAUGE_ART.1 * s,
                ),
                Pickable::IGNORE,
            ))
            .with_children(|crop| {
                crop.spawn((
                    gauge_art_node(GP_GAUGE_ART.0 * s, GP_GAUGE_ART.1 * s),
                    ImageNode {
                        image: asset_server.load(GP_GAUGE_DDJ),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            });
    });

    // The four fixed labels.
    for (rect, key, fallback, color, justify) in [
        (
            LEVEL_LABEL_RECT,
            "UIO_CHARINFO_STT_LEVEL",
            "Level",
            LEVEL_LABEL_COLOR,
            Justify::Left,
        ),
        (
            LEADER_LABEL_RECT,
            "UIIT_STT_GUILD_LEADER",
            "Guild master",
            LABEL_COLOR,
            Justify::Left,
        ),
        (
            MEMBER_LABEL_RECT,
            "UIIT_STT_GUILDSMAN_NUM",
            "Number",
            LABEL_COLOR,
            Justify::Right,
        ),
        (
            GP_LABEL_RECT,
            "UIIT_STT_GUILD_POINT",
            "Guild point (GP)",
            LABEL_COLOR,
            Justify::Left,
        ),
    ] {
        page.spawn((
            Text::new(ui_strings.get_or(key, fallback).to_string()),
            text_font(7.5),
            TextColor(color),
            TextLayout::justify(justify),
            abs_node(rect, s),
            Pickable::IGNORE,
        ));
    }

    // The six runtime statics, all starting in the guildless empty state.
    for (field, rect, justify) in [
        (GuildInfoField::Name, NAME_RECT, Justify::Left),
        (GuildInfoField::Level, LEVEL_RECT, Justify::Left),
        (GuildInfoField::Leader, LEADER_RECT, Justify::Left),
        (GuildInfoField::MemberCount, MEMBER_NUM_RECT, Justify::Right),
        (GuildInfoField::GpPercent, GP_PERCENT_RECT, Justify::Center),
        (GuildInfoField::GpValue, GP_VALUE_RECT, Justify::Center),
    ] {
        page.spawn((
            field,
            Text::new(EMPTY.to_string()),
            text_font(7.5),
            TextColor(VALUE_COLOR),
            TextLayout::justify(justify),
            abs_node(rect, s),
            Pickable::IGNORE,
        ));
    }

    // --- §MemberView + §SortBtn --------------------------------------------

    // GDR_GUILD_MEMBER_VIEW_BG: the right gutter tile.
    page.spawn((
        abs_node(MEMBER_VIEW_BG_RECT, s),
        ImageNode {
            image: asset_server.load(BG_TILE_DDJ),
            image_mode: NodeImageMode::Tiled {
                tile_x: true,
                tile_y: true,
                stretch_value: s,
            },
            ..default()
        },
        Pickable::IGNORE,
    ));

    // GDR_GUILD_MEMBER_VIEW_BLACKSQUARE: the list plate + its 4px trim.
    let (mx, my, mw, mh) = MEMBER_PLATE_RECT;
    page.spawn((
        abs_node(MEMBER_PLATE_RECT, s),
        BackgroundColor(Color::BLACK),
        Pickable::IGNORE,
    ));
    for ((x, y, w, h), piece) in blacksquare_ring(mw, mh) {
        page.spawn(image(
            (mx + x, my + y, w, h),
            format!("{BLACKSQUARE_DIR}{piece}.ddj"),
        ));
    }

    // The header strip: left cap, the four sort plates, right cap — then the
    // condition button *on top of* the left cap (see SORT_CONDITION).
    page.spawn(image(SORT_CAP_LEFT, SORT_CAP_LEFT_DDJ.to_string()));
    for (x, w, ddj, key, fallback) in SORT_BUTTONS {
        page.spawn(image((x, SORT_BUTTON_Y, w, SORT_BUTTON_H), ddj.to_string()))
            .with_children(|header| {
                header.spawn((
                    Text::new(ui_strings.get_or(key, fallback).to_string()),
                    text_font(7.5),
                    TextColor(VALUE_COLOR),
                    TextLayout::justify(Justify::Center),
                    Node {
                        position_type: PositionType::Absolute,
                        top: Val::Px(6.0 * s),
                        width: Val::Percent(100.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            });
    }
    page.spawn(image(SORT_CAP_RIGHT, SORT_CAP_RIGHT_DDJ.to_string()));
    page.spawn(image(SORT_CONDITION, SORT_CONDITION_DDJ.to_string()));

    // The six visible member rows, each a slot-local transcription of
    // `ifguildmemberslot.txt` translated to the manager's origin.
    let (lx, ly, _, _) = MEMBER_LIST_RECT;
    for row in 0..VISIBLE_ROWS {
        let top = ly + row as f32 * ROW_PITCH;
        page.spawn((
            GuildRosterOnline(row),
            {
                let mut node = abs_node(
                    (
                        lx + SLOT_ONOFF.0,
                        top + SLOT_ONOFF.1,
                        SLOT_ONOFF.2,
                        SLOT_ONOFF.3,
                    ),
                    s,
                );
                node.display = Display::None;
                node
            },
            ImageNode {
                image: asset_server.load(SLOT_ONOFF_OFF_DDJ),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
        for (column, rect) in [
            (RosterColumn::Name, SLOT_NAME),
            (RosterColumn::Level, SLOT_LEVEL),
            (RosterColumn::Grade, SLOT_GRADE),
            (RosterColumn::DonatedGp, SLOT_DONATED_GP),
        ] {
            page.spawn((
                GuildRosterCell { row, column },
                Text::new(String::new()),
                text_font(7.5),
                TextColor(VALUE_COLOR),
                TextLayout::justify(Justify::Center),
                abs_node((lx + rect.0, top + rect.1, rect.2, rect.3), s),
                Pickable::IGNORE,
            ));
        }
    }

    // --- §NotifySubBox ------------------------------------------------------

    page.spawn(label_static(
        NOTICE_LABEL_RECT,
        ui_strings
            .get_or("UIIT_STT_GUILD_COMMON_KNOW", "Notice")
            .to_string(),
        LABEL_COLOR,
        Justify::Left,
        &text_font(7.5),
        s,
    ));
    page.spawn((
        GuildNoticeField::Subject,
        Text::new(String::new()),
        text_font(7.5),
        TextColor(VALUE_COLOR),
        TextLayout::justify(Justify::Left),
        abs_node(NOTICE_SUBJECT_TEXT_RECT, s),
        Pickable::IGNORE,
    ));
    // the invisible CIFSelectableArea over the strip
    page.spawn((
        Button,
        Hovered::default(),
        abs_node(NOTICE_SUBJECT_RECT, s),
        Pickable::default(),
    ))
    .observe(on_notice_subject);
    page.spawn(image(NOTICE_EDIT_BTN, NOTICE_EDIT_BTN_DDJ.to_string()));

    // --- §NotifyContents — the read pane, hidden until the strip is clicked --

    let mut pane_node = abs_node(NOTICE_PANE_RECT, s);
    pane_node.display = Display::None;
    page.spawn((GuildNoticePane, pane_node, Pickable::IGNORE))
        .with_children(|pane| {
            // the plate: flat black + its 4px trim
            pane.spawn((
                abs_node(NOTICE_PLATE_RECT, s),
                BackgroundColor(Color::BLACK),
                Pickable::IGNORE,
            ));
            let (px, py, pw, ph) = NOTICE_PLATE_RECT;
            for ((x, y, w, h), piece) in blacksquare_ring(pw, ph) {
                pane.spawn((
                    abs_node((px + x, py + y, w, h), s),
                    ImageNode {
                        image: asset_server.load(format!("{BLACKSQUARE_DIR}{piece}.ddj")),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            }
            for (rect, ddj) in [
                (NOTICE_BG02_RECT, NOTICE_BG02_DDJ),
                (NOTICE_BG01_RECT, BG_TILE_DDJ),
            ] {
                pane.spawn((
                    abs_node(rect, s),
                    ImageNode {
                        image: asset_server.load(ddj),
                        image_mode: NodeImageMode::Tiled {
                            tile_x: true,
                            tile_y: true,
                            stretch_value: s,
                        },
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            }
            pane.spawn((
                GuildNoticeField::Body,
                Text::new(String::new()),
                text_font(7.5),
                TextColor(VALUE_COLOR),
                TextLayout::justify(Justify::Left),
                abs_node(NOTICE_TEXT_RECT, s),
                Pickable::IGNORE,
            ));
            pane.spawn((
                Button,
                Hovered::default(),
                abs_node(NOTICE_OK_RECT, s),
                ImageNode {
                    image: asset_server.load(NOTICE_OK_DDJ),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
            ))
            .observe(on_notice_confirm)
            .with_children(|button| {
                button.spawn((
                    Text::new(ui_strings.get_or("UIIS_CTL_CONFIRM", "OK").to_string()),
                    text_font(7.5),
                    TextColor(NOTICE_OK_COLOR),
                    TextLayout::justify(Justify::Center),
                    Node {
                        position_type: PositionType::Absolute,
                        top: Val::Px(6.0 * s),
                        width: Val::Percent(100.0),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            });
        });

    // --- §Command — the action column ---------------------------------------
    //
    // Slot 1 ("Join") is live: it invites the click-selected player via
    // 0x70F3. The other four stay presentational, and not for lack of an
    // opcode — expel is name-addressed and needs a roster-row selection this
    // page does not have, and leave/disband/promote each carry one `u32` whose
    // meaning the decompile does not give. `letter.rs` set the precedent: draw
    // the plate and its caption rather than attach an observer that would send
    // an invented value.
    for (slot, (y, (key, fallback))) in COMMAND_YS
        .iter()
        .zip(command_keys(position_grant))
        .enumerate()
    {
        let mut button = page.spawn(image(
            (COMMAND_X, *y, COMMAND_BTN_SIZE.0, COMMAND_BTN_SIZE.1),
            COMMAND_BTN_DDJ.to_string(),
        ));
        if slot == COMMAND_SLOT_INVITE {
            button.insert((Button, Hovered::default(), Pickable::default()));
            button.observe(on_guild_invite);
        }
        button.with_children(|button| {
            button.spawn((
                Text::new(ui_strings.get_or(key, fallback).to_string()),
                text_font(7.5),
                TextColor(COMMAND_TEXT_COLOR),
                TextLayout::justify(Justify::Center),
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(6.0 * s),
                    width: Val::Percent(100.0),
                    ..default()
                },
                Pickable::IGNORE,
            ));
        });
    }
}

/// A plain `CIFStatic` label bundle — the roster/notice sections spawn enough
/// of them that the tuple is worth a name.
fn label_static(
    rect: (f32, f32, f32, f32),
    text: String,
    color: Color,
    justify: Justify,
    font: &TextFont,
    s: f32,
) -> impl Bundle {
    (
        Text::new(text),
        font.clone(),
        TextColor(color),
        TextLayout::justify(justify),
        abs_node(rect, s),
        Pickable::IGNORE,
    )
}

/// The six 4px trim pieces of a `com_blacksquare_` plate over a `w x h` box.
fn blacksquare_ring(w: f32, h: f32) -> [((f32, f32, f32, f32), &'static str); 6] {
    let p = BLACKSQUARE_PIECE;
    [
        ((0.0, 0.0, p, p), "left_up"),
        ((w - p, 0.0, p, p), "right_up"),
        ((0.0, p, p, h - 2.0 * p), "left_side"),
        ((w - p, p, p, h - 2.0 * p), "right_side"),
        ((0.0, h - p, p, p), "left_down"),
        ((w - p, h - p, p, p), "right_down"),
    ]
}

/// Push `GuildRoster`'s members onto the six visible rows.
///
/// No scrolling: the `CIFScrollManager` row pool is the shared widget of #56
/// and is not built here, so a guild with more than six members shows its
/// first six. Stated rather than silently truncated.
pub fn update_guild_roster(
    roster: Res<GuildRoster>,
    asset_server: Res<AssetServer>,
    mut cells: Query<(&GuildRosterCell, &mut Text)>,
    mut dots: Query<(&GuildRosterOnline, &mut Node, &mut ImageNode)>,
) {
    if !roster.is_changed() {
        return;
    }
    let members = roster
        .data
        .as_ref()
        .map(|data| data.members.as_slice())
        .unwrap_or(&[]);
    for (cell, mut text) in cells.iter_mut() {
        let next = cell_text(cell.column, members.get(cell.row));
        if text.0 != next {
            text.0 = next;
        }
    }
    for (dot, mut node, mut image) in dots.iter_mut() {
        match members.get(dot.0) {
            Some(member) => {
                node.display = Display::Flex;
                image.image = asset_server.load(if member.is_offline {
                    SLOT_ONOFF_OFF_DDJ
                } else {
                    SLOT_ONOFF_ON_DDJ
                });
            }
            None => node.display = Display::None,
        }
    }
}

/// Width of an art-sized static after clipping at the page's right edge.
/// The `[U]` in `hud-guild-window.md` §9-U4 is *how* the original handles the
/// two overflows; clipping is our stated decision, and this is the one place
/// it happens.
fn clipped_width(x: f32, art_w: f32) -> f32 {
    (PAGE_SIZE.0 - x).min(art_w).max(0.0)
}

/// Push `GuildRoster` onto the six runtime statics whenever the record moves.
pub fn update_guild_info(
    roster: Res<GuildRoster>,
    mut fields: Query<(&GuildInfoField, &mut Text)>,
) {
    if !roster.is_changed() {
        return;
    }
    for (field, mut text) in fields.iter_mut() {
        let next = field_text(*field, roster.data.as_ref());
        if text.0 != next {
            text.0 = next;
        }
    }
}

/// The 8 pieces of a `*_wnd_` frame ring over a `w x h` box, at `p` px pieces.
fn ring(w: f32, h: f32, p: f32) -> [((f32, f32, f32, f32), &'static str); 8] {
    [
        ((0.0, 0.0, p, p), "left_up"),
        ((p - 1.0, 0.0, w - 2.0 * p + 2.0, p), "mid_up"),
        ((w - p, 0.0, p, p), "right_up"),
        ((0.0, p - 1.0, p, h - 2.0 * p + 2.0), "left_side"),
        ((w - p, p - 1.0, p, h - 2.0 * p + 2.0), "right_side"),
        ((0.0, h - p, p, p), "left_down"),
        ((p - 1.0, h - p, w - 2.0 * p + 2.0, p), "mid_down"),
        ((w - p, h - p, p, p), "right_down"),
    ]
}

#[cfg(test)]
mod test {
    use super::*;
    use packets::agent::guild::{GuildData, GuildMember};

    fn member(name: &str, is_master: bool) -> GuildMember {
        GuildMember {
            member_id: 1,
            name: name.to_string(),
            unk_u8_01: 0,
            level: 40,
            guild_points: 0,
            permissions: 0,
            unk_u32_01: 0,
            unk_u32_02: 0,
            unk_u32_03: 0,
            nickname: String::new(),
            model_id: 0,
            is_master,
            is_offline: false,
        }
    }

    fn record() -> GuildData {
        GuildData {
            guild_id: 7,
            name: "Roadmen".to_string(),
            level: 3,
            guild_points: 12_345,
            notice: String::new(),
            message: String::new(),
            unk_u32_00: 0,
            unk_u8_00: 0,
            member_count: 2,
            members: vec![member("Grunt", false), member("Master", true)],
        }
    }

    /// The header strip is transcribed with its **overlapping** seams: the
    /// plates butt into each other by −1/−3/−2 and the last one ends exactly
    /// on the right cap. Reproducing that is the point — a "corrected" clean
    /// tiling would not be vanilla's strip.
    #[test]
    fn sort_header_seams_overlap_exactly_as_ifguild_declares() {
        let x: Vec<f32> = SORT_BUTTONS.iter().map(|b| b.0).collect();
        let w: Vec<f32> = SORT_BUTTONS.iter().map(|b| b.1).collect();
        assert_eq!(x, vec![39.0, 170.0, 211.0, 261.0]);
        assert_eq!(w, vec![132.0, 44.0, 52.0, 68.0]);
        assert_eq!(x[0] + w[0] - x[1], 1.0); // −1
        assert_eq!(x[1] + w[1] - x[2], 3.0); // −3
        assert_eq!(x[2] + w[2] - x[3], 2.0); // −2
        assert_eq!(x[3] + w[3], SORT_CAP_RIGHT.0); // 261+68 = 329, no gap
                                                   // the strip spans exactly the scroll manager's 17..345
        assert_eq!(SORT_CAP_LEFT.0, MEMBER_LIST_RECT.0);
        assert_eq!(
            SORT_CAP_RIGHT.0 + SORT_CAP_RIGHT.2,
            MEMBER_LIST_RECT.0 + MEMBER_LIST_RECT.2
        );
    }

    /// Six rows at pitch 23 fit the manager's 139 height exactly
    /// (`139 = 6·23 + 1`), and every slot-local cell fits the row's 24 px.
    #[test]
    fn six_member_rows_fit_the_scroll_manager() {
        assert_eq!(VISIBLE_ROWS as f32 * ROW_PITCH + 1.0, MEMBER_LIST_RECT.3);
        let last_top = (VISIBLE_ROWS - 1) as f32 * ROW_PITCH;
        assert!(last_top + ROW_HEIGHT <= MEMBER_LIST_RECT.3 + ROW_PITCH - ROW_HEIGHT + 1.0);
        for (x, y, w, h) in [
            SLOT_ONOFF,
            SLOT_RACE_MARK,
            SLOT_NAME,
            SLOT_LEVEL,
            SLOT_GRADE,
            SLOT_DONATED_GP,
        ] {
            assert!(
                y + h <= ROW_HEIGHT,
                "cell {x},{y},{w},{h} overflows the row"
            );
            assert!(
                x + w <= MEMBER_LIST_RECT.2,
                "cell {x},{y},{w},{h} overflows the manager width"
            );
        }
        // the doc's alignment check: every cell centre falls in its column
        assert!(SLOT_NAME.0 + SLOT_NAME.2 / 2.0 > SORT_BUTTONS[0].0 - MEMBER_LIST_RECT.0);
        assert_eq!(SLOT_DONATED_GP.0 + SLOT_DONATED_GP.2, 302.0);
    }

    /// The roster cells bind the record, and the Grade column stays blank:
    /// `GuildMember` carries no grade field, so filling it would be inventing
    /// the column's meaning.
    #[test]
    fn roster_cells_bind_the_member_record_and_leave_grade_blank() {
        let members = record().members;
        let grunt = members.first();
        assert_eq!(cell_text(RosterColumn::Name, grunt), "Grunt");
        assert_eq!(cell_text(RosterColumn::Level, grunt), "40");
        assert_eq!(cell_text(RosterColumn::DonatedGp, grunt), "0");
        assert_eq!(cell_text(RosterColumn::Grade, grunt), "");
        // past the roster's end every cell is empty, not a stale row
        for column in [
            RosterColumn::Name,
            RosterColumn::Level,
            RosterColumn::Grade,
            RosterColumn::DonatedGp,
        ] {
            assert_eq!(cell_text(column, None), "");
        }
    }

    /// The roster pane's own three rects stay inside the page, and the plate
    /// contains the row area it hosts.
    #[test]
    fn member_view_rects_fit_the_page_and_contain_the_row_area() {
        let (pw, ph) = PAGE_SIZE;
        for (x, y, w, h) in [MEMBER_PLATE_RECT, MEMBER_LIST_RECT, MEMBER_VIEW_BG_RECT] {
            assert!(x + w <= pw, "rect {x},{y},{w},{h} overflows the page width");
            assert!(
                y + h <= ph,
                "rect {x},{y},{w},{h} overflows the page height"
            );
        }
        assert!(MEMBER_PLATE_RECT.0 <= MEMBER_LIST_RECT.0);
        assert!(
            MEMBER_PLATE_RECT.1 + MEMBER_PLATE_RECT.3 >= MEMBER_LIST_RECT.1 + MEMBER_LIST_RECT.3
        );
    }

    /// The three y=138 panes share their anchor — that shared rect *is* the
    /// client's "if" — so the read pane must fully cover the roster plate it
    /// replaces, and only one may be visible.
    #[test]
    fn the_notice_pane_covers_the_roster_plate_it_replaces() {
        assert_eq!(NOTICE_PANE_RECT.1, MEMBER_PLATE_RECT.1); // both anchored at y=138
        assert!(NOTICE_PANE_RECT.0 <= MEMBER_PLATE_RECT.0);
        assert!(
            NOTICE_PANE_RECT.0 + NOTICE_PANE_RECT.2 >= MEMBER_PLATE_RECT.0 + MEMBER_PLATE_RECT.2
        );
        assert!(
            NOTICE_PANE_RECT.1 + NOTICE_PANE_RECT.3 >= MEMBER_PLATE_RECT.1 + MEMBER_PLATE_RECT.3
        );
        // and the pane still fits the page
        assert!(NOTICE_PANE_RECT.0 + NOTICE_PANE_RECT.2 <= PAGE_SIZE.0);
        assert!(NOTICE_PANE_RECT.1 + NOTICE_PANE_RECT.3 <= PAGE_SIZE.1);
    }

    /// Every `ifguildnotifycontents.txt` child is host-relative and fits the
    /// `6,138,440,177` host — the reading that makes the file consistent.
    #[test]
    fn notice_pane_children_are_host_relative_and_fit() {
        for (x, y, w, h) in [
            NOTICE_PLATE_RECT,
            NOTICE_BG02_RECT,
            NOTICE_TEXT_RECT,
            NOTICE_SCROLL_RECT,
            NOTICE_BG01_RECT,
            NOTICE_OK_RECT,
        ] {
            assert!(
                x + w <= NOTICE_PANE_RECT.2,
                "child {x},{y},{w},{h} overflows the host width"
            );
            assert!(
                y + h <= NOTICE_PANE_RECT.3,
                "child {x},{y},{w},{h} overflows the host height"
            );
        }
        // the body text sits inside its bg tile, which sits inside the plate
        assert!(NOTICE_TEXT_RECT.0 >= NOTICE_BG02_RECT.0);
        assert!(NOTICE_BG02_RECT.0 >= NOTICE_PLATE_RECT.0);
    }

    /// The strip shows the notice **title** and the pane its **body** — the
    /// same split `GuildNoticeEditRequest { title, message }` writes back.
    #[test]
    fn notice_strip_shows_the_title_and_the_pane_the_body() {
        let mut data = record();
        data.notice = "Server maintenance".to_string();
        data.message = "We move at 20:00.".to_string();
        assert_eq!(
            notice_text(GuildNoticeField::Subject, Some(&data)),
            "Server maintenance"
        );
        assert_eq!(
            notice_text(GuildNoticeField::Body, Some(&data)),
            "We move at 20:00."
        );
        // guildless: empty, not a stale notice
        assert_eq!(notice_text(GuildNoticeField::Subject, None), "");
        assert_eq!(notice_text(GuildNoticeField::Body, None), "");
    }

    /// The §NotifySubBox strip's own three rects sit above the pane anchor and
    /// inside the page.
    #[test]
    fn notify_subbox_strip_fits_above_the_pane_anchor() {
        for (x, y, w, h) in [
            NOTICE_LABEL_RECT,
            NOTICE_SUBJECT_RECT,
            NOTICE_SUBJECT_TEXT_RECT,
            NOTICE_EDIT_BTN,
        ] {
            assert!(x + w <= PAGE_SIZE.0, "rect {x},{y},{w},{h} overflows width");
            assert!(y < NOTICE_PANE_RECT.1 + NOTICE_PANE_RECT.3);
        }
        // the subject text starts right of the label and ends before the button
        assert!(NOTICE_SUBJECT_TEXT_RECT.0 >= NOTICE_LABEL_RECT.0 + NOTICE_LABEL_RECT.2);
        assert!(NOTICE_SUBJECT_TEXT_RECT.0 + NOTICE_SUBJECT_TEXT_RECT.2 <= NOTICE_EDIT_BTN.0);
    }

    /// The five slots are the vanilla 27-pitch column and the whole column

    /// The wire path is attached to the caption the data names "Join", not to a
    /// slot index that a later reorder could silently move.
    #[test]
    fn the_live_command_slot_is_the_join_button() {
        assert_eq!(
            command_keys(false)[COMMAND_SLOT_INVITE].0,
            "UIIT_STT_GUILD_JOIN"
        );
        assert_eq!(
            command_keys(true)[COMMAND_SLOT_INVITE].0,
            "UIIT_STT_GUILD_JOIN"
        );
    }

    /// fits the page (`353 + 88 = 441 <= 451`).
    #[test]
    fn command_column_keeps_the_vanilla_pitch_and_fits() {
        assert_eq!(COMMAND_YS, [142.0, 169.0, 196.0, 223.0, 250.0]);
        for pair in COMMAND_YS.windows(2) {
            assert_eq!(pair[1] - pair[0], 27.0);
        }
        assert_eq!(COMMAND_X + COMMAND_BTN_SIZE.0, 441.0);
        assert!(COMMAND_X + COMMAND_BTN_SIZE.0 <= PAGE_SIZE.0);
        assert!(COMMAND_YS[4] + COMMAND_BTN_SIZE.1 <= PAGE_SIZE.1);
    }

    /// Six buttons, five slots: ids 105 and 106 share `353,250` byte-for-byte,
    /// so exactly one caption is ever drawn in the last slot and the flag is
    /// the only thing that picks it.
    #[test]
    fn slot_five_draws_exactly_one_of_the_two_colliding_buttons() {
        let default = command_keys(false);
        let flagged = command_keys(true);
        assert_eq!(default.len(), COMMAND_YS.len());
        assert_eq!(flagged.len(), COMMAND_YS.len());
        // the first four are identical in both configurations
        assert_eq!(default[..4], flagged[..4]);
        // ...and only the fifth differs
        assert_eq!(default[4], COMMAND_SLOT5_DEFAULT);
        assert_eq!(flagged[4], COMMAND_SLOT5_POSITION_GRANT);
        assert_ne!(default[4], flagged[4]);
        // the position-grant surface is off unless asked for
        assert!(!crate::plugins::config::guild::GuildSettings::default().position_grant);
    }

    /// The captions are the six string keys `ifguild.txt` binds, and nothing
    /// else — no invented sixth slot, no renamed command.
    #[test]
    fn command_captions_are_the_ifguild_string_keys() {
        let keys: Vec<&str> = command_keys(false)
            .iter()
            .chain(std::iter::once(&COMMAND_SLOT5_POSITION_GRANT))
            .map(|(key, _)| *key)
            .collect();
        assert_eq!(
            keys,
            vec![
                "UIIT_STT_GUILD_JOIN",           // id 101, :586
                "UIIT_CTL_AUTHORITY_GRANT",      // id 102, :567
                "UIIT_STT_GUILD_EXPULSION",      // id 103, :548
                "UIIT_STT_GUILD_EXIT",           // id 104, :529
                "UIIT_STT_GUILD_NAME_GRANT",     // id 105, :510
                "UIIT_CTL_GUILD_POSITION_GRANT", // id 106, :491
            ]
        );
    }

    /// Transcription pin: every §GuildInfo rect is the one `ifguild.txt`
    /// declares, so a later refactor cannot quietly nudge the layout.
    #[test]
    fn guild_info_rects_are_the_ifguild_txt_rects() {
        assert_eq!(INFO_WND_POS, (6.0, 4.0)); // :15
        assert_eq!(FRAME_RECT, (6.0, 103.0, 440.0, 211.0)); // :34
        assert_eq!(BG_RECT, (27.0, 105.0, 403.0, 58.0)); // :53
        assert_eq!(MARK_POS, (144.0, 16.0)); // :228
        assert_eq!(NAME_RECT, (166.0, 18.0, 80.0, 14.0)); // :209
        assert_eq!(LEVEL_LABEL_RECT, (381.0, 18.0, 34.0, 15.0)); // :304
        assert_eq!(LEVEL_RECT, (419.0, 18.0, 17.0, 15.0)); // :190
        assert_eq!(LEADER_LABEL_RECT, (28.0, 51.0, 84.0, 14.0)); // :285
        assert_eq!(LEADER_RACE_RECT, (90.0, 49.0, 16.0, 16.0)); // :171
        assert_eq!(LEADER_RECT, (112.0, 51.0, 110.0, 14.0)); // :152
        assert_eq!(MEMBER_LABEL_RECT, (261.0, 51.0, 67.0, 14.0)); // :266
        assert_eq!(MEMBER_NUM_RECT, (339.0, 51.0, 67.0, 14.0)); // :133
        assert_eq!(BAR_BOARD_POS, (120.0, 67.0)); // :342
        assert_eq!(GP_LABEL_RECT, (28.0, 75.0, 89.0, 14.0)); // :247
        assert_eq!(GP_GAUGE_POS, (128.0, 76.0)); // :323
        assert_eq!(GP_PERCENT_RECT, (168.0, 75.0, 64.0, 14.0)); // :114
        assert_eq!(GP_VALUE_RECT, (276.0, 75.0, 129.0, 14.0)); // :95
        assert_eq!(GP_BUTTON_POS, (410.0, 72.0)); // :76
    }

    /// Exactly two art-sized statics run past the 451x320 page, and both are
    /// the ones §3.10 measured — everything else must fit as authored.
    #[test]
    fn only_the_two_documented_guild_statics_overflow_the_page() {
        let (pw, ph) = PAGE_SIZE;
        for (x, y, w, h) in [
            FRAME_RECT,
            BG_RECT,
            NAME_RECT,
            LEVEL_LABEL_RECT,
            LEVEL_RECT,
            LEADER_LABEL_RECT,
            LEADER_RACE_RECT,
            LEADER_RECT,
            MEMBER_LABEL_RECT,
            MEMBER_NUM_RECT,
            GP_LABEL_RECT,
            GP_PERCENT_RECT,
            GP_VALUE_RECT,
            (MARK_POS.0, MARK_POS.1, MARK_SIZE.0, MARK_SIZE.1),
            (
                GP_GAUGE_POS.0,
                GP_GAUGE_POS.1,
                GP_GAUGE_ART.0,
                GP_GAUGE_ART.1,
            ),
            (
                GP_BUTTON_POS.0,
                GP_BUTTON_POS.1,
                GP_BUTTON_SIZE.0,
                GP_BUTTON_SIZE.1,
            ),
        ] {
            assert!(x + w <= pw, "rect {x},{y},{w},{h} overflows the page width");
            assert!(
                y + h <= ph,
                "rect {x},{y},{w},{h} overflows the page height"
            );
        }
        // The two that do overflow (+143 and +45) are clipped, not clamped
        // away and not squashed: the art keeps its native width inside.
        assert_eq!(INFO_WND_POS.0 + INFO_WND_ART.0, 594.0);
        assert_eq!(BAR_BOARD_POS.0 + BAR_BOARD_ART.0, 496.0);
        assert_eq!(clipped_width(INFO_WND_POS.0, INFO_WND_ART.0), 445.0);
        assert_eq!(clipped_width(BAR_BOARD_POS.0, BAR_BOARD_ART.0), 331.0);
        // A static that fits is never shrunk by the clip helper.
        assert_eq!(clipped_width(MARK_POS.0, MARK_SIZE.0), MARK_SIZE.0);
    }

    /// Guildless renders the same layout with a defined empty state — the
    /// page is not hidden and no field is left blank.
    #[test]
    fn guildless_guild_page_shows_the_empty_state() {
        for field in [
            GuildInfoField::Name,
            GuildInfoField::Level,
            GuildInfoField::Leader,
            GuildInfoField::MemberCount,
            GuildInfoField::GpPercent,
            GuildInfoField::GpValue,
        ] {
            assert_eq!(field_text(field, None), EMPTY, "{field:?}");
        }
    }

    /// The header binds `GuildRoster`: name, level, the master's name and the
    /// member count all come from the record, GP is thousands-grouped.
    #[test]
    fn guild_header_binds_the_guild_record() {
        let data = record();
        assert_eq!(field_text(GuildInfoField::Name, Some(&data)), "Roadmen");
        assert_eq!(field_text(GuildInfoField::Level, Some(&data)), "3");
        assert_eq!(field_text(GuildInfoField::Leader, Some(&data)), "Master");
        assert_eq!(field_text(GuildInfoField::MemberCount, Some(&data)), "2");
        assert_eq!(field_text(GuildInfoField::GpValue, Some(&data)), "12,345");
    }

    /// A record with no master flag must not invent one.
    #[test]
    fn guild_leader_falls_back_when_no_member_claims_the_master_flag() {
        let mut data = record();
        for m in data.members.iter_mut() {
            m.is_master = false;
        }
        assert_eq!(field_text(GuildInfoField::Leader, Some(&data)), EMPTY);
    }

    /// No GP threshold exists client-side, so the percent readout stays empty
    /// and the gauge fill stays at zero even for a guild with points — the
    /// alternative is inventing a maximum (module doc).
    #[test]
    fn gp_gauge_stays_empty_until_a_threshold_exists() {
        let data = record();
        assert_eq!(field_text(GuildInfoField::GpPercent, Some(&data)), EMPTY);
        assert_eq!(gauge_fill_width(0.0, GP_GAUGE_ART.0), Val::Px(0.0));
    }
}
