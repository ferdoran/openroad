//! Player-facing game options: parse the original client's `SROptionSet.dat`
//! (`sroptionset`), model them semantically ([`options::GameOptions`]), and
//! persist openroad's own copy to a writable YAML file (`persistence`). The UI
//! that edits these lives in separate plugins.
//!
//! [`live`] documents (and pins) the one mechanism by which a changed setting
//! reaches its consumer without a restart, and carries the audit of every
//! `ClientConfig` group (#647).

pub mod keymap;
pub mod live;
pub mod options;
pub mod persistence;
pub mod sroptionset;
pub mod window_positions;

use bevy::prelude::*;

use options::GameOptions;

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameOptions>()
            .add_systems(Startup, persistence::load_user_settings)
            .add_systems(Update, persistence::save_on_change);
    }
}
