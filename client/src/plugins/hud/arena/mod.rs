//! Battle Arena — the client's arena state and its first visible surface.
//!
//! Idea: the original ships **five** descriptor files for one feature
//! (`arena_rule_select`, `arena_guilduser_select`, `arena_game_score`,
//! `arena_game_rank`, `arena_game_result`) because its 2DT format has no way
//! to say "these are phases of one thing". We do: one [`ArenaState`] carries
//! the phase, the score and the rank rows, and each surface is a projection of
//! it (`docs/re/ui/hud-arena-windows.md` §8). This module is the state plus
//! the first of those surfaces — the in-match score strip and rank board,
//! which the doc's build plan names as the first slice because they are the
//! smallest and the most visible.
//!
//! Everything here is driven by `0x34D2`, the scheduler broadcast wired in
//! `packets::agent::barena`. That ordering is the doc's own: an arena window
//! built before the arena state exists is an empty box.
//!
//! What is **not** here, deliberately: registration, the formation picker and
//! the result sheet (the other three descriptors), and any outbound
//! register/cancel packet — its opcode is an unresolved `0x74D2`-vs-`0x74D3`
//! conflict (`docs/re/systems/battle-arena.md` §9.1), and picking one would be
//! inventing the wire.

use bevy::prelude::*;

use packets::agent::barena::{BArenaOp, BArenaOperation, BArenaTeam, BArenaUpdate};

pub mod scoreboard;

/// Where the scheduler says the round is. One state machine instead of five
/// windows' worth of independent visibility flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArenaPhase {
    /// No round is being advertised.
    #[default]
    Idle,
    /// Registration is open (`0x02`) or has just closed (`0x03`).
    Registering,
    /// One of the two start alarms has fired (`0x0D` 5 min, `0x0E` 1 min).
    Starting,
    /// The round is running (`0x04`, or any scoreboard update).
    InMatch,
    /// The round produced a result (`0x09`) and has not been cleared yet.
    Results,
}

/// Both team totals, held once and rendered into every readout that shows them
/// — the original repeats the digit/separator/digit triplet at two sizes
/// (`hud-arena-windows.md` §3.5, §3.6) and two independently laid-out copies of
/// one number is how they drift.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArenaScore {
    pub red: u32,
    pub blue: u32,
}

/// One row of the rank board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArenaRankRow {
    pub team: Option<BArenaTeam>,
    pub name: String,
    pub points: u32,
}

#[derive(Resource, Default)]
pub struct ArenaState {
    pub phase: ArenaPhase,
    pub score: ArenaScore,
    /// The rows the server sent, in the order it sent them. A `Vec`, not five
    /// fixed slots: the descriptor's 5 rows are a viewport height, not a
    /// capacity (`hud-arena-windows.md` §8).
    pub ranks: Vec<ArenaRankRow>,
    /// `RegistrationBegin`'s mode mask (`0x20` CTF, `0x40` Score), kept for the
    /// registration window that is not built yet.
    pub game_type_mask: Option<u16>,
    /// `GameStartTime`'s round length in milliseconds.
    pub max_time_ms: Option<u32>,
}

impl ArenaState {
    /// The in-match overlays exist while the round runs. Nothing else in the
    /// arena set is built yet, so this is the whole visibility rule.
    pub fn board_visible(&self) -> bool {
        self.phase == ArenaPhase::InMatch
    }
}

/// Fold one `0x34D2` broadcast into the state.
///
/// A free function rather than a system body so the transition table is
/// testable without an `App`: the packet is the only input the arena has, and
/// the phase it produces decides whether anything is drawn at all.
pub fn apply_operation(state: &mut ArenaState, packet: &BArenaOperation) {
    match packet.operation() {
        Some(BArenaOp::RegistrationBegin) => {
            state.phase = ArenaPhase::Registering;
            state.game_type_mask = packet.game_type_mask();
        }
        Some(BArenaOp::RegistrationEnd) => state.phase = ArenaPhase::Registering,
        Some(BArenaOp::GameStartAlarmIn5 | BArenaOp::GameStartAlarmIn1) => {
            state.phase = ArenaPhase::Starting
        }
        Some(BArenaOp::GameStart) => {
            state.phase = ArenaPhase::InMatch;
            state.score = ArenaScore::default();
            state.ranks.clear();
        }
        Some(BArenaOp::GameStartTime) => {
            state.phase = ArenaPhase::InMatch;
            state.max_time_ms = packet
                .body
                .get(0..4)
                .and_then(|head| Some(u32::from_le_bytes(head.try_into().ok()?)));
        }
        Some(BArenaOp::GameResult) => state.phase = ArenaPhase::Results,
        Some(BArenaOp::GameEnd) => {
            *state = ArenaState::default();
        }
        Some(BArenaOp::Update) => match packet.update() {
            // A scoreboard arriving while we think the round has not started
            // is authoritative: we may have joined mid-round, or missed 0x04.
            Some(BArenaUpdate::Scoreboard {
                total_red,
                total_blue,
                ranks,
            }) => {
                state.phase = ArenaPhase::InMatch;
                state.score = ArenaScore {
                    red: total_red,
                    blue: total_blue,
                };
                state.ranks = ranks
                    .into_iter()
                    .map(|entry| ArenaRankRow {
                        team: entry.team,
                        name: entry.name,
                        points: entry.points,
                    })
                    .collect();
            }
            Some(BArenaUpdate::Countdown { max_time, .. }) => {
                state.phase = ArenaPhase::InMatch;
                state.max_time_ms = Some(max_time);
            }
            // GainedPoints and the uncaptured CTF sub-ops change no surface we
            // draw yet; the totals come from the scoreboard either way.
            _ => {}
        },
        None => {
            debug!(
                "arena: unhandled 0x34D2 op {:#04X} ({} body bytes)",
                packet.op,
                packet.body.len()
            );
        }
    }
}

fn on_barena_operation(mut packets: MessageReader<BArenaOperation>, mut state: ResMut<ArenaState>) {
    for packet in packets.read() {
        apply_operation(&mut state, packet);
    }
}

pub struct ArenaPlugin;

impl Plugin for ArenaPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ArenaState>()
            .add_systems(Update, on_barena_operation)
            .add_plugins(scoreboard::ArenaScoreboardPlugin);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use bytes::{BufMut, Bytes, BytesMut};

    fn packet(bytes: &[u8]) -> BArenaOperation {
        BArenaOperation::try_from(Bytes::copy_from_slice(bytes)).unwrap()
    }

    /// The two cycles in `packet_dump/0x34d2.log` walk the state machine end to
    /// end, and `GameEnd` puts it back where it started — otherwise a stale
    /// score strip would survive into the next round.
    #[test]
    fn the_captured_cycle_walks_the_phase_machine_and_clears_at_the_end() {
        let mut state = ArenaState::default();
        assert_eq!(state.phase, ArenaPhase::Idle);

        apply_operation(&mut state, &packet(&[0x02, 0x00, 0x40, 0x00]));
        assert_eq!(state.phase, ArenaPhase::Registering);
        assert_eq!(state.game_type_mask, Some(0x0040));
        assert!(!state.board_visible(), "no board before the round runs");

        apply_operation(&mut state, &packet(&[0x03, 0x00, 0x00, 0x00]));
        assert_eq!(state.phase, ArenaPhase::Registering);
        apply_operation(&mut state, &packet(&[0x0D, 0x00, 0x00, 0x00]));
        assert_eq!(state.phase, ArenaPhase::Starting);
        apply_operation(&mut state, &packet(&[0x0E, 0x00, 0x00, 0x00]));
        assert_eq!(state.phase, ArenaPhase::Starting);

        apply_operation(&mut state, &packet(&[0x04, 0x00, 0x00, 0x00]));
        assert!(state.board_visible());

        // 0x0500 — the two-byte GameEnd our capture ends each cycle with
        apply_operation(&mut state, &packet(&[0x05, 0x00]));
        assert_eq!(state.phase, ArenaPhase::Idle);
        assert!(!state.board_visible());
        assert_eq!(state.game_type_mask, None);
    }

    /// A scoreboard is the round's own evidence that it is running: joining
    /// mid-round means never seeing 0x04, and the board must still appear.
    #[test]
    fn a_scoreboard_update_fills_the_board_and_implies_the_round_runs() {
        let mut body = BytesMut::new();
        body.put_u8(0xFF);
        body.put_u8(0x41);
        body.put_u32_le(12);
        body.put_u32_le(7);
        body.put_u8(1);
        body.put_u8(1);
        body.put_u16_le(4);
        body.put_slice(b"Sura");
        body.put_u32_le(7);

        let mut state = ArenaState::default();
        apply_operation(&mut state, &packet(&body));
        assert!(state.board_visible());
        assert_eq!(state.score, ArenaScore { red: 12, blue: 7 });
        assert_eq!(
            state.ranks,
            vec![ArenaRankRow {
                team: Some(BArenaTeam::Blue),
                name: "Sura".into(),
                points: 7,
            }]
        );

        // the authored 5 rows are a viewport, not a capacity
        let mut wide = BytesMut::new();
        wide.put_u8(0xFF);
        wide.put_u8(0x41);
        wide.put_u32_le(0);
        wide.put_u32_le(0);
        wide.put_u8(8);
        for i in 0..8u32 {
            wide.put_u8(0);
            wide.put_u16_le(2);
            wide.put_slice(b"ab");
            wide.put_u32_le(i);
        }
        apply_operation(&mut state, &packet(&wide));
        assert_eq!(state.ranks.len(), 8);
    }

    /// An op we have never captured must not move the phase or wipe the board.
    #[test]
    fn an_unknown_op_leaves_the_state_alone() {
        let mut state = ArenaState::default();
        apply_operation(&mut state, &packet(&[0x04, 0x00, 0x00, 0x00]));
        state.score = ArenaScore { red: 3, blue: 4 };
        apply_operation(&mut state, &packet(&[0x0B, 0x00, 0x00, 0x00]));
        assert_eq!(state.phase, ArenaPhase::InMatch);
        assert_eq!(state.score, ArenaScore { red: 3, blue: 4 });
    }
}
