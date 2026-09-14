//! Persist [`GameOptions`] to a SEPARATE, writable YAML file.
//!
//! `config.yaml` stays an author-controlled, read-only input; the player's own
//! tweaks are the machine-written record in `<cwd>/user_settings.yaml`, so the
//! two never clobber each other. Loading is best-effort and never blocks boot:
//! a missing file is the normal first run, a corrupt one is logged and ignored.
//!
//! We never write the original client's `SROptionSet.dat`; [`import_sroptionset`]
//! only reads one in (dormant until the UI wires an import action).

use std::path::{Path, PathBuf};

use bevy::prelude::*;

use super::options::GameOptions;
use super::sroptionset::parse_sroptionset;

/// `<cwd>/user_settings.yaml` — the working-dir-relative path idiom the rest of
/// the client uses (e.g. the screenshot writer).
pub fn user_settings_path() -> PathBuf {
    let mut p = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    p.push("user_settings.yaml");
    p
}

/// Startup: overwrite the options resource from the saved file if present. A
/// missing file is normal (first run); a corrupt one is logged and ignored so
/// a bad edit can never block boot.
pub fn load_user_settings(mut options: ResMut<GameOptions>) {
    let path = user_settings_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    match serde_yaml::from_str::<GameOptions>(&text) {
        Ok(loaded) => {
            *options = loaded;
            info!("[settings] loaded user settings from {}", path.display());
        }
        Err(e) => warn!("[settings] ignoring unreadable {}: {e}", path.display()),
    }
}

/// Update: write the file whenever the options actually change. The first
/// observation only seeds the baseline (no write), so merely loading or
/// defaulting never creates / rewrites the file — only a real change does.
/// Writes are best-effort: a failure is logged, never fatal.
pub fn save_on_change(options: Res<GameOptions>, mut last: Local<Option<GameOptions>>) {
    match last.as_ref() {
        Some(prev) if *prev == *options => {}
        None => *last = Some(options.clone()),
        Some(_) => {
            let path = user_settings_path();
            match serde_yaml::to_string(&*options) {
                Ok(yaml) => {
                    if let Err(e) = std::fs::write(&path, yaml) {
                        warn!("[settings] failed to write {}: {e}", path.display());
                    } else {
                        info!("[settings] saved user settings to {}", path.display());
                    }
                }
                Err(e) => warn!("[settings] failed to serialize settings: {e}"),
            }
            *last = Some(options.clone());
        }
    }
}

/// Import-only: parse an original-client `SROptionSet.dat` into [`GameOptions`].
/// Never round-trips or overwrites the `.dat` — openroad persists to YAML only.
#[allow(dead_code)]
pub fn import_sroptionset(path: impl AsRef<Path>) -> std::io::Result<GameOptions> {
    let bytes = std::fs::read(path)?;
    Ok(GameOptions::from_records(
        &parse_sroptionset(&bytes).records,
    ))
}
