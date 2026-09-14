//! HUD behaviour knobs.

use serde::Deserialize;

/// Where a party member's 28x28 portrait comes from.
///
/// The original leaves this genuinely unresolved: all three
/// `CIFStaticWithPictureClip` portrait controls carry `DDJ=""`, so the art is
/// picked in code (`docs/re/ui/hud-party-window.md` §9-U1). Decoding the
/// candidate art settles what it is *not* — `pt_face.ddj` is a flat opaque
/// black 28x28 plate, `qpt_face.ddj` an opaque black disc, `pt_no_face.ddj` a
/// translucent placeholder and the `qpt_face_faraway_*` family an opacity
/// ladder of one blue disc. None of them is a face, so they are the backdrop
/// and the status tint; the face itself has to be looked up per member.
#[derive(Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PartyPortraitSource {
    /// Resolve `characterdata`'s own icon column from the member's `model_id`
    /// (`CharacterDataRow::icon_path`). Data-derived, so it needs no guess.
    #[default]
    Icon,
    /// Pick the race/gender art from the model id instead. Cheaper and always
    /// available, but which variant belongs to which race is inference.
    Race,
    /// Draw the backdrop plate only.
    None,
}

/// Party-surface knobs. Every one of these is a **stated deviation** from the
/// original — see each field.
#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct PartySettings {
    /// Interpolate member HP/MP bars from the vitals of members who happen to
    /// be spawned nearby, instead of the wire's own 10 % steps.
    ///
    /// Off by default because the coarseness is the original's: the party wire
    /// packs both bars into one byte's nibbles, so a member's HP is only ever
    /// known to the nearest 10 % and the roster gauges genuinely have 11
    /// levels. Smoothing looks better and is strictly more information than the
    /// party packet carries, which is why it is opt-in rather than the default.
    pub smooth_vitals: bool,
    /// Show each member's two mastery icons in the roster and the quick board.
    ///
    /// The original shows masteries **nowhere** in either surface — there is no
    /// mastery control in any of the 70 blocks of the party trees
    /// (`docs/re/ui/hud-party-window.md` §3i); only the match-board's
    /// join-request dialog draws them. The ids are on the wire regardless
    /// (0x3065's records carry both), so this is an addition, not an invention.
    pub show_masteries: bool,
    /// Which portrait source to use. See [`PartyPortraitSource`].
    pub portrait_source: PartyPortraitSource,
    /// `AARRGGBB` tint for the match-board row of the party you are in.
    ///
    /// Ours, not the original's: no vanilla source names a colour for "your own
    /// entry", and the board has no such concept in its data at all. Parsed
    /// with the shared [`parse_argb`](crate::plugins::config::chat::parse_argb)
    /// like every other configurable colour here; a malformed value falls back
    /// to the default with a warning rather than failing the config.
    pub own_party_color: String,
}

impl Default for PartySettings {
    fn default() -> Self {
        Self {
            smooth_vitals: false,
            // On by default: it is what was asked for, and the data is free.
            show_masteries: true,
            portrait_source: PartyPortraitSource::default(),
            own_party_color: "FF7FB2FF".into(),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct HudSettings {
    /// Lay the skill window's board out from skilldata's native grid columns
    /// (57-60: UI_SkillTab/Page/Column/Row — the exact vanilla placement).
    /// Off falls back to the derived layout (skillgroup.txt rows + mastery
    /// levels), kept as the escape hatch while the native grid soaks.
    pub skill_window_native_grid: bool,
    /// Vitals percentage at or below which the mini-info's animated low-HP/MP
    /// warning overlays (`ifplayerminiinfo.txt` ids 77/78) light up. The
    /// original ships the whole animation in data but no threshold
    /// (`docs/re/ui/hud-player-mini-info.md` §9), so this is a knob, not a
    /// constant; `0` (the default) keeps the overlays off entirely.
    pub low_vitals_caution_percent: u8,
    /// Seconds the zone-entry region banner stays up. The original's dwell
    /// time is code-side and unknown (`docs/re/ui/region-banner.md` §9), so it
    /// is a knob rather than a baked constant; `0` disables the banner.
    pub region_banner_seconds: f32,
    /// Seconds a `CIFNotify` toast stays up. The corpus has no timer field of
    /// any kind (`docs/re/ui/notify-toast-widgets.md` §3.2), so the dwell is
    /// code-side in the original too; `0` disables toasts. 4s reads roughly a
    /// 12-word line at 200 wpm.
    pub toast_seconds: f32,
    /// The uniform multiplier applied to every HUD surface's transcribed
    /// geometry. **Not** a value from the original: vanilla authors its rects
    /// in a fixed 1:1 window space sized for 1024x768-era screens, so a
    /// verbatim transcription is unreadably small on a modern display
    /// (`plugins::hud::scale`). 1.5 is what the whole HUD was built against;
    /// raise it on a HiDPI screen, set it to 1.0 for the original's own pixel
    /// sizes. Must be finite and > 0 or it is refused with a warning.
    pub hud_scale: f32,
    /// Restore the original's warning tint. The original gives
    /// `GDR_WARNING_WND` the byte-identical `Color` of `GDR_UPDATE_QUEST_INFO`
    /// (`255,119,119,251`), i.e. no severity signal at all; we default to a
    /// distinct amber for accessibility.
    pub toast_original_warning_color: bool,
    /// Hide the mini-info's stat-up (`+`) button entirely while there is
    /// nothing to spend, instead of drawing it greyed.
    ///
    /// **A stated deviation, and the default.** The original draws it always:
    /// `GDR_PMI_BTN_STATUP` carries `Style=0` while all fifteen
    /// conditionally-drawn decorations of that tree carry `Style=64`
    /// (`docs/re/ui/hud-player-mini-info.md` §3), and it ships a `_disable`
    /// art variant for exactly this state. We hide it because a permanently
    /// greyed button is a control that is almost never actionable — the wallet
    /// is empty for every minute between level-ups — and a `+` that appears
    /// only when there is something to spend is itself the notification.
    /// Set `false` for the original's greyed button.
    ///
    /// This does **not** touch the character window's own `+STR`/`+INT`
    /// buttons: that window exists to spend points, and hiding them there
    /// would remove the affordance rather than declutter it.
    pub hide_statup_when_empty: bool,
    /// Milliseconds per frame of the animated item-icon overlays
    /// (`icon/item/etc/icon_edge_*.ddj` — the sweep on Seal-grade and Nasrun
    /// items, and the summoned-COS marker).
    ///
    /// A knob rather than a constant because the archive does not fix it: the
    /// sheets are real data and their grids are measured, but **no resinfo
    /// control references them**, so the original's cadence lives in its code
    /// and is unrecoverable. Lower is faster; `0` freezes on frame 0.
    pub icon_effect_frame_ms: f32,
    /// **Fallback** length for the cast/delay gauge, in seconds.
    ///
    /// No longer the source of truth: an item's cast time is in its own itemdata
    /// row (column 119, milliseconds — `[V]`, see
    /// `crate::plugins::hud::cast_gauge`), so this only covers a cast we cannot
    /// attribute to an item at all. It stays at the shortest measured return,
    /// because a bar that finishes early and waits beats one still crawling when
    /// the action fires.
    pub cast_gauge_seconds: f32,
    /// Seconds the quickslot arm indicator (`ub_slot_arrow.ddj`) stays at full
    /// opacity after a slot is used, before it fades out and the slot disarms.
    ///
    /// Ours, not the original's: the ring is real vanilla art
    /// (`GDR_QS_INIDCTION`) but nothing in resinfo carries a dwell, so the
    /// original's timing is code-side and unrecovered. It used to be infinite
    /// here, which left the last-used slot framed for the rest of the session.
    /// `0` disarms as soon as the fade completes; a negative value is refused
    /// in favour of the default.
    ///
    /// Note this is not purely cosmetic: arming is also what a second mouse
    /// press casts from, so once it expires a click on that slot arms again
    /// rather than firing. Ring and behaviour deliberately expire together.
    pub quickslot_arm_hold_seconds: f32,
    /// Seconds the arm indicator takes to fade out once
    /// [`Self::quickslot_arm_hold_seconds`] has elapsed. `0` makes it vanish
    /// instantly at the end of the hold.
    pub quickslot_arm_fade_seconds: f32,
    /// Party-surface knobs. See [`PartySettings`].
    pub party: PartySettings,
}

impl Default for HudSettings {
    fn default() -> Self {
        Self {
            skill_window_native_grid: true,
            low_vitals_caution_percent: 0,
            region_banner_seconds: 4.0,
            toast_seconds: 4.0,
            hud_scale: crate::plugins::hud::scale::DEFAULT_HUD_SCALE,
            toast_original_warning_color: false,
            hide_statup_when_empty: true,
            icon_effect_frame_ms: crate::plugins::hud::inventory::ui::DEFAULT_ICON_EFFECT_FRAME_MS,
            cast_gauge_seconds: crate::plugins::hud::cast_gauge::CAST_GAUGE_SECONDS,
            quickslot_arm_hold_seconds: 2.5,
            quickslot_arm_fade_seconds: 0.5,
            party: PartySettings::default(),
        }
    }
}
