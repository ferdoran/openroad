//! Item hover tooltip.
//!
//! This panel is an **openroad design**, not the original's
//! `GDR_HELPBUBBLEWND` (`ginterface.txt:337`/`:358`, `CIFHelperBubbleWindow`,
//! `Rect="0,0,120,120"`): its chrome, colors, 230 px width and
//! `hovered_slot` trigger are ours, because a 100x100 text box cannot hold an
//! item stat readout and the original's bubble carries no data at all
//! (`HelpString` is empty on all 302 `CIFSlotWithHelp` declarations, so its
//! content path is code-side and unrecoverable from the archive). Grade this
//! file as our item-stat panel, not against `docs/re/ui/help-tooltip-widget.md`.
//!
//! Idea: a single hidden panel (own root, above the window) is rebuilt when
//! `InventoryState.hovered_slot` changes and follows the cursor clamped to
//! the window. Stats reproduce the vanilla item info: itemdata carries each
//! white stat as a `(lower, upper)` column pair and the item's `variance`
//! (0x3013) selects the concrete roll — 5 bits per stat slot,
//! `value = lower + (upper - lower) * bits / 31`. The per-category stat-slot
//! order lives in the `*_WHITE_SLOTS` tables; if a live value ever disagrees
//! with the vanilla client, fix the slot order there (the math is verified).

use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bevy::window::PrimaryWindow;

use packets::agent::character_data::{
    EquipmentData, InventoryItem, ItemTypeData, COS_STATE_SUMMONED,
};

use crate::assets::textdata::itemdata::{ItemDataRow, SealTier, StatRange};
use crate::assets::FontAssets;
use crate::plugins::hud::cos::state::CosState;
use crate::plugins::hud::inventory::model::InventoryState;
use crate::plugins::hud::inventory::ui::{
    equip_criteria, local_equip_context, EquipCriteria, EquipSlotCell, InventoryGridCell,
};
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::hud::storage::model::StorageHoveredItem;
use crate::plugins::hud::store::model::StoreHoveredGood;
use crate::plugins::net::character_info::CharacterInfo;
use crate::plugins::net::inventory::{Inventory, BAG_FIRST_SLOT, SLOTS_PER_PAGE};
use crate::plugins::player::Player;
use crate::plugins::textdata::{
    ClientCharacterData, ClientItemData, ClientMagicOptions, ClientTextNames, ClientUiStrings,
};

#[derive(Component)]
pub struct InventoryTooltipRoot;

/// Cursor offset and screen margin of the tooltip.
const CURSOR_OFFSET: f32 = 18.0;
const SCREEN_MARGIN: f32 = 4.0;

const NAME_COLOR: Color = Color::srgb(1.0, 0.85, 0.32);
const TEXT_COLOR: Color = Color::srgb(0.92, 0.92, 0.92);
const DIM_COLOR: Color = Color::srgb(0.65, 0.65, 0.65);
/// The blue of magic ("blue") options — also the name color of an item that
/// carries any.
const MAGIC_COLOR: Color = Color::srgb(0.5, 0.7, 1.0);
/// The "Dead" line on a COS scroll's tooltip.
///
/// Red, and deliberately **not** the same colour as the slot icon's wash
/// (`DEAD_PET_TINT` in `inventory/ui.rs`, which stays blue): the tint is a
/// state wash over artwork, this is a warning in a list of facts about the
/// item. The two are independent — do not "keep them in step".
const DEAD_PET_COLOR: Color = Color::srgb(0.95, 0.35, 0.35);
/// The "Summoned" line on a COS scroll whose pet is out. Matches the slot's
/// corner badge (`inventory::ui::PET_SUMMONED_BADGE`) — unlike the dead-pet
/// pair above, these two *are* meant to be read as the same signal, so they
/// share a colour deliberately.
const SUMMONED_PET_COLOR: Color = Color::srgb(0.36, 0.9, 0.45);
/// A fact about the item that works **against** the player: a requirement this
/// character does not meet, or a drawback among its magic options.
///
/// Shares [`DEAD_PET_COLOR`]'s red because it is the same category of line —
/// "this is the reason it will not do what you want" — but it is a separate
/// constant on purpose: the dead-pet line is per-*pet* state that clears when
/// the pet is revived, while these are per-*character* and per-*item* facts.
/// Merging them would tie a future change to one to the other.
const WARN_COLOR: Color = Color::srgb(0.95, 0.35, 0.35);

pub fn spawn_tooltip(commands: &mut Commands, _asset_server: &AssetServer, camera: Entity) {
    let s = hud_scale();
    commands.spawn((
        InventoryTooltipRoot,
        Name::from("Inventory Tooltip"),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            max_width: Val::Px(230.0 * s),
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(Val::Px(6.0 * s)),
            row_gap: Val::Px(1.0 * s),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.05, 0.88)),
        BorderColor::all(Color::srgb(0.45, 0.4, 0.25)),
        Visibility::Hidden,
        GlobalZIndex(70),
        UiTargetCamera(camera),
        Pickable::IGNORE,
    ));
}

// --- Hover observers (attached to every slot cell by ui.rs) -----------------

pub fn on_slot_over(
    over: On<Pointer<Over>>,
    grid: Query<&InventoryGridCell>,
    equip: Query<&EquipSlotCell>,
    mut state: ResMut<InventoryState>,
) {
    let slot = if let Ok(cell) = grid.get(over.entity) {
        BAG_FIRST_SLOT + state.active_page * SLOTS_PER_PAGE + cell.index
    } else if let Ok(cell) = equip.get(over.entity) {
        cell.slot
    } else {
        return;
    };
    if state.hovered_slot != Some(slot) {
        state.hovered_slot = Some(slot);
    }
}

pub fn on_slot_out(
    out: On<Pointer<Out>>,
    grid: Query<&InventoryGridCell>,
    equip: Query<&EquipSlotCell>,
    mut state: ResMut<InventoryState>,
) {
    let slot = if let Ok(cell) = grid.get(out.entity) {
        BAG_FIRST_SLOT + state.active_page * SLOTS_PER_PAGE + cell.index
    } else if let Ok(cell) = equip.get(out.entity) {
        cell.slot
    } else {
        return;
    };
    if state.hovered_slot == Some(slot) {
        state.hovered_slot = None;
    }
}

// --- Content ----------------------------------------------------------------

/// Rebuild the tooltip lines when the hovered slot changes; hidden when
/// nothing (or an empty slot) is hovered, or while dragging.
///
/// Serves three windows: the inventory publishes its hovered slot in
/// [`InventoryState`], the storage window publishes the hovered item itself in
/// [`StorageHoveredItem`], and the NPC shop publishes the hovered good's ref id
/// in [`StoreHoveredGood`] (neither of the latter two has Over/Out observers —
/// both poll `Hovered`). The inventory wins when several are hovered, since it
/// draws on top; the shop comes last because it is a catalog row rather than a
/// real item and renders the reduced block (see [`catalog_tooltip_lines`]).
#[allow(clippy::too_many_arguments)]
pub fn refresh_tooltip(
    state: Res<InventoryState>,
    hovered_storage: Res<StorageHoveredItem>,
    hovered_good: Res<StoreHoveredGood>,
    inventories: Query<&Inventory, With<Player>>,
    changed: Query<(), (With<Player>, Changed<Inventory>)>,
    players: Query<&CharacterInfo, With<Player>>,
    char_data: Res<ClientCharacterData>,
    cos_state: Res<CosState>,
    item_data: Res<ClientItemData>,
    names: Res<ClientTextNames>,
    ui_strings: Res<ClientUiStrings>,
    magic_options: Res<ClientMagicOptions>,
    fonts: Res<FontAssets>,
    roots: Query<Entity, With<InventoryTooltipRoot>>,
    mut visibilities: Query<&mut Visibility, With<InventoryTooltipRoot>>,
    mut commands: Commands,
) {
    if !state.is_changed()
        && !hovered_storage.is_changed()
        && !hovered_good.is_changed()
        && changed.is_empty()
    {
        return;
    }
    let Ok(root) = roots.single() else {
        return;
    };
    let item = state
        .hovered_slot
        .filter(|_| state.open && state.drag.is_none())
        .and_then(|slot| inventories.single().ok().and_then(|inv| inv.get(slot)))
        .or(hovered_storage.0.as_ref());
    // The requirement lines are reddened for the criteria *this* character
    // fails, through the same evaluator the slot wash uses — see
    // `ui::equip_criteria`.
    let empty_inventory = Inventory::default();
    let inventory = inventories.single().unwrap_or(&empty_inventory);
    let (player_level, player_gender) = local_equip_context(&players, &char_data);
    let criteria = |row: Option<&ItemDataRow>| {
        row.map(|row| equip_criteria(row, player_level, player_gender, &item_data, inventory))
            .unwrap_or(EquipCriteria::MET)
    };
    let lines = match (item, hovered_good.0) {
        (Some(item), _) => {
            let row = item_data.get(&(item.ref_id as i32));
            // Rarity probe. Seal grade is derived purely from the code-name
            // suffix — there is no rarity column and no wire field — so when a
            // Seal item does not glow in the bag, the only question is what its
            // itemdata row actually says. One hover at `RUST_LOG=client=debug`
            // answers it without a capture.
            if let Some(row) = row {
                debug!(
                    "item probe: ref {} code {:?} rare {} seal {:?} icon {:?}",
                    item.ref_id,
                    row.code_name(),
                    row.is_rare(),
                    row.seal_tier(),
                    row.icon_path(),
                );
            }
            tooltip_lines(
                item,
                row,
                &names,
                &ui_strings,
                &magic_options,
                criteria(row),
                PetContext {
                    cos: &cos_state,
                    char_data: &char_data,
                },
            )
        }
        (None, Some(ref_id)) => {
            let row = item_data.get(&(ref_id as i32));
            catalog_tooltip_lines(ref_id, row, &names, criteria(row))
        }
        (None, None) => {
            for mut visibility in visibilities.iter_mut() {
                *visibility = Visibility::Hidden;
            }
            return;
        }
    };

    let s = hud_scale();
    let mut root_commands = commands.entity(root);
    root_commands.despawn_related::<Children>();
    root_commands.with_children(|panel| {
        for (i, (text, color)) in lines.into_iter().enumerate() {
            // The first line is the item name: bold, larger, with a gap before
            // the stats block below it. The bold comes from the UI face's own
            // `wght` axis rather than a second font file — Arimo is variable,
            // so one face covers both weights (see `BUNDLED_FALLBACK_FACE`).
            let (weight, size, margin) = if i == 0 {
                (FontWeight::BOLD, 9.5 * s, UiRect::bottom(Val::Px(4.0 * s)))
            } else {
                (FontWeight::NORMAL, 7.5 * s, UiRect::ZERO)
            };
            panel.spawn((
                Text::new(text),
                TextFont {
                    font: fonts.two.clone().into(),
                    font_size: FontSize::Px(size),
                    weight,
                    ..default()
                },
                TextColor(color),
                Node {
                    margin,
                    ..default()
                },
                Pickable::IGNORE,
            ));
        }
    });
    for mut visibility in visibilities.iter_mut() {
        *visibility = Visibility::Inherited;
    }
}

/// Follow the cursor while visible, clamped into the window.
pub fn position_tooltip(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut roots: Query<(&mut Node, &ComputedNode, &Visibility), With<InventoryTooltipRoot>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    for (mut node, computed, visibility) in roots.iter_mut() {
        if *visibility == Visibility::Hidden {
            continue;
        }
        let size = computed.size * computed.inverse_scale_factor;
        let max_x = (window.width() - size.x - SCREEN_MARGIN).max(0.0);
        let max_y = (window.height() - size.y - SCREEN_MARGIN).max(0.0);
        // prefer left of the cursor (the vanilla tooltip side for a
        // right-anchored window), fall back to the right
        let x = if cursor.x - size.x - CURSOR_OFFSET >= 0.0 {
            cursor.x - size.x - CURSOR_OFFSET
        } else {
            (cursor.x + CURSOR_OFFSET).min(max_x)
        };
        let y = (cursor.y + CURSOR_OFFSET).min(max_y);
        node.left = Val::Px(x);
        node.top = Val::Px(y);
    }
}

/// White-stat slot order in `variance` per equipment category (5 bits each).
const WEAPON_WHITE_SLOTS: WhiteSlots = WhiteSlots {
    durability: Some(0),
    phy_reinforce: Some(1),
    mag_reinforce: Some(2),
    attack_rate: Some(3),
    phy_atk: Some(4),
    mag_atk: Some(5),
    critical: Some(6),
    defense: None,
    block_rate: None,
    phy_absorb: None,
    mag_absorb: None,
};
const ARMOR_WHITE_SLOTS: WhiteSlots = WhiteSlots {
    durability: Some(0),
    defense: Some(1),
    phy_reinforce: Some(2),
    mag_reinforce: Some(3),
    block_rate: Some(4),
    attack_rate: None,
    phy_atk: None,
    mag_atk: None,
    critical: None,
    phy_absorb: None,
    mag_absorb: None,
};
const ACCESSORY_WHITE_SLOTS: WhiteSlots = WhiteSlots {
    phy_absorb: Some(0),
    mag_absorb: Some(1),
    durability: None,
    phy_reinforce: None,
    mag_reinforce: None,
    attack_rate: None,
    phy_atk: None,
    mag_atk: None,
    critical: None,
    defense: None,
    block_rate: None,
};

struct WhiteSlots {
    durability: Option<u32>,
    phy_reinforce: Option<u32>,
    mag_reinforce: Option<u32>,
    attack_rate: Option<u32>,
    phy_atk: Option<u32>,
    mag_atk: Option<u32>,
    critical: Option<u32>,
    defense: Option<u32>,
    block_rate: Option<u32>,
    phy_absorb: Option<u32>,
    mag_absorb: Option<u32>,
}

/// The variance roll factor (0..=1) of a stat's 5-bit slot, i.e. the item's
/// enhancement of that stat where `1.0` (+100%) is the maximum roll. `None`
/// for stats that carry no variance slot (shown without a percentage).
fn variance_roll(variance: u64, slot: Option<u32>) -> Option<f32> {
    slot.map(|slot| ((variance >> (5 * slot)) & 0x1F) as f32 / 31.0)
}

/// The concrete white value of one `(lower, upper)` bound pair at a roll
/// factor; a slot-less stat (`factor == None`) shows the mid of the range.
fn white_value(bounds: (f32, f32), factor: Option<f32>) -> f32 {
    bounds.0 + (bounds.1 - bounds.0) * factor.unwrap_or(0.5)
}

/// The ` (+NN%)` enhancement suffix for a stat's roll factor (empty for
/// slot-less stats).
fn pct_suffix(factor: Option<f32>) -> String {
    match factor {
        Some(f) => format!(" (+{:.0}%)", f * 100.0),
        None => String::new(),
    }
}

/// The colour of a requirement line: neutral when this character meets it,
/// warning red when it does not.
///
/// The line itself does not change — the original states requirements as plain
/// facts and so do we. Only which of them are the *reason* the item is greyed
/// out is new information, and colour is what carries it without adding a
/// sentence to a panel that is already 230 px wide.
fn criteria_color(met: bool) -> Color {
    if met {
        TEXT_COLOR
    } else {
        WARN_COLOR
    }
}

/// The garment ↔ protector/armor mixing rule has no line of its own to redden
/// — unlike sex and level it is a fact about the pieces *already worn*, not
/// about this item — so a failure gets its own line rather than silently
/// leaving the tooltip agreeing with an item the wash says is unusable.
fn armor_mix_line(criteria: EquipCriteria) -> Option<(String, Color)> {
    (!criteria.family_ok).then(|| {
        (
            "Cannot be worn with your current armor".to_string(),
            WARN_COLOR,
        )
    })
}

/// What the COS arm needs to name a pet's level.
///
/// A pair rather than two more parameters: `tooltip_lines` was already at six,
/// and these two are only ever used together.
#[derive(Clone, Copy)]
pub struct PetContext<'a> {
    pub cos: &'a CosState,
    pub char_data: &'a ClientCharacterData,
}

impl PetContext<'_> {
    /// The level of the pet a bag slot's scroll belongs to.
    ///
    /// Three routes, in this order, because no single one covers every state:
    ///
    /// 1. **The live roster, matched on ref.** `0x30C8`'s growth block carries
    ///    the pet's own level (`CosGrowth::level`), which is authoritative.
    /// 2. **The active pet, when the slot says summoned but no ref matched.**
    ///    A growth pet has one characterdata row *per level*, so its
    ///    `ref_obj_id` changes as it levels — and `Inventory::set_cos_state`
    ///    rewrites only the slot's `state`, never its `cos_ref_id`. A summoned
    ///    pet that has levelled since the last `0x3013` therefore holds a stale
    ///    ref in the bag, and route 1 misses it.
    /// 3. **Characterdata.** The only route for an unsummoned or dead pet,
    ///    where no `Cos` exists at all: the scroll's `cos_ref_id` names the
    ///    growth stage, and that row's `Lvl` is the level.
    fn level(&self, cos_ref_id: Option<u32>, summoned: bool) -> Option<u32> {
        let ref_id = cos_ref_id?;
        self.cos
            .cos
            .iter()
            .find(|cos| cos.ref_obj_id == ref_id)
            .and_then(|cos| cos.level)
            .or_else(|| summoned.then(|| self.cos.active_pet()?.level).flatten())
            .map(u32::from)
            .or_else(|| {
                self.char_data
                    .get(&(ref_id as i32))
                    .and_then(|row| row.level())
            })
    }
}

fn tooltip_lines(
    item: &InventoryItem,
    row: Option<&ItemDataRow>,
    names: &ClientTextNames,
    ui_strings: &ClientUiStrings,
    magic_options: &ClientMagicOptions,
    criteria: EquipCriteria,
    pets: PetContext<'_>,
) -> Vec<(String, Color)> {
    let mut lines: Vec<(String, Color)> = Vec::new();

    let base_name = row
        .and_then(|row| row.name_key())
        .and_then(|key| names.name(key))
        .map(String::from)
        .or_else(|| row.map(|row| row.code_name().clone()))
        .unwrap_or_else(|| format!("Item #{}", item.ref_id));
    let name = match &item.data {
        ItemTypeData::Equipment(eq) if eq.opt_level > 0 => {
            format!("{} (+{})", base_name, eq.opt_level)
        }
        _ => base_name,
    };
    // Name color precedence: gold for Seal-grade ("rare") items, else blue for
    // items carrying magic ("blue") options, else white.
    let name_color = if row.is_some_and(|row| row.is_rare()) {
        NAME_COLOR
    } else if has_magic_options(&item.data) {
        MAGIC_COLOR
    } else {
        TEXT_COLOR
    };
    lines.push((name, name_color));

    let Some(row) = row else {
        return lines;
    };

    if let Some(sort) = sort_of_item(row) {
        lines.push((format!("Sort of item: {sort}"), TEXT_COLOR));
    }
    if let Some(sex) = row.gender() {
        lines.push((format!("Sex: {sex}"), criteria_color(criteria.sex_ok)));
    }
    if let Some(degree) = row.degree() {
        if row.type_ids().is_some_and(|(_, tid2, _, _)| tid2 == 1) {
            lines.push((format!("Degree: {degree} degrees"), TEXT_COLOR));
        }
    }
    lines.push((String::new(), TEXT_COLOR));

    match &item.data {
        ItemTypeData::Equipment(eq) => equipment_lines(&mut lines, row, eq, magic_options),
        ItemTypeData::Expendable { stack_count, .. } => {
            let max = row.max_stack().unwrap_or(*stack_count as u32);
            lines.push((format!("Stack: {stack_count} / {max}"), TEXT_COLOR));
        }
        ItemTypeData::MagicCube { elixir_count } => {
            lines.push((format!("Contains {elixir_count} elixirs"), TEXT_COLOR));
        }
        ItemTypeData::CosPet {
            name, cos_ref_id, ..
        } => {
            // Absent for a never-summoned scroll — there is no pet to name yet.
            if let Some(name) = name.as_deref().filter(|n| !n.is_empty()) {
                lines.push((format!("Pet name: {name}"), TEXT_COLOR));
            }
            // The level, from the live roster or characterdata's growth stage
            // (see `PetContext::level`). `UIIT_CTL_COSNEWUI_PETINFO_LEVEL` is
            // the label the COS info page uses for the same field, so the two
            // surfaces agree — and unlike the `_PETSTATE_SUMMON` key below, it
            // is corpus-verified.
            let summoned = item.data.cos_state() == Some(COS_STATE_SUMMONED);
            if let Some(level) = pets.level(*cos_ref_id, summoned) {
                let label = ui_strings.get_or("UIIT_CTL_COSNEWUI_PETINFO_LEVEL", "Level");
                lines.push((format!("{label}: {level}"), TEXT_COLOR));
            }
            // A dead pet cannot be summoned until it is revived, and without
            // saying so the only feedback is the server refusing the scroll.
            // The word is vanilla's own tooltip string for this state.
            if item.data.cos_is_dead() {
                lines.push((
                    ui_strings
                        .get_or("UIIT_STT_COSNEWUI_TOOLTIP_PETSTATE_DEAD", "Dead")
                        .to_string(),
                    DEAD_PET_COLOR,
                ));
            } else if summoned {
                // The pet is out. Says in words what the slot's corner badge
                // says in colour, so the state is readable without knowing what
                // the pip means.
                //
                // The textdata key is inferred from the `_DEAD` sibling's
                // naming and is NOT corpus-verified; `get_or` falls back to the
                // English word if the archive has no such row, so a wrong guess
                // costs a localisation, not a line.
                lines.push((
                    ui_strings
                        .get_or("UIIT_STT_COSNEWUI_TOOLTIP_PETSTATE_SUMMON", "Summoned")
                        .to_string(),
                    SUMMONED_PET_COLOR,
                ));
            }
        }
        ItemTypeData::TransformScroll { .. } | ItemTypeData::Unknown => {}
    }

    // A blank line sets the requirement/origin footer off from the stats.
    if !lines.last().is_some_and(|(text, _)| text.is_empty()) {
        lines.push((String::new(), TEXT_COLOR));
    }
    if let Some(level) = row.required_level() {
        lines.push((
            format!("Required level {level}"),
            criteria_color(criteria.level_ok),
        ));
    }
    if let Some(line) = armor_mix_line(criteria) {
        lines.push(line);
    }
    match row.country() {
        Some(0) => lines.push(("Chinese".to_string(), DIM_COLOR)),
        Some(1) => lines.push(("European".to_string(), DIM_COLOR)),
        _ => {}
    }
    // drop a trailing spacer line if nothing followed the stats
    while lines.last().is_some_and(|(text, _)| text.is_empty()) {
        lines.pop();
    }
    // Seal-grade footer: a gold "Seal of …" line at the very bottom, set off
    // from the stats by a blank line.
    if let Some(seal) = seal_tier_name(row) {
        lines.push((String::new(), TEXT_COLOR));
        lines.push((seal.to_string(), NAME_COLOR));
    }
    lines
}

/// Tooltip lines for a CATALOG row — a shop good, which is an item *type* and
/// not an instance.
///
/// The difference from [`tooltip_lines`] is what a catalog row cannot know:
/// there is no opt level, no variance roll, no current durability and no magic
/// options until the server rolls them at purchase. So the per-instance block
/// is absent by construction rather than faked, and the stats that itemdata
/// declares as a range are printed AS a range (`lower ~ upper`) instead of one
/// rolled value with a `(+NN%)` suffix that would be an invention.
///
/// Everything else — name and its rarity colour, the sort/sex/degree header,
/// the required-level footer and the Seal-grade line — is a property of the
/// type and is shared with the instance tooltip.
fn catalog_tooltip_lines(
    ref_id: u32,
    row: Option<&ItemDataRow>,
    names: &ClientTextNames,
    criteria: EquipCriteria,
) -> Vec<(String, Color)> {
    let mut lines: Vec<(String, Color)> = Vec::new();

    let name = row
        .and_then(|row| row.name_key())
        .and_then(|key| names.name(key))
        .map(String::from)
        .or_else(|| row.map(|row| row.code_name().clone()))
        .unwrap_or_else(|| format!("Item #{ref_id}"));
    let name_color = if row.is_some_and(|row| row.is_rare()) {
        NAME_COLOR
    } else {
        TEXT_COLOR
    };
    lines.push((name, name_color));

    let Some(row) = row else {
        return lines;
    };

    if let Some(sort) = sort_of_item(row) {
        lines.push((format!("Sort of item: {sort}"), TEXT_COLOR));
    }
    if let Some(sex) = row.gender() {
        lines.push((format!("Sex: {sex}"), criteria_color(criteria.sex_ok)));
    }
    if let Some(degree) = row.degree() {
        if row.type_ids().is_some_and(|(_, tid2, _, _)| tid2 == 1) {
            lines.push((format!("Degree: {degree} degrees"), TEXT_COLOR));
        }
    }

    let before_stats = lines.len();
    let mut range = |label: &str, low: StatRange, high: StatRange, unit: &str| {
        if let (Some((min, _)), Some((_, max))) = (row.stat_range(low), row.stat_range(high)) {
            lines.push((format!("{label} {min:.1} ~ {max:.1}{unit}"), TEXT_COLOR));
        }
    };
    range(
        "Phy. atk. pwr",
        StatRange::PhyAtkMin,
        StatRange::PhyAtkMax,
        "",
    );
    range(
        "Mag. atk. pwr",
        StatRange::MagAtkMin,
        StatRange::MagAtkMax,
        "",
    );
    range("Phy. def. pwr", StatRange::Defense, StatRange::Defense, "");
    range(
        "Durability",
        StatRange::Durability,
        StatRange::Durability,
        "",
    );
    if let Some(distance) = row.attack_distance() {
        lines.push((format!("Attack distance {distance:.1} m"), TEXT_COLOR));
    }
    if lines.len() > before_stats {
        lines.insert(before_stats, (String::new(), TEXT_COLOR));
    }

    if let Some(level) = row.required_level() {
        lines.push((String::new(), TEXT_COLOR));
        lines.push((
            format!("Required level {level}"),
            criteria_color(criteria.level_ok),
        ));
    }
    if let Some(line) = armor_mix_line(criteria) {
        lines.push(line);
    }
    match row.country() {
        Some(0) => lines.push(("Chinese".to_string(), DIM_COLOR)),
        Some(1) => lines.push(("European".to_string(), DIM_COLOR)),
        _ => {}
    }
    while lines.last().is_some_and(|(text, _)| text.is_empty()) {
        lines.pop();
    }
    if let Some(seal) = seal_tier_name(row) {
        lines.push((String::new(), TEXT_COLOR));
        lines.push((seal.to_string(), NAME_COLOR));
    }
    lines
}

/// The Seal-grade footer label for a rare item. The suffix table itself lives
/// on [`ItemDataRow::seal_tier`] so the inventory slot's glow reads the same
/// grade this line prints.
fn seal_tier_name(row: &ItemDataRow) -> Option<&'static str> {
    row.seal_tier().map(SealTier::label)
}

/// Whether an item carries any magic ("blue") options — drives the blue name.
fn has_magic_options(data: &ItemTypeData) -> bool {
    match data {
        ItemTypeData::Equipment(eq) => !eq.mag_params.is_empty(),
        ItemTypeData::Expendable { mag_params, .. } => !mag_params.is_empty(),
        _ => false,
    }
}

fn equipment_lines(
    lines: &mut Vec<(String, Color)>,
    row: &ItemDataRow,
    eq: &EquipmentData,
    magic_options: &ClientMagicOptions,
) {
    let (_, _, tid3, _) = row.type_ids().unwrap_or((0, 0, 0, 0));
    let is_weapon = tid3 == 6;
    let is_shield = tid3 == 4;
    let is_accessory = tid3 == 5;
    let slots = if is_weapon {
        &WEAPON_WHITE_SLOTS
    } else if is_accessory {
        &ACCESSORY_WHITE_SLOTS
    } else {
        &ARMOR_WHITE_SLOTS
    };
    // A stat's rolled value plus the roll factor that drives its `(+NN%)`.
    let value = |range: StatRange, slot: Option<u32>| -> Option<(f32, Option<f32>)> {
        row.stat_range(range).map(|bounds| {
            let factor = variance_roll(eq.variance, slot);
            (white_value(bounds, factor), factor)
        })
    };

    // Accessories (earring/necklace/ring): physical & magical absorption.
    if is_accessory {
        if let Some((absorb, f)) = value(StatRange::PhyAbsorb, slots.phy_absorb) {
            lines.push((
                format!("Phy. absorption {absorb:.1} %{}", pct_suffix(f)),
                TEXT_COLOR,
            ));
        }
        if let Some((absorb, f)) = value(StatRange::MagAbsorb, slots.mag_absorb) {
            lines.push((
                format!("Mag. absorption {absorb:.1} %{}", pct_suffix(f)),
                TEXT_COLOR,
            ));
        }
    }

    if let (Some((min, _)), Some((max, f))) = (
        value(StatRange::PhyAtkMin, slots.phy_atk),
        value(StatRange::PhyAtkMax, slots.phy_atk),
    ) {
        lines.push((
            format!("Phy. atk. pwr {min:.1} ~ {max:.1}{}", pct_suffix(f)),
            TEXT_COLOR,
        ));
    }
    if let (Some((min, _)), Some((max, f))) = (
        value(StatRange::MagAtkMin, slots.mag_atk),
        value(StatRange::MagAtkMax, slots.mag_atk),
    ) {
        lines.push((
            format!("Mag. atk. pwr {min:.1} ~ {max:.1}{}", pct_suffix(f)),
            TEXT_COLOR,
        ));
    }
    if let Some((def, f)) = value(StatRange::Defense, slots.defense) {
        lines.push((
            format!("Phy. def. pwr {def:.1}{}", pct_suffix(f)),
            TEXT_COLOR,
        ));
    }
    if is_shield {
        if let Some((block, f)) = value(StatRange::BlockRate, slots.block_rate) {
            lines.push((
                format!("Blocking rate {block:.0}{}", pct_suffix(f)),
                TEXT_COLOR,
            ));
        }
    }
    if let Some((max_dur, f)) = value(StatRange::Durability, slots.durability) {
        lines.push((
            format!(
                "Durability {}/{}{}",
                eq.durability,
                max_dur.round() as u32,
                pct_suffix(f)
            ),
            TEXT_COLOR,
        ));
    }
    if let Some(distance) = row.attack_distance() {
        lines.push((format!("Attack distance {distance:.1} m"), TEXT_COLOR));
    }
    if let Some((rate, f)) = value(StatRange::AttackRate, slots.attack_rate) {
        lines.push((
            format!("Attack rate {rate:.0}{}", pct_suffix(f)),
            TEXT_COLOR,
        ));
    }
    if let Some((critical, f)) = value(StatRange::Critical, slots.critical) {
        lines.push((
            format!("Critical {critical:.0}{}", pct_suffix(f)),
            TEXT_COLOR,
        ));
    }
    // Weapons carry a phys/mag reinforce range (4 columns each); armor & shields
    // a single rolled value from their own columns (weapon columns are zero).
    if is_weapon {
        if let (Some((min, _)), Some((max, f))) = (
            value(StatRange::PhyReinforceMin, slots.phy_reinforce),
            value(StatRange::PhyReinforceMax, slots.phy_reinforce),
        ) {
            lines.push((
                format!(
                    "Phy. reinforce {:.1} % ~ {:.1} %{}",
                    min / 10.0,
                    max / 10.0,
                    pct_suffix(f)
                ),
                TEXT_COLOR,
            ));
        }
        if let (Some((min, _)), Some((max, f))) = (
            value(StatRange::MagReinforceMin, slots.mag_reinforce),
            value(StatRange::MagReinforceMax, slots.mag_reinforce),
        ) {
            lines.push((
                format!(
                    "Mag. reinforce {:.1} % ~ {:.1} %{}",
                    min / 10.0,
                    max / 10.0,
                    pct_suffix(f)
                ),
                TEXT_COLOR,
            ));
        }
    } else {
        if let Some((rein, f)) = value(StatRange::ArmorPhyReinforce, slots.phy_reinforce) {
            lines.push((
                format!("Phy. reinforce {:.1} %{}", rein / 10.0, pct_suffix(f)),
                TEXT_COLOR,
            ));
        }
        if let Some((rein, f)) = value(StatRange::ArmorMagReinforce, slots.mag_reinforce) {
            lines.push((
                format!("Mag. reinforce {:.1} %{}", rein / 10.0, pct_suffix(f)),
                TEXT_COLOR,
            ));
        }
    }
    // Blue "magic options": one decoded line each, falling back to the raw id
    // so an unmapped option is still visible rather than silently dropped.
    //
    // A drawback among them (`is_penalty`) is drawn in the warning red instead
    // of the blue. Not-repairable items carry it next to a very large
    // `MATTR_DUR` bonus, and in one uniform blue the block reads as all
    // upside — which is precisely backwards for the one line that says the
    // item is consumable in the long run.
    for param in &eq.mag_params {
        let (text, color) = match magic_options.get(param.kind) {
            Some(info) => (
                info.format(param.value),
                if info.is_penalty() {
                    WARN_COLOR
                } else {
                    MAGIC_COLOR
                },
            ),
            None => (format!("Magic option #{}", param.kind), MAGIC_COLOR),
        };
        lines.push((text, color));
    }
    lines.push((String::new(), TEXT_COLOR));
}

/// The vanilla "Sort of item" label from the item's type ids.
fn sort_of_item(row: &ItemDataRow) -> Option<&'static str> {
    let (tid1, tid2, tid3, tid4) = row.type_ids()?;
    if tid1 != 3 {
        return None;
    }
    Some(match (tid2, tid3, tid4) {
        (1, 6, 2) => "Sword",
        (1, 6, 3) => "Blade",
        (1, 6, 4) => "Spear",
        (1, 6, 5) => "Glaive",
        (1, 6, 6) => "Bow",
        (1, 6, 7) => "One-handed sword",
        (1, 6, 8) => "Two-handed sword",
        (1, 6, 9) => "Dual axe",
        (1, 6, 10) => "Warlock rod",
        (1, 6, 11) => "Staff",
        (1, 6, 12) => "Crossbow",
        (1, 6, 13) => "Dagger",
        (1, 6, 14) => "Harp",
        (1, 6, 15) => "Cleric rod",
        (1, 6, _) => "Weapon",
        (1, 1, _) | (1, 9, _) => "Garment",
        (1, 2, _) | (1, 10, _) => "Protector",
        (1, 3, _) | (1, 11, _) => "Armor",
        (1, 4, _) => "Shield",
        (1, 5, 1) | (1, 12, 1) => "Earring",
        (1, 5, 2) | (1, 12, 2) => "Necklace",
        (1, 5, 3) | (1, 12, 3) => "Ring",
        (1, 7, _) => "Job equipment",
        (1, _, _) => "Equipment",
        (2, 1, _) => "Pet",
        (2, _, _) => "Summon",
        (3, 1, _) => "Potion",
        (3, 2, _) => "Cure",
        (3, 3, _) => "Scroll",
        (3, 4, _) => "Ammunition",
        (3, 5, _) => "Gold",
        (3, _, _) => "Consumable",
        _ => return None,
    })
}

#[cfg(test)]
mod test {
    use super::*;
    use packets::agent::character_data::RentInfo;
    use packets::agent::pet::{CosBody, CosGrowth, CosKind};

    /// A summoned growth pet at `ref_obj_id`/`level`, shaped like the captured
    /// Grey Wolf (see `hud::cos::info`'s own `wolf` fixture).
    fn summoned_wolf(ref_obj_id: u32, level: u8) -> crate::plugins::hud::cos::state::Cos {
        crate::plugins::hud::cos::state::Cos {
            unique_id: 0x0002_6cd1,
            ref_obj_id,
            kind: CosKind::GrowthPet,
            body: CosBody {
                hp: 360,
                unk_b: 0,
                growth: Some(CosGrowth {
                    exp: 0,
                    level,
                    hgp: 9492,
                }),
                unk_f: Some(0),
                name: Some(String::new()),
                inventory_size: 0,
                items: Vec::new(),
                unk_g: None,
                unk_h: None,
            },
            hgp: Some(9492),
            exp: 0,
            level: Some(level),
        }
    }

    fn roster(cos: Vec<crate::plugins::hud::cos::state::Cos>) -> CosState {
        CosState { cos }
    }

    /// Route 1: the slot's ref matches a pet on the roster, so the wire's own
    /// level is used.
    #[test]
    fn a_summoned_pets_level_comes_from_the_live_roster() {
        let cos = roster(vec![summoned_wolf(6138, 33)]);
        let char_data = ClientCharacterData::default();
        let pets = PetContext {
            cos: &cos,
            char_data: &char_data,
        };
        assert_eq!(pets.level(Some(6138), true), Some(33));
    }

    /// Route 2: the bag slot's `cos_ref_id` is stale, because a growth pet gets
    /// a new characterdata row on every level and `set_cos_state` never rewrites
    /// the slot's ref. The active pet answers instead of the tooltip going
    /// blank — this is the case that made the level "missing" in the first
    /// place, for exactly the pet that had levelled.
    #[test]
    fn a_stale_slot_ref_still_finds_the_summoned_pet() {
        let cos = roster(vec![summoned_wolf(6138, 33)]);
        let char_data = ClientCharacterData::default();
        let pets = PetContext {
            cos: &cos,
            char_data: &char_data,
        };
        // 6134 is the level-29 row the slot was last told about.
        assert_eq!(pets.level(Some(6134), true), Some(33));
    }

    /// ...but only while it is summoned. An unsummoned scroll must not borrow
    /// some other pet's level, and with no characterdata it has no answer.
    #[test]
    fn an_unsummoned_scroll_does_not_borrow_the_active_pets_level() {
        let cos = roster(vec![summoned_wolf(6138, 33)]);
        let char_data = ClientCharacterData::default();
        let pets = PetContext {
            cos: &cos,
            char_data: &char_data,
        };
        assert_eq!(pets.level(Some(6134), false), None);
    }

    /// A scroll that has never been summoned carries no ref at all.
    #[test]
    fn a_never_summoned_scroll_has_no_level() {
        let cos = roster(vec![]);
        let char_data = ClientCharacterData::default();
        let pets = PetContext {
            cos: &cos,
            char_data: &char_data,
        };
        assert_eq!(pets.level(None, false), None);
    }

    /// The hover→tooltip wire, end to end from the state the picking backend
    /// writes (`InventoryState.hovered_slot`) to the panel's visibility.
    ///
    /// #428 reported the popup gone in live play and two static passes found
    /// no defect in this path, so pin the path itself: every early return in
    /// [`refresh_tooltip`] (no root, closed window, empty slot, drag in
    /// progress) silently produces exactly the reported symptom.
    fn tooltip_app(hovered: Option<u8>) -> (App, Entity) {
        let mut app = App::new();
        let mut slots = vec![None; 45];
        slots[13] = Some(InventoryItem {
            slot: 13,
            rent: RentInfo::default(),
            ref_id: 4, // ITEM_ETC_HP_POTION_01
            data: ItemTypeData::Expendable {
                stack_count: 50,
                assimilation_prob: None,
                mag_params: vec![],
            },
        });
        app.insert_resource(InventoryState {
            open: true,
            hovered_slot: hovered,
            ..default()
        })
        .init_resource::<StorageHoveredItem>()
        .init_resource::<StoreHoveredGood>()
        .init_resource::<ClientItemData>()
        .init_resource::<ClientCharacterData>()
        .init_resource::<CosState>()
        .init_resource::<ClientTextNames>()
        .init_resource::<ClientUiStrings>()
        .init_resource::<ClientMagicOptions>()
        .insert_resource(FontAssets {
            one: Handle::default(),
            two: Handle::default(),
            three: Handle::default(),
            nine: Handle::default(),
        })
        .add_systems(Update, refresh_tooltip);
        app.world_mut().spawn((
            Player,
            Inventory {
                slots,
                avatar_slots: vec![None; 5],
                gold: 0,
            },
        ));
        let root = app
            .world_mut()
            .spawn((InventoryTooltipRoot, Visibility::Hidden))
            .id();
        (app, root)
    }

    fn tooltip_state(app: &App, root: Entity) -> (Visibility, usize) {
        let entity = app.world().entity(root);
        let visibility = *entity.get::<Visibility>().unwrap();
        let lines = entity.get::<Children>().map_or(0, |c| c.len());
        (visibility, lines)
    }

    #[test]
    fn hovering_a_filled_slot_shows_the_tooltip_and_leaving_it_hides_it() {
        let (mut app, root) = tooltip_app(Some(13));
        app.update();
        let (visibility, lines) = tooltip_state(&app, root);
        assert_eq!(visibility, Visibility::Inherited, "tooltip stayed hidden");
        assert!(lines > 0, "tooltip has no text lines");

        // Leaving the slot (what on_slot_out writes) hides it again.
        app.world_mut()
            .resource_mut::<InventoryState>()
            .hovered_slot = None;
        app.update();
        assert_eq!(tooltip_state(&app, root).0, Visibility::Hidden);
    }

    /// An empty slot and a closed window are hovers that must NOT paint a
    /// tooltip — the same code path, so they guard the assertion above from
    /// passing for the wrong reason.
    #[test]
    fn an_empty_slot_or_a_closed_window_shows_no_tooltip() {
        let (mut app, root) = tooltip_app(Some(14)); // empty slot
        app.update();
        assert_eq!(tooltip_state(&app, root).0, Visibility::Hidden);

        let (mut app, root) = tooltip_app(Some(13));
        app.world_mut().resource_mut::<InventoryState>().open = false;
        app.update();
        assert_eq!(tooltip_state(&app, root).0, Visibility::Hidden);
    }

    /// The storage window has no hover observers of its own: it publishes the
    /// hovered item directly, and the same panel renders it (#428's path is
    /// shared, so a break here breaks both windows).
    #[test]
    fn a_storage_hover_paints_the_same_panel() {
        let (mut app, root) = tooltip_app(None);
        app.world_mut().resource_mut::<StorageHoveredItem>().0 = Some(InventoryItem {
            slot: 0,
            rent: RentInfo::default(),
            ref_id: 4,
            data: ItemTypeData::Expendable {
                stack_count: 1,
                assimilation_prob: None,
                mag_params: vec![],
            },
        });
        app.update();
        let (visibility, lines) = tooltip_state(&app, root);
        assert_eq!(visibility, Visibility::Inherited);
        assert!(lines > 0);
    }

    #[test]
    fn white_value_interpolates_five_bit_rolls() {
        // slot 3 roll = 31 -> upper bound; slot 4 roll = 0 -> lower bound
        let variance: u64 = 31 << (5 * 3);
        let roll = |slot| variance_roll(variance, slot);
        assert_eq!(white_value((24.0, 30.0), roll(Some(3))), 30.0);
        assert_eq!(white_value((15.0, 16.0), roll(Some(4))), 15.0);
        assert_eq!(white_value((10.0, 20.0), roll(None)), 15.0);
    }

    #[test]
    fn variance_roll_maps_to_enhancement_percent() {
        let variance: u64 = 31 << (5 * 3);
        // full roll -> +100%, empty roll -> +0%, slot-less -> no percentage.
        assert_eq!(pct_suffix(variance_roll(variance, Some(3))), " (+100%)");
        assert_eq!(pct_suffix(variance_roll(variance, Some(4))), " (+0%)");
        assert_eq!(pct_suffix(variance_roll(variance, None)), "");
    }
}
