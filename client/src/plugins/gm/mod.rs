//! GM commands (typed in chat as `/…`) and their visible effects.
//!
//! Idea: a `/`-prefixed chat line is a command, not a message — the chat input
//! routes it here as a [`SlashCommand`] instead of sending it. `/invisible`
//! (and `/invincible`) map to the 0x7010 GM command; the server only honours
//! them for privileged accounts. Invisibility is authoritative from the
//! server: it arrives as a 0x30BF body-state update (and at login in the
//! character data), which we mirror onto [`GmState`] and render by dropping
//! the local character's alpha. A successful 0xB010 also optimistically flips
//! the toggle, so the effect shows even if the server doesn't echo 0x30BF to
//! the caller.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use packets::agent::prelude::{
    body_state_is_invisible, hidden_render, EntityStateUpdate, GmCommand, GmResponse, HiddenRender,
};
use packets::Packet;

use crate::assets::bmt::sheen::SroSheenMaterial;
use crate::net::connection::SilkroadConnection;
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::net::character_info::CharacterInfo;
use crate::plugins::net::entities::{NetworkEntities, NetworkId, RemoteBodyState, RemoteEntity};
use crate::plugins::player::Player;
use crate::plugins::textdata::{
    ClientCharacterData, ClientCharacterIndex, ClientItemData, ClientItemIndex,
};
use crate::scenes::SceneState;

/// Rendered alpha of an invisible character (a faint ghost, as in the
/// original client where GMs see their own invisible body translucent).
const GHOST_ALPHA: f32 = 0.3;

/// A `/`-prefixed chat line, forwarded from the chat input (leading `/`
/// stripped, trimmed).
#[derive(Message)]
pub struct SlashCommand(pub String);

/// The local player's GM-visible state.
#[derive(Resource, Default)]
pub struct GmState {
    /// Whether the local character is currently invisible.
    pub invisible: bool,
    /// Whether the local account is a game master, from `PlayerExtras.gm` in
    /// the login record. It decides whether another character's GM
    /// invisibility is shown as a ghost or not shown at all.
    pub is_gm: bool,
    /// An `/invisible` was sent and is awaiting its 0xB010 ack (to flip the
    /// toggle optimistically when the server doesn't echo a 0x30BF to us).
    pending_invisible: bool,
}

/// The original material handle a ghosted mesh had, for restore.
#[derive(Component)]
enum GhostOriginal {
    Standard(Handle<StandardMaterial>),
    Sheen(Handle<SroSheenMaterial>),
}

pub struct GmPlugin;

impl Plugin for GmPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GmState>()
            .add_message::<SlashCommand>()
            .add_systems(
                Update,
                (
                    dispatch_slash_commands,
                    on_gm_response,
                    invisibility_from_state_update,
                    invisibility_from_character_data,
                    remote_body_state_from_updates,
                    render_invisibility,
                    render_remote_invisibility,
                )
                    .run_if(in_state(SceneState::GameWorld)),
            );
    }
}

/// Route a slash command to its GM packet. Unknown commands only log.
fn dispatch_slash_commands(
    mut reader: MessageReader<SlashCommand>,
    conn: Query<&SilkroadConnection, With<AgentConnection>>,
    item_index: Option<Res<ClientItemIndex>>,
    item_data: Option<Res<ClientItemData>>,
    char_index: Option<Res<ClientCharacterIndex>>,
    char_data: Option<Res<ClientCharacterData>>,
    mut gm: ResMut<GmState>,
) {
    for SlashCommand(line) in reader.read() {
        // The original's command registry lowercases the typed name before
        // looking it up (`0x0053fd40`), so `/Makeitem` and `/makeitem` are the
        // same command. Arguments keep their case — codenames are matched
        // exactly against the textdata tables.
        let mut tokens = line.split_whitespace();
        let command = tokens.next().unwrap_or("").to_lowercase();
        let args: Vec<&str> = tokens.collect();
        // A command can be more than one packet: `/zoe2` batches (see
        // `zoe2_commands`), so the whole dispatch works in terms of a list.
        let requests = match command.as_str() {
            "invisible" => Ok(vec![GmCommand::Invisible]),
            "invincible" => Ok(vec![GmCommand::Invincible]),
            "makeitem" => parse_make_item(&args, item_index.as_deref(), item_data.as_deref())
                .map(|request| vec![request]),
            "loadmonster" => {
                parse_load_monster(&args, char_index.as_deref()).map(|request| vec![request])
            }
            "zoe" => parse_zoe(&args, false, char_index.as_deref(), char_data.as_deref()),
            "zoe2" => parse_zoe(&args, true, char_index.as_deref(), char_data.as_deref()),
            other => {
                warn!("gm: unknown command /{other}");
                continue;
            }
        };
        let requests = match requests {
            Ok(requests) => requests,
            Err(reason) => {
                warn!("gm: /{command} — {reason}");
                continue;
            }
        };
        let Ok(conn) = conn.single() else {
            warn!("gm: not sending /{command}, no agent connection");
            continue;
        };
        info!("gm: sending /{command} ({} packet(s))", requests.len());
        for request in requests {
            if let Err(e) = conn.get_sender().send(Packet::from(request.clone()).into()) {
                error!("network: failed to send GM command: {}", e.0);
                break;
            }
            if request == GmCommand::Invisible {
                gm.pending_invisible = true;
            }
        }
    }
}

/// itemdata `TypeID1` of the item family — the original's own gate on
/// `/makeitem`'s first argument (`0x005484xx`, `tid & 0x1C == 0x0C`).
const ITEM_TYPE_ID1: u32 = 3;
/// itemdata `TypeID2` of the stackable (ETC) family, which is the branch the
/// original clamps the count on (`0x00538410`, `tid & 0x60 == 0x60`).
const ETC_TYPE_ID2: u32 = 3;

/// Which table a command's first argument names, for the error message.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RefTable {
    Item,
    Character,
}

impl RefTable {
    /// What the argument names, for the error message.
    fn subject(self) -> &'static str {
        match self {
            RefTable::Item => "item",
            RefTable::Character => "monster",
        }
    }

    /// The textdata table it is looked up in.
    fn table(self) -> &'static str {
        match self {
            RefTable::Item => "itemdata",
            RefTable::Character => "characterdata",
        }
    }
}

/// Resolve a `/`-command's ref argument the way the original does: by
/// **codename**, against the loaded table.
///
/// One rule for both tables — items for `/makeitem`, characters for
/// `/loadmonster` and `/zoe2` — so the three commands cannot drift in how they
/// accept an argument.
///
/// Deviation, deliberate and additive: a bare decimal ref id is accepted too.
/// The original only takes a codename (it holds the ref-object table and the
/// server never sees the name), but a numeric form costs nothing and makes
/// probing an unnamed row possible. Codename is tried first, so a numeric
/// codename could never be shadowed.
fn resolve_ref(
    arg: &str,
    lookup: Option<&dyn Fn(&str) -> Option<i32>>,
    table: RefTable,
) -> Result<u32, String> {
    if let Some(id) = lookup.and_then(|lookup| lookup(arg)) {
        return Ok(id as u32);
    }
    if let Ok(ref_id) = arg.parse::<u32>() {
        return Ok(ref_id);
    }
    Err(format!(
        "unknown {} codename {arg:?} ({} {})",
        table.subject(),
        table.table(),
        if lookup.is_some() {
            "loaded"
        } else {
            "not loaded yet"
        }
    ))
}

/// Argument 2: `swscanf("%d")` then **truncated to its low byte**. Parsed as
/// `i64` first so the original's own tolerance for out-of-range input is kept
/// (it wraps rather than refusing).
fn parse_count_byte(arg: &str) -> Result<u8, String> {
    arg.parse::<i64>()
        .map(|n| n as u8)
        .map_err(|_| format!("{arg:?} is not a number"))
}

/// `/makeitem <codename|ref_id> <value>` → [`GmCommand::MakeItem`].
///
/// Mirrors the original's validation rather than inventing one: exactly two
/// arguments, the item must resolve, its `TypeID1` must be the item family,
/// and the count byte is clamped to `[1, MaxStack]` **only** for stackable
/// rows — equipment passes through untouched, which is why a `255` on a weapon
/// reaches the server as `0xFF`.
fn parse_make_item(
    args: &[&str],
    index: Option<&ClientItemIndex>,
    item_data: Option<&ClientItemData>,
) -> Result<GmCommand, String> {
    let [item, value] = args else {
        return Err("usage: /makeitem <codename|ref_id> <value>".to_string());
    };
    let lookup = index.map(|index| move |name: &str| index.id(name));
    let ref_id = resolve_ref(
        item,
        lookup.as_ref().map(|f| f as &dyn Fn(&str) -> Option<i32>),
        RefTable::Item,
    )?;
    let value = parse_count_byte(value)?;
    make_item_command(ref_id, value, item_data)
}

/// Build a [`GmCommand::MakeItem`], applying the original's own type gate and
/// stack clamp. The single place that rule lives — the chat command and the
/// dev COS window both go through it, so neither can drift from the other.
pub fn make_item_command(
    ref_id: u32,
    value: u8,
    item_data: Option<&ClientItemData>,
) -> Result<GmCommand, String> {
    // The gate needs the row; with itemdata not yet loaded we cannot check it,
    // and refusing would be worse than sending — the server validates too.
    let Some(row) = item_data.and_then(|data| data.get(&(ref_id as i32))) else {
        return Ok(GmCommand::MakeItem { ref_id, value });
    };
    match row.type_ids() {
        Some((ITEM_TYPE_ID1, ETC_TYPE_ID2, _, _)) => {
            // Stackable: the original clamps into [1, MaxStack], so a 0 or an
            // over-stack never reaches the wire.
            let max = row.max_stack().unwrap_or(1).clamp(1, u8::MAX as u32) as u8;
            Ok(GmCommand::MakeItem {
                ref_id,
                value: value.clamp(1, max),
            })
        }
        // Equipment and the rest of the item family: sent untouched, which is
        // how a `255` on a weapon reaches the server as 0xFF.
        Some((ITEM_TYPE_ID1, ..)) => Ok(GmCommand::MakeItem { ref_id, value }),
        Some((tid1, ..)) => Err(format!(
            "ref {ref_id} is TypeID1 {tid1}, not an item ({ITEM_TYPE_ID1})"
        )),
        None => Err(format!("ref {ref_id} has no TypeID columns")),
    }
}

/// `/loadmonster <codename|ref_id> <count> [rarity]` →
/// [`GmCommand::LoadMonster`]. `rarity` defaults to 0 (normal).
///
/// The monster's ref lives in **characterdata**, so this resolves against
/// [`ClientCharacterIndex`] — the reason that index exists.
fn parse_load_monster(
    args: &[&str],
    index: Option<&ClientCharacterIndex>,
) -> Result<GmCommand, String> {
    let (monster, count, rarity) = match args {
        [monster, count] => (monster, count, None),
        [monster, count, rarity] => (monster, count, Some(rarity)),
        _ => return Err("usage: /loadmonster <codename|ref_id> <count> [rarity]".to_string()),
    };
    Ok(load_monster_command(
        resolve_monster_ref(monster, index)?,
        parse_count_byte(count)?,
        rarity
            .map(|r| parse_count_byte(r))
            .transpose()?
            .unwrap_or(0),
    ))
}

/// Resolve a monster argument against characterdata.
fn resolve_monster_ref(arg: &str, index: Option<&ClientCharacterIndex>) -> Result<u32, String> {
    let lookup = index.map(|index| move |name: &str| index.id(name));
    resolve_ref(
        arg,
        lookup.as_ref().map(|f| f as &dyn Fn(&str) -> Option<i32>),
        RefTable::Character,
    )
}

/// Build a [`GmCommand::LoadMonster`]. The counterpart to
/// [`make_item_command`], and the single place the count floor lives — the
/// chat command and the GM dev window both go through it.
pub fn load_monster_command(ref_id: u32, count: u8, rarity: u8) -> GmCommand {
    GmCommand::LoadMonster {
        ref_id,
        // A spawn of zero is not a command, it is a typo.
        count: count.max(1),
        rarity,
    }
}

/// The most monsters one `Zoe` packet may ask for.
///
/// `Zoe` itself accepts up to 255, but `Zoe2` splits anything larger than this
/// into chunks of exactly 200 — so 200 is the batch size the original considers
/// safe, and the reason `Zoe2` exists at all.
pub const ZOE_BATCH: u16 = 200;

/// `/zoe` — one spawn packet. The original clamps only the ceiling here (a 0
/// or a negative count leaks through as `count & 0xFF`); we floor it at 1 too,
/// because a spawn of zero is a typo rather than a command.
pub fn zoe_command(ref_id: u32, count: u8) -> GmCommand {
    GmCommand::Zoe {
        ref_id,
        count: count.max(1),
    }
}

/// `/zoe2` — the same packet, batched.
///
/// `Zoe2` is **not** a distinct sub-command: it emits `Zoe`'s `0x000C` and its
/// whole contribution is client-side, splitting `count` into chunks of
/// [`ZOE_BATCH`] so the server is never asked for more at once.
///
/// **Stated deviation:** the original also *paces* the chunks, taking an
/// optional third argument as a delay in seconds and draining a queue on a
/// timer (which is why `~Zoe2 <mob> 500` with no delay argument throws in the
/// original). We send the chunks immediately and ignore a delay argument. The
/// packets are identical; only the spacing differs, and a timer-driven queue
/// is not worth its state here.
pub fn zoe2_commands(ref_id: u32, count: u16) -> Vec<GmCommand> {
    let count = count.max(1);
    let mut out = Vec::new();
    let mut remaining = count;
    while remaining > 0 {
        let chunk = remaining.min(ZOE_BATCH);
        out.push(GmCommand::Zoe {
            ref_id,
            count: chunk as u8,
        });
        remaining -= chunk;
    }
    out
}

/// `/zoe <codename|ref_id> <count>` and
/// `/zoe2 <codename|ref_id> <count> [delay]`.
///
/// Both gate argument 1 on `TypeID 1/2/1` — character / NPC / monster — which
/// is the original's own gate and exactly what `CharacterDataRow::is_monster`
/// tests. `zoe2` tolerates a trailing delay argument (the original requires one
/// above 200) and ignores it; see [`zoe2_commands`].
fn parse_zoe(
    args: &[&str],
    batched: bool,
    index: Option<&ClientCharacterIndex>,
    char_data: Option<&ClientCharacterData>,
) -> Result<Vec<GmCommand>, String> {
    let verb = if batched { "zoe2" } else { "zoe" };
    let (monster, count) = match args {
        [monster, count] => (monster, count),
        // The original's Zoe2 permits a 4th token (the pace delay); Zoe does
        // not. We accept it either way and drop it.
        [monster, count, _delay] if batched => (monster, count),
        _ => {
            return Err(format!(
                "usage: /{verb} <codename|ref_id> <count>{}",
                if batched { " [delay]" } else { "" }
            ))
        }
    };
    let ref_id = resolve_monster_ref(monster, index)?;
    // The gate needs the row; without characterdata loaded we cannot check it,
    // and refusing would be worse than sending — the server validates too.
    if let Some(row) = char_data.and_then(|data| data.get(&(ref_id as i32))) {
        if !row.is_monster() {
            return Err(format!(
                "ref {ref_id} ({}) is not a monster (TypeID 1/2/1)",
                row.code_name()
            ));
        }
    }
    let count: u16 = count
        .parse::<i64>()
        .map(|n| n.clamp(1, u16::MAX as i64) as u16)
        .map_err(|_| format!("{count:?} is not a number"))?;
    Ok(if batched {
        zoe2_commands(ref_id, count)
    } else {
        vec![zoe_command(ref_id, count.min(u8::MAX as u16) as u8)]
    })
}

/// Log the GM ack; on success, optimistically flip a pending invisibility
/// toggle (covers servers that don't echo a 0x30BF body update to the caller;
/// [`invisibility_from_state_update`] overrides with the authoritative value
/// if one does arrive).
fn on_gm_response(mut reader: MessageReader<GmResponse>, mut gm: ResMut<GmState>) {
    for msg in reader.read() {
        // The echoed sub-command is the whole value of this ack: it names
        // which command the server just judged, so an unprivileged account or
        // a wrong sub-id is legible from one log line instead of a guess.
        if msg.is_success() {
            info!("gm: command {:#06x} accepted", msg.gm_command_id);
            if gm.pending_invisible {
                gm.invisible = !gm.invisible;
                gm.pending_invisible = false;
            }
        } else {
            warn!(
                "gm: command {:#06x} rejected (result {})",
                msg.gm_command_id, msg.result
            );
            gm.pending_invisible = false;
        }
    }
}

/// Authoritative invisibility: a 0x30BF body-state update for the local
/// player.
fn invisibility_from_state_update(
    mut reader: MessageReader<EntityStateUpdate>,
    player: Query<&NetworkId, With<Player>>,
    mut gm: ResMut<GmState>,
) {
    let Ok(NetworkId(local_uid)) = player.single() else {
        reader.clear();
        return;
    };
    for msg in reader.read() {
        if msg.unique_id != *local_uid {
            continue;
        }
        if let Some(invisible) = msg.body_invisibility() {
            gm.invisible = invisible;
            gm.pending_invisible = false;
        }
    }
}

/// Seed invisibility **and GM-ness** from the character data at login (a GM who
/// logged out invisible stays invisible).
///
/// `PlayerExtras.gm` has been on the wire and parsed all along; nothing read it
/// until another character's invisibility needed to know whether we are
/// entitled to see it.
fn invisibility_from_character_data(
    info: Query<&CharacterInfo, (With<Player>, Added<CharacterInfo>)>,
    mut gm: ResMut<GmState>,
) {
    if let Ok(info) = info.single() {
        if let Some(body_state) = info.body_state {
            gm.invisible = body_state_is_invisible(body_state);
        }
        if let Some(extras) = info.extras.as_ref() {
            gm.is_gm = extras.gm;
        }
    }
}

/// Track other **characters'** body state from 0x30BF kind 4.
///
/// `combat`'s reader takes kinds 0 and 1 off the same message and has no
/// business with this one; the render policy lives here, so the tracking does
/// too. Scoped to `RemoteEntity::Player` — see [`RemoteBodyState`] for why that
/// scope is load-bearing rather than tidiness.
///
/// An update whose entity has not spawned yet is retried rather than dropped,
/// for the same reason and on the same budget as `combat`'s reader
/// (`STATE_UPDATE_RETRY_FRAMES`): a character can be announced invisible in the
/// same breath as their spawn record, and the uid does not enter
/// `NetworkEntities` until the deferred spawn command applies.
fn remote_body_state_from_updates(
    mut reader: MessageReader<EntityStateUpdate>,
    net: Res<NetworkEntities>,
    kinds: Query<&RemoteEntity, Without<Player>>,
    mut pending: Local<Vec<(EntityStateUpdate, u8)>>,
    mut commands: Commands,
) {
    pending.extend(
        reader
            .read()
            .filter(|msg| msg.body_state_value().is_some())
            .map(|msg| (msg.clone(), 0)),
    );
    pending.retain_mut(|(msg, attempts)| {
        let Some(entity) = net.get(msg.unique_id) else {
            *attempts += 1;
            return *attempts < crate::plugins::combat::STATE_UPDATE_RETRY_FRAMES;
        };
        if let (Ok(RemoteEntity::Player), Some(state)) = (kinds.get(entity), msg.body_state_value())
        {
            // `try_insert`: the entity came from the network and can despawn
            // between the message and the command applying.
            commands.entity(entity).try_insert(RemoteBodyState(state));
        }
        false
    });
}

/// The queries the ghost recipe reads, bundled so the two policies that use it
/// can pass them along without eleven arguments each.
#[derive(SystemParam)]
struct GhostMaterials<'w, 's> {
    children: Query<'w, 's, &'static Children>,
    std_meshes: Query<'w, 's, &'static MeshMaterial3d<StandardMaterial>>,
    sheen_meshes: Query<'w, 's, &'static MeshMaterial3d<SroSheenMaterial>>,
    ghosted: Query<'w, 's, &'static GhostOriginal>,
    std_assets: ResMut<'w, Assets<StandardMaterial>>,
    sheen_assets: ResMut<'w, Assets<SroSheenMaterial>>,
}

/// Make every mesh under `root` translucent, or restore it.
///
/// One recipe for both the local `/invisible` toggle and another GM's
/// invisibility: the material swap is fiddly (two material types, a remembered
/// original, and meshes that stream in *after* the state changed), and two
/// copies of it would drift. Idempotent per mesh — `GhostOriginal` is both the
/// restore record and the "already handled" mark — which is what lets this run
/// every frame and still catch late-loading body parts.
fn set_ghosted(mats: &mut GhostMaterials, commands: &mut Commands, root: Entity, ghost: bool) {
    let descendants: Vec<Entity> = mats.children.iter_descendants(root).collect();
    for entity in descendants {
        let is_ghosted = mats.ghosted.contains(entity);
        if ghost && !is_ghosted {
            // Swap this mesh's material for a translucent clone, remembering
            // the original so it can be restored.
            if let Ok(mat) = mats.std_meshes.get(entity) {
                if let Some(mut clone) = mats.std_assets.get(mat.0.id()).cloned() {
                    clone.base_color = clone.base_color.with_alpha(GHOST_ALPHA);
                    clone.alpha_mode = AlphaMode::Blend;
                    let ghost = mats.std_assets.add(clone);
                    commands.entity(entity).try_insert((
                        MeshMaterial3d(ghost),
                        GhostOriginal::Standard(mat.0.clone()),
                    ));
                }
            } else if let Ok(mat) = mats.sheen_meshes.get(entity) {
                if let Some(mut clone) = mats.sheen_assets.get(mat.0.id()).cloned() {
                    clone.base.base_color = clone.base.base_color.with_alpha(GHOST_ALPHA);
                    clone.base.alpha_mode = AlphaMode::Blend;
                    let ghost = mats.sheen_assets.add(clone);
                    commands
                        .entity(entity)
                        .try_insert((MeshMaterial3d(ghost), GhostOriginal::Sheen(mat.0.clone())));
                }
            }
        } else if !ghost && is_ghosted {
            // Restore the original material; the translucent clone is dropped.
            match mats.ghosted.get(entity) {
                Ok(GhostOriginal::Standard(handle)) => {
                    commands
                        .entity(entity)
                        .try_insert(MeshMaterial3d(handle.clone()));
                }
                Ok(GhostOriginal::Sheen(handle)) => {
                    commands
                        .entity(entity)
                        .try_insert(MeshMaterial3d(handle.clone()));
                }
                Err(_) => {}
            }
            commands.entity(entity).remove::<GhostOriginal>();
        }
    }
}

/// Drop (or restore) the local character's rendered alpha to match
/// [`GmState::invisible`]. Runs every frame so body meshes that stream in
/// while invisible still get ghosted; already-handled meshes are skipped.
///
/// Your own body is always the *ghost* treatment, never hidden: all three
/// invisible body states mean "others cannot see you", and a client that drew
/// nothing at all would leave the player unable to see themselves.
fn render_invisibility(
    gm: Res<GmState>,
    player: Query<Entity, With<Player>>,
    mut mats: GhostMaterials,
    mut commands: Commands,
) {
    let Ok(root) = player.single() else {
        return;
    };
    set_ghosted(&mut mats, &mut commands, root, gm.invisible);
}

/// Draw other **characters** according to their body state: a GM sees a
/// GM-invisible character as a ghost, everyone else sees nothing, and stealth
/// is nobody's to see.
///
/// The hiding half is a plain `Visibility` on the character root and needs none
/// of the material machinery — but a hidden root is also un-ghosted first, so a
/// character that comes back into view does not surface still translucent.
fn render_remote_invisibility(
    gm: Res<GmState>,
    mut remotes: Query<
        (Entity, &RemoteBodyState, &mut Visibility),
        (With<RemoteEntity>, Without<Player>),
    >,
    kinds: Query<&RemoteEntity>,
    mut mats: GhostMaterials,
    mut commands: Commands,
) {
    for (entity, body, mut visibility) in remotes.iter_mut() {
        // Characters only. `RemoteBodyState` is only inserted on players, but
        // the query cannot say so, and the census in `hidden_render` is why
        // this must not widen by accident.
        if !matches!(kinds.get(entity), Ok(RemoteEntity::Player)) {
            continue;
        }
        let render = hidden_render(body.0, gm.is_gm);
        let hide = render == HiddenRender::Hidden;
        let wanted = if hide {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
        set_ghosted(
            &mut mats,
            &mut commands,
            entity,
            render == HiddenRender::Ghost,
        );
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::assets::textdata::characterdata::{CharacterData, CharacterDataRow};
    use crate::assets::textdata::itemdata::{ItemData, ItemDataRow};
    use std::collections::HashMap;

    /// The two ids used throughout: the staff from the request that prompted
    /// this work, and a stackable to exercise the clamp branch.
    const STAFF_REF: i32 = 25627;
    const SCROLL_REF: i32 = 100;

    /// A row wide enough to carry `MaxStack` (column 57), with only the
    /// columns these helpers read filled in.
    fn row(type_ids: (u32, u32, u32, u32), max_stack: u32) -> ItemDataRow {
        let mut fields = vec![String::new(); 60];
        fields[9] = type_ids.0.to_string();
        fields[10] = type_ids.1.to_string();
        fields[11] = type_ids.2.to_string();
        fields[12] = type_ids.3.to_string();
        fields[57] = max_stack.to_string();
        ItemDataRow(fields)
    }

    fn item_data() -> ClientItemData {
        ClientItemData::from_data(ItemData(HashMap::from([
            // ITEM_EU_STAFF_11_SET_A_RARE — equipment (3/1/6/15)
            (STAFF_REF, row((3, 1, 6, 15), 1)),
            // a stackable ETC row (3/3/…), MaxStack 20
            (SCROLL_REF, row((3, 3, 3, 2), 20)),
            // a non-item row, which /makeitem must refuse
            (999, row((1, 2, 3, 1), 1)),
        ])))
    }

    fn index() -> ClientItemIndex {
        ClientItemIndex::from_pairs([
            ("ITEM_EU_STAFF_11_SET_A_RARE".to_string(), STAFF_REF),
            ("ITEM_COS_P_FLUTE".to_string(), SCROLL_REF),
        ])
    }

    const MOB_REF: i32 = 1907;
    const NPC_REF: i32 = 2000;

    /// A characterdata row: codename at 2, TypeID1..4 at 9..12.
    fn char_row(code: &str, type_ids: (u32, u32, u32, u32)) -> CharacterDataRow {
        let mut fields = vec![String::new(); 60];
        fields[2] = code.to_string();
        fields[9] = type_ids.0.to_string();
        fields[10] = type_ids.1.to_string();
        fields[11] = type_ids.2.to_string();
        fields[12] = type_ids.3.to_string();
        CharacterDataRow(fields)
    }

    fn char_data() -> ClientCharacterData {
        ClientCharacterData::from_table(CharacterData(HashMap::from([
            // a monster: TypeID 1/2/1, the gate zoe/zoe2 apply
            (MOB_REF, char_row("MOB_CH_TIGERWOMAN", (1, 2, 1, 1))),
            // a plain NPC: character family, but not a monster
            (NPC_REF, char_row("NPC_CH_POTION", (1, 2, 2, 1))),
        ])))
    }

    fn char_index() -> ClientCharacterIndex {
        ClientCharacterIndex::from_pairs([
            ("MOB_CH_TIGERWOMAN".to_string(), MOB_REF),
            ("NPC_CH_POTION".to_string(), NPC_REF),
        ])
    }

    /// The command that prompted all of this: an equipment row, so the count
    /// byte is **not** clamped and 255 reaches the wire intact.
    #[test]
    fn makeitem_resolves_the_codename_and_passes_equipment_values_through() {
        let command = parse_make_item(
            &["ITEM_EU_STAFF_11_SET_A_RARE", "255"],
            Some(&index()),
            Some(&item_data()),
        )
        .expect("the staff resolves");

        assert_eq!(
            command,
            GmCommand::MakeItem {
                ref_id: STAFF_REF as u32,
                value: 255,
            }
        );
    }

    /// Stackables take the original's clamp into `[1, MaxStack]`, so neither a
    /// 0 nor an over-stack can reach the server.
    #[test]
    fn makeitem_clamps_only_stackable_rows() {
        let clamp = |value: &str| {
            parse_make_item(
                &["ITEM_COS_P_FLUTE", value],
                Some(&index()),
                Some(&item_data()),
            )
            .unwrap()
        };
        let value_of = |command| match command {
            GmCommand::MakeItem { value, .. } => value,
            other => panic!("expected MakeItem, got {other:?}"),
        };

        assert_eq!(value_of(clamp("5")), 5);
        assert_eq!(value_of(clamp("0")), 1, "0 clamps up to one item");
        assert_eq!(value_of(clamp("200")), 20, "clamped down to MaxStack");
        // 256 truncates to 0 through the low byte, then clamps to 1 — the
        // original's `swscanf` + byte truncation, not a rejection.
        assert_eq!(value_of(clamp("256")), 1);
    }

    /// The original gates argument 1 on `TypeID1 == 3`, and rejects a codename
    /// it cannot resolve rather than inventing a ref id.
    #[test]
    fn makeitem_rejects_non_items_and_unknown_names() {
        assert!(parse_make_item(&["999", "1"], Some(&index()), Some(&item_data())).is_err());
        assert!(
            parse_make_item(&["NOT_AN_ITEM", "1"], Some(&index()), Some(&item_data())).is_err()
        );
        // wrong argument count — the original requires exactly two
        assert!(parse_make_item(
            &["ITEM_EU_STAFF_11_SET_A_RARE"],
            Some(&index()),
            Some(&item_data())
        )
        .is_err());
        assert!(parse_make_item(&[], Some(&index()), Some(&item_data())).is_err());
    }

    /// Our stated deviation: a bare ref id works too, and the codename still
    /// wins when both could match.
    #[test]
    fn makeitem_also_accepts_a_numeric_ref_id() {
        let command = parse_make_item(&["25627", "3"], Some(&index()), Some(&item_data())).unwrap();
        assert_eq!(
            command,
            GmCommand::MakeItem {
                ref_id: STAFF_REF as u32,
                value: 3,
            }
        );
    }

    /// With itemdata not yet loaded the gate cannot run, and refusing would be
    /// worse than sending — the server validates too.
    #[test]
    fn makeitem_sends_unvalidated_when_itemdata_is_missing() {
        let command = parse_make_item(&["25627", "255"], None, None).unwrap();
        assert_eq!(
            command,
            GmCommand::MakeItem {
                ref_id: STAFF_REF as u32,
                value: 255,
            }
        );
    }

    /// `/zoe` is one packet; `/zoe2` is the *same* packet batched. Both carry
    /// sub-command 0x0C — Zoe2 is not a distinct sub-command on the wire.
    #[test]
    fn zoe_and_zoe2_emit_the_same_sub_command() {
        let zoe = parse_zoe(
            &["MOB_CH_TIGERWOMAN", "5"],
            false,
            Some(&char_index()),
            Some(&char_data()),
        )
        .unwrap();
        assert_eq!(
            zoe,
            vec![GmCommand::Zoe {
                ref_id: MOB_REF as u32,
                count: 5,
            }]
        );

        let zoe2 = parse_zoe(
            &["MOB_CH_TIGERWOMAN", "5"],
            true,
            Some(&char_index()),
            Some(&char_data()),
        )
        .unwrap();
        assert_eq!(zoe2, zoe, "below the batch size the two are identical");
        assert!(zoe.iter().all(|c| c.code() == 0x0C));
    }

    /// Above [`ZOE_BATCH`] the original splits into chunks of 200; the total
    /// asked for must survive the split exactly.
    #[test]
    fn zoe2_batches_in_chunks_of_two_hundred() {
        let counts = |n: u16| {
            zoe2_commands(MOB_REF as u32, n)
                .into_iter()
                .map(|c| match c {
                    GmCommand::Zoe { count, .. } => count as u16,
                    other => panic!("expected Zoe, got {other:?}"),
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(counts(1), vec![1]);
        assert_eq!(counts(200), vec![200]);
        assert_eq!(counts(201), vec![200, 1]);
        assert_eq!(counts(500), vec![200, 200, 100]);
        // nothing is lost or invented in the split
        for n in [1u16, 7, 200, 201, 399, 500, 1000] {
            assert_eq!(counts(n).iter().sum::<u16>(), n, "total for {n}");
        }
        // a zero is a typo, not a command
        assert_eq!(counts(0), vec![1]);
    }

    /// Both zoe forms gate argument 1 on `TypeID 1/2/1`, so an NPC or an item
    /// is refused rather than spawned.
    #[test]
    fn zoe_refuses_anything_that_is_not_a_monster() {
        for batched in [false, true] {
            assert!(parse_zoe(
                &["NPC_CH_POTION", "1"],
                batched,
                Some(&char_index()),
                Some(&char_data())
            )
            .is_err());
            assert!(parse_zoe(
                &["NOT_A_MOB", "1"],
                batched,
                Some(&char_index()),
                Some(&char_data())
            )
            .is_err());
        }
        // `/zoe` takes exactly two arguments; `/zoe2` tolerates the original's
        // optional pace delay and ignores it.
        assert!(parse_zoe(
            &["MOB_CH_TIGERWOMAN", "5", "0.5"],
            false,
            Some(&char_index()),
            Some(&char_data())
        )
        .is_err());
        assert!(parse_zoe(
            &["MOB_CH_TIGERWOMAN", "5", "0.5"],
            true,
            Some(&char_index()),
            Some(&char_data())
        )
        .is_ok());
    }

    #[test]
    fn loadmonster_defaults_rarity_and_needs_at_least_one() {
        assert_eq!(
            parse_load_monster(&["1907", "0"], None).unwrap(),
            GmCommand::LoadMonster {
                ref_id: 1907,
                count: 1,
                rarity: 0,
            }
        );
        assert_eq!(
            parse_load_monster(&["1907", "3", "1"], None).unwrap(),
            GmCommand::LoadMonster {
                ref_id: 1907,
                count: 3,
                rarity: 1,
            }
        );
        assert!(parse_load_monster(&["1907"], None).is_err());
    }
}
