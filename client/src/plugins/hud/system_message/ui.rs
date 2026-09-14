//! System-message view geometry (`GDR_SYSTEM_MESSAGE_VIEW`, `CIFSystemMessage`
//! id 68) — the transcription layer of `docs/re/ui/hud-system-message.md` §3.
//!
//! Idea: this module answers the one question that has to be settled *before*
//! any node is spawned — **what hosts this surface**. The original instantiates
//! id 68 in `ginterface.txt:124`, the same Create section that instantiates the
//! chat board, so it is a **sibling** of chat, not a child of it: its own
//! 8-block tree (`ifsystemmessage.txt`), no input control, no channel tabs, and
//! a filter board over message *classes* rather than chat's *channels*
//! (`docs/re/ui/hud-system-message.md` §2). Folding it into chat's tree — as a
//! tab, a ring-buffer kind or a child node — would therefore be structurally
//! wrong however it is styled, which is why the surface gets its own module and
//! its own log resource ([`super::model::SystemMessageLog`]) rather than another
//! `ChatLineKind`.
//!
//! What sibling-hood does *not* settle is the pixel arbitration, and that is why
//! nothing is spawned here yet: id 68's screen frame is **fully contained** in
//! the chat board's on all four edges (§3.4), resinfo has no `ZOrder`, `Visible`
//! or `Modal` key in its whole 30-key grammar, and the tree's own extent
//! (353×285) contradicts the host frame (350×126) by 159 px of height. Both are
//! open UNKNOWNs (§9-U1/U6) whose resolving read is a `wndpos.dat` / static-RE
//! pass. So this module lands the *data* — every rect, id and art path cited to
//! its resinfo line, with the arithmetic the doc verifies pinned as tests — and
//! leaves the spawn to the pass that can place it without covering our collapsed
//! chat on every pixel.

/// Host frame on screen, `ginterface.txt:133` `Rect = 0,552,350,126` (id 68 at
/// `:132`). Sibling of `GDR_CHAT_BOARD`, both created in `ginterface.txt`'s
/// single `Create` section.
pub const HOST_RECT: (f32, f32, f32, f32) = (0.0, 552.0, 350.0, 126.0);

/// `GDR_CHAT_BOARD`'s screen frame, `docs/re/ui/hud-chat.md:16` — quoted here
/// only so the containment relation is checkable in one place.
pub const CHAT_BOARD_RECT: (f32, f32, f32, f32) = (0.0, 546.0, 399.0, 398.0);

/// Underbar top, `docs/re/ui/scene-game-hud-composition.md:18`.
pub const UNDERBAR_TOP: f32 = 684.0;

// --- the 8 blocks of `resinfo/ifsystemmessage.txt`, window-local -------------
// Rects are `(x, y, w, h)`; the trailing comment is the block's resinfo line and
// its `ID`. The `SYETEM` misspelling is the data's own (§9-U8).

/// `GDR_SYETEM_MESSAGE_OPTBOARD:CIFChatOptionBoard` — `:6`, ID 50,
/// `Style=64` (`:16`), art `ifcommon\window_all.ddj` (`:10`). Pops **upward**:
/// bottom at local `-155 + 156 = 1`, overlapping the background's top row.
pub const OPTBOARD_RECT: (f32, f32, f32, f32) = (207.0, -155.0, 144.0, 156.0);
/// Its atlas region, `ifsystemmessage.txt:19-22` → `window_all.ddj`
/// (401,0)-(542,153). **Not** the whisper list's (741,166)-(882,319) that
/// `hud/chat/ui.rs:130` records — same 141×153 size, different panel (§4.2).
pub const OPTBOARD_CROP: (f32, f32, f32, f32) = (401.0, 0.0, 542.0, 153.0);

/// `GDR_SYETEM_MESSAGE_VSCROLL:CIFVerticalScroll` — `:25`, ID 24. Parts come
/// from `ifverticalscroll.txt`, not from this file.
pub const VSCROLL_RECT: (f32, f32, f32, f32) = (337.0, 37.0, 16.0, 195.0);

/// `GDR_SYETEM_MESSAGE_TEXTBOX:CIFTextBox` — `:44`, ID 19. The only content
/// node; the surface has no input control of any class.
pub const TEXTBOX_RECT: (f32, f32, f32, f32) = (10.0, 15.0, 316.0, 252.0);

/// `GDR_SYETEM_MESSAGE_SIZE_BTN:CIFButton` — `:63`, ID 15, art
/// `chattingwnd\chat_zoom.ddj` (`:67`). Declares `w,h = 0,0`; the effective
/// size is the art's [`BUTTON_ART`] 16×20 — [S], corroborated by chat's
/// `GDR_BTN_CHAT_SIZE` binding the same art with an explicit `16,20`.
pub const SIZE_BTN_POS: (f32, f32) = (337.0, 265.0);

/// `GDR_SYETEM_MESSAGE_CHATOPTION_BTN:CIFButton` — `:82`, ID 13, art
/// `chattingwnd\chat_filter_button.ddj` (`:86`). Opens [`OPTBOARD_RECT`].
pub const FILTER_BTN_POS: (f32, f32) = (337.0, 0.0);

/// Both buttons measure 16×20 in their DDJ headers (§4.2), and neither ships a
/// `_disable` frame — 3-state art.
pub const BUTTON_ART: (f32, f32) = (16.0, 20.0);

/// Background 3-slice, `chattingwnd\chat_window.ddj` — `:139` ID 5 (up),
/// `:120` ID 6 (mid), `:101` ID 7 (down).
pub const BG_UP_RECT: (f32, f32, f32, f32) = (0.0, 0.0, 335.0, 4.0);
pub const BG_MID_RECT: (f32, f32, f32, f32) = (0.0, 4.0, 335.0, 277.0);
pub const BG_DOWN_RECT: (f32, f32, f32, f32) = (0.0, 281.0, 335.0, 4.0);

/// The strip is 384×12 and its `u` run is `0 → 0.992188` = **381** source px
/// (§3.3) — but this tree stretches those 381 into **335**, where chat draws
/// the same 381 at native width (`hud/chat/ui.rs` `CHAT_BG_SLICE_W`). Do not
/// copy chat's constant; the squash is a property of the shipped data.
pub const BG_SOURCE_W: f32 = 381.0;
pub const BG_SLICE_W: f32 = 335.0;
pub const BG_SLICE_H: f32 = 4.0;

pub const BG_ART: &str = "media://interface/chattingwnd/chat_window.ddj";
pub const SIZE_BTN_ART: &str = "chat_zoom";
pub const FILTER_BTN_ART: &str = "chat_filter_button";

#[cfg(test)]
mod tests {
    use super::*;

    /// `docs/re/ui/hud-system-message.md` §3.3: the background is a contiguous
    /// vertical 3-slice that closes exactly on the tree's own bottom.
    #[test]
    fn background_three_slice_is_contiguous_and_closes_at_285() {
        assert_eq!(BG_UP_RECT.1 + BG_UP_RECT.3, BG_MID_RECT.1);
        assert_eq!(BG_MID_RECT.1 + BG_MID_RECT.3, BG_DOWN_RECT.1);
        assert_eq!(BG_DOWN_RECT.1 + BG_DOWN_RECT.3, 285.0);
        // 4 + 277 + 4 == 285
        assert_eq!(BG_UP_RECT.3 + BG_MID_RECT.3 + BG_DOWN_RECT.3, 285.0);
    }

    /// The `u` run of `chat_window.ddj` (384 wide) is exactly 381 px, and this
    /// tree squashes it into the 335-wide background.
    #[test]
    fn background_source_run_is_381_and_is_squashed_to_335() {
        assert_eq!(0.9921875_f32 * 384.0, BG_SOURCE_W);
        assert_eq!(BG_SLICE_W, 335.0);
        assert!(BG_SLICE_W < BG_SOURCE_W);
    }

    /// §3.3: the control column sits 2 px right of the 335-wide background and
    /// sets the tree's own width, 353.
    #[test]
    fn control_column_sets_the_tree_width_to_353() {
        assert_eq!(FILTER_BTN_POS.0, VSCROLL_RECT.0);
        assert_eq!(SIZE_BTN_POS.0, VSCROLL_RECT.0);
        assert_eq!(VSCROLL_RECT.0 + VSCROLL_RECT.2, 353.0);
        assert_eq!(VSCROLL_RECT.0 - BG_SLICE_W, 2.0);
    }

    /// The size button's 16×20 art lands flush on the background's bottom —
    /// the third corroboration of the `w,h=0,0` blocks' effective size.
    #[test]
    fn size_button_art_lands_flush_on_the_background_bottom() {
        assert_eq!(SIZE_BTN_POS.1 + BUTTON_ART.1, 285.0);
        assert_eq!(FILTER_BTN_POS.1, 0.0);
    }

    /// §3.3: the filter board pops upward, its bottom overlapping the
    /// background's top row by 1 px, and it overhangs the host's 350 by 1.
    #[test]
    fn filter_board_pops_upward_over_the_top_row() {
        assert_eq!(OPTBOARD_RECT.1 + OPTBOARD_RECT.3, 1.0);
        assert!(OPTBOARD_RECT.1 < 0.0);
        assert_eq!(OPTBOARD_RECT.0 + OPTBOARD_RECT.2, 351.0);
        assert_eq!(HOST_RECT.2, 350.0);
        // shipped atlas region is 141x153, stretched into the 144x156 rect
        assert_eq!(OPTBOARD_CROP.2 - OPTBOARD_CROP.0, 141.0);
        assert_eq!(OPTBOARD_CROP.3 - OPTBOARD_CROP.1, 153.0);
    }

    /// §3.3: the scroll does not span the textbox — the doc's "delta 57" is the
    /// difference of the two *heights* (252 - 195), and it splits into 22 px
    /// missing at the top (`37 - 15`) and 35 px at the bottom (`267 - 232`).
    /// The first version of this test asserted 57 on the *bottom* edges and the
    /// gate caught it at 35: both numbers are real, they measure different
    /// things. Recorded as a property of the data, so we do not silently "fix"
    /// it into chat's drift.
    #[test]
    fn scroll_extent_is_57px_shorter_than_the_textbox() {
        // extents, `ifsystemmessage.txt:44` (textbox) and `:25` (scroll)
        assert_eq!(TEXTBOX_RECT.1 + TEXTBOX_RECT.3, 267.0);
        assert_eq!(VSCROLL_RECT.1 + VSCROLL_RECT.3, 232.0);
        // the doc's 57: 252-tall textbox against a 195-tall scroll
        assert_eq!(TEXTBOX_RECT.3 - VSCROLL_RECT.3, 57.0);
        // and how those 57 are distributed
        let top_gap = VSCROLL_RECT.1 - TEXTBOX_RECT.1;
        let bottom_gap = (TEXTBOX_RECT.1 + TEXTBOX_RECT.3) - (VSCROLL_RECT.1 + VSCROLL_RECT.3);
        assert_eq!(top_gap, 22.0);
        assert_eq!(bottom_gap, 35.0);
        assert_eq!(top_gap + bottom_gap, TEXTBOX_RECT.3 - VSCROLL_RECT.3);
    }

    /// §3.4, the five sums that close: the visible frame is the collapsed-chat
    /// slot inset 6 px top and bottom. [S] on the reading, [V] on the sums.
    #[test]
    fn host_frame_is_the_collapsed_chat_slot_inset_6px() {
        let (_, sys_top, _, sys_h) = HOST_RECT;
        let (_, chat_top, _, _) = CHAT_BOARD_RECT;
        assert_eq!(sys_top + sys_h, 678.0);
        assert_eq!(UNDERBAR_TOP - (sys_top + sys_h), 6.0);
        assert_eq!(sys_top - chat_top, 6.0);
        let collapsed_h = UNDERBAR_TOP - chat_top;
        assert_eq!(collapsed_h, 138.0);
        assert_eq!(collapsed_h - sys_h, 12.0);
        // the widths deliberately do NOT follow the pattern
        assert_eq!(CHAT_BOARD_RECT.2 - HOST_RECT.2, 49.0);
    }

    /// The structural fact this module exists to record: id 68 is a *sibling*
    /// of the chat board whose frame is fully contained in it, so the two
    /// cannot both be opaque in the same pixels and the arbitration is not in
    /// the data (§9-U1/U6). Spawning is blocked on that read.
    #[test]
    fn host_frame_is_fully_contained_in_the_chat_board() {
        let (sx, sy, sw, sh) = HOST_RECT;
        let (cx, cy, cw, ch) = CHAT_BOARD_RECT;
        assert!(sx >= cx);
        assert!(sy >= cy);
        assert!(sx + sw <= cx + cw);
        assert!(sy + sh <= cy + ch);
    }

    /// §3.4: the tree's own extent (353×285) contradicts the host frame
    /// (350×126) — at screen y 552 a 285-tall tree ends off a 768-tall canvas.
    #[test]
    fn tree_extent_contradicts_the_host_frame() {
        let tree_w = VSCROLL_RECT.0 + VSCROLL_RECT.2;
        let tree_h = BG_DOWN_RECT.1 + BG_DOWN_RECT.3;
        assert_eq!(tree_w - HOST_RECT.2, 3.0);
        assert_eq!(tree_h - HOST_RECT.3, 159.0);
        assert!(HOST_RECT.1 + tree_h > 768.0);
    }
}
