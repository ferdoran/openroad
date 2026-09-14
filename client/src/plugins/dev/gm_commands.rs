//! GM commands dev window — the 0x7010 family with searchable pickers.
//!
//! Idea: the GM commands are all `{u16 sub_id, u32 ref_id, …}` (see
//! `packets/src/agent/ingame.rs`), and the hard part of using them is never
//! the packet — it is knowing the ref id. `/makeitem` wants an itemdata
//! codename and `/loadmonster` a characterdata one, out of ~12k and ~14k rows,
//! so typing them from memory is the real barrier. This window is a filter box
//! and a dropdown over each table, which is the same shape the COS spawner
//! already uses for its own picker.
//!
//! The command *rules* are not reimplemented here. `make_item_command` and
//! `load_monster_command` (`plugins::gm`) carry the original's type gate and
//! clamps, and both the chat commands and these buttons go through them — so a
//! button cannot drift from what `/makeitem` does.
//!
//! Egui note: the systems must run in `EguiPrimaryContextPass`, not `Update` —
//! in `Update` the window renders but never receives input.

use bevy::prelude::*;
use bevy_inspector_egui::bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use bevy_inspector_egui::egui;

use packets::agent::prelude::GmCommand;
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::plugins::gm::{load_monster_command, make_item_command, zoe2_commands, GmState};
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::textdata::{ClientCharacterData, ClientItemData, ClientTextNames};
use crate::scenes::in_playable_world;

use super::dev_windows_visible;

/// itemdata `TypeID1` of the item family — the outer gate on the item picker,
/// so the scan drops the non-item rows before any string compare.
const ITEM_TYPE_ID1: u32 = 3;
/// itemdata `TypeID2` of the stackable (ETC) family. The `/makeitem` count
/// byte is a quantity for these and an **enchantment level** for everything
/// else in the item family, so the spinner's label follows this.
const ETC_TYPE_ID2: u32 = 3;

/// How many rows a dropdown shows. The tables are far larger; the filter box
/// is the way to reach past this, exactly as the COS picker works.
const MAX_ROWS: usize = 60;

#[derive(Resource, Default)]
struct GmCommandsState {
    item_filter: String,
    item_picked: Option<i32>,
    item_count: u8,
    monster_filter: String,
    monster_picked: Option<i32>,
    monster_count: u8,
}

pub struct GmCommandsPlugin;

impl Plugin for GmCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GmCommandsState>().add_systems(
            EguiPrimaryContextPass,
            gm_commands_window
                .run_if(in_playable_world)
                .run_if(dev_windows_visible),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn gm_commands_window(
    mut contexts: EguiContexts,
    mut state: ResMut<GmCommandsState>,
    item_data: Option<Res<ClientItemData>>,
    char_data: Option<Res<ClientCharacterData>>,
    names: Option<Res<ClientTextNames>>,
    gm: Res<GmState>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    // A spawn of zero monsters is never what the button means. The item
    // spinner is *not* floored the same way: on equipment its value is an
    // enchantment level, where +0 is a legitimate ask.
    if state.monster_count == 0 {
        state.monster_count = 1;
    }

    egui::Window::new("GM Commands")
        .default_open(false)
        .show(ctx, |ui| {
            if conn.single().is_err() {
                ui.label("offline — GM commands need an agent connection");
            }

            // ── /makeitem ────────────────────────────────────────────────
            ui.heading("Make item");
            ui.horizontal(|ui| {
                ui.label("Filter:");
                ui.text_edit_singleline(&mut state.item_filter);
            });
            match item_data.as_deref().and_then(|data| data.data()) {
                None => {
                    ui.label("itemdata loading…");
                }
                Some(items) => {
                    let filter = state.item_filter.to_lowercase();
                    let mut matches: Vec<(i32, &str)> = items
                        .iter()
                        // TypeID1 == 3 first: it is the cheap gate, and it is
                        // also `make_item_command`'s own precondition.
                        .filter(|(_, row)| matches!(row.type_ids(), Some((ITEM_TYPE_ID1, ..))))
                        .filter(|(_, row)| {
                            filter.is_empty() || row.code_name().to_lowercase().contains(&filter)
                        })
                        .map(|(id, row)| (*id, row.code_name().as_str()))
                        .collect();
                    matches.sort_unstable_by_key(|(id, _)| *id);
                    let total = matches.len();
                    matches.truncate(MAX_ROWS);
                    if state.item_picked.is_none()
                        || !matches.iter().any(|(id, _)| Some(*id) == state.item_picked)
                    {
                        state.item_picked = matches.first().map(|(id, _)| *id);
                    }
                    let label_of = |id: i32| -> String {
                        items
                            .get(&id)
                            .map(|row| {
                                let name = names
                                    .as_deref()
                                    .and_then(|n| row.name_key().and_then(|key| n.name(key)))
                                    .unwrap_or("?");
                                format!("{name} ({}) #{id}", row.code_name())
                            })
                            .unwrap_or_default()
                    };
                    // `label_of` borrows `items` while `state` is borrowed
                    // mutably by the filter box, so the pick round-trips
                    // through a local — the same dance the COS picker does.
                    let selected = state
                        .item_picked
                        .map(label_of)
                        .unwrap_or_else(|| "—".into());
                    let mut picked = state.item_picked;
                    egui::ComboBox::from_id_salt("gm_item")
                        .width(360.0)
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            for (id, _) in &matches {
                                ui.selectable_value(&mut picked, Some(*id), label_of(*id));
                            }
                        });
                    state.item_picked = picked;
                    if total > MAX_ROWS {
                        ui.small(format!(
                            "{total} matches, showing {MAX_ROWS} — narrow the filter"
                        ));
                    }

                    // The one byte means two things by item class, so the
                    // spinner says which: an enchantment level for equipment
                    // (where +0 is legal, hence the range floor of 0), a
                    // quantity for a stackable.
                    let stackable = state
                        .item_picked
                        .and_then(|id| items.get(&id))
                        .and_then(|row| row.type_ids())
                        .is_some_and(|(_, tid2, _, _)| tid2 == ETC_TYPE_ID2);
                    ui.horizontal(|ui| {
                        if stackable {
                            ui.label("Count:");
                            ui.add(egui::DragValue::new(&mut state.item_count).range(1..=255));
                        } else {
                            ui.label("Enchant:");
                            ui.add(
                                egui::DragValue::new(&mut state.item_count)
                                    .range(0..=255)
                                    .prefix("+"),
                            );
                        }
                        if ui.button("Make item").clicked() {
                            if let Some(ref_id) = state.item_picked {
                                match make_item_command(
                                    ref_id as u32,
                                    state.item_count,
                                    item_data.as_deref(),
                                ) {
                                    Ok(request) => send(&conn, request),
                                    Err(reason) => warn!("gm window: /makeitem — {reason}"),
                                }
                            }
                        }
                    });
                }
            }

            ui.separator();

            // ── /loadmonster and /zoe2 ───────────────────────────────────
            ui.heading("Spawn monster");
            ui.horizontal(|ui| {
                ui.label("Filter:");
                ui.text_edit_singleline(&mut state.monster_filter);
            });
            match char_data.as_deref().and_then(|data| data.data()) {
                None => {
                    ui.label("characterdata loading…");
                }
                Some(chars) => {
                    let filter = state.monster_filter.to_lowercase();
                    let mut matches: Vec<(i32, &str)> = chars
                        .iter()
                        .filter(|(_, row)| row.is_monster())
                        .filter(|(_, row)| {
                            filter.is_empty() || row.code_name().to_lowercase().contains(&filter)
                        })
                        .map(|(id, row)| (*id, row.code_name().as_str()))
                        .collect();
                    matches.sort_unstable_by_key(|(id, _)| *id);
                    let total = matches.len();
                    matches.truncate(MAX_ROWS);
                    if state.monster_picked.is_none()
                        || !matches
                            .iter()
                            .any(|(id, _)| Some(*id) == state.monster_picked)
                    {
                        state.monster_picked = matches.first().map(|(id, _)| *id);
                    }
                    let label_of = |id: i32| -> String {
                        chars
                            .get(&id)
                            .map(|row| {
                                let name = names
                                    .as_deref()
                                    .and_then(|n| row.name_key().and_then(|key| n.name(key)))
                                    .unwrap_or("?");
                                format!(
                                    "{name} (lv{}) {} #{id}",
                                    row.level().unwrap_or(0),
                                    row.code_name()
                                )
                            })
                            .unwrap_or_default()
                    };
                    let selected = state
                        .monster_picked
                        .map(label_of)
                        .unwrap_or_else(|| "—".into());
                    let mut picked = state.monster_picked;
                    egui::ComboBox::from_id_salt("gm_monster")
                        .width(360.0)
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            for (id, _) in &matches {
                                ui.selectable_value(&mut picked, Some(*id), label_of(*id));
                            }
                        });
                    state.monster_picked = picked;
                    if total > MAX_ROWS {
                        ui.small(format!(
                            "{total} matches, showing {MAX_ROWS} — narrow the filter"
                        ));
                    }

                    ui.horizontal(|ui| {
                        ui.label("Count:");
                        ui.add(egui::DragValue::new(&mut state.monster_count).range(1..=255));
                        if ui.button("Spawn").clicked() {
                            if let Some(ref_id) = state.monster_picked {
                                send(
                                    &conn,
                                    load_monster_command(ref_id as u32, state.monster_count, 0),
                                );
                            }
                        }
                        if ui
                            .button("Spawn & kill")
                            .on_hover_text("zoe2 — spawns the monsters and kills them")
                            .clicked()
                        {
                            if let Some(ref_id) = state.monster_picked {
                                // zoe2 batches: one packet per 200 monsters.
                                for request in
                                    zoe2_commands(ref_id as u32, state.monster_count as u16)
                                {
                                    send(&conn, request);
                                }
                            }
                        }
                    });
                }
            }

            ui.separator();

            // ── toggles ──────────────────────────────────────────────────
            ui.horizontal(|ui| {
                let invisible = if gm.invisible {
                    "Invisible: on"
                } else {
                    "Invisible: off"
                };
                if ui.button(invisible).clicked() {
                    send(&conn, GmCommand::Invisible);
                }
                if ui.button("Invincible").clicked() {
                    send(&conn, GmCommand::Invincible);
                }
            });
        });
}

fn send(conn: &Query<&SilkroadConnection, With<AgentConnection>>, request: GmCommand) {
    let Ok(conn) = conn.single() else {
        warn!("gm window: no agent connection");
        return;
    };
    info!("gm window: sending sub-command {:#06x}", request.code());
    if let Err(e) = conn.get_sender().send(Packet::from(request).into()) {
        error!("gm window: failed to send GM command: {}", e.0);
    }
}
