//! Offline preview of the generic two-option confirm
//! (`GDR_CHECK_CONFIRM_WND`, #579): raises one question in `SCENE=ui_testing`
//! and logs the answer, so the dialog has a live call site without inventing
//! a gameplay flow for it — the original's own call sites are UNKNOWN
//! (`docs/re/ui/registry-orphan-widgets.md` §7 U2).

use bevy::prelude::*;

use crate::plugins::ui_v2::choice_confirm::{
    ChoiceConfirmRequest, ChoiceConfirmState, ChoiceConfirmed,
};
use crate::scenes::SceneState;

pub struct ChoiceConfirmUiPreviewPlugin;

impl Plugin for ChoiceConfirmUiPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(SceneState::UiTesting), open_choice_confirm_preview)
            .add_systems(Update, log_choice.run_if(in_state(SceneState::UiTesting)));
    }
}

/// The preview's own tag; call sites use their own.
const PREVIEW_TAG: &str = "ui_testing_preview";

fn open_choice_confirm_preview(mut state: ResMut<ChoiceConfirmState>) {
    state.ask(ChoiceConfirmRequest {
        tag: PREVIEW_TAG,
        prompt: "Which option do you confirm?".into(),
        options: vec!["First option".into(), "Second option".into()],
    });
}

fn log_choice(mut answered: MessageReader<ChoiceConfirmed>) {
    for answer in answered.read() {
        info!(
            "choice confirm: `{}` answered with option {}",
            answer.tag, answer.option
        );
    }
}
