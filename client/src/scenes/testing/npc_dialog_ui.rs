//! Idea: an offline preview of the NPC dialog + store windows — a stand-in
//! Jangan blacksmith entity (real characterdata ref 2003, so npcchat,
//! textquest speech and the ref-shop chain all resolve against real data)
//! with the capture-verified talk options, opened straight into the dialog.
//! Clicking "Trade in the shop." exercises the real store window, tabs,
//! prices and the quantity modal (the buy send is a no-op offline — there is
//! no agent connection). Run with `SCENE=ui_testing`.

use bevy::prelude::*;

use crate::plugins::cursor::interactions::entity_select::SelectedEntity;
use crate::plugins::hud::npc_dialog::model::{DialogPage, NpcDialogState};
use crate::plugins::net::entities::{
    CharacterRef, DisplayName, NetworkId, NpcTalkOptions, RemoteEntity,
};
use crate::scenes::SceneState;

pub struct NpcDialogUiPreviewPlugin;

impl Plugin for NpcDialogUiPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(SceneState::UiTesting), seed_mock_npc);
    }
}

/// NPC_CH_SMITH (Blacksmith Chulsan), characterdata_5000.txt.
const SMITH_REF: u32 = 2003;

fn seed_mock_npc(
    mut commands: Commands,
    mut dialog: ResMut<NpcDialogState>,
    mut selected: ResMut<SelectedEntity>,
) {
    let npc = commands
        .spawn((
            Name::from("Preview Smith"),
            RemoteEntity::Npc,
            NetworkId(9999),
            CharacterRef(SMITH_REF),
            DisplayName("Blacksmith Chulsan".into()),
            // capture-verified option set: talk, store, storage, teleport.
            // Only debug-logged now — every dialog option is data-derived
            // (this preview NPC's codename decides what it offers).
            NpcTalkOptions(vec![0x01, 0x02, 0x04, 0x20]),
            Transform::default(),
        ))
        .id();
    // keep the session's deselect-guard happy in the preview
    selected.0 = Some(npc);
    *dialog = NpcDialogState::Open {
        npc,
        page: DialogPage::Options,
    };
}
