//! Community window state: open/closed plus which of the six pages shows.
//!
//! Idea: `GDR_COMMUNITY:CIFCommunity` (`resinfo/ginterface.txt:541`, id 23,
//! `Rect="0,0,477,393"`) is the shell six page controls hang off, each declared
//! in `resinfo/ifcommunity.txt` on the one shared rect `13,61,451,320`. The
//! resinfo grammar has no `Visible` key, so one-page-at-a-time is the inferred
//! reading of that shared rect (the same idiom as the COS pages and the option
//! panes). Page ids are the data's own: Guild 10, GuildRelations 11, WarState
//! 12, Friend 13, Letter 14, Blocking 15.
//!
//! `ifcommunity.txt` guards its first page with `#ifdef
//! CHATTING_BLOCKING_SYSTEM` / `#else #ifdef WHISPER_BLOCKING_SYSTEM`; both
//! symbols are defined in `config/define.txt`, so the **first** branch wins and
//! `GDR_COMMUNITY_BLOCKING:CIFBlocking` (id 15) is the live page, while
//! `CIFWhisperBlocking` is dead (`docs/re/ui/hud-guild-window.md:52`).

use bevy::prelude::*;

/// The six live pages of the community shell, by their `ifcommunity.txt` id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommunityPage {
    /// `GDR_COMMUNITY_GUILD:CIFGuild`, id 10.
    Guild,
    /// `GDR_COMMUNITY_GUILD_RELATIONS:CIFGuildRelations`, id 11.
    GuildRelations,
    /// `GDR_COMMUNITY_WAR_STATE:CIFWarState`, id 12.
    WarState,
    /// `GDR_COMMUNITY_FRIEND:CIFFriend`, id 13.
    Friend,
    /// `GDR_COMMUNITY_LETTER:CIFLetter`, id 14 — the mail list page.
    Letter,
    /// `GDR_COMMUNITY_BLOCKING:CIFBlocking`, id 15 (the live blocking page).
    Blocking,
}

impl CommunityPage {
    /// Every page, in the id order `ifcommunity.txt` declares them.
    pub const ALL: [CommunityPage; 6] = [
        CommunityPage::Guild,
        CommunityPage::GuildRelations,
        CommunityPage::WarState,
        CommunityPage::Friend,
        CommunityPage::Letter,
        CommunityPage::Blocking,
    ];

    /// The control id `ifcommunity.txt` gives the page.
    pub fn id(self) -> u16 {
        match self {
            CommunityPage::Guild => 10,
            CommunityPage::GuildRelations => 11,
            CommunityPage::WarState => 12,
            CommunityPage::Friend => 13,
            CommunityPage::Letter => 14,
            CommunityPage::Blocking => 15,
        }
    }
}

/// Which mail sub-window is up over the shell, if any.
///
/// `ifletter.txt` hosts read (`CIFLetterRead`, id 55) and write
/// (`CIFLetterWrite`, id 51) on the *same* `0,0,439,271` rect, so they are
/// mutually exclusive by construction rather than by a rule we added. The
/// third section, `MultiLetter` (`GDR_GUILD_LETTER`, id 60), is the guild
/// broadcast composer and is not built here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LetterSubWindow {
    /// Neither sub-window is up — the list page shows.
    #[default]
    None,
    /// `GDR_LETTER_READ:CIFLetterRead`, id 55.
    Read,
    /// `GDR_LETTER_WRITE:CIFLetterWrite`, id 51.
    Write,
}

/// Open/closed state and page selection of the community window.
#[derive(Resource)]
pub struct CommunityState {
    pub open: bool,
    pub page: CommunityPage,
    /// The mail sub-window over the shell, if any.
    pub letter_sub: LetterSubWindow,
}

impl Default for CommunityState {
    /// The data has no selection key at all, so whichever page the shell opens
    /// on is a code-side choice rather than a vanilla default. Mail is the
    /// retained one: it was picked when it was the only page with a body, and
    /// Guild rendering too is no reason to move it, because the entry points
    /// that mean a specific page now say so — the under-bar menu's Guild row
    /// sets `page` before it opens the shell (`underbar/menu_popup.rs`).
    fn default() -> Self {
        Self {
            open: false,
            page: CommunityPage::Letter,
            letter_sub: LetterSubWindow::None,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Ids are the data's, and the whole live set is present: the `#ifdef`
    /// resolution keeps `CIFBlocking` (15) and drops `CIFWhisperBlocking`.
    #[test]
    fn page_ids_match_ifcommunity() {
        let ids: Vec<u16> = CommunityPage::ALL.iter().map(|p| p.id()).collect();
        assert_eq!(ids, vec![10, 11, 12, 13, 14, 15]);
    }
}
