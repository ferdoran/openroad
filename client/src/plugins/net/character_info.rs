//! Components holding server-sent character data on in-world entities.
//!
//! Deliberately entity-agnostic: today only the local player receives them
//! (from CHARACTER_DATA, 0x3013), but remote players and NPCs share the same
//! wire blocks, so the same components will carry their data later.

use bevy::prelude::*;

use packets::agent::character_data::{
    CharacterStats, EntityState, InventoryItem, JobInfo, KnownSkill, Mastery, ParsedCharacterInfo,
    PlayerExtras,
};

/// The character record the server sent for this entity. Sections the parser
/// could not extract stay `None`/empty (see `packets::agent::character_data`).
#[derive(Component, Clone, Debug, Default)]
#[allow(dead_code)] // parsed-but-unconsumed SRO data; UI/gameplay reads land later
pub struct CharacterInfo {
    pub name: Option<String>,
    pub stats: Option<CharacterStats>,
    pub inventory_size: Option<u8>,
    pub inventory: Vec<InventoryItem>,
    pub avatar_items: Vec<InventoryItem>,
    pub masteries: Vec<Mastery>,
    pub skills: Vec<KnownSkill>,
    pub completed_quests: Vec<u32>,
    pub job: Option<JobInfo>,
    pub extras: Option<PlayerExtras>,
    /// The character's body state at login (invisibility/stealth/berserk), from
    /// the state block — `None` if the state block was absent.
    pub body_state: Option<u8>,
}

impl From<&ParsedCharacterInfo> for CharacterInfo {
    fn from(info: &ParsedCharacterInfo) -> Self {
        Self {
            name: info.name.clone(),
            stats: info.stats,
            inventory_size: info.inventory_size,
            inventory: info.inventory.clone().unwrap_or_default(),
            avatar_items: info.avatar_items.clone().unwrap_or_default(),
            masteries: info.masteries.clone().unwrap_or_default(),
            skills: info.skills.clone().unwrap_or_default(),
            completed_quests: info.completed_quests.clone().unwrap_or_default(),
            job: info.job.clone(),
            extras: info.extras.clone(),
            body_state: info.state.as_ref().map(|s| s.body_state),
        }
    }
}

/// Server-authoritative movement speeds (game units per second) of an
/// in-world character, from its spawn/state block or a speed update (0x30D0).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct MovementSpeed {
    pub walk: f32,
    pub run: f32,
    pub hwan: f32,
}

impl MovementSpeed {
    /// go-sro's defaults for a fresh character; used until the server tells us
    /// otherwise.
    pub const DEFAULT: Self = Self {
        walk: 16.0,
        run: 100.0,
        hwan: 100.0,
    };

    /// Speeds from a spawn/state block, falling back per-field to the defaults
    /// when the server sent zero or garbage.
    pub fn from_state(state: &EntityState) -> Self {
        let sane = |v: f32, fallback: f32| {
            if v.is_finite() && v > 0.0 {
                v
            } else {
                fallback
            }
        };
        Self {
            walk: sane(state.walk_speed, Self::DEFAULT.walk),
            run: sane(state.run_speed, Self::DEFAULT.run),
            hwan: sane(state.hwan_speed, Self::DEFAULT.hwan),
        }
    }

    /// Apply a speed update (0x30D0), keeping the old value where the server
    /// sent zero or garbage.
    pub fn apply_update(&mut self, walk: f32, run: f32) {
        let sane = |v: f32| (v.is_finite() && v > 0.0).then_some(v);
        if let Some(walk) = sane(walk) {
            self.walk = walk;
        }
        if let Some(run) = sane(run) {
            self.run = run;
        }
    }

    /// The speed movement currently happens at. SRO characters run by default;
    /// a walk/run toggle is a follow-up.
    pub fn current(&self) -> f32 {
        self.run
    }
}
