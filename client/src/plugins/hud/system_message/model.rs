//! System-message data model: the five message classes and the printf-template
//! feed that composes their text.
//!
//! Idea: unlike chat, this surface's text does **not** exist on the wire. The
//! original's `GDR_SYSTEM_MESSAGE_VIEW` (`CIFSystemMessage`, id 68) has no input
//! control and no channel selector; the client holds **69**
//! `UIIT_MSG_STATE_*` templates in `textuisystem.txt` (lines 1588-4156, all
//! `enable=1`), **16 of them carrying printf specifiers**, and fills them from
//! gameplay deltas. Zero of the 69 is referenced by any resinfo file — they are
//! runtime-only by construction. See `docs/re/ui/hud-system-message.md` §3/§4.4.
//!
//! So the load-bearing piece is the composition rule, and it is what this module
//! owns: the class taxonomy and a formatter for the original's specifier set.
//! The surface itself is not spawned yet — its placement is provably co-located
//! with the chat frame and the arbitration is not in the data (§8-1, §9-U1/U6),
//! so drawing it before that read would overlap our collapsed chat on every
//! pixel.

use std::collections::VecDeque;

use bevy::prelude::*;

/// Ring buffer capacity. Mirrors `chat::model::CHAT_HISTORY_CAP`; the original's
/// own scrollback depth is not in the data.
pub const SYSTEM_MESSAGE_HISTORY_CAP: usize = 1000;

/// The five message **classes** of the filter board (`CIFChatOptionBoard`,
/// `ifsystemmessage.txt:6`), with their `textuisystem.txt` labels.
///
/// These are classes, not channels: chat's five tabs are a different axis, so
/// relabelling chat tabs into these loses information (`docs/re/ui/hud-system-message.md`
/// §3). Which of the 69 templates each class gates is **UNKNOWN** (§9-U2) — the
/// mapping lives in the original's code, not in its data — so no template is
/// assigned to a class here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemMessageClass {
    /// textuisystem :2105 `Acquisition`
    Acquisition,
    /// textuisystem :2106 `Combat`
    Combat,
    /// textuisystem :2107 `Status`
    Status,
    /// textuisystem :2108 `Party`
    Party,
    /// textuisystem :2109 `Game`
    Game,
}

impl SystemMessageClass {
    /// The five classes in the filter board's own order (`ifchatoptionboard.txt`
    /// ids 10..14).
    pub const ALL: [SystemMessageClass; 5] = [
        SystemMessageClass::Acquisition,
        SystemMessageClass::Combat,
        SystemMessageClass::Status,
        SystemMessageClass::Party,
        SystemMessageClass::Game,
    ];

    /// textuisystem key for the checkbox label. English fallbacks live at the
    /// call site, as everywhere else in the HUD.
    pub fn label_key(self) -> &'static str {
        match self {
            SystemMessageClass::Acquisition => "UIIT_STT_CHATTING_GAIN_MSG",
            SystemMessageClass::Combat => "UIIT_STT_CHATTING_BATTLE_MSG",
            SystemMessageClass::Status => "UIIT_STT_CHATTING_STATE_MSG",
            SystemMessageClass::Party => "UIIT_STT_CHATTING_PARTY_MSG",
            SystemMessageClass::Game => "UIIT_STT_CHATTING_GAME_SYS_MSG",
        }
    }
}

/// One composed line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemMessageLine {
    pub class: SystemMessageClass,
    pub text: String,
}

/// The scrollback. A resource for the same reason chat's is: the feed systems
/// write it, the (not yet built) view reads it.
#[derive(Resource, Default)]
pub struct SystemMessageLog {
    lines: VecDeque<SystemMessageLine>,
}

impl SystemMessageLog {
    pub fn push(&mut self, line: SystemMessageLine) {
        if self.lines.len() >= SYSTEM_MESSAGE_HISTORY_CAP {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub fn iter(&self) -> impl Iterator<Item = &SystemMessageLine> {
        self.lines.iter()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

/// Fill one `UIIT_MSG_STATE_*` template with its arguments.
///
/// The original's templates are C `printf` strings and use exactly three
/// specifiers across the 16 that carry any: `%s`, `%d` and MSVC's 64-bit
/// `%I64d` (`docs/re/ui/hud-system-message.md` §4.4b, e.g. `[%I64d]Experience
/// Points gained.`, `[%d]gold gained.`, `Dropped [%s] amount of gold on the
/// ground.`). Substitution is positional, and the caller supplies already
/// formatted arguments — the numeric grouping/locale rules are not in the data.
///
/// Deliberately not a printf implementation: unknown specifiers and a short
/// argument list are left **verbatim** rather than guessed at or panicked on, so
/// a template we have not characterised shows as itself instead of as garbage.
/// `%%` is the one escape the dialect needs.
pub fn format_template(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    let mut next_arg = 0;

    while let Some(pos) = rest.find('%') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];

        // Longest specifier first: `%I64d` must not be read as `%I` + `64d`.
        let spec = ["%I64d", "%s", "%d", "%%"]
            .into_iter()
            .find(|spec| tail.starts_with(spec));

        match spec {
            Some("%%") => {
                out.push('%');
                rest = &tail["%%".len()..];
            }
            Some(spec) => {
                match args.get(next_arg) {
                    Some(arg) => {
                        out.push_str(arg);
                        next_arg += 1;
                    }
                    // Fewer arguments than specifiers: keep the specifier, so the
                    // defect is visible instead of silently swallowed.
                    None => out.push_str(spec),
                }
                rest = &tail[spec.len()..];
            }
            None => {
                // A `%` that starts no specifier we know: pass it through.
                out.push('%');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three specifier-carrying templates quoted verbatim in
    /// `docs/re/ui/hud-system-message.md` §4.4b, at their textuisystem lines.
    const GAIN_EXP: &str = "[%I64d]Experience Points gained."; // :1986
    const GAIN_GOLD: &str = "[%d]gold gained."; // :1989
    const GAIN_GOLD_DROP: &str = "Dropped [%s] amount of gold on the ground."; // :1996

    #[test]
    fn format_template_fills_the_original_printf_specifiers() {
        assert_eq!(
            format_template(GAIN_EXP, &["1234567890123"]),
            "[1234567890123]Experience Points gained."
        );
        assert_eq!(format_template(GAIN_GOLD, &["250"]), "[250]gold gained.");
        assert_eq!(
            format_template(GAIN_GOLD_DROP, &["1,024"]),
            "Dropped [1,024] amount of gold on the ground."
        );
    }

    /// 53 of the 69 templates carry no specifier at all, so the common case must
    /// be byte-identical.
    #[test]
    fn format_template_leaves_specifier_free_templates_alone() {
        let frozen = "You are under frozen status."; // :1612 shape
        assert_eq!(format_template(frozen, &[]), frozen);
        assert_eq!(format_template(frozen, &["ignored"]), frozen);
    }

    /// A short argument list or an uncharacterised specifier must degrade to the
    /// template's own text, never to a panic or to a half-eaten string.
    #[test]
    fn format_template_degrades_visibly_instead_of_guessing() {
        assert_eq!(format_template(GAIN_EXP, &[]), GAIN_EXP);
        assert_eq!(format_template("100%% sure", &[]), "100% sure");
        assert_eq!(format_template("%u unknown", &["x"]), "%u unknown");
        // `%I64d` must win over `%d` inside it
        assert_eq!(format_template("%I64d", &["7"]), "7");
    }

    /// The five filter classes are the original's own set, in the filter board's
    /// order, keyed to textuisystem :2105-:2109.
    #[test]
    fn message_classes_carry_the_filter_board_keys() {
        assert_eq!(SystemMessageClass::ALL.len(), 5);
        assert_eq!(
            SystemMessageClass::ALL.map(|c| c.label_key()),
            [
                "UIIT_STT_CHATTING_GAIN_MSG",
                "UIIT_STT_CHATTING_BATTLE_MSG",
                "UIIT_STT_CHATTING_STATE_MSG",
                "UIIT_STT_CHATTING_PARTY_MSG",
                "UIIT_STT_CHATTING_GAME_SYS_MSG",
            ]
        );
    }

    #[test]
    fn log_drops_the_oldest_line_at_capacity() {
        let mut log = SystemMessageLog::default();
        for i in 0..SYSTEM_MESSAGE_HISTORY_CAP + 5 {
            log.push(SystemMessageLine {
                class: SystemMessageClass::Acquisition,
                text: i.to_string(),
            });
        }
        assert_eq!(log.len(), SYSTEM_MESSAGE_HISTORY_CAP);
        assert_eq!(log.iter().next().expect("oldest").text, "5");
    }
}
