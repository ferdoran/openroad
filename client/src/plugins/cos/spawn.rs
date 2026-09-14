//! Shared COS entity builder, used by the network spawn consumer
//! (`game_scene::spawn_remote_entity`) and the offline dev spawner — one
//! component recipe so a locally spawned horse behaves exactly like a
//! server-spawned one.

use bevy::prelude::*;

use crate::plugins::dynamic_resource_loader::{MirroredResource, UnloadedResource};
use crate::plugins::net::character_info::MovementSpeed;
use crate::plugins::net::entities::{
    CharacterRef, DisplayName, EntityVitals, NeedsGroundSnap, NetworkId, RemoteEntity,
    RemoteMovement,
};
use crate::plugins::textdata::{ClientCharacterData, ClientTextNames, ClientUiStrings};
use crate::util::mesh::{mirrored, needs_winding_reversal};
use packets::agent::pet::CosKind;

use super::{CosEntity, CosOwner};

/// Vanilla's placeholder for a pet nobody has named yet — "No name" in the
/// user's `textuisystem.txt`.
///
/// It is the runtime placeholder, not merely a window default: the info page
/// defaults its name plate to it (`docs/re/systems/pet-growth-cos.md` §4), and
/// the string is referenced from `FUN_008aa340`
/// (`docs/re/net/inbound/pet-cos.md` §0x30C9) — the handler that also processes
/// renames.
pub const COS_NO_NAME_KEY: &str = "UIIT_STT_COSNEWUI_TITLE";

/// What to call a pet: the name its owner gave it, else the localized
/// "No name".
///
/// **Empty is the same as absent.** The wire sends a zero-length string for an
/// unnamed pet rather than omitting the field — `packet_dump/0x30c8.log`'s Grey
/// Wolf has `0000` where the name length goes — so an `Option` alone does not
/// answer "is it named?", and every caller that assumed it did rendered a blank.
pub fn pet_display_name(given: Option<&str>, ui_strings: &ClientUiStrings) -> String {
    given
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| ui_strings.get_or(COS_NO_NAME_KEY, "No name"))
        .to_string()
}

/// Everything the builder needs beyond the loaded tables.
pub struct CosSpawnParams {
    pub ref_id: u32,
    pub unique_id: u32,
    pub kind: CosKind,
    /// Owner-given pet name (attack/pick pets); mounts use the localized
    /// characterdata name.
    pub pet_name: Option<String>,
    /// The summoner's character name, which the spawn record carries for every
    /// non-horse COS. Drawn under the pet's own name on its nameplate.
    pub owner_name: Option<String>,
    pub owner_uid: Option<u32>,
    pub transform: Transform,
    pub movement: RemoteMovement,
    pub speed: MovementSpeed,
}

/// Spawn a COS as a remote entity (`RemoteEntity::Npc` + [`CosEntity`]).
/// Returns `None` when the ref has no characterdata row.
pub fn spawn_cos_entity(
    commands: &mut Commands,
    asset_server: &AssetServer,
    char_data: &ClientCharacterData,
    names: Option<&ClientTextNames>,
    ui_strings: Option<&ClientUiStrings>,
    params: CosSpawnParams,
) -> Option<Entity> {
    let row = char_data.get(&(params.ref_id as i32))?;
    // A pet is a *named* thing, so an unnamed one reads "No name" rather than
    // falling back to its species. The kinds that cannot be named carry no name
    // field on the wire at all (`FUN_009a1900` gates it on `kind in {2,3}`), so
    // they keep the localized model name.
    let display = match (params.kind.is_pet(), ui_strings) {
        (true, Some(ui_strings)) => pet_display_name(params.pet_name.as_deref(), ui_strings),
        _ => params
            .pet_name
            .clone()
            .filter(|n| !n.is_empty())
            .or_else(|| {
                names
                    .and_then(|names| row.name_key().and_then(|key| names.name(key)))
                    .map(str::to_string)
            })
            .unwrap_or_else(|| row.code_name().clone()),
    };
    // A mount or pet is an SRO character resource like any other, so it carries
    // the same LH -> RH placement mirror the rest of the world does
    // (`util::mesh`). Producers hand this in as a plain translation, and
    // applying it here rather than at each of them keeps the one convention in
    // one place. `riding.rs` already documents the body wrapper as mirrored —
    // this is what finally makes that true.
    let transform = mirrored(params.transform);
    let mut cmd = commands.spawn((
        Name::from(format!("cos {}", params.unique_id)),
        DisplayName(display),
        RemoteEntity::Npc,
        CosEntity {
            kind: params.kind,
            owner_uid: params.owner_uid,
        },
        NetworkId(params.unique_id),
        CharacterRef(params.ref_id),
        params.movement,
        params.speed,
        NeedsGroundSnap,
        transform,
        Visibility::default(),
    ));
    if needs_winding_reversal(&transform.to_matrix()) {
        cmd.insert(MirroredResource);
    }
    // The summoner's name, drawn as the plate's sub-line. `[S]` that vanilla
    // draws it — the evidence is that the spawn record carries it for every
    // non-horse COS and nothing else consumes it.
    if let Some(owner) = params.owner_name.filter(|n| !n.trim().is_empty()) {
        cmd.insert(CosOwner(owner));
    }
    // Live HP bar denominator until the 0x30C8 PetData arrives (pick pets have
    // no HP at all — their characterdata max is still a sane placeholder).
    if let Some(max_hp) = row.max_hp() {
        cmd.insert(EntityVitals::full(max_hp));
    }
    // Growth-ladder rows reuse their base model via OrgObjCode (same chase as
    // monsters' summon clones).
    if let Some(path) = char_data.model_path(row) {
        cmd.insert(UnloadedResource(asset_server.load(path)));
        if row.resource_path().is_none() {
            cmd.insert(crate::commands::MaterialVariant::Clone);
        }
    }
    Some(cmd.id())
}

#[cfg(test)]
mod test {
    use super::*;

    /// The case the capture actually produces. `packet_dump/0x30c8.log`'s Grey
    /// Wolf sends `0000` where the name length goes, so the name arrives as
    /// `Some("")` — an `Option` alone cannot tell that apart from a real name,
    /// which is why every consumer used to render a blank.
    #[test]
    fn an_empty_wire_name_is_the_same_as_no_name() {
        let ui_strings = ClientUiStrings::default();
        // No table loaded in a unit test, so `get_or` yields the fallback —
        // the live string is "No name" (`textuisystem.txt`, checked against the
        // user's Media).
        assert_eq!(pet_display_name(None, &ui_strings), "No name");
        assert_eq!(pet_display_name(Some(""), &ui_strings), "No name");
        assert_eq!(pet_display_name(Some("   "), &ui_strings), "No name");
    }

    /// A name the owner actually gave survives untouched, trimmed.
    #[test]
    fn a_given_name_wins() {
        let ui_strings = ClientUiStrings::default();
        assert_eq!(pet_display_name(Some("Rex"), &ui_strings), "Rex");
        assert_eq!(pet_display_name(Some("  Rex  "), &ui_strings), "Rex");
    }
}
