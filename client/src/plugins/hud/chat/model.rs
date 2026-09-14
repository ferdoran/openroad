//! Chat data model: a single ring buffer of typed lines that every tab
//! filters, plus the window/input state.
//!
//! Idea: the server's chat channels (packets `chat_type`) don't map 1:1 onto
//! the five vanilla tabs — the All tab is a catch-all for proximity/global/
//! system traffic and whispers appear in All/Party/Guild too. So lines are
//! stored once, tagged with a [`ChatLineKind`], and [`ChatTab::shows`] decides
//! per tab. Own sent messages don't enter the history until the server acks
//! them (0xB025), parked in [`ChatState::pending`] keyed by the rolling
//! `chat_index` the request carried.

use std::collections::HashMap;
use std::collections::VecDeque;

use bevy::prelude::*;

use packets::agent::chat::chat_type;

use crate::plugins::config::chat::ChatColors;

/// Ring buffer capacity (kept messages, all channels combined).
pub const CHAT_HISTORY_CAP: usize = 1000;
/// Cap on Text rows the list actually spawns, for layout cost; the ring still
/// stores [`CHAT_HISTORY_CAP`].
pub const MAX_RENDERED_LINES: usize = 300;

/// Display category of a chat line (finer than the five tabs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatLineKind {
    All,
    AllGm,
    Npc,
    Global,
    Notice,
    Stall,
    Party,
    Guild,
    Union,
    Academy,
    WhisperFrom,
    WhisperTo,
    /// Client-generated info lines (errors, restriction notices).
    System,
}

impl ChatLineKind {
    /// Line color from the user-configurable palette (`config.yaml` `chat.colors`).
    pub fn color(self, colors: &ChatColors) -> Color {
        match self {
            ChatLineKind::All | ChatLineKind::Npc | ChatLineKind::Stall => colors.normal,
            ChatLineKind::AllGm | ChatLineKind::Notice | ChatLineKind::System => colors.gm_notice,
            ChatLineKind::Global => colors.global,
            ChatLineKind::Party => colors.party,
            ChatLineKind::Guild => colors.guild,
            ChatLineKind::Union => colors.union,
            ChatLineKind::Academy => colors.academy,
            ChatLineKind::WhisperFrom | ChatLineKind::WhisperTo => colors.whisper,
        }
    }

    /// Whether clicking this line's sender starts a whisper to them.
    pub fn sender_is_whisper_target(self) -> bool {
        !matches!(
            self,
            ChatLineKind::Npc | ChatLineKind::Notice | ChatLineKind::System
        )
    }
}

/// One line in the chat history.
#[derive(Clone, Debug)]
pub struct ChatLine {
    pub kind: ChatLineKind,
    /// Sender display name; `None` for notices and system lines. For
    /// [`ChatLineKind::WhisperTo`] it holds the *recipient*.
    pub sender: Option<String>,
    pub text: String,
}

/// `UIIT_CHATERR_WHISPER_FROM_MESSAGE` / `_TO_MESSAGE` (textuisystem L1733 /
/// L1732) — the English values, used when the table is missing.
pub const WHISPER_FROM_FALLBACK: &str = "FROM";
pub const WHISPER_TO_FALLBACK: &str = "TO";
pub const WHISPER_FROM_KEY: &str = "UIIT_CHATERR_WHISPER_FROM_MESSAGE";
/// `UIIT_STT_CANT_CHATTING` (textuisystem L3065) — the chat-restriction
/// readout's text, a `%d`-templated string.
pub const CANT_CHATTING_KEY: &str = "UIIT_STT_CANT_CHATTING";
pub const CANT_CHATTING_FALLBACK: &str = "Chat Restricted: %d seconds";

/// Fill `UIIT_STT_CANT_CHATTING`'s `%d` with the remaining seconds.
///
/// The table is authored for the original's printf-style formatter, so the
/// placeholder is `%d`, not `{}`. A translation that drops the placeholder
/// still has to show the number, so it is appended rather than lost.
pub fn format_restriction(template: &str, seconds: u32) -> String {
    if template.contains("%d") {
        template.replacen("%d", &seconds.to_string(), 1)
    } else {
        format!("{template} ({seconds})")
    }
}
pub const WHISPER_TO_KEY: &str = "UIIT_CHATERR_WHISPER_TO_MESSAGE";

impl ChatLine {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            kind: ChatLineKind::System,
            sender: None,
            text: text.into(),
        }
    }

    /// The rendered line text (vanilla `sender:message` style; whispers use
    /// the `name(FROM|TO):message` form).
    ///
    /// The two whisper markers are localized: `UIIT_CHATERR_WHISPER_FROM_MESSAGE`
    /// (textuisystem L1733, "FROM") and `_TO_MESSAGE` (L1732, "TO"). They are
    /// passed in rather than looked up here so this stays a pure function.
    pub fn display_with(&self, from: &str, to: &str) -> String {
        match (self.kind, &self.sender) {
            (ChatLineKind::Notice, _) => format!("(Notice):{}", self.text),
            (ChatLineKind::WhisperFrom, Some(name)) => format!("{name}({from}):{}", self.text),
            (ChatLineKind::WhisperTo, Some(name)) => format!("{name}({to}):{}", self.text),
            (_, Some(name)) => format!("{}:{}", name, self.text),
            (_, None) => self.text.clone(),
        }
    }

    /// [`Self::display_with`] with the English fallbacks, for callers that
    /// have no string table at hand.
    pub fn display(&self) -> String {
        self.display_with(WHISPER_FROM_FALLBACK, WHISPER_TO_FALLBACK)
    }
}

/// The shared message ring buffer every tab filters from.
#[derive(Resource, Default)]
pub struct ChatHistory {
    lines: VecDeque<ChatLine>,
}

impl ChatHistory {
    pub fn push(&mut self, line: ChatLine) {
        if self.lines.len() >= CHAT_HISTORY_CAP {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub fn iter(&self) -> impl Iterator<Item = &ChatLine> {
        self.lines.iter()
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

/// The five vanilla viewer tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatTab {
    #[default]
    All,
    Party,
    Guild,
    Alliance,
    Academy,
}

impl ChatTab {
    /// Tab-bar label (with the channel prefix char, like vanilla).
    pub fn label(self) -> &'static str {
        match self {
            ChatTab::All => "All",
            ChatTab::Party => "#Party",
            ChatTab::Guild => "@Guild",
            ChatTab::Alliance => "%Union",
            ChatTab::Academy => "&Academy",
        }
    }

    /// Whether a line of `kind` is visible on this tab.
    pub fn shows(self, kind: ChatLineKind) -> bool {
        let whisper = matches!(kind, ChatLineKind::WhisperFrom | ChatLineKind::WhisperTo);
        match self {
            ChatTab::All => !matches!(
                kind,
                ChatLineKind::Party
                    | ChatLineKind::Guild
                    | ChatLineKind::Union
                    | ChatLineKind::Academy
            ),
            ChatTab::Party => kind == ChatLineKind::Party || whisper,
            ChatTab::Guild => kind == ChatLineKind::Guild || whisper,
            ChatTab::Alliance => kind == ChatLineKind::Union,
            ChatTab::Academy => kind == ChatLineKind::Academy,
        }
    }

    /// The wire chat type an unprefixed message sent from this tab uses.
    pub fn default_chat_type(self) -> u8 {
        match self {
            ChatTab::All => chat_type::ALL,
            ChatTab::Party => chat_type::PARTY,
            ChatTab::Guild => chat_type::GUILD,
            ChatTab::Alliance => chat_type::UNION,
            ChatTab::Academy => chat_type::ACADEMY,
        }
    }
}

/// Viewer/input state.
#[derive(Resource, Debug)]
pub struct ChatState {
    pub active_tab: ChatTab,
    /// Auto-follow the newest line; cleared when the user scrolls up.
    pub stick_to_bottom: bool,
    /// Rolling id stamped on outgoing requests, echoed by 0xB025.
    pub next_chat_index: u8,
    /// Sent lines awaiting the server ack, keyed by their `chat_index`.
    pub pending: HashMap<u8, ChatLine>,
    /// `Time::elapsed_secs_f64` until which sending is blocked (0x302D).
    pub restricted_until: Option<f64>,
    /// The input row is only visible while typing (vanilla Enter toggle).
    pub input_open: bool,
    /// Hide button: collapse to just the tab bar.
    pub collapsed: bool,
    /// Zoom button: full-height (`true`) vs the small default view.
    pub expanded: bool,
    pub whisper_panel_open: bool,
    /// Last character we received a whisper from — the target `/Reply` (and its
    /// `/r`, `/re`, `/R` spellings, textuisystem L672-675) needs. The original
    /// keeps this state too; nothing in the data says whether it survives a
    /// zone change or is cleared on logout, so we simply keep the newest
    /// sender for the session (`docs/re/ui/hud-chat.md` §6-11).
    pub last_whisper_from: Option<String>,
    /// Which channel an unprefixed line is sent on — the 2009 chat-mode
    /// dropdown's selection (`GDR_CHAT_MODE_*`), not the tab being read.
    /// Clicking a tab moves it along, so the pre-dropdown behaviour (the
    /// active tab decides) is what an untouched dropdown still does.
    pub mode: ChatTab,
    /// The chat-mode dropdown body (`Section = CreateChatMode`) is open.
    pub mode_open: bool,
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            active_tab: ChatTab::All,
            stick_to_bottom: true,
            next_chat_index: 0,
            pending: HashMap::new(),
            restricted_until: None,
            input_open: false,
            collapsed: false,
            expanded: false,
            whisper_panel_open: false,
            last_whisper_from: None,
            mode: ChatTab::All,
            mode_open: false,
        }
    }
}

#[cfg(test)]
mod whisper_marker_tests {
    use super::*;

    /// The `(FROM)`/`(TO)` markers are localizable
    /// (`UIIT_CHATERR_WHISPER_FROM_MESSAGE` L1733 / `_TO_MESSAGE` L1732), not
    /// baked English — they used to be `format!`ed in place.
    #[test]
    fn whisper_markers_come_from_the_string_table() {
        let line = ChatLine {
            kind: ChatLineKind::WhisperFrom,
            sender: Some("Meng".into()),
            text: "hi".into(),
        };
        assert_eq!(line.display_with("VON", "AN"), "Meng(VON):hi");
        assert_eq!(line.display(), "Meng(FROM):hi");

        let to = ChatLine {
            kind: ChatLineKind::WhisperTo,
            sender: Some("Meng".into()),
            text: "hi".into(),
        };
        assert_eq!(to.display_with("VON", "AN"), "Meng(AN):hi");
        // a plain line is unaffected by either marker
        let plain = ChatLine {
            kind: ChatLineKind::Global,
            sender: Some("Meng".into()),
            text: "hi".into(),
        };
        assert_eq!(plain.display_with("VON", "AN"), "Meng:hi");
    }

    /// `UIIT_STT_CANT_CHATTING` is printf-templated ("Chat Restricted: %d
    /// seconds"); we used to ignore the table and print our own English
    /// sentence.
    #[test]
    fn restriction_text_fills_the_printf_placeholder() {
        assert_eq!(
            format_restriction(CANT_CHATTING_FALLBACK, 30),
            "Chat Restricted: 30 seconds"
        );
        // a translation without the placeholder still has to show the number
        assert_eq!(
            format_restriction("Chat gesperrt", 30),
            "Chat gesperrt (30)"
        );
        // only the first placeholder is consumed
        assert_eq!(format_restriction("%d/%d", 5), "5/%d");
    }
}
