//! Party-matching board state: the page the server sent, the sort the player
//! chose, and which of the four dialogs is up.
//!
//! Idea: the board is a **pure view of one server page**. `0x706C` asks for a
//! page index and `0xB06C` answers with that page's entries plus how many pages
//! exist; there is no server-side search and no server-side sort in the wire
//! family at all. So paging is a request and sorting is local, and this module
//! keeps those two apart so neither can quietly become the other.

use bevy::prelude::*;

use packets::agent::party::{
    PartyData, PartyMatchCreationResponse, PartyMatchDeleteResponse, PartyMatchEditedResponse,
    PartyMatchEntry, PartyMatchJoinAck, PartyMatchJoinNotify, PartyMatchListResponse,
};

use crate::plugins::hud::chat::model::{ChatHistory, ChatLine};
use crate::plugins::net::party::{party_error_text, PartyRoster};
use crate::plugins::settings::keymap::KEY_PARTY_MATCH;
use crate::plugins::settings::options::GameOptions;

/// Rows the board draws at once — twelve `CIFPartyMatchSlot` blocks, fixed, no
/// scrollbar behaviour behind them. Paging is the server's, via the spinner.
pub const MATCH_ROWS: usize = 12;

/// Which column the list is ordered by. The eight header cells are `CIFButton`s
/// — they are sort buttons, and the sort they drive is ours because the wire
/// carries no ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MatchSort {
    /// Server order, i.e. the page exactly as it arrived.
    #[default]
    Wire,
    Number,
    Race,
    Name,
    Title,
    Purpose,
    Members,
    Level,
}

/// The dialog currently over the board, if any. One at a time: all four are
/// modal in the original and two of them are answers to the same question.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum MatchDialog {
    #[default]
    None,
    /// "Form party" — also reused for "Change entry", which is why the entry
    /// being edited rides along.
    Register { editing: Option<u32> },
    /// "Auto match" — a *request to be placed*, not an advertisement.
    Auto,
    /// Somebody applied to our advertised party (`0x706D`).
    ReqJoin(Box<PartyMatchJoinNotify>),
    /// We applied and are waiting (`0x706D` out, `0xB06D` back).
    JoinProgress { number: u32, elapsed: f32 },
}

/// Everything the board draws.
#[derive(Resource, Debug, Default)]
pub struct PartyMatchState {
    pub open: bool,
    /// The page the server last sent.
    pub entries: Vec<PartyMatchEntry>,
    pub page_index: u8,
    pub page_count: u8,
    /// The match number of the selected row, if any.
    pub selected: Option<u32>,
    pub sort: MatchSort,
    /// Descending when the same header is clicked twice.
    pub descending: bool,
    pub dialog: MatchDialog,
    /// The form the register/auto dialogs are editing.
    pub form: MatchForm,
    /// The name of the leader of the party we are in, mirrored from
    /// `PartyRoster`. The listed entry whose `master_name` matches is ours: it
    /// pins to the top and draws in the configured tint.
    ///
    /// **Not the party number.** This keyed off `PartyRoster::party_number`
    /// against `PartyMatchEntry::number` and never once matched, because those
    /// are two different id spaces: a capture has the roster's party at number 9
    /// (0x3065) while our own listing is number 5 (0xB069/0xB06C, same minute).
    /// Nor is `PartyMatchEntry::registered_at` a leader id despite go-sro's
    /// name for it — see its doc comment. The leader's *name* is the only field
    /// the two sides share, it is unique per server, and it is the same join
    /// both map surfaces already use for party members.
    pub own_party_master: Option<String>,
    /// The number of the listing **we** advertised, taken from our own
    /// 0xB069/0xB06A success arm and given up when 0xB06B withdraws it.
    ///
    /// The second key exists because the first one cannot cover the case that
    /// matters most: the server creates **no party** for a solo registration.
    /// A capture has 0x7069 → 0xB069 with no 0x3065 anywhere after it — the
    /// first roster push arrives only when somebody joins — so until then
    /// there is no leader to name and [`Self::own_party_master`] is `None`.
    ///
    /// Neither key subsumes the other. This one covers a listing with no party
    /// behind it yet; the name covers joining somebody *else's* party, and
    /// covers a relogin that never saw an ack. A solo listing after a relogin
    /// is identifiable by neither, and simply does not tint.
    pub own_listing: Option<u32>,
}

/// The register dialog's four sections, and the auto dialog's three.
#[derive(Clone, Debug)]
pub struct MatchForm {
    pub purpose: u8,
    pub level_min: u8,
    pub level_max: u8,
    pub title: String,
    /// Auto-match only: which race to be placed with. `None` = "Open".
    pub race: Option<u8>,
}

impl Default for MatchForm {
    fn default() -> Self {
        Self {
            purpose: packets::agent::party::PARTY_PURPOSE_HUNTING,
            level_min: 1,
            level_max: 140,
            title: String::new(),
            race: None,
        }
    }
}

impl PartyMatchState {
    /// The page's entries in the order the board should draw them.
    ///
    /// Cloned rather than sorted in place: `entries` is the server's page and
    /// re-ordering it would make "what the server sent" unrecoverable, so a
    /// later page compare (or a re-sort back to `Wire`) could not work.
    pub fn sorted(&self) -> Vec<&PartyMatchEntry> {
        let mut rows: Vec<&PartyMatchEntry> = self.entries.iter().collect();
        match self.sort {
            MatchSort::Wire => {}
            MatchSort::Number => rows.sort_by_key(|e| e.number),
            MatchSort::Race => rows.sort_by_key(|e| e.race_type),
            MatchSort::Name => rows.sort_by(|a, b| a.master_name.cmp(&b.master_name)),
            MatchSort::Title => rows.sort_by(|a, b| a.title.cmp(&b.title)),
            MatchSort::Purpose => rows.sort_by_key(|e| e.purpose),
            MatchSort::Members => rows.sort_by_key(|e| e.member_count),
            MatchSort::Level => rows.sort_by_key(|e| e.level_min),
        }
        if self.descending && self.sort != MatchSort::Wire {
            rows.reverse();
        }
        // Our own party is pinned above the sorted rows, always — including
        // under a descending sort. It is the row the player is looking for,
        // and pinning keeps the tint and the position from disagreeing.
        if let Some(at) = rows.iter().position(|entry| self.is_own_party(entry)) {
            let entry = rows.remove(at);
            rows.insert(0, entry);
        }
        rows
    }

    /// Whether this listing is ours — either the one we advertised, or the one
    /// advertising the party we are in. See the two fields for why it takes
    /// both keys and not one.
    pub fn is_own_party(&self, entry: &PartyMatchEntry) -> bool {
        self.own_listing == Some(entry.number)
            || self.own_party_master.as_deref() == Some(entry.master_name.as_str())
    }

    /// Click a column header: the same column again flips the direction, a new
    /// one starts ascending.
    pub fn sort_by(&mut self, column: MatchSort) {
        if self.sort == column {
            self.descending = !self.descending;
        } else {
            self.sort = column;
            self.descending = false;
        }
    }

    /// The selected entry, if it is still on this page.
    pub fn selected_entry(&self) -> Option<&PartyMatchEntry> {
        let number = self.selected?;
        self.entries.iter().find(|entry| entry.number == number)
    }

    /// Which *drawn row* holds the selection, given the order [`Self::sorted`]
    /// produced.
    ///
    /// Row index and entry are not interchangeable: sorting reorders them and
    /// pinning our own party moves one to the front, so the selected row is
    /// found by looking the number up in the order actually on screen. Takes
    /// the sorted slice rather than recomputing it, so the painter and this
    /// cannot disagree about what "row 3" means.
    pub fn selected_row(&self, sorted: &[&PartyMatchEntry]) -> Option<usize> {
        let number = self.selected?;
        sorted.iter().position(|entry| entry.number == number)
    }
}

/// `KeyPartyMatch` toggles the board — `E` by default.
pub fn toggle_party_match(
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<crate::plugins::hud::chat::model::ChatState>,
    options: Res<GameOptions>,
    mut state: ResMut<PartyMatchState>,
) {
    let Some(key) = options.key_for(KEY_PARTY_MATCH) else {
        return;
    };
    if keys.just_pressed(key) && !chat.input_open {
        state.open = !state.open;
    }
}

/// 0xB06C — one page of the list.
///
/// An empty answer is a real answer: `has_data` false means "nothing to list",
/// which must clear the board rather than leave the previous page on screen.
pub fn on_match_list(
    mut reader: MessageReader<PartyMatchListResponse>,
    mut state: ResMut<PartyMatchState>,
) {
    for message in reader.read() {
        match message.page.as_ref() {
            Some(page) => {
                // De-duplicate by `number`. This is not defensive coding: a
                // capture of a real vSRO 1.188 server (`packet_dump/0xb06c.log`)
                // shows one response with `party_count = 2` listing the SAME
                // entry twice — same number, same master JID, same name. The
                // wire format allows it (`parties` is a plain sized list) and
                // the server does it, so the board would draw the row twice.
                // Keeping the first occurrence preserves the server's order.
                let mut seen = Vec::with_capacity(page.parties.len());
                state.entries = page
                    .parties
                    .iter()
                    .filter(|entry| {
                        let fresh = !seen.contains(&entry.number);
                        if fresh {
                            seen.push(entry.number);
                        }
                        fresh
                    })
                    .cloned()
                    .collect();
                state.page_index = page.page_index;
                state.page_count = page.page_count;
            }
            None => {
                state.entries.clear();
                state.page_index = 0;
                state.page_count = 0;
            }
        }
        // A selection that is no longer on the page cannot stay selected.
        if state.selected_entry().is_none() {
            state.selected = None;
        }
    }
}

/// 0x706D inbound — somebody wants to join the party we advertised.
pub fn on_join_request(
    mut reader: MessageReader<PartyMatchJoinNotify>,
    mut state: ResMut<PartyMatchState>,
) {
    for message in reader.read() {
        state.dialog = MatchDialog::ReqJoin(Box::new(message.clone()));
    }
}

/// 0xB06D — the ack for our own application.
///
/// Success closes the waiting dialog only when the join actually resolved: the
/// success arm carries a `PartyMatchingJoinResult`, not a bare "yes", and the
/// party itself arrives separately as 0x3065.
pub fn on_join_ack(
    mut reader: MessageReader<PartyMatchJoinAck>,
    mut state: ResMut<PartyMatchState>,
    mut history: Option<ResMut<ChatHistory>>,
) {
    for message in reader.read() {
        info!(
            "party match: 0xB06D result={} join_result={:?} error={:?}",
            message.result, message.join_result, message.error_code
        );
        if message.result != 1 {
            state.dialog = MatchDialog::None;
            report(&mut history, message.error_code, "join");
        }
    }
}

/// Close the wait dialog when the join actually resolves.
///
/// `0xB06D`'s success arm is only an acknowledgement that the request was
/// accepted for delivery — the join itself completes when the party arrives as
/// `0x3065`. Nothing closed the dialog on that path, so an accepted applicant
/// sat behind a modal they could not dismiss.
pub fn close_join_progress_on_party(
    mut reader: MessageReader<PartyData>,
    mut state: ResMut<PartyMatchState>,
) {
    // Read first, and only touch the state if something arrived: `ResMut`'s
    // deref marks the resource changed, and the dialog painter keys off that.
    if reader.read().count() == 0 {
        return;
    }
    if matches!(state.dialog, MatchDialog::JoinProgress { .. }) {
        state.dialog = MatchDialog::None;
    }
}

/// Mirror the name of our party's leader out of the roster, so the board can
/// tell which listed entry is ours.
///
/// The leader lookup is the one `party::model::roster_rows` already uses:
/// `master_join_id` names a member, and that member's name is what 0xB06C
/// advertises as `master_name`. A roster-only push carries no leader, and a
/// presence mask can omit a name, so this is honestly `None` in both cases —
/// no tint beats tinting the wrong row.
pub fn sync_own_party_master(roster: Option<Res<PartyRoster>>, mut state: ResMut<PartyMatchState>) {
    let own = roster
        .filter(|roster| roster.is_active())
        .and_then(|roster| {
            roster
                .master_join_id
                .and_then(|id| roster.member(id))
                .and_then(|leader| leader.name.clone())
        })
        .filter(|name| !name.is_empty());
    if state.own_party_master != own {
        state.own_party_master = own;
    }
}

/// Ask the board to re-read the current page. Raised when the server confirms a
/// registration or an edit, so a freshly formed party shows up without the
/// player pressing Refresh.
#[derive(Message, Default)]
pub struct ReloadMatchList;

/// 0xB069 / 0xB06A — our advertisement was accepted, or refused with a code.
pub fn on_form_acks(
    mut created: MessageReader<PartyMatchCreationResponse>,
    mut edited: MessageReader<PartyMatchEditedResponse>,
    mut state: ResMut<PartyMatchState>,
    mut history: Option<ResMut<ChatHistory>>,
    mut reload: MessageWriter<ReloadMatchList>,
) {
    for message in created.read() {
        if message.result == 1 {
            state.dialog = MatchDialog::None;
            // The echoed record names the listing we just made. It is the only
            // way to recognise our own row before anyone joins, because the
            // server raises no party — and so no leader name — for a solo
            // registration.
            state.own_listing = message.entry.as_ref().map(|entry| entry.party_number);
            reload.write(ReloadMatchList);
        } else {
            report(&mut history, message.error_code, "registration");
        }
    }
    for message in edited.read() {
        if message.result == 1 {
            state.dialog = MatchDialog::None;
            state.own_listing = message.entry.as_ref().map(|entry| entry.party_number);
            reload.write(ReloadMatchList);
        } else {
            report(&mut history, message.error_code, "change");
        }
    }
}

/// 0xB06B — the withdrawal.
pub fn on_delete_ack(
    mut reader: MessageReader<PartyMatchDeleteResponse>,
    mut state: ResMut<PartyMatchState>,
    mut history: Option<ResMut<ChatHistory>>,
) {
    for message in reader.read() {
        if message.result == 1 {
            if let Some(number) = message.number {
                state.entries.retain(|entry| entry.number != number);
                if state.selected == Some(number) {
                    state.selected = None;
                }
                // The listing we were tracking is gone. Only give up the key
                // when the withdrawal names *our* number — the board can
                // watch somebody else's row disappear.
                if state.own_listing == Some(number) {
                    state.own_listing = None;
                }
            }
        } else {
            report(&mut history, message.error_code, "deletion");
        }
    }
}

/// Push one failure line into the chat log, when there is one.
///
/// `ChatHistory` is optional for the same reason the party acks make it
/// optional: the headless netcheck harness builds no HUD, and Bevy does not
/// skip a system whose `ResMut` is missing — it panics the schedule.
fn report(history: &mut Option<ResMut<ChatHistory>>, code: Option<u16>, verb: &str) {
    let text = match code.and_then(party_error_text) {
        Some(text) => text.to_string(),
        None => match code {
            Some(code) => format!("Party {verb} failed (code {code})."),
            None => format!("Party {verb} failed."),
        },
    };
    match history {
        Some(history) => history.push(ChatLine::system(text)),
        None => info!("party match (headless): {text}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use packets::agent::party::{
        PartyMatchForm, PartyMatchListPage, PartyMemberCore, PartyMemberMask, PartySetup,
        PARTY_PURPOSE_HUNTING,
    };

    fn entry(number: u32, name: &str, level_min: u8, members: u8) -> PartyMatchEntry {
        PartyMatchEntry {
            number,
            registered_at: 0,
            master_name: name.to_string(),
            race_type: 0,
            member_count: members,
            setup: PartySetup::EXP_SHARED,
            purpose: PARTY_PURPOSE_HUNTING,
            level_min,
            level_max: 140,
            title: format!("t{number}"),
        }
    }

    fn state_with(entries: Vec<PartyMatchEntry>) -> PartyMatchState {
        PartyMatchState {
            entries,
            ..Default::default()
        }
    }

    /// The default order is the server's, untouched — the board is a view of
    /// one page and "no sort" has to mean exactly that.
    #[test]
    fn the_default_order_is_the_wire_order() {
        let state = state_with(vec![entry(3, "C", 20, 2), entry(1, "A", 10, 4)]);
        let numbers: Vec<u32> = state.sorted().iter().map(|e| e.number).collect();
        assert_eq!(numbers, vec![3, 1]);
    }

    /// A header click sorts; the same header again flips direction; a different
    /// header starts ascending again.
    #[test]
    fn clicking_a_header_sorts_then_flips() {
        let mut state = state_with(vec![entry(3, "C", 20, 2), entry(1, "A", 10, 4)]);

        state.sort_by(MatchSort::Number);
        assert_eq!(
            state.sorted().iter().map(|e| e.number).collect::<Vec<_>>(),
            vec![1, 3]
        );

        state.sort_by(MatchSort::Number);
        assert!(state.descending);
        assert_eq!(
            state.sorted().iter().map(|e| e.number).collect::<Vec<_>>(),
            vec![3, 1]
        );

        state.sort_by(MatchSort::Name);
        assert!(!state.descending, "a new column starts ascending");
        assert_eq!(
            state
                .sorted()
                .iter()
                .map(|e| e.master_name.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "C"]
        );
    }

    /// Sorting must not disturb the page itself, or "back to wire order" and
    /// any later page comparison would be impossible.
    #[test]
    fn sorting_leaves_the_server_page_untouched() {
        let mut state = state_with(vec![entry(3, "C", 20, 2), entry(1, "A", 10, 4)]);
        state.sort_by(MatchSort::Number);
        let _ = state.sorted();
        assert_eq!(state.entries[0].number, 3, "the page is still the page");

        state.sort = MatchSort::Wire;
        assert_eq!(
            state.sorted().iter().map(|e| e.number).collect::<Vec<_>>(),
            vec![3, 1]
        );
    }

    /// The board de-duplicates by number, because a real vSRO server sends a
    /// page with `party_count = 2` listing the SAME entry twice
    /// (`packet_dump/0xb06c.log`). The wire format allows it, so this is not
    /// defensive coding — it is the capture.
    #[test]
    fn a_page_listing_one_party_twice_draws_it_once() {
        let mut app = App::new();
        app.init_resource::<PartyMatchState>()
            .add_message::<PartyMatchListResponse>()
            .add_systems(Update, on_match_list);

        app.world_mut().write_message(PartyMatchListResponse {
            has_data: true,
            page: Some(PartyMatchListPage {
                page_count: 1,
                page_index: 0,
                party_count: 2,
                parties: vec![entry(3, "priavte", 1, 1), entry(3, "priavte", 1, 1)],
            }),
        });
        app.update();

        let state = app.world().resource::<PartyMatchState>();
        assert_eq!(state.entries.len(), 1, "the duplicate row survived");
        assert_eq!(state.entries[0].number, 3);
    }

    /// Our own party pins to the top under EVERY sort, including descending —
    /// the tint and the position have to agree wherever the player looks.
    #[test]
    fn our_own_party_pins_to_the_top_under_every_sort() {
        let mut state = state_with(vec![
            entry(1, "Alice", 10, 2),
            entry(2, "Bob", 20, 3),
            entry(3, "Carol", 30, 4),
        ]);
        state.own_party_master = Some("Carol".into());

        for sort in [
            MatchSort::Wire,
            MatchSort::Number,
            MatchSort::Name,
            MatchSort::Level,
            MatchSort::Members,
        ] {
            for descending in [false, true] {
                state.sort = sort;
                state.descending = descending;
                let rows = state.sorted();
                assert_eq!(
                    rows[0].number, 3,
                    "own party lost the top under {sort:?} descending={descending}"
                );
                assert_eq!(rows.len(), 3, "pinning must not drop or duplicate a row");
            }
        }
        assert!(state.is_own_party(&entry(3, "Carol", 30, 4)));
        assert!(!state.is_own_party(&entry(1, "Alice", 10, 2)));
    }

    /// Being in no party at all pins nothing.
    #[test]
    fn no_own_party_leaves_the_order_alone() {
        let state = state_with(vec![entry(1, "Alice", 10, 2), entry(2, "Bob", 20, 3)]);
        assert_eq!(state.own_party_master, None);
        assert_eq!(
            state.sorted().iter().map(|e| e.number).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    /// The selected row is found by number in the order actually drawn, not by
    /// the index the entry had on the wire — sorting reorders the rows and
    /// pinning moves one to the front, so those two disagree in general.
    #[test]
    fn the_selection_follows_the_row_it_is_drawn_in() {
        let mut state = state_with(vec![
            entry(1, "Alice", 10, 2),
            entry(2, "Bob", 20, 3),
            entry(3, "Carol", 30, 4),
        ]);

        // wire order: Carol is row 2
        state.selected = Some(3);
        assert_eq!(state.selected_row(&state.sorted()), Some(2));

        // reversed: Carol is row 0
        state.sort = MatchSort::Number;
        state.descending = true;
        assert_eq!(state.selected_row(&state.sorted()), Some(0));

        // pinned to the top as our own party, under that same descending sort
        state.own_party_master = Some("Alice".into());
        state.selected = Some(1);
        assert_eq!(state.selected_row(&state.sorted()), Some(0));
        // ...and Carol has been pushed down by the pin
        state.selected = Some(3);
        assert_eq!(state.selected_row(&state.sorted()), Some(1));

        // nothing selected, and a selection that left the page
        state.selected = None;
        assert_eq!(state.selected_row(&state.sorted()), None);
        state.selected = Some(99);
        assert_eq!(state.selected_row(&state.sorted()), None);
    }

    /// The regression guard for the defect itself: the roster's `party_number`
    /// and a listing's `number` are **different id spaces**, so a row whose
    /// number happens to equal our party's must not be taken for ours. A
    /// capture has the roster at party 9 while our own listing is number 5.
    #[test]
    fn a_matching_number_is_not_what_makes_a_row_ours() {
        let mut state = state_with(vec![entry(9, "Stranger", 10, 2), entry(5, "Ahri", 20, 3)]);
        // in a party led by "Ahri", whose party_number (0x3065) is 9
        state.own_party_master = Some("Ahri".into());

        assert!(!state.is_own_party(&entry(9, "Stranger", 10, 2)));
        assert!(state.is_own_party(&entry(5, "Ahri", 20, 3)));
        assert_eq!(state.sorted()[0].number, 5);
    }

    /// The defect the name key could not reach: the server raises **no party**
    /// for a solo registration, so there is no leader to name and nothing to
    /// match on until somebody joins. The ack's own number is what identifies
    /// the listing in the meantime.
    #[test]
    fn our_listing_is_ours_before_anyone_has_joined() {
        let mut app = App::new();
        app.init_resource::<PartyMatchState>()
            .add_message::<PartyMatchCreationResponse>()
            .add_message::<PartyMatchEditedResponse>()
            .add_message::<PartyMatchDeleteResponse>()
            .add_message::<ReloadMatchList>()
            .add_systems(Update, (on_form_acks, on_delete_ack));
        app.world_mut()
            .resource_mut::<PartyMatchState>()
            .entries
            .push(entry(6, "priavte", 1, 1));

        app.world_mut().write_message(PartyMatchCreationResponse {
            result: 1,
            entry: Some(PartyMatchForm {
                party_number: 6,
                unknown: 0,
                setup: PartySetup::EXP_SHARED,
                purpose: PARTY_PURPOSE_HUNTING,
                level_min: 1,
                level_max: 140,
                title: String::new(),
            }),
            error_code: None,
        });
        app.update();

        let state = app.world().resource::<PartyMatchState>();
        assert_eq!(state.own_listing, Some(6));
        assert_eq!(
            state.own_party_master, None,
            "there is no party, so no leader — the point of the second key"
        );
        assert!(state.is_own_party(&entry(6, "priavte", 1, 1)));
        assert!(!state.is_own_party(&entry(7, "somebody", 1, 1)));

        // withdrawing it gives the key up again...
        app.world_mut().write_message(PartyMatchDeleteResponse {
            result: 1,
            number: Some(6),
            error_code: None,
        });
        app.update();
        assert_eq!(app.world().resource::<PartyMatchState>().own_listing, None);
    }

    /// ...but somebody else's withdrawal does not. The board watches every
    /// row disappear, only one of them is ours.
    #[test]
    fn another_listing_going_away_leaves_our_key_alone() {
        let mut app = App::new();
        app.init_resource::<PartyMatchState>()
            .add_message::<PartyMatchDeleteResponse>()
            .add_systems(Update, on_delete_ack);
        app.world_mut()
            .resource_mut::<PartyMatchState>()
            .own_listing = Some(6);

        app.world_mut().write_message(PartyMatchDeleteResponse {
            result: 1,
            number: Some(7),
            error_code: None,
        });
        app.update();
        assert_eq!(
            app.world().resource::<PartyMatchState>().own_listing,
            Some(6)
        );
    }

    /// The leader's name comes off the roster the same way the roster page
    /// finds the crown, and stays `None` when the server has not named one —
    /// no tint beats tinting somebody else's row.
    #[test]
    fn the_own_party_master_is_the_rosters_leader() {
        let mut app = App::new();
        app.init_resource::<PartyMatchState>()
            .init_resource::<PartyRoster>()
            .add_systems(Update, sync_own_party_master);

        let leader = |id: u32, name: &str| PartyMemberCore {
            presence: PartyMemberMask::MEMBER_ID | PartyMemberMask::NAME,
            member_id: Some(id),
            name: Some(name.to_string()),
            ..Default::default()
        };

        // no party -> nothing
        app.update();
        assert_eq!(
            app.world().resource::<PartyMatchState>().own_party_master,
            None
        );

        // a party whose leader the server named
        app.world_mut()
            .resource_mut::<PartyRoster>()
            .apply_data(&PartyData {
                presence: 0x03,
                party_number: 9,
                master_join_id: Some(4),
                setup: Some(0),
                member_count: Some(2),
                members: vec![leader(4, "priavte"), leader(1, "Ahri")],
            });
        app.update();
        assert_eq!(
            app.world().resource::<PartyMatchState>().own_party_master,
            Some("priavte".to_string())
        );

        // a leader the roster cannot name resolves to nothing at all
        app.world_mut()
            .resource_mut::<PartyRoster>()
            .apply_data(&PartyData {
                presence: 0x03,
                party_number: 9,
                master_join_id: Some(99),
                setup: Some(0),
                member_count: Some(1),
                members: vec![leader(1, "Ahri")],
            });
        app.update();
        assert_eq!(
            app.world().resource::<PartyMatchState>().own_party_master,
            None
        );
    }

    /// The wait dialog closes when the party actually arrives (0x3065) — the
    /// signal that the join resolved. `0xB06D` only acknowledges delivery, so
    /// nothing used to close it and the applicant was stuck behind a modal.
    #[test]
    fn the_party_push_closes_the_wait_dialog() {
        let mut app = App::new();
        app.init_resource::<PartyMatchState>()
            .add_message::<PartyData>()
            .add_systems(Update, close_join_progress_on_party);
        app.world_mut().resource_mut::<PartyMatchState>().dialog = MatchDialog::JoinProgress {
            number: 3,
            elapsed: 1.0,
        };

        app.world_mut().write_message(PartyData {
            presence: 0x03,
            party_number: 8,
            master_join_id: Some(4),
            setup: Some(0),
            member_count: Some(0),
            members: Vec::new(),
        });
        app.update();

        assert_eq!(
            app.world().resource::<PartyMatchState>().dialog,
            MatchDialog::None
        );
    }

    /// ...and it leaves any other dialog alone, so a party forming while the
    /// register form is open does not close the form under the player.
    #[test]
    fn the_party_push_does_not_disturb_another_dialog() {
        let mut app = App::new();
        app.init_resource::<PartyMatchState>()
            .add_message::<PartyData>()
            .add_systems(Update, close_join_progress_on_party);
        app.world_mut().resource_mut::<PartyMatchState>().dialog =
            MatchDialog::Register { editing: None };

        app.world_mut().write_message(PartyData {
            presence: 0x03,
            party_number: 8,
            master_join_id: Some(4),
            setup: Some(0),
            member_count: Some(0),
            members: Vec::new(),
        });
        app.update();

        assert_eq!(
            app.world().resource::<PartyMatchState>().dialog,
            MatchDialog::Register { editing: None }
        );
    }

    /// An empty answer clears the board. Leaving the previous page up would
    /// show parties the server just said are not there.
    #[test]
    fn an_empty_page_clears_the_board() {
        let mut app = App::new();
        app.init_resource::<PartyMatchState>()
            .add_message::<PartyMatchListResponse>()
            .add_systems(Update, on_match_list);

        app.world_mut().resource_mut::<PartyMatchState>().entries = vec![entry(1, "A", 10, 4)];
        app.world_mut().write_message(PartyMatchListResponse {
            has_data: false,
            page: None,
        });
        app.update();

        let state = app.world().resource::<PartyMatchState>();
        assert!(state.entries.is_empty());
        assert_eq!(state.page_count, 0);
    }

    /// A populated page replaces the board and keeps the server's paging.
    #[test]
    fn a_populated_page_replaces_the_board() {
        let mut app = App::new();
        app.init_resource::<PartyMatchState>()
            .add_message::<PartyMatchListResponse>()
            .add_systems(Update, on_match_list);

        app.world_mut().write_message(PartyMatchListResponse {
            has_data: true,
            page: Some(PartyMatchListPage {
                page_count: 3,
                page_index: 1,
                party_count: 2,
                parties: vec![entry(7, "G", 30, 1), entry(8, "H", 40, 2)],
            }),
        });
        app.update();

        let state = app.world().resource::<PartyMatchState>();
        assert_eq!(state.entries.len(), 2);
        assert_eq!((state.page_index, state.page_count), (1, 3));
    }

    /// A selection that the new page no longer contains is dropped, so the six
    /// actions cannot fire at a party that is not on screen.
    #[test]
    fn a_selection_that_left_the_page_is_cleared() {
        let mut app = App::new();
        app.init_resource::<PartyMatchState>()
            .add_message::<PartyMatchListResponse>()
            .add_systems(Update, on_match_list);
        app.world_mut().resource_mut::<PartyMatchState>().selected = Some(42);

        app.world_mut().write_message(PartyMatchListResponse {
            has_data: true,
            page: Some(PartyMatchListPage {
                page_count: 1,
                page_index: 0,
                party_count: 1,
                parties: vec![entry(7, "G", 30, 1)],
            }),
        });
        app.update();

        assert_eq!(app.world().resource::<PartyMatchState>().selected, None);
    }

    /// A withdrawn entry leaves the board; a refused withdrawal does not.
    #[test]
    fn the_delete_ack_removes_only_on_success() {
        let mut app = App::new();
        app.init_resource::<PartyMatchState>()
            .add_message::<PartyMatchDeleteResponse>()
            .add_systems(Update, on_delete_ack);
        app.world_mut().resource_mut::<PartyMatchState>().entries =
            vec![entry(7, "G", 30, 1), entry(8, "H", 40, 2)];

        app.world_mut().write_message(PartyMatchDeleteResponse {
            result: 2,
            number: None,
            error_code: Some(11292),
        });
        app.update();
        assert_eq!(app.world().resource::<PartyMatchState>().entries.len(), 2);

        app.world_mut().write_message(PartyMatchDeleteResponse {
            result: 1,
            number: Some(7),
            error_code: None,
        });
        app.update();
        let state = app.world().resource::<PartyMatchState>();
        assert_eq!(state.entries.len(), 1);
        assert_eq!(state.entries[0].number, 8);
    }
}
