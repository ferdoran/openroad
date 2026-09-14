//! Combat-presentation knobs.

use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct CombatSettings {
    /// Seconds a knocked-down body stays prone (`DOWN_RM`, type 63) between the
    /// fall (`DOWN`, 62) and getting up (`DOWN_UP`, 65).
    ///
    /// **Negative (the default) means "derive it from the data"**, which is
    /// what `player::knockdown_hold_secs` does: the victim's own characterdata
    /// `KO_RecoverTime` (col 82) if it reads as a duration, else the authored
    /// length of the lying-down clip itself. Set a value >= 0 to override both.
    ///
    /// Why derived rather than a constant: the displacement arms of
    /// 0xB070/0xB071 are what say a hit knocked its target down, but **no
    /// packet in the corpus carries how long the body stays there** and the
    /// original's value lives in its code
    /// (`docs/re/formats/anim-state-coverage.md` §9). Both fallbacks are real
    /// game data, so no number is invented — but the column's unit is still
    /// `[U]`, hence the clamp documented on
    /// `CharacterDataRow::knockdown_recover_secs`.
    ///
    /// A model whose animation group ships no DOWN block is unaffected: it
    /// plays no knockdown at all.
    pub knockdown_hold_seconds: f32,

    /// How long a hotkey press may wait for the server's action slot before it
    /// is dropped as stale (see
    /// [`ActionSlot`](crate::plugins::combat::action_slot::ActionSlot)).
    ///
    /// **Origin, not invention:** the window has to cover the longest thing a
    /// press can legitimately wait for, and that is the caster's own clip, not
    /// the server's action window — the slot holds a press until the cast it
    /// follows has finished animating. The longest authored chain animation in
    /// the corpus is `skill_ch_sword_chain_h.ban` at 5.1 s (the same
    /// measurement `skills::cast::MAX_SWING_QUEUE_LAG_SECS` is derived from),
    /// so 5.5 s is that plus slack. Sized to the server's action window
    /// instead (median 1.38 s, p90 2.95 s over `packet_dump/0xb074.log`) a
    /// press made during a chain expired before the body ever freed.
    ///
    /// It may safely exceed the 4 s a corpse lingers for, because a buffered
    /// press whose target dies is dropped by an explicit liveness check rather
    /// than by keeping this window short enough to expire first.
    ///
    /// **Stated deviation (ADR 0009):** buffering presses at all is ours, not
    /// the original's — no capture of the original client's outbound traffic
    /// exists to copy. The knob is here because ADR-0009 asks for one wherever
    /// a change touches pacing/feel *and* the request rate a live server sees;
    /// this is both. `0.0` keeps the newest-press-wins collapse but never holds
    /// a press across a busy slot.
    pub action_buffer_seconds: f32,
}

impl Default for CombatSettings {
    fn default() -> Self {
        CombatSettings {
            // Negative = derive; see the field docs.
            knockdown_hold_seconds: -1.0,
            // longest authored chain clip (5.1 s) plus slack; see the docs.
            action_buffer_seconds: 5.5,
        }
    }
}
