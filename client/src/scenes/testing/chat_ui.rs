//! Idea: an offline preview of the chat window with a seeded mock history
//! covering every line kind (colors), enough lines to exercise scrolling and
//! a few whispers for the whisper-partner panel. Run with `SCENE=ui_testing`.
//! The chat's own Update systems are already gated to also run in
//! `SceneState::UiTesting`, so this only spawns the window and fills the ring.

use bevy::prelude::*;

use crate::plugins::hud::chat::model::{ChatHistory, ChatLine, ChatLineKind};
use crate::plugins::hud::chat::ui::spawn_chat_window;
use crate::scenes::SceneState;

pub struct ChatUiPreviewPlugin;

impl Plugin for ChatUiPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            OnEnter(SceneState::UiTesting),
            (spawn_chat_window, seed_mock_history).chain(),
        );
    }
}

fn seed_mock_history(mut history: ResMut<ChatHistory>) {
    let named = |kind, sender: &str, text: &str| ChatLine {
        kind,
        sender: Some(sender.to_string()),
        text: text.to_string(),
    };
    let seed = [
        ChatLine {
            kind: ChatLineKind::Notice,
            sender: None,
            text: "Welcome to Terminus! Have Fun, florian0".to_string(),
        },
        named(ChatLineKind::All, "florian0", "The coloring"),
        named(
            ChatLineKind::AllGm,
            "GM_Hera",
            "Server maintenance at midnight.",
        ),
        named(ChatLineKind::Npc, "Storage Keeper", "Welcome, traveler."),
        named(ChatLineKind::Global, "TraderJoe", "WTS +7 spear, pm me!"),
        named(ChatLineKind::Stall, "ShopGirl", "Cheap elixirs here"),
        named(ChatLineKind::Party, "Aellia", "pull the next pack"),
        named(ChatLineKind::Guild, "Baruk", "guild war tonight?"),
        named(ChatLineKind::Union, "Ceres", "union meeting at Jangan gate"),
        named(ChatLineKind::Academy, "Dorin", "any tips for leveling?"),
        named(ChatLineKind::WhisperFrom, "Elyra", "got a sec?"),
        named(ChatLineKind::WhisperTo, "Elyra", "sure, what's up"),
        named(ChatLineKind::WhisperFrom, "Fenn", "trade later?"),
        ChatLine::system("You are restricted from chatting for 10 seconds."),
    ];
    for line in seed {
        history.push(line);
    }
    // bulk lines so the list overflows and the scrollbar has something to do
    for i in 0..40 {
        history.push(named(
            ChatLineKind::All,
            "florian0",
            &format!("scroll filler line {i} — the quick brown fox jumps over the lazy dog"),
        ));
        if i % 5 == 0 {
            history.push(named(
                ChatLineKind::Party,
                "Aellia",
                &format!("party filler {i}"),
            ));
        }
    }
}
