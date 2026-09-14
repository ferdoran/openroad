//! Party roster window state and the pure functions its view needs.
//!
//! Idea: the window itself is a fixed 8-row board — vanilla hard-declares one
//! pinned leader row plus seven `CIFPartySlot` instances and ships no scrollbar
//! and no paging control, so "which member is drawn in which row" is a total
//! function of the roster rather than a scroll state. That function lives here,
//! next to the two vitals readings, so both this window and the quick-party
//! board answer it identically and neither has to re-derive it from a comment.

use bevy::prelude::*;

use packets::agent::party::{PartyMemberCore, PartySetup};

use crate::plugins::cursor::interactions::entity_select::SelectedEntity;
use crate::plugins::hud::chat::model::{ChatHistory, ChatLine, ChatState};
use crate::plugins::net::entities::{NetworkId, RemoteEntity};
use crate::plugins::net::party::{PartyAction, PartyRoster};
use crate::plugins::settings::keymap::KEY_PARTY;
use crate::plugins::settings::options::GameOptions;

/// Rows the board draws: one pinned leader row plus seven slots. Eight is also
/// the party's own maximum, confirmed three independent ways — the EXP-shared
/// capacity rule, this 1+7 layout, and the dead legacy tree's `PNAME0..7`
/// (`docs/re/ui/hud-party-window.md` §3h).
pub const PARTY_ROWS: usize = 8;

/// Whether the party window is up. Toggled by `KeyParty` and by the under-bar
/// menu row; the window itself is spawned once and hidden, like every other
/// HUD window here.
#[derive(Resource, Default, Debug)]
pub struct PartyWindowState {
    pub open: bool,
    /// The member whose row context menu is open, if any — the payload of the
    /// page's claim on the shared popup (see `hud::context_menu::ContextMenuOwner`).
    pub context_member: Option<u32>,
}

/// The `KeyParty` shortcut toggles the window — unless the chat input is
/// capturing keystrokes. Rebindable via the Key Map tab; `P` by default.
pub fn toggle_party(
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatState>,
    options: Res<GameOptions>,
    mut state: ResMut<PartyWindowState>,
) {
    let Some(key) = options.key_for(KEY_PARTY) else {
        return;
    };
    if keys.just_pressed(key) && !chat.input_open {
        state.open = !state.open;
    }
}

/// The roster page's "Invite" button, resolved against the current selection.
///
/// The button carries no target of its own — vanilla's does not either — so it
/// invites whoever is selected, which is the same resolution the right-click
/// target menu already performs. A non-player selection, or none at all, is
/// reported rather than sent: `PartyAction::Invite(0)` would be a real packet
/// aimed at nothing.
pub fn send_party_invite_request(
    mut requests: MessageReader<crate::plugins::hud::party::ui::PartyWindowRequest>,
    selected: Res<SelectedEntity>,
    players: Query<(&NetworkId, &RemoteEntity)>,
    mut actions: MessageWriter<PartyAction>,
    mut history: Option<ResMut<ChatHistory>>,
) {
    for request in requests.read() {
        if !matches!(
            request,
            crate::plugins::hud::party::ui::PartyWindowRequest::Invite
        ) {
            continue;
        }
        let target = selected
            .0
            .and_then(|entity| players.get(entity).ok())
            .filter(|(_, kind)| **kind == RemoteEntity::Player)
            .map(|(id, _)| id.0);
        match target {
            Some(id) => {
                actions.write(PartyAction::Invite(id));
            }
            None => {
                let text = "Select a player to invite.".to_string();
                match history.as_mut() {
                    Some(history) => history.push(ChatLine::system(text)),
                    None => info!("party: {text}"),
                }
            }
        }
    }
}

/// Which member each of the eight rows draws.
///
/// Row 0 is vanilla's pinned header row and belongs to the party leader: the
/// slot template declares **no** crown control while the header does, so a
/// leader sitting in a slot could never render one (§3b). Two consequences
/// worth stating:
///
/// - when the leader is unknown — a roster-only push carries no
///   `master_join_id` — row 0 still takes the first member rather than sitting
///   empty, because an 8-member party would otherwise lose one off the end.
///   It simply gets no crown, which is the honest rendering of "we don't know".
/// - the remaining members keep **wire order**. The server's order is the only
///   ordering anyone can agree on; sorting by name or level here would make our
///   row numbering disagree with the quick board and with the original.
pub fn roster_rows(roster: &PartyRoster) -> Vec<Option<&PartyMemberCore>> {
    let mut rows: Vec<Option<&PartyMemberCore>> = vec![None; PARTY_ROWS];
    let leader = roster.master_join_id.and_then(|id| roster.member(id));
    let leader_id = leader.and_then(|m| m.member_id);

    // Row 0: the leader, or — when the server has not named one — whoever the
    // server listed first.
    rows[0] = leader.or_else(|| roster.members.first());
    let pinned = rows[0].and_then(|m| m.member_id).or(leader_id);

    let mut next = 1;
    for member in &roster.members {
        if next >= PARTY_ROWS {
            break;
        }
        if member.member_id.is_some() && member.member_id == pinned {
            continue;
        }
        rows[next] = Some(member);
        next += 1;
    }
    rows
}

/// The local player's own party JID, found by name.
///
/// The client is never told its own JID: 0x3065 names the *leader*, not "you",
/// and the party wire shares no id with the entity stream. So the only join
/// available is the character name, which is the same one
/// [`quick_party_rows`], the map markers and the party-chat echo filter all
/// use. `None` when the roster has not named us — a partial presence mask is
/// enough for that.
pub fn local_member_id(roster: &PartyRoster, local_name: Option<&str>) -> Option<u32> {
    let local = local_name?;
    roster
        .members
        .iter()
        .find(|member| member.name.as_deref() == Some(local))
        .and_then(|member| member.member_id)
}

/// Whether the local player leads this party — the gate on Banish.
///
/// `false` when either half is unknown, which is the honest answer and the same
/// one [`PartyRoster::is_leader`] gives: offering a kick we cannot perform is
/// worse than not offering it.
pub fn we_lead(roster: &PartyRoster, local_name: Option<&str>) -> bool {
    local_member_id(roster, local_name).is_some_and(|id| roster.is_leader(id))
}

/// Slots on the quick-party board — one fewer than the party holds.
///
/// That asymmetry is the board's whole reading: vanilla puts the mini-info
/// (which is *you*) at screen `4,7` and this board directly beneath it at
/// `4,137`, and the board declares seven slots against a party of eight. So the
/// board lists **the other members**; your own bars are the panel above it.
pub const QUICK_PARTY_ROWS: usize = 7;

/// Which member each quick-board slot draws: everyone but the local player, in
/// wire order.
///
/// The local player is matched by name, the same way the minimap's party signs
/// already filter themselves out. That is a name comparison and not an id one
/// because the client is never told its own party JID — 0x3065 names the
/// leader, not "you" — so until a capture says otherwise this is the only
/// join available. A member with no name yet simply stays on the board.
pub fn quick_party_rows<'a>(
    roster: &'a PartyRoster,
    local_name: Option<&str>,
) -> Vec<Option<&'a PartyMemberCore>> {
    let mut rows: Vec<Option<&PartyMemberCore>> = vec![None; QUICK_PARTY_ROWS];
    let mut next = 0;
    for member in &roster.members {
        if next >= QUICK_PARTY_ROWS {
            break;
        }
        let is_local = match (local_name, member.name.as_deref()) {
            (Some(local), Some(name)) => local == name,
            _ => false,
        };
        if is_local {
            continue;
        }
        rows[next] = Some(member);
        next += 1;
    }
    rows
}

/// The HP and MP fractions to draw for one member, in 0..=1.
///
/// The wire only ever knows a member's bars to the nearest 10 %: the party
/// packet packs both into one byte's nibbles. That coarseness is the
/// original's, so it is the default.
///
/// `precise_hp` is the same member's real HP fraction when they happen to be
/// spawned near us, and is used only when the player opted into smoothing — a
/// member out of range has no reading at all, so the setting degrades to the
/// wire values rather than to a lie.
///
/// **MP is never smoothed, whatever the setting says.** A spawned remote entity
/// carries `EntityVitals { hp, max_hp }` and nothing else: the server simply
/// does not tell us another player's mana. So the MP bar keeps its 10 % steps
/// even beside a smoothed HP bar, and that asymmetry is the protocol's, not a
/// bug in the setting.
pub fn member_vitals(
    member: &PartyMemberCore,
    precise_hp: Option<f32>,
    smooth: bool,
) -> (f32, f32) {
    // A record whose mask never named hp/mp says nothing about the bars. Empty
    // is the only reading that does not invent a value.
    let (wire_hp, mp) = match member.hp_mp() {
        Some(packed) => (
            packed.hp_percent() as f32 / 100.0,
            packed.mp_percent() as f32 / 100.0,
        ),
        None => (0.0, 0.0),
    };
    let hp = match (smooth, precise_hp) {
        (true, Some(precise)) => precise.clamp(0.0, 1.0),
        _ => wire_hp,
    };
    (hp, mp)
}

/// The sharing mode in force: the party's own byte when there is a party, the
/// pending choice when there is not.
///
/// One function because three surfaces ask the same question and must not
/// answer it differently — the roster window's readout, the Form-party
/// dialog's section 3, and the `setup` byte a 0x7069 actually carries. Each of
/// those has been wrong on its own at least once.
///
/// The split is not a preference. A `0x7069` with `party_number = 0` is what
/// **creates** the party, so its `setup` byte is what sets the mode — that is
/// the one moment the modal's choice can apply, and 0x7060 (the invite path)
/// already honours it in `net::party`. Once the party exists its mode is the
/// server's: no C→S verb in the whole family changes a live party's setup, so
/// showing or sending anything but `roster.setup` would be a fiction.
pub fn effective_setup(roster: Option<&PartyRoster>, pending: PartySetup) -> PartySetup {
    match roster.filter(|roster| roster.is_active()) {
        Some(roster) => roster.setup,
        None => pending,
    }
}

/// The two `textuisystem` keys for the bottom mode readout, `(item, exp)`.
///
/// Both lines are always drawn — vanilla declares a static and a diamond for
/// each — and each reads either "auto share" or "free-for-all" off its own
/// `PartySetup` bit. The strings are looked up, never hardcoded: this run of
/// keys appears in no resinfo file at all, so the original assigns them in code
/// and they are the only place the wording lives (§3h).
pub fn mode_keys(setup: PartySetup) -> (&'static str, &'static str) {
    (
        if setup.is_item_shared() {
            "UIIT_STT_PARTY_ITEM_SHARE"
        } else {
            "UIIT_STT_PARTY_ITEM_SELF"
        },
        if setup.is_exp_shared() {
            "UIIT_STT_PARTY_EXP_SHARE"
        } else {
            "UIIT_STT_PARTY_EXP_SELF"
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::net::party::tests::{core, party_data};
    use packets::agent::party::{PartyMemberCore as Core, PartyMemberMask};

    fn roster_of(members: Vec<Core>, leader: Option<u32>) -> PartyRoster {
        let mut roster = PartyRoster::default();
        let mut data = party_data(members);
        data.master_join_id = leader;
        roster.apply_data(&data);
        roster
    }

    /// The defect: with no party, `roster.setup` is the default 0, so the
    /// window's readout ignored whatever the mode modal had just committed and
    /// "Set" looked like a control that did nothing.
    #[test]
    fn the_readout_follows_the_pending_choice_until_a_party_exists() {
        let chosen = PartySetup(PartySetup::EXP_SHARED | PartySetup::ITEM_SHARED);

        // no party at all: the pending choice is the only truth there is
        let (item, exp) = mode_keys(effective_setup(None, chosen));
        assert_eq!(exp, "UIIT_STT_PARTY_EXP_SHARE");
        assert_eq!(item, "UIIT_STT_PARTY_ITEM_SHARE");

        // an empty roster is "no party" too — this is the state the window is
        // actually in while you set the mode before inviting anyone
        let (item, exp) = mode_keys(effective_setup(Some(&PartyRoster::default()), chosen));
        assert_eq!(exp, "UIIT_STT_PARTY_EXP_SHARE");
        assert_eq!(item, "UIIT_STT_PARTY_ITEM_SHARE");
    }

    /// Once the party exists its mode is the server's. No C→S verb changes a
    /// live party's setup, so a pending byte that disagrees is stale and must
    /// not be shown as though it had taken.
    #[test]
    fn a_live_partys_mode_wins_over_a_stale_pending_one() {
        let mut roster = roster_of(vec![core(1, "Alice", 25000)], Some(1));
        roster.setup = PartySetup(PartySetup::ITEM_SHARED);

        let pending = PartySetup(PartySetup::EXP_SHARED);
        let setup = effective_setup(Some(&roster), pending);
        assert_eq!(setup, PartySetup(PartySetup::ITEM_SHARED));

        let (item, exp) = mode_keys(setup);
        assert_eq!(item, "UIIT_STT_PARTY_ITEM_SHARE");
        assert_eq!(
            exp, "UIIT_STT_PARTY_EXP_SELF",
            "the pending EXP bit is stale"
        );
    }

    /// The gate on Banish. Both halves have to line up — we have to be
    /// findable in the roster by name, and that record's JID has to be the
    /// leader's — and either one missing means `false`, because offering a
    /// kick we cannot perform is worse than not offering it.
    #[test]
    fn we_lead_only_when_the_roster_says_our_own_record_is_the_leader() {
        let members = vec![core(4, "priavte", 25000), core(1, "Ahri", 25000)];
        // the captured party: leader JID 4, which is "priavte"
        let roster = roster_of(members.clone(), Some(4));

        assert_eq!(local_member_id(&roster, Some("priavte")), Some(4));
        assert!(we_lead(&roster, Some("priavte")));
        assert!(
            !we_lead(&roster, Some("Ahri")),
            "a member is not the leader"
        );

        // a name the roster does not carry, and no name at all
        assert_eq!(local_member_id(&roster, Some("Nobody")), None);
        assert_eq!(local_member_id(&roster, None), None);
        assert!(!we_lead(&roster, Some("Nobody")));
        assert!(!we_lead(&roster, None));

        // a roster-only push never named a leader: nobody leads as far as we
        // know, which is the honest answer and not "we do"
        let leaderless = roster_of(members, None);
        assert_eq!(local_member_id(&leaderless, Some("priavte")), Some(4));
        assert!(!we_lead(&leaderless, Some("priavte")));
    }

    /// The leader is pinned to row 0 wherever the server listed them, and the
    /// rest keep wire order behind them.
    #[test]
    fn the_leader_is_pinned_to_the_header_row() {
        let roster = roster_of(
            vec![
                core(1, "Alice", 25000),
                core(2, "Bob", 25000),
                core(3, "Carol", 25000),
            ],
            Some(2),
        );

        let rows = roster_rows(&roster);
        assert_eq!(rows[0].unwrap().member_id, Some(2), "leader is pinned");
        assert_eq!(rows[1].unwrap().member_id, Some(1));
        assert_eq!(rows[2].unwrap().member_id, Some(3));
        assert!(rows[3].is_none());
        assert_eq!(rows.len(), PARTY_ROWS);
    }

    /// A full party must not lose its eighth member just because the server has
    /// not told us who leads — row 0 takes the first member instead of sitting
    /// empty.
    #[test]
    fn an_unknown_leader_still_fills_every_row() {
        let members: Vec<Core> = (1..=8).map(|i| core(i, "M", 25000)).collect();
        let roster = roster_of(members, None);

        let rows = roster_rows(&roster);
        assert!(rows.iter().all(|row| row.is_some()), "no row is empty");
        let drawn: Vec<u32> = rows
            .iter()
            .filter_map(|row| row.and_then(|m| m.member_id))
            .collect();
        assert_eq!(drawn, (1..=8).collect::<Vec<u32>>(), "each member once");
    }

    /// The quick board is the *other* members: your own bars are the mini-info
    /// panel directly above it, which is why seven slots serve a party of eight.
    #[test]
    fn the_quick_board_leaves_the_local_player_out() {
        let roster = roster_of(
            vec![
                core(1, "Alice", 25000),
                core(2, "Bob", 25000),
                core(3, "Carol", 25000),
            ],
            Some(1),
        );

        let rows = quick_party_rows(&roster, Some("Bob"));
        assert_eq!(rows.len(), QUICK_PARTY_ROWS);
        let drawn: Vec<&str> = rows
            .iter()
            .filter_map(|row| row.and_then(|m| m.name.as_deref()))
            .collect();
        assert_eq!(drawn, vec!["Alice", "Carol"]);

        // With nobody to match, everyone is drawn rather than nobody.
        let rows = quick_party_rows(&roster, None);
        assert_eq!(rows.iter().filter(|row| row.is_some()).count(), 3);
    }

    /// An empty party draws nothing at all, including in the pinned row.
    #[test]
    fn an_empty_party_draws_no_rows() {
        let empty = PartyRoster::default();
        let rows = roster_rows(&empty);
        assert!(rows.iter().all(|row| row.is_none()));
    }

    /// The bars are the wire's 10 % steps unless smoothing is on AND the member
    /// is close enough to have a real reading.
    #[test]
    fn vitals_prefer_the_wire_unless_smoothing_has_something_better() {
        let member = core(1, "Alice", 25000); // hp_mp 0x5A -> 100 % / 50 %
        assert_eq!(member_vitals(&member, None, false), (1.0, 0.5));
        // smoothing with nothing nearby falls back rather than inventing
        assert_eq!(member_vitals(&member, None, true), (1.0, 0.5));
        // a precise reading is ignored while the setting is off
        assert_eq!(member_vitals(&member, Some(0.37), false), (1.0, 0.5));
        // ...and applies to HP ONLY when it is on: the server never sends
        // another player's mana, so MP keeps its 10 % steps beside it.
        assert_eq!(member_vitals(&member, Some(0.37), true), (0.37, 0.5));
        // an out-of-range reading cannot push a bar past its ends
        assert_eq!(member_vitals(&member, Some(2.0), true), (1.0, 0.5));
    }

    /// A record whose mask never named hp/mp has no bars to draw — empty, not
    /// full, because "unknown" must not read as "healthy".
    #[test]
    fn a_member_without_hp_mp_draws_empty_bars() {
        let member = Core {
            presence: PartyMemberMask::LEVEL,
            level: Some(20),
            ..Default::default()
        };
        assert_eq!(member_vitals(&member, None, false), (0.0, 0.0));
    }

    /// Each readout line follows its own bit, and both lines always exist.
    #[test]
    fn the_mode_readout_follows_each_bit_separately() {
        let (item, exp) = mode_keys(PartySetup(PartySetup::EXP_SHARED));
        assert_eq!(item, "UIIT_STT_PARTY_ITEM_SELF");
        assert_eq!(exp, "UIIT_STT_PARTY_EXP_SHARE");

        let (item, exp) = mode_keys(PartySetup(PartySetup::ITEM_SHARED));
        assert_eq!(item, "UIIT_STT_PARTY_ITEM_SHARE");
        assert_eq!(exp, "UIIT_STT_PARTY_EXP_SELF");
    }
}
