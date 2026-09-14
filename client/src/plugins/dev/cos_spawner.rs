//! Dev egui window to summon and ride COS vehicles (EP-19.1 test harness).
//!
//! Two paths, mirroring the feature's offline-first design: **Spawn locally**
//! builds the COS through the same `spawn_cos_entity` recipe the network path
//! uses (synthetic uid, characterdata speeds) so riding is testable without a
//! server; **Summon via scroll** drives the real wire flow — find a matching
//! summon scroll in the live inventory and use it (0x704C), optionally GM-
//! `/makeitem`-ing one first — and then the ordinary 0x30C8/spawn consumers
//! take over. Board/dismount/follow/unsummon buttons emit [`CosCommand`]s,
//! the same channel the HUD command bar uses.

use bevy::prelude::*;
use bevy_inspector_egui::bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use bevy_inspector_egui::egui;

use crate::plugins::gm::make_item_command;
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::plugins::config::ClientConfig;
use crate::plugins::cos::spawn::{spawn_cos_entity, CosSpawnParams};
use crate::plugins::cos::{ActiveCosList, CosCommand, CosStatus, RiderState};
use crate::plugins::hud::cos::state::{Cos, CosState, HGP_FULL};
use crate::plugins::hud::underbar::cast::UseItemRequest;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::character_info::MovementSpeed;
use crate::plugins::net::entities::RemoteMovement;
use crate::plugins::net::inventory::Inventory;
use crate::plugins::player::Player;
use crate::plugins::textdata::{ClientCharacterData, ClientItemData, ClientTextNames};
use crate::scenes::in_playable_world;
use packets::agent::pet::{CosBody, CosGrowth, CosKind, COS_HGP_FULL, COS_INVENTORY_PAGE_SLOTS};

use super::dev_windows_visible;

/// Locally allocated uids start here — far above anything a server assigns,
/// same convention as the skills scene's training dummies.
const LOCAL_UID_BASE: u32 = 0x8000_0000;

#[derive(Resource)]
struct CosSpawnerState {
    filter: String,
    picked: Option<i32>,
    next_uid: u32,
}

impl Default for CosSpawnerState {
    fn default() -> Self {
        Self {
            filter: String::new(),
            picked: None,
            next_uid: LOCAL_UID_BASE,
        }
    }
}

pub struct CosSpawnerPlugin;

impl Plugin for CosSpawnerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CosSpawnerState>().add_systems(
            EguiPrimaryContextPass,
            cos_spawner_window
                .run_if(in_playable_world)
                .run_if(dev_windows_visible),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn cos_spawner_window(
    mut contexts: EguiContexts,
    mut state: ResMut<CosSpawnerState>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    char_data: Option<Res<ClientCharacterData>>,
    item_data: Option<Res<ClientItemData>>,
    names: Option<Res<ClientTextNames>>,
    config: Option<Res<ClientConfig>>,
    mut list: ResMut<ActiveCosList>,
    mut cos_state: ResMut<CosState>,
    rider: Res<RiderState>,
    player: Query<&Transform, With<Player>>,
    inventories: Query<&Inventory, With<Player>>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    mut cos_commands: MessageWriter<CosCommand>,
    mut item_uses: MessageWriter<UseItemRequest>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let Some(char_data) = char_data.as_deref() else {
        return;
    };
    egui::Window::new("COS (mounts & pets)")
        .default_open(false)
        .show(ctx, |ui| {
            let Some(data) = char_data.data() else {
                ui.label("characterdata loading…");
                return;
            };

            // ── Picker over rideable COS rows ────────────────────────────
            ui.horizontal(|ui| {
                ui.label("Filter:");
                ui.text_edit_singleline(&mut state.filter);
            });
            let filter = state.filter.to_lowercase();
            // Every player-summonable COS kind, not just the rideable two:
            // the pet windows (bag page, AI switch, HP/HGP gauges) can only be
            // exercised offline if a growth/pick pet can be spawned here.
            // `Fellow` (tid4 5) is a guild guard, not the player's summon.
            let mut matches: Vec<(i32, &str)> = data
                .iter()
                .filter(|(_, row)| {
                    matches!(
                        row.cos_kind(),
                        Some(CosKind::Vehicle)
                            | Some(CosKind::Transport)
                            | Some(CosKind::GrowthPet)
                            | Some(CosKind::GrabPet)
                    )
                })
                .filter(|(_, row)| {
                    filter.is_empty() || row.code_name().to_lowercase().contains(&filter)
                })
                .map(|(id, row)| (*id, row.code_name().as_str()))
                .collect();
            matches.sort_unstable_by_key(|(id, _)| *id);
            matches.truncate(60);
            if state.picked.is_none() || !matches.iter().any(|(id, _)| Some(*id) == state.picked) {
                state.picked = matches.first().map(|(id, _)| *id);
            }
            let label_of = |id: i32| -> String {
                data.get(&id)
                    .map(|row| {
                        let name = names
                            .as_deref()
                            .and_then(|n| row.name_key().and_then(|key| n.name(key)))
                            .unwrap_or("?");
                        format!(
                            "{} ({}, run {})",
                            name,
                            row.code_name(),
                            row.run_speed().unwrap_or(0.0)
                        )
                    })
                    .unwrap_or_default()
            };
            let selected_label = state.picked.map(label_of).unwrap_or_else(|| "—".into());
            let mut picked = state.picked;
            egui::ComboBox::from_id_salt("cos_vehicle")
                .width(280.0)
                .selected_text(selected_label)
                .show_ui(ui, |ui| {
                    for (id, _) in &matches {
                        ui.selectable_value(&mut picked, Some(*id), label_of(*id));
                    }
                });
            state.picked = picked;

            // ── Offline: local spawn ─────────────────────────────────────
            if ui.button("Spawn locally").clicked() {
                if let (Some(ref_id), Ok(player_transform)) = (state.picked, player.single()) {
                    spawn_local_cos(
                        &mut commands,
                        &asset_server,
                        char_data,
                        names.as_deref(),
                        &mut state,
                        &mut list,
                        &mut cos_state,
                        ref_id,
                        player_transform,
                    );
                }
            }

            // ── Online: scroll summon ────────────────────────────────────
            let online = conn.single().is_ok();
            if online {
                ui.separator();
                let item_use_enabled = config
                    .as_deref()
                    .map(|c| c.network_settings.item_use_enabled)
                    .unwrap_or(false);
                let scroll = state
                    .picked
                    .and_then(|ref_id| find_summon_scroll(char_data, item_data.as_deref(), ref_id));
                match scroll {
                    Some((scroll_ref, in_inventory)) => {
                        ui.label(format!(
                            "Scroll ref {} {}",
                            scroll_ref,
                            if in_inventory(&inventories) {
                                "(in inventory)"
                            } else {
                                "(NOT in inventory)"
                            }
                        ));
                        if !item_use_enabled {
                            ui.colored_label(
                                egui::Color32::YELLOW,
                                "network_settings.item_use_enabled is off — 0x704C won't send",
                            );
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Use scroll (0x704C)").clicked() {
                                item_uses.write(UseItemRequest {
                                    ref_id: scroll_ref,
                                    slot: None,
                                    target_slot: None,
                                });
                            }
                            if ui.button("GM /makeitem scroll").clicked() {
                                send_gm_makeitem(&conn, item_data.as_deref(), scroll_ref);
                            }
                        });
                    }
                    None => {
                        ui.label("no summon scroll maps to this COS");
                    }
                }
            }

            // ── Active summons ───────────────────────────────────────────
            if !list.0.is_empty() {
                ui.separator();
                ui.heading("Active");
            }
            for status in list.0.clone() {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{} (uid {}{})",
                        status.name.as_deref().unwrap_or("unnamed"),
                        status.unique_id,
                        if status.local_only { ", local" } else { "" }
                    ));
                    let mounted_here = rider.0 == Some(status.unique_id);
                    if status.kind.is_rideable() {
                        if mounted_here {
                            if ui.button("Dismount").clicked() {
                                cos_commands.write(CosCommand::Dismount);
                            }
                        } else if ui.button("Board").clicked() {
                            cos_commands.write(CosCommand::Board(status.unique_id));
                        }
                    }
                    if ui.button("Follow").clicked() {
                        cos_commands.write(CosCommand::Follow(status.unique_id));
                    }
                    if ui.button("Unsummon").clicked() {
                        cos_commands.write(CosCommand::Unsummon(status.unique_id));
                    }
                });
            }
        });
}

/// Spawn the picked COS a few units in front of the player, through the same
/// recipe as the network path, and register it as a `local_only` summon.
#[allow(clippy::too_many_arguments)]
fn spawn_local_cos(
    commands: &mut Commands,
    asset_server: &AssetServer,
    char_data: &ClientCharacterData,
    names: Option<&ClientTextNames>,
    state: &mut CosSpawnerState,
    list: &mut ActiveCosList,
    cos_state: &mut CosState,
    ref_id: i32,
    player_transform: &Transform,
) {
    let Some(row) = char_data.get(&ref_id) else {
        return;
    };
    let Some(kind) = row.cos_kind() else { return };
    let uid = state.next_uid;
    state.next_uid += 1;
    // In front of the player, facing the same way.
    let forward = player_transform.rotation * -Vec3::Z;
    let transform = Transform::from_translation(
        player_transform.translation + forward.with_y(0.0).normalize_or_zero() * 15.0,
    )
    .with_rotation(player_transform.rotation);
    // Characterdata speeds (cols 46/47), used RAW: they are already in the
    // unit the server sends and the world measures (units/second), so a local
    // spawn moves at exactly the speed the same COS has online. Sane fallbacks
    // if the columns surprise us; the picker label shows the parsed value.
    let speed = MovementSpeed {
        walk: row.walk_speed().unwrap_or(24.0),
        run: row.run_speed().unwrap_or(48.0),
        hwan: row.run_speed().unwrap_or(48.0),
    };
    let movement = RemoteMovement {
        target: None,
        speed: speed.current(),
        walking: false,
    };
    let spawned = spawn_cos_entity(
        commands,
        asset_server,
        char_data,
        names,
        // The dev window spawns without a string table, so an unnamed pet falls
        // back to the model name here rather than to a localized "No name".
        None,
        CosSpawnParams {
            ref_id: ref_id as u32,
            unique_id: uid,
            kind,
            pet_name: None,
            owner_name: None,
            owner_uid: None,
            transform,
            movement,
            speed,
        },
    );
    if spawned.is_some() {
        info!(
            "cos spawner: spawned {} locally as uid {}",
            row.code_name(),
            uid
        );
        list.0.push(CosStatus {
            unique_id: uid,
            ref_id: ref_id as u32,
            kind,
            // Locally spawned COS are never named by anyone.
            name: None,
            hp: row.max_hp().unwrap_or(1),
            hp_max: row.max_hp().unwrap_or(1),
            local_only: true,
        });
        // A pet's windows read `CosState`, which the network path fills from
        // 0x30C8 — so a local pet has to stand in for that packet or the info,
        // bag and setup pages have nothing to draw. The body is the shape
        // 0x30C8 *would* have carried: a growth block for the attack pet, a
        // settings word and a bag for the pick pet.
        if kind.is_pet() {
            let growth = (kind == CosKind::GrowthPet).then(|| CosGrowth {
                exp: 0,
                level: row.level().unwrap_or(1) as u8,
                hgp: COS_HGP_FULL,
            });
            cos_state.cos.push(Cos {
                unique_id: uid,
                ref_obj_id: ref_id as u32,
                kind,
                body: CosBody {
                    hp: row.max_hp().unwrap_or(1),
                    unk_b: 0,
                    growth,
                    unk_f: Some(0),
                    // The zero-length string the wire sends for an unnamed pet
                    // — the case the "No name" fallback exists for.
                    name: Some(String::new()),
                    // The pick pet's capacity is normally the wire's byte;
                    // one full page is enough to exercise the grid offline.
                    inventory_size: if kind == CosKind::GrabPet {
                        COS_INVENTORY_PAGE_SLOTS
                    } else {
                        0
                    },
                    items: Vec::new(),
                    unk_g: None,
                    unk_h: None,
                },
                hgp: Some(HGP_FULL),
                exp: 0,
                level: growth.map(|g| g.level),
            });
        }
    }
}

/// The summon-scroll itemdata row for a COS characterdata ref: a `3/3/3/2`
/// item whose `Desc1_128` codename matches the COS row (laddered scrolls match
/// on the base-codename prefix). Returns the scroll ref id and an
/// in-inventory probe.
fn find_summon_scroll(
    char_data: &ClientCharacterData,
    item_data: Option<&ClientItemData>,
    cos_ref: i32,
) -> Option<(u32, impl Fn(&Query<&Inventory, With<Player>>) -> bool)> {
    let item_data = item_data?;
    let cos_code = char_data.get(&cos_ref)?.code_name().clone();
    let items = item_data.data()?;
    let scroll_ref = items
        .iter()
        .filter(|(_, row)| row.is_cos_summon_scroll())
        .find(|(_, row)| {
            row.cos_code_name().is_some_and(|code| {
                // Exact for plain scrolls; laddered scrolls carry the base
                // codename and the server picks the tier (COS_C_OSTRICH ->
                // COS_C_OSTRICH_20).
                code == cos_code || cos_code.starts_with(&format!("{code}_"))
            })
        })
        .map(|(id, _)| *id as u32)?;
    Some((
        scroll_ref,
        move |inventories: &Query<&Inventory, With<Player>>| {
            inventories
                .single()
                .is_ok_and(|inv| inv.slots.iter().flatten().any(|i| i.ref_id == scroll_ref))
        },
    ))
}

/// `/makeitem` for the picked COS's summon scroll (0x7010 sub-command 0x07).
///
/// Asks for one scroll and lets [`make_item_command`] apply the original's
/// clamp — scrolls are stackable, so the count must be at least 1. This used
/// to send `opt: 0` under sub-command 0x06, which is `LoadMonster`: a monster
/// spawn one byte short of its own layout, never a scroll.
fn send_gm_makeitem(
    conn: &Query<&SilkroadConnection, With<AgentConnection>>,
    item_data: Option<&ClientItemData>,
    ref_id: u32,
) {
    let Ok(conn) = conn.single() else { return };
    let request = match make_item_command(ref_id, 1, item_data) {
        Ok(request) => request,
        Err(reason) => {
            warn!("cos spawner: /makeitem {ref_id} refused — {reason}");
            return;
        }
    };
    info!("cos spawner: GM makeitem {}", ref_id);
    if let Err(e) = conn.get_sender().send(Packet::from(request).into()) {
        error!("cos spawner: failed to send makeitem: {}", e.0);
    }
}
