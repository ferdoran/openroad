//! "This player is running a stall" — the entity-level half of the stall
//! system (#782, EP-16.2).
//!
//! Idea: three server pushes describe a stall in the world without any window
//! being open — `EntityStallCreate` (0x30B8), `EntityStallTitleUpdate`
//! (0x30BB) and `EntityStallDestroy` (0x30B9). Each names an entity by its
//! unique id, so this module resolves it through [`NetworkEntities`] and
//! attaches, edits or removes one [`StallOwner`] component. Rendering is the
//! nameplate pipeline's job (`plugins::hud::nameplates`): the title becomes a
//! second line over the owner's head, exactly the way the guild tag already
//! does, rather than a second floating-text system.
//!
//! The decoration is **carried, not drawn.** `EntityStallCreate` ships a
//! `decoration_id: u32` and neither `docs/re/systems/stall.md` nor
//! `docs/net-stall-0x30B7.md` records what resource that id resolves to — no
//! table, no path, no census. ADR-0009 names guessing a model path as the
//! defect, so the id is kept on the component for whoever finds the table and
//! nothing is spawned for it.

use bevy::prelude::*;

use packets::agent::stall::{EntityStallCreate, EntityStallDestroy, EntityStallTitleUpdate};

use crate::plugins::net::entities::NetworkEntities;

/// Present on an entity that is running a player stall.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct StallOwner {
    /// The stall's title, as the server sent it. Shown over the owner.
    pub title: String,
    /// `EntityStallCreate::decoration_id` — the stall's avatar/model id.
    /// Deliberately unresolved: see the module note.
    pub decoration_id: u32,
}

/// 0x30B8 — a stall appeared on an entity.
fn on_stall_created(
    mut created: MessageReader<EntityStallCreate>,
    index: Res<NetworkEntities>,
    mut commands: Commands,
) {
    for stall in created.read() {
        let Some(entity) = index.get(stall.unique_id) else {
            continue;
        };
        commands.entity(entity).insert(StallOwner {
            title: stall.title.clone(),
            decoration_id: stall.decoration_id,
        });
    }
}

/// 0x30BB — the stall was renamed. Title edits arrive here and not on 0xB0BA
/// (`packets/src/agent/stall.rs:209-214`), so this is the only writer.
fn on_stall_title_updated(
    mut updates: MessageReader<EntityStallTitleUpdate>,
    index: Res<NetworkEntities>,
    mut owners: Query<&mut StallOwner>,
) {
    for update in updates.read() {
        let Some(entity) = index.get(update.unique_id) else {
            continue;
        };
        // A rename for an entity we never saw open a stall is dropped rather
        // than synthesised: without 0x30B8 we have no decoration id, and
        // inventing one would put a wrong value in the component.
        if let Ok(mut owner) = owners.get_mut(entity) {
            if owner.title != update.title {
                owner.title = update.title.clone();
            }
        }
    }
}

/// 0x30B9 — the stall is gone.
fn on_stall_destroyed(
    mut destroyed: MessageReader<EntityStallDestroy>,
    index: Res<NetworkEntities>,
    mut commands: Commands,
) {
    for stall in destroyed.read() {
        let Some(entity) = index.get(stall.unique_id) else {
            continue;
        };
        commands.entity(entity).remove::<StallOwner>();
    }
}

/// Self-registration. The three opcodes are already Bevy messages (the
/// `packets` opcode table registers every wired opcode), so this only adds the
/// systems.
pub struct StallEntitiesPlugin;

impl Plugin for StallEntitiesPlugin {
    fn build(&self, app: &mut App) {
        // Repeated here so the plugin stands alone in a unit test, exactly as
        // `NetworkEntitiesPlugin` does for its two.
        app.add_message::<EntityStallCreate>()
            .add_message::<EntityStallDestroy>()
            .add_message::<EntityStallTitleUpdate>()
            .add_systems(
                Update,
                (on_stall_created, on_stall_title_updated, on_stall_destroyed).chain(),
            );
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::plugins::net::entities::{NetworkEntitiesPlugin, NetworkId};

    fn app_with_entity(unique_id: u32) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((NetworkEntitiesPlugin, StallEntitiesPlugin));
        let entity = app.world_mut().spawn(NetworkId(unique_id)).id();
        (app, entity)
    }

    /// Open, rename, close — the whole life of a stall marker, through the
    /// unique-id index and nothing else.
    #[test]
    fn a_stall_is_attached_renamed_and_removed_by_unique_id() {
        let (mut app, entity) = app_with_entity(4242);

        app.world_mut().write_message(EntityStallCreate {
            unique_id: 4242,
            title: "Cheap elixirs".to_string(),
            decoration_id: 7,
        });
        app.update();
        let owner = app.world().get::<StallOwner>(entity).cloned();
        assert_eq!(
            owner,
            Some(StallOwner {
                title: "Cheap elixirs".to_string(),
                decoration_id: 7,
            })
        );

        app.world_mut().write_message(EntityStallTitleUpdate {
            unique_id: 4242,
            title: "Elixirs SOLD OUT".to_string(),
        });
        app.update();
        let owner = app.world().get::<StallOwner>(entity).unwrap();
        assert_eq!(owner.title, "Elixirs SOLD OUT");
        // the decoration id survives a rename
        assert_eq!(owner.decoration_id, 7);

        app.world_mut().write_message(EntityStallDestroy {
            unique_id: 4242,
            error_code: 0,
        });
        app.update();
        assert!(app.world().get::<StallOwner>(entity).is_none());
    }

    /// A push for an id we do not know must not panic and must not create an
    /// entity out of thin air — the index is the only source of truth.
    #[test]
    fn a_push_for_an_unknown_entity_is_dropped() {
        let (mut app, entity) = app_with_entity(1);

        app.world_mut().write_message(EntityStallCreate {
            unique_id: 9999,
            title: "ghost".to_string(),
            decoration_id: 0,
        });
        app.world_mut().write_message(EntityStallTitleUpdate {
            unique_id: 9999,
            title: "still a ghost".to_string(),
        });
        app.world_mut().write_message(EntityStallDestroy {
            unique_id: 9999,
            error_code: 0,
        });
        app.update();
        assert!(app.world().get::<StallOwner>(entity).is_none());
    }

    /// A rename with no preceding create is dropped rather than synthesised:
    /// 0x30BB carries no decoration id.
    #[test]
    fn a_rename_without_a_create_does_not_invent_a_stall() {
        let (mut app, entity) = app_with_entity(7);

        app.world_mut().write_message(EntityStallTitleUpdate {
            unique_id: 7,
            title: "no stall here".to_string(),
        });
        app.update();
        assert!(app.world().get::<StallOwner>(entity).is_none());
    }
}
