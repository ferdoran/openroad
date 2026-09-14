//! Idea: an offline preview of the in-game minimap with a mocked player
//! position (Jangan) and a handful of fake entity dots, so the window layout,
//! tile grid orientation, arrow rotation and zoom buttons can be iterated
//! without a live server. Run with `SCENE=ui_testing`. The minimap's own
//! Update systems are already gated to also run in `SceneState::UiTesting`;
//! the local `Player` entity comes from the mini-info preview, so this only
//! anchors the world origin and spawns the mock dot entities.

use bevy::prelude::*;

use crate::plugins::hud::minimap::spawn_minimap;
use crate::plugins::net::entities::{RemoteEntity, UniqueMonster};
use crate::plugins::world_origin::WorldOrigin;
use crate::scenes::SceneState;

pub struct MinimapUiPreviewPlugin;

impl Plugin for MinimapUiPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            OnEnter(SceneState::UiTesting),
            (spawn_minimap, spawn_mock_map_data).chain(),
        );
    }
}

/// Anchor the (identity-transform) preview player in Jangan, region 169x96:
/// render (0,0,0) then reads back as server-global (169*1920, 96*1920), so
/// the 168..170 x 95..97 tiles load and the readout shows ~(6528, 768).
/// Render -X is server east (the mirrored X), so the monster pack sits east
/// of the player, the NPCs west, and one unique north.
fn spawn_mock_map_data(mut commands: Commands, mut origin: ResMut<WorldOrigin>) {
    *origin = WorldOrigin(Vec3::new(-(169.0 * 1920.0), 0.0, 96.0 * 1920.0));

    let mock = [
        (RemoteEntity::Monster, Vec3::new(-150.0, 0.0, 60.0), false),
        (RemoteEntity::Monster, Vec3::new(-220.0, 0.0, 120.0), false),
        (RemoteEntity::Monster, Vec3::new(-180.0, 0.0, -80.0), false),
        (RemoteEntity::Monster, Vec3::new(30.0, 0.0, 300.0), true),
        (RemoteEntity::Npc, Vec3::new(120.0, 0.0, -50.0), false),
        (RemoteEntity::Npc, Vec3::new(90.0, 0.0, 40.0), false),
        (RemoteEntity::Player, Vec3::new(-60.0, 0.0, -200.0), false),
        (RemoteEntity::Player, Vec3::new(250.0, 0.0, 180.0), false),
    ];
    for (kind, position, unique) in mock {
        let mut cmd = commands.spawn((
            Name::from("minimap preview dot"),
            kind,
            Transform::from_translation(position),
            Visibility::default(),
        ));
        if unique {
            cmd.insert(UniqueMonster);
        }
    }
}
