//! The one owner of the server's action slot.
//!
//! Idea: vSRO gives a character **one** action slot. A `0x7074` sent while
//! another action runs is refused with a `01 02` start-rejected ack and
//! nothing else. Four call sites used to write to that slot independently —
//! the skill cast, the auto-attack order, the pickup order and the cast
//! retry — so nothing arbitrated between them and every keypress went
//! straight onto the wire. In a fight the player mashes hotkeys, and the
//! captures show what that costs: of 463 requests in `packet_dump/c2s/
//! 0x7074.log`, **178 (38%) are answered by a slot-busy rejection**, and the
//! log holds 29 runs of three-or-more byte-identical sends less than 600 ms
//! apart — the worst being 14 identical `CastSkill` requests over 2.2 s.
//!
//! So every producer now hands its intent here instead of sending, and this
//! module owns the wire. At most **one** intent is held: a newer press
//! replaces the one waiting, because the thing the player pressed last is the
//! thing they want. Mashing one skill therefore collapses to a single
//! request, and pressing B then C casts C. Replaying the capture through this
//! policy turns those 463 requests into 361, folding 99 presses that today
//! become rejections the player experiences as "nothing happened".
//!
//! It is the same shape [`PlayerCommands::move_to`](crate::plugins::player::PlayerCommands)
//! already uses for the movement lane: a new order replaces the queued one
//! rather than appending to it.
//!
//! **Stated deviation (ADR 0009).** No capture of the *original* client's
//! outbound traffic exists in this repository and no note records what it does
//! with a re-press, so this policy is ours, not vanilla's. Its rationale is
//! the paragraph above: fewer refused requests against someone else's server,
//! and the player's latest intent winning instead of being silently dropped.
//! The one duration it needs is sourced rather than invented — see
//! [`CombatSettings::action_buffer_seconds`](crate::plugins::config::combat::CombatSettings).
//!
//! Item use (`0x704C`) is deliberately **not** routed through here: it is a
//! different opcode with its own gate, and nothing in the corpus shows it
//! contending for the action slot. A potion pressed mid-cast must still go out
//! at once.

use bevy::prelude::*;

use packets::agent::prelude::{
    ActionCommand, ActionTarget, ObjectActionRequest, ObjectActionResponse,
};
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::plugins::config::ClientConfig;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::entities::NetworkId;

/// One thing the player can ask the action slot to do. Everything that turns
/// into a `0x7074 Execute` is one of these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Cast {
        skill_id: u32,
        target: Option<Entity>,
    },
    Attack(Entity),
    Pickup(Entity),
}

impl Action {
    /// The world entity this action names, if any. A self-buff
    /// (`Cast { target: None }`) names nothing.
    fn target(&self) -> Option<Entity> {
        match *self {
            Action::Cast { target, .. } => target,
            Action::Attack(entity) | Action::Pickup(entity) => Some(entity),
        }
    }
}

/// A cast/attack/pickup the server refused. Emitted so the presentation layer
/// can retract whatever it optimistically played for it — the skills plugin
/// reads this to drop a click-predicted swing.
///
/// This is the *only* honest way to attribute a refusal. The ack's `code` byte
/// cannot do it: correlating `packet_dump/c2s/0x7074.log` against
/// `packet_dump/0xb074.log` shows `Failed { code: 1 }` answering a **cast** 45
/// times and `Started { code: 1 }` answering a cast 138 times, so the byte is
/// not the action type. Because this module keeps at most one request in
/// flight, the refused action is simply known.
#[derive(Message)]
pub struct ActionRefused(pub Action);

/// A request sent and awaiting its first ack.
struct InFlight {
    action: Action,
    sent_at: f64,
    /// Attempts already spent on this action (1 = the original send). A
    /// re-parked action that is refused a second time is dropped instead of
    /// bouncing between here and [`ActionSlot::next`] forever — the bound the
    /// old `CastRetry::just_retried` provided.
    attempts: u8,
}

/// The intent waiting for the slot to free.
struct Pending {
    action: Action,
    /// When the player asked for it, so a stale one can be dropped.
    at: f64,
    attempts: u8,
}

/// How long a request may wait for its FIRST ack before we stop counting the
/// slot as ours.
///
/// Origin: send→next-ack latency over the whole `packet_dump/c2s/0x7074.log` ×
/// `packet_dump/0xb074.log` correlation is median 107 ms, p90 219 ms. Two
/// seconds is ~9× that p90 — long enough that a live round trip never trips
/// it, short enough that an ack the server never sends cannot park the slot.
const ACK_TIMEOUT_SECS: f64 = 2.0;

/// How many times one intent may be sent before it is abandoned.
///
/// Two — the original press plus one re-try after a slot-busy refusal. The
/// slot frees within one action window (median 1.38 s in
/// `packet_dump/0xb074.log`), so a request refused on both attempts is being
/// refused for a reason we do not model, and a third try would only earn the
/// same answer.
const MAX_ATTEMPTS: u8 = 2;

/// The client's model of the server's single action slot, plus the one thing
/// the player most recently asked it to do.
#[derive(Resource, Default)]
pub struct ActionSlot {
    /// An action the server told us it started and has not ended. Tracked from
    /// the `0xB074` acks, not from our own sends, since we have no visibility
    /// into server state until it answers — the server also runs actions we
    /// never requested (it restarts its own auto-attack loop after a cast).
    server_busy: bool,
    /// Our request between the send and its first ack.
    in_flight: Option<InFlight>,
    next: Option<Pending>,
}

impl ActionSlot {
    /// Ask the slot to do `action`. Replaces whatever was waiting — the newest
    /// press wins.
    pub fn request(&mut self, action: Action, now: f64) {
        if let Some(displaced) = self.next.replace(Pending {
            action,
            at: now,
            attempts: 0,
        }) {
            debug!(
                "action slot: {:?} replaces the waiting {:?}",
                action, displaced.action
            );
        }
    }

    /// Whether the slot cannot take a new request right now. Gates the skill
    /// plugin's click prediction: predicting while the slot is occupied plays
    /// a throwaway swing that the server's refusal then contradicts.
    pub fn occupied(&self) -> bool {
        self.server_busy || self.in_flight.is_some()
    }

    /// Drop the waiting intent and send a `Cancel`'s worth of forgetting — a
    /// deliberate stop must not be followed by the thing it interrupted.
    pub fn clear_pending(&mut self) {
        self.next = None;
    }

    /// Drop the waiting intent if it names `entity` — called when that entity
    /// dies, so the slot frees for the player's next press instead of holding
    /// an intent that can no longer do anything.
    pub fn forget_target(&mut self, entity: Entity) {
        if self.next.as_ref().and_then(|p| p.action.target()) == Some(entity) {
            let dropped = self.next.take().expect("checked above");
            debug!(
                "action slot: {:?} dropped — its target died",
                dropped.action
            );
        }
    }

    /// The in-flight request was refused because the slot was already taken.
    /// Puts it back to wait for the running action to end and returns it, so
    /// the caller can announce the refusal.
    ///
    /// Bounded: a request refused on every attempt must not bounce between the
    /// wire and the buffer forever — the job the old `CastRetry::just_retried`
    /// did.
    fn reject_in_flight(&mut self, now: f64) -> Option<Action> {
        let flight = self.in_flight.take()?;
        if flight.attempts >= MAX_ATTEMPTS {
            info!(
                "action slot: {:?} refused every try — giving up",
                flight.action
            );
        } else {
            self.next = Some(Pending {
                action: flight.action,
                at: now,
                attempts: flight.attempts,
            });
        }
        Some(flight.action)
    }

    /// Stop counting a request as ours once its ack window has passed, so an
    /// ack the server never sends cannot park the slot forever.
    fn release_stalled_send(&mut self, now: f64) {
        if self
            .in_flight
            .as_ref()
            .is_some_and(|f| now - f.sent_at > ACK_TIMEOUT_SECS)
        {
            let flight = self.in_flight.take().expect("checked above");
            debug!("action slot: no ack for {:?} — releasing", flight.action);
        }
    }

    /// The waiting intent, if it may go out now.
    ///
    /// Two reasons to **drop** it, checked first so a doomed intent frees the
    /// slot for the player's next press rather than occupying it:
    ///
    /// - its target died while it waited. The buffer made this reachable: the
    ///   press is held for seconds where it used to go out at once, and a
    ///   corpse keeps its network id for [`CORPSE_LINGER_SECS`] — so without
    ///   this the follow-up skill fires at a dead monster.
    /// - it has gone stale. The fight has moved on and firing it now would act
    ///   on an intent the player has forgotten about.
    ///
    /// Two reasons to **hold** it: the server's slot is occupied, or our own
    /// body is still playing the cast this one follows. The second is what
    /// makes "first finishes, second follows" true on the wire and not just in
    /// the animation queue — the server frees its slot (median 1.38 s) well
    /// before a cast clip ends (2.4-5.1 s for a chain).
    ///
    /// Every drop is logged: a press that vanishes without a trace is what
    /// this module exists to stop.
    fn take_ready(
        &mut self,
        now: f64,
        buffer_secs: f64,
        mid_cast: bool,
        is_dead: impl Fn(Entity) -> bool,
    ) -> Option<Pending> {
        let pending = self.next.as_ref()?;
        if pending.action.target().is_some_and(is_dead) {
            let dropped = self.next.take().expect("checked above");
            info!(
                "action slot: {:?} — its target is dead, dropped",
                dropped.action
            );
            return None;
        }
        if now - pending.at > buffer_secs {
            let dropped = self.next.take().expect("checked above");
            info!("action slot: {:?} went stale — dropped", dropped.action);
            return None;
        }
        if self.occupied() || mid_cast {
            return None; // keep waiting
        }
        self.next.take()
    }

    /// Record that `pending` is now on the wire.
    fn mark_sent(&mut self, pending: &Pending, now: f64) {
        self.in_flight = Some(InFlight {
            action: pending.action,
            sent_at: now,
            attempts: pending.attempts + 1,
        });
    }
}

/// Fold the `0xB074` acks into the slot's state, re-parking a refused request
/// and announcing every refusal as an [`ActionRefused`].
pub fn apply_action_acks(
    time: Res<Time>,
    mut acks: MessageReader<ObjectActionResponse>,
    mut slot: ResMut<ActionSlot>,
    mut refusals: MessageWriter<ActionRefused>,
) {
    let now = time.elapsed_secs_f64();
    for ack in acks.read() {
        match ack {
            // Refused: the slot was already taken by something else, which is
            // still running — so `server_busy` is deliberately left alone.
            // The request itself goes back to the front of the queue and is
            // re-sent once the running action ends.
            ObjectActionResponse::Started { .. } if ack.is_rejected() => {
                // `None` = an ack for something we did not send.
                if let Some(refused) = slot.reject_in_flight(now) {
                    refusals.write(ActionRefused(refused));
                }
            }
            ObjectActionResponse::Started { .. } => {
                slot.in_flight = None;
                slot.server_busy = true;
            }
            ObjectActionResponse::Ended { .. } => {
                slot.server_busy = false;
            }
            // A refusal on the request's own merits (a cooldown the client's
            // execution-time mirror never saw start, a precondition we do not
            // model). It ends nothing and starts nothing, so `server_busy` is
            // untouched — but the request will never execute, so it is NOT
            // re-parked, and whatever was played for it has to come back off.
            ObjectActionResponse::Failed { .. } => {
                if let Some(flight) = slot.in_flight.take() {
                    info!("action slot: {:?} refused by the server", flight.action);
                    refusals.write(ActionRefused(flight.action));
                }
            }
            ObjectActionResponse::Unknown { .. } => {}
        }
    }
}

/// Send the waiting intent as soon as the slot and the caster's body are both
/// free, or drop it once its target has died or it has gone stale.
///
/// Ordered after every producer so a press onto a free slot still reaches the
/// wire in the same frame it was made — the buffer costs latency only when
/// something really is busy.
pub fn drive_action_slot(
    time: Res<Time>,
    config: Res<ClientConfig>,
    mut slot: ResMut<ActionSlot>,
    ids: Query<&NetworkId>,
    liveness: super::TargetLiveness,
    players: Query<Entity, With<crate::plugins::player::Player>>,
    children: Query<&Children>,
    casting: crate::plugins::player::CastingWrappers,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    let now = time.elapsed_secs_f64();
    slot.release_stalled_send(now);
    let buffer_secs = config.combat.action_buffer_seconds.max(0.0) as f64;
    // A cast still on the body holds the next action back. Not `is_mid_one_shot`
    // — that also reports a flinch, and stalling the wire on the most
    // interruptible clip there is would be worse than sending early.
    let mid_cast = players
        .single()
        .is_ok_and(|player| crate::plugins::player::is_mid_cast(player, &children, &casting));
    let Some(pending) = slot.take_ready(now, buffer_secs, mid_cast, |entity| {
        liveness.is_known_dead(entity)
    }) else {
        return;
    };
    let Ok(conn) = conn.single() else {
        // Offline sandbox: no agent connection, so nothing to send to.
        return;
    };
    let target = |entity: Entity| match ids.get(entity) {
        Ok(NetworkId(uid)) => Some(ActionTarget::Entity { unique_id: *uid }),
        Err(_) => None,
    };
    let command = match pending.action {
        Action::Cast {
            skill_id,
            target: t,
        } => {
            // A self-buff goes out untargeted on purpose; only an entity we
            // cannot resolve at all is a reason to drop the request.
            let wire_target = match t {
                Some(entity) => match target(entity) {
                    Some(resolved) => resolved,
                    None => {
                        warn!("action slot: cast {skill_id} target is gone — dropped");
                        return;
                    }
                },
                None => ActionTarget::None,
            };
            ActionCommand::CastSkill {
                ref_skill_id: skill_id,
                target: wire_target,
            }
        }
        Action::Attack(entity) => match target(entity) {
            Some(resolved) => ActionCommand::Attack(resolved),
            None => return,
        },
        Action::Pickup(entity) => match target(entity) {
            Some(resolved) => ActionCommand::Pickup(resolved),
            None => return,
        },
    };
    info!("action slot: sending {:?} (0x7074)", pending.action);
    if let Err(e) = conn
        .get_sender()
        .send(Packet::from(ObjectActionRequest::Execute(command)).into())
    {
        error!("network: failed to send ObjectActionRequest: {}", e.0);
        return;
    }
    slot.mark_sent(&pending, now);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cast(id: u32) -> Action {
        Action::Cast {
            skill_id: id,
            target: None,
        }
    }

    /// The whole point: mashing one hotkey is one request, not fourteen. The
    /// capture's worst run is exactly this shape — 14 identical `CastSkill`
    /// sends over 2.2 s.
    #[test]
    fn mashing_one_skill_leaves_a_single_intent() {
        let mut slot = ActionSlot::default();
        for step in 0..14 {
            slot.request(cast(7185), step as f64 * 0.15);
        }
        assert_eq!(
            slot.next.as_ref().map(|p| p.action),
            Some(cast(7185)),
            "one intent survives the burst"
        );
    }

    /// Newest wins: press B then C and C is what casts.
    #[test]
    fn a_newer_press_replaces_the_waiting_one() {
        let mut slot = ActionSlot::default();
        slot.request(cast(1), 0.0);
        slot.request(cast(2), 0.1);
        assert_eq!(slot.next.as_ref().map(|p| p.action), Some(cast(2)));
    }

    /// `occupied` is the union of both signals: our own request awaiting its
    /// first ack, and an action the server told us it started. The second
    /// matters because the server runs actions we never asked for.
    #[test]
    fn the_slot_is_occupied_by_either_signal() {
        let mut slot = ActionSlot::default();
        assert!(!slot.occupied(), "an idle slot takes a request at once");
        slot.server_busy = true;
        assert!(slot.occupied());
        slot.server_busy = false;
        slot.in_flight = Some(InFlight {
            action: cast(1),
            sent_at: 0.0,
            attempts: 1,
        });
        assert!(slot.occupied(), "a sent request still owns the slot");
    }

    /// A free slot must not cost the press any latency — the buffer exists
    /// for the busy case only.
    #[test]
    fn a_press_onto_a_free_slot_goes_out_at_once() {
        let mut slot = ActionSlot::default();
        slot.request(cast(7185), 10.0);
        assert_eq!(
            slot.take_ready(10.0, 3.0, false, |_| false)
                .map(|p| p.action),
            Some(cast(7185))
        );
    }

    /// While the slot is occupied the intent waits rather than being dropped
    /// or sent into a refusal, and goes out on the frame it frees.
    #[test]
    fn a_press_waits_for_the_slot_and_then_goes() {
        let mut slot = ActionSlot::default();
        slot.server_busy = true;
        slot.request(cast(7185), 10.0);
        assert!(
            slot.take_ready(10.1, 3.0, false, |_| false).is_none(),
            "still waiting"
        );
        assert!(slot.next.is_some(), "and still held, not dropped");
        slot.server_busy = false;
        assert_eq!(
            slot.take_ready(10.2, 3.0, false, |_| false)
                .map(|p| p.action),
            Some(cast(7185))
        );
    }

    /// Past the buffer window the fight has moved on: firing then would act on
    /// an intent the player has forgotten, possibly at a dead target. Dropped,
    /// not sent late.
    #[test]
    fn a_stale_intent_is_dropped_rather_than_fired_late() {
        let mut slot = ActionSlot::default();
        slot.request(cast(7185), 10.0);
        assert!(slot.take_ready(13.5, 3.0, false, |_| false).is_none());
        assert!(
            slot.next.is_none(),
            "and it is gone, not left to fire later"
        );
    }

    /// A request whose ack never arrives must not park the slot forever.
    #[test]
    fn a_send_with_no_ack_releases_the_slot() {
        let mut slot = ActionSlot::default();
        slot.mark_sent(
            &Pending {
                action: cast(7185),
                at: 0.0,
                attempts: 0,
            },
            0.0,
        );
        slot.release_stalled_send(ACK_TIMEOUT_SECS - 0.1);
        assert!(
            slot.occupied(),
            "a live round trip must not trip the release"
        );
        slot.release_stalled_send(ACK_TIMEOUT_SECS + 0.1);
        assert!(!slot.occupied());
    }

    /// The two refusal shapes differ in one thing only: whether the request
    /// still has a chance. A slot-busy rejection means something else was
    /// running, so the request goes back to wait for it; a `Failed` means the
    /// server declined it on its merits, so re-firing would earn the same
    /// answer. Both announce the refusal, since either way the presentation
    /// layer may have played something for it.
    #[test]
    fn a_busy_slot_re_parks_the_request_but_a_refusal_does_not() {
        use packets::agent::prelude::ACTION_START_REJECTED;

        fn answer(ack: ObjectActionResponse) -> (bool, usize) {
            let mut app = App::new();
            app.init_resource::<Time>()
                .init_resource::<ActionSlot>()
                .add_message::<ObjectActionResponse>()
                .add_message::<ActionRefused>()
                .add_systems(Update, apply_action_acks);
            app.world_mut().resource_mut::<ActionSlot>().mark_sent(
                &Pending {
                    action: cast(7185),
                    at: 0.0,
                    attempts: 0,
                },
                0.0,
            );
            app.world_mut().write_message(ack);
            app.update();
            let re_parked = app.world().resource::<ActionSlot>().next.is_some();
            let refusals = app
                .world_mut()
                .resource_mut::<bevy::ecs::message::Messages<ActionRefused>>()
                .drain()
                .count();
            (re_parked, refusals)
        }

        assert_eq!(
            answer(ObjectActionResponse::Started {
                code: ACTION_START_REJECTED
            }),
            (true, 1),
            "a busy slot parks the request to try again"
        );
        assert_eq!(
            answer(ObjectActionResponse::Failed {
                code: 0,
                error: 0x4004
            }),
            (false, 1),
            "a refusal on the merits is announced but never retried"
        );
    }

    /// A request that is refused every time it is tried must not bounce
    /// between the wire and the buffer forever.
    #[test]
    fn a_twice_refused_request_gives_up() {
        let mut slot = ActionSlot::default();
        slot.in_flight = Some(InFlight {
            action: cast(7185),
            sent_at: 0.0,
            attempts: 2,
        });
        slot.reject_in_flight(1.0);
        assert!(slot.next.is_none(), "it stops being re-parked");
    }

    /// The reported defect: a press held against a busy slot must not go out
    /// at a monster that died while it waited. The buffer is what made this
    /// reachable — a corpse keeps its network id for 4 s, so resolving the id
    /// at send time is not enough on its own.
    #[test]
    fn a_press_whose_target_died_is_dropped() {
        let victim = Entity::from_raw_u32(7).unwrap();
        let mut slot = ActionSlot::default();
        slot.request(
            Action::Cast {
                skill_id: 7185,
                target: Some(victim),
            },
            10.0,
        );
        assert!(slot.take_ready(10.1, 3.0, false, |e| e == victim).is_none());
        assert!(slot.next.is_none(), "and it does not linger in the buffer");
    }

    /// ...but only the ones that actually name the dead thing. A self-buff
    /// names nothing, and an action at some other entity is unaffected.
    #[test]
    fn an_unrelated_intent_survives_a_death() {
        let corpse = Entity::from_raw_u32(7).unwrap();
        let mut slot = ActionSlot::default();
        slot.request(cast(7185), 10.0); // self-buff, target: None
        assert!(
            slot.take_ready(10.1, 3.0, false, |e| e == corpse).is_some(),
            "an untargeted cast has nothing to be dead"
        );
        slot.request(Action::Pickup(Entity::from_raw_u32(9).unwrap()), 10.0);
        assert!(
            slot.take_ready(10.1, 3.0, false, |e| e == corpse).is_some(),
            "another entity's death is not this action's business"
        );
    }

    /// `forget_target` frees the slot the moment the target dies, rather than
    /// leaving a doomed intent to occupy it until the send-time check.
    #[test]
    fn forgetting_a_target_drops_only_its_own_intent() {
        let victim = Entity::from_raw_u32(7).unwrap();
        let other = Entity::from_raw_u32(8).unwrap();
        let mut slot = ActionSlot::default();
        slot.request(Action::Attack(victim), 10.0);
        slot.forget_target(other);
        assert!(slot.next.is_some(), "someone else's death changes nothing");
        slot.forget_target(victim);
        assert!(slot.next.is_none());
    }

    /// The second reported defect: the server frees its slot (median 1.38 s)
    /// well before a cast clip ends (2.4-5.1 s for a chain), so releasing on
    /// the server alone sent the follow-up mid-animation.
    #[test]
    fn a_press_waits_for_the_cast_clip_to_finish() {
        let mut slot = ActionSlot::default();
        slot.request(cast(7185), 10.0);
        assert!(
            slot.take_ready(11.4, 5.5, true, |_| false).is_none(),
            "the server slot is free but our cast is still playing"
        );
        assert!(slot.next.is_some(), "held, not dropped");
        assert_eq!(
            slot.take_ready(12.8, 5.5, false, |_| false)
                .map(|p| p.action),
            Some(cast(7185)),
            "and it goes out when the clip ends"
        );
    }

    /// The window has to outlast the clip it now waits on, or a press made
    /// during the longest authored chain (5.1 s) would expire before the body
    /// ever freed. This is what ties `action_buffer_seconds` to the clip
    /// length rather than to the server's action window.
    #[test]
    fn a_press_survives_the_longest_chain_clip() {
        /// `skill_ch_sword_chain_h.ban`, the longest chain animation in the
        /// corpus (see `skills::cast::MAX_SWING_QUEUE_LAG_SECS`).
        const LONGEST_CLIP_SECS: f64 = 5.1;
        let mut slot = ActionSlot::default();
        slot.request(cast(7185), 0.0);
        assert!(
            slot.take_ready(LONGEST_CLIP_SECS, 5.5, true, |_| false)
                .is_none(),
            "still playing"
        );
        assert!(
            slot.take_ready(LONGEST_CLIP_SECS + 0.01, 5.5, false, |_| false)
                .is_some(),
            "the press outlives the clip it was waiting for"
        );
    }

    /// A deliberate stop must not be chased by the thing it interrupted.
    #[test]
    fn cancelling_forgets_the_waiting_intent() {
        let mut slot = ActionSlot::default();
        slot.request(cast(7185), 0.0);
        slot.clear_pending();
        assert!(slot.next.is_none());
    }
}
