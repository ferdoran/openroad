//! `CIFNotify` toasts — the transient centred banners drawn over the world
//! (`docs/re/ui/notify-toast-widgets.md`).
//!
//! Idea: the original ships **three** `CIFNotify` instances that differ only by
//! id and tint (`ginterface.txt:143` `GDR_UPDATE_QUEST_INFO` id 42, `:201`
//! `GDR_WARNING_WND` id 35, `:740` `GDR_NOTICE` id 20). A descriptor cannot say
//! "same widget, different severity", so the data repeats the widget; here it is
//! one [`ToastKind`] enum and two three-arm matches.
//!
//! What the data actually gives us is almost nothing, and that is the finding:
//! all three carry `Rect="0,0,0,0"`, `DDJ=""`, `Text=""`, `Style=0`, `HAlign=1`
//! and white `FontColor`, so there is **no authored geometry and no art** — a
//! toast sizes to its string at runtime. The single body block
//! (`ifnotify.txt:6 GDR_NOTIFY_TXT:CIFStatic`) is a bare `Rect="0,0,600,60"`
//! text box with no background. The only per-instance geometry in the whole
//! family is `GDR_NOTICE`'s `ClientRect="0,6,0,6"` — 6 px of top/bottom padding.
//!
//! Consequences we keep honest:
//!
//! * Quest-update and warning share the byte-identical colour `255,119,119,251`;
//!   only the notice differs (`255,236,174,252`). The data does **not** encode
//!   severity, so a red warning would be invention. Our distinct warning tint is
//!   a stated deviation behind [`HudSettings::toast_original_warning_color`].
//! * There is no timer field anywhere in the corpus — the dwell time is
//!   code-side in the original too, so it is a config knob
//!   (`hud.toast_seconds`), not a baked constant.
//! * Lane fact 8 (resinfo stores `\n` as a literal two-character escape) does
//!   **not** apply here: `Text=""` on all three blocks, so our toast strings come
//!   from the wire or from code, never from a resinfo string.

use bevy::prelude::*;
use bevy::ui::UiTargetCamera;

use crate::assets::FontAssets;
use crate::plugins::config::ClientConfig;

/// `GDR_NOTIFY_TXT` `Rect="0,0,600,60"` (`ifnotify.txt:6`) — used as the text
/// box's *maximum* width and *minimum* height: the widget auto-sizes (all three
/// parents are `Rect=0,0,0,0`), so a long or multi-line string grows downwards
/// instead of being clipped. Whether the original clips or grows is UNKNOWN
/// (§9-U4).
const BODY_MAX_WIDTH: f32 = 600.0;
const BODY_MIN_HEIGHT: f32 = 60.0;

/// `FontIndex=0` on every block. The only font-index legend in the data
/// (`server_dep/silkroad/event/event_interface.txt:2`) reads index 0 as "9",
/// taken as a point size — [S], the resinfo `FontIndex` → face mapping is a
/// lane-wide UNKNOWN.
const FONT_SIZE: f32 = 9.0;

/// `GDR_NOTICE`'s `ClientRect="0,6,0,6"` — the family's only authored geometry.
const NOTICE_PADDING: f32 = 6.0;

/// Where the stack hangs. **openroad choice**: the three parents are
/// `Rect=0,0,0,0`, so the original's screen anchor is not in the data. Below the
/// region banner (`region_banner::BANNER_TOP` 96 + its 104px plate) so a zone
/// change and a toast do not overlap.
const STACK_TOP: f32 = 208.0;

/// Vertical gap between stacked toasts. **openroad choice** — the original
/// declares no stack at all.
const STACK_GAP: f32 = 4.0;

/// Which of the three `CIFNotify` instances a toast is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    /// `GDR_UPDATE_QUEST_INFO`, id 42.
    QuestUpdate,
    /// `GDR_WARNING_WND`, id 35.
    Warning,
    /// `GDR_NOTICE`, id 20.
    Notice,
}

/// The original's `Color=` on `GDR_UPDATE_QUEST_INFO` and `GDR_WARNING_WND`
/// (ARGB `255,119,119,251`) — the same value on both.
const ORIGINAL_TINT: Color = Color::srgb_u8(119, 119, 251);
/// `GDR_NOTICE`'s `Color=` (ARGB `255,236,174,252`).
const NOTICE_TINT: Color = Color::srgb_u8(236, 174, 252);
/// **Stated deviation** — the original gives warnings the *identical* tint as a
/// quest update, i.e. severity carried by nothing. This amber is ours, for
/// accessibility; `hud.toast_original_warning_color: true` restores
/// [`ORIGINAL_TINT`].
const WARNING_TINT_DEVIATION: Color = Color::srgb_u8(255, 176, 64);

impl ToastKind {
    /// The original `ID=` of the instance this kind stands for.
    pub fn original_id(self) -> u16 {
        match self {
            ToastKind::QuestUpdate => 42,
            ToastKind::Warning => 35,
            ToastKind::Notice => 20,
        }
    }

    /// The instance's `Color=`. `original_warning` restores the original's
    /// (identical to the quest-update) warning tint.
    pub fn tint(self, original_warning: bool) -> Color {
        match self {
            ToastKind::QuestUpdate => ORIGINAL_TINT,
            ToastKind::Warning if original_warning => ORIGINAL_TINT,
            ToastKind::Warning => WARNING_TINT_DEVIATION,
            ToastKind::Notice => NOTICE_TINT,
        }
    }

    /// Vertical `ClientRect` padding: 6 px on `GDR_NOTICE`, zero on the other
    /// two. Why only the notice has it is UNKNOWN (§9-U5).
    pub fn vertical_padding(self) -> f32 {
        match self {
            ToastKind::Notice => NOTICE_PADDING,
            _ => 0.0,
        }
    }
}

/// Show a toast. Any plugin can write this; one system spawns and expires them.
#[derive(Message, Clone, Debug)]
pub struct ShowToast {
    pub kind: ToastKind,
    pub text: String,
}

impl ShowToast {
    pub fn new(kind: ToastKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }
}

/// A live toast and its remaining dwell time in seconds.
#[derive(Component)]
pub struct Toast {
    pub kind: ToastKind,
    pub remaining: f32,
}

/// Spawn a node per [`ShowToast`]. The stack is laid out top-down in spawn
/// order; nothing here is data, see [`STACK_TOP`].
pub fn spawn_toasts(
    mut reader: MessageReader<ShowToast>,
    config: Res<ClientConfig>,
    fonts: Res<FontAssets>,
    cameras: Query<Entity, With<Camera2d>>,
    mut commands: Commands,
) {
    let dwell = config.hud.toast_seconds;
    if dwell <= 0.0 {
        reader.clear();
        return;
    }
    let original_warning = config.hud.toast_original_warning_color;
    let Some(camera) = cameras.iter().next() else {
        return;
    };

    for msg in reader.read() {
        if msg.text.is_empty() {
            continue;
        }
        // WCAG: the overlay is transient, so the log carries it too.
        info!("toast[{:?}]: {}", msg.kind, msg.text);
        let padding = msg.kind.vertical_padding();
        commands.spawn((
            Toast {
                kind: msg.kind,
                remaining: dwell,
            },
            Name::from("Toast"),
            Text(msg.text.clone()),
            TextFont {
                font: fonts.nine.clone().into(),
                font_size: FontSize::Px(FONT_SIZE),
                ..default()
            },
            // FontColor is white on all three blocks; the per-instance Color is
            // the only tint the widget has, and with no background art it can
            // only land on the text.
            TextColor(msg.kind.tint(original_warning)),
            // HAlign=1 — centred.
            TextLayout::justify(Justify::Center),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Px(STACK_TOP),
                margin: UiRect::left(Val::Px(-BODY_MAX_WIDTH / 2.0)),
                width: Val::Px(BODY_MAX_WIDTH),
                min_height: Val::Px(BODY_MIN_HEIGHT),
                padding: UiRect::vertical(Val::Px(padding)),
                ..default()
            },
            GlobalZIndex(70),
            Pickable::IGNORE,
            UiTargetCamera(camera),
        ));
    }
}

/// Tick the dwell timers, expire finished toasts, and keep the stack packed.
pub fn update_toasts(
    time: Res<Time>,
    mut toasts: Query<(Entity, &mut Toast, &mut Node)>,
    mut commands: Commands,
) {
    let delta = time.delta_secs();
    // Oldest first, so the stack order is stable as entries expire.
    let mut live: Vec<(Entity, f32)> = Vec::new();
    for (entity, mut toast, _) in toasts.iter_mut() {
        toast.remaining -= delta;
        if toast.remaining <= 0.0 {
            commands.entity(entity).despawn();
        } else {
            live.push((entity, toast.remaining));
        }
    }
    live.sort_by(|a, b| a.1.total_cmp(&b.1));
    for (row, (entity, _)) in live.iter().enumerate() {
        if let Ok((_, _, mut node)) = toasts.get_mut(*entity) {
            node.top = Val::Px(stack_offset(row));
        }
    }
}

/// Top offset of the `row`-th live toast.
fn stack_offset(row: usize) -> f32 {
    STACK_TOP + row as f32 * (BODY_MIN_HEIGHT + STACK_GAP)
}

pub fn cleanup_toasts(mut commands: Commands, toasts: Query<Entity, With<Toast>>) {
    for entity in toasts.iter() {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The three instances, verbatim from `ginterface.txt:143/201/740`.
    #[test]
    fn the_three_cifnotify_instances_are_transcribed() {
        assert_eq!(ToastKind::QuestUpdate.original_id(), 42);
        assert_eq!(ToastKind::Warning.original_id(), 35);
        assert_eq!(ToastKind::Notice.original_id(), 20);

        // Color= is ARGB: quest-update and warning are byte-identical, the
        // notice is not. Restoring the original warning tint must collapse the
        // first two onto one colour — that is the fidelity statement.
        assert_eq!(
            ToastKind::QuestUpdate.tint(true),
            Color::srgb_u8(119, 119, 251)
        );
        assert_eq!(
            ToastKind::Warning.tint(true),
            ToastKind::QuestUpdate.tint(true)
        );
        assert_eq!(ToastKind::Notice.tint(true), Color::srgb_u8(236, 174, 252));
        assert_ne!(
            ToastKind::Notice.tint(true),
            ToastKind::QuestUpdate.tint(true)
        );

        // ...and the deviation only ever moves the warning.
        assert_ne!(
            ToastKind::Warning.tint(false),
            ToastKind::Warning.tint(true)
        );
        assert_eq!(
            ToastKind::QuestUpdate.tint(false),
            ToastKind::QuestUpdate.tint(true)
        );
        assert_eq!(ToastKind::Notice.tint(false), ToastKind::Notice.tint(true));
    }

    /// `ClientRect="0,6,0,6"` on `GDR_NOTICE` and `"0,0,0,0"` on the other two
    /// is the family's only authored geometry — it must not leak onto the
    /// others, and the body box stays the `ifnotify.txt:6` 600x60.
    #[test]
    fn only_the_notice_carries_the_six_pixel_padding() {
        assert_eq!(ToastKind::Notice.vertical_padding(), 6.0);
        assert_eq!(ToastKind::QuestUpdate.vertical_padding(), 0.0);
        assert_eq!(ToastKind::Warning.vertical_padding(), 0.0);
        assert_eq!((BODY_MAX_WIDTH, BODY_MIN_HEIGHT), (600.0, 60.0));
    }

    /// The stack is an openroad choice, so pin only its invariants: the first
    /// row sits at the anchor and rows never overlap the 60px body.
    #[test]
    fn stacked_toasts_do_not_overlap() {
        assert_eq!(stack_offset(0), STACK_TOP);
        for row in 0..4 {
            assert!(stack_offset(row + 1) - stack_offset(row) >= BODY_MIN_HEIGHT);
        }
    }
}

/// Self-registration for the CIFNotify toasts (#474) (#558). The HUD registry holds one line per
/// window, so two windows landing in the same lap no longer collide on it.
pub struct ToastPlugin;

impl Plugin for ToastPlugin {
    fn build(&self, app: &mut App) {
        use crate::scenes::SceneState;

        app.add_message::<ShowToast>()
            .add_systems(OnExit(SceneState::GameWorld), cleanup_toasts)
            // CIFNotify toasts (#474): transient, no chrome, over the world —
            // the same shape as hitcount/magic_state_board.
            .add_systems(
                Update,
                (spawn_toasts, update_toasts)
                    .chain()
                    .run_if(super::hud_scenes),
            );
    }
}
