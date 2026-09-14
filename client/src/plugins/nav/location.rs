//! Which nav surface a mover is standing on.
//!
//! SRO's walkable world is two overlapping surfaces: the per-region terrain
//! height field of a `.nvm`, and the triangle nav meshes carried by map objects
//! inside their `.bms` files (bridge decks, stairs, building floors). They are
//! not joined by any link in the data — the original client tracks which one an
//! actor is on and resolves movement against *that* one alone.
//!
//! Doing the same is what makes bridges work. Terrain running under a bridge is
//! usually water or cliff, and its blocked edges would stop a mover mid-deck if
//! the terrain were consulted while standing on the object. Height has the
//! mirrored problem: a purely geometric "highest reachable surface" pick can
//! snap the mover back down to the ground.
//!
//! The state is deliberately just an entity, not an entity plus a cell index. A
//! cell index would have to be re-validated against a possibly-unloaded asset
//! every frame, whereas the entity's own liveness *is* the validity check: a
//! `Query::get` miss means the region streamed out and the location is stale.
//! See [`NavLocation::Unresolved`].

use bevy::prelude::*;

/// The nav surface a mover is currently on. Attached to anything that walks.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NavLocation {
    /// Not known yet, or no longer trustworthy: freshly spawned, teleported,
    /// nav data not streamed in, or the object we were standing on despawned.
    /// The next movement query re-resolves it geometrically.
    ///
    /// This is the only recovery path, and every stale case funnels into it —
    /// there is no separate invalidation bookkeeping.
    #[default]
    Unresolved,
    /// On the terrain height field.
    Terrain,
    /// On a map object's walkable nav mesh. `object` is the entity carrying
    /// [`ObjectNavMesh`](super::ObjectNavMesh) — a *resource* entity, so a
    /// compound map object (a bridge split across several `.cpd` parts) owns
    /// one of these per part and moving along it transfers between them.
    OnObject { object: Entity },
}

impl NavLocation {
    /// The object being stood on, if any.
    pub fn object(&self) -> Option<Entity> {
        match self {
            NavLocation::OnObject { object } => Some(*object),
            _ => None,
        }
    }

    pub fn is_resolved(&self) -> bool {
        !matches!(self, NavLocation::Unresolved)
    }
}

/// Outcome of a movement step ([`NavMeshRaycast::step`](super::NavMeshRaycast::step)).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NavStep {
    /// The step crossed an impassable edge. The mover must stop.
    ///
    /// Only ever returned for an actual blocked edge — never for missing data,
    /// which would wall movers in at streaming seams.
    Blocked,
    /// No nav data covering the destination is loaded yet. The caller should
    /// hold its current position and retry next frame.
    Unknown,
    Moved {
        position: Vec3,
        location: NavLocation,
    },
}
