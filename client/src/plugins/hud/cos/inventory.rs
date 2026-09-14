//! COS inventory page — `resinfo/ifcosinventory.txt`, the second of the
//! shell's three pages (`GGDR_COS_INVENTORY:CIFCOSInventory`, id 122).
//!
//! Idea: this is the pick pet's bag, and it is the *same* lattice the player's
//! own inventory draws — `com_lattice_` quarters on a fixed grid — so the art
//! and the cell anatomy come straight from `hud::inventory::ui` rather than
//! being rebuilt. What differs is only the grid's shape and where the items
//! come from: 7 columns × 4 rows here against the inventory's 4 × 8.
//!
//! **Geometry is transcribed, not designed** (`docs/re/ui/cos-pet-window.md`
//! §3, `docs/re/systems/pet-growth-cos.md`): the lattice is `41,59,252,144`
//! inside an `opt_inner_box_` at `21,27,290,235`, the pet's name sits at
//! `49,30` and the page spinner at `140,271,52,18`. 252/7 and 144/4 are both
//! exactly 36, the shared cell size — which is the check that the 7×4 reading
//! is right, and it lands on [`COS_INVENTORY_PAGE_SLOTS`] (28), the page size
//! the original's own slot math uses.
//!
//! **Capacity is not 28.** The grid is one *page*; how many slots actually
//! exist is `0x30C8`'s `inventory_size` byte (growth pets 0, ability pets up
//! to 140), so cells past the pet's capacity render dimmed and refuse drops.

use bevy::prelude::*;

use packets::agent::character_data::ItemTypeData;
use packets::agent::inventory::InventoryOperationRequest;
use packets::agent::pet::{CosKind, COS_INVENTORY_PAGE_SLOTS};

use crate::assets::FontAssets;
use crate::plugins::hud::cos::state::CosState;
use crate::plugins::hud::game_window::abs_node;

/// `CIFLattice com_lattice_` — the 7×4 grid's rect inside the page.
const LATTICE_RECT: (f32, f32, f32, f32) = (41.0, 59.0, 252.0, 144.0);
/// `opt_inner_box_` — the sunken panel the lattice sits in.
const INNER_BOX_RECT: (f32, f32, f32, f32) = (21.0, 27.0, 290.0, 235.0);
/// The pet's name, above the box.
const NAME_RECT: (f32, f32, f32, f32) = (49.0, 30.0, 240.0, 14.0);
/// The page spinner (`< 1/5 >`), below the box.
const SPINNER_RECT: (f32, f32, f32, f32) = (140.0, 271.0, 52.0, 18.0);

const GRID_COLS: u8 = 7;
const GRID_ROWS: u8 = 4;
/// Shared with the player's inventory — and the reason 252×144 divides evenly.
const CELL: f32 = 36.0;
const ICON_INSET: f32 = 2.0;
const ICON_SIZE: f32 = 28.0;

const LATTICE_DIR: &str = "media://interface/ifcommon/lattice_window/com_lattice_";
/// The sunken panel is an 8-piece kit under `interface/option/` — the same one
/// [`super::setup`] draws. This used to load a single
/// `ifcommon/opt_inner_box_middle.ddj`, which is neither the folder the kit
/// lives in nor a file that exists in the archive at all, so the panel drew
/// nothing and logged a missing asset on every open.
const INNER_BOX_DIR: &str = "media://interface/option/opt_inner_box_";
const INNER_BOX_PIECE: f32 = 4.0;

/// Colour of a cell past the pet's `inventory_size` — the slot exists on the
/// page but not in the bag.
const BEYOND_CAPACITY_TINT: Color = Color::srgba(0.0, 0.0, 0.0, 0.45);

/// One cell of the pet bag, indexed within the current page.
#[derive(Component, Clone, Copy)]
pub struct CosGridCell {
    pub index: u8,
}

/// The icon inside a [`CosGridCell`].
#[derive(Component, Clone, Copy)]
pub struct CosSlotIcon;

/// The stack-count label inside a [`CosGridCell`].
#[derive(Component, Clone, Copy)]
pub struct CosSlotCount;

/// The dimming overlay for cells past capacity.
#[derive(Component, Clone, Copy)]
pub struct CosSlotBlocked;

/// The pet-name static above the grid.
#[derive(Component, Clone, Copy)]
pub struct CosBagName;

/// Which page of the bag is shown; `inventory_size` decides how many exist.
#[derive(Resource, Default)]
pub struct CosBagPage(pub u8);

impl CosBagPage {
    /// How many pages a bag of `capacity` slots needs (at least one, so an
    /// empty growth-pet bag still draws its frame).
    pub fn page_count(capacity: u8) -> u8 {
        capacity.div_ceil(COS_INVENTORY_PAGE_SLOTS).max(1)
    }

    /// The absolute bag slot a cell on this page stands for.
    pub fn slot_of(&self, index: u8) -> u8 {
        self.0 * COS_INVENTORY_PAGE_SLOTS + index
    }
}

/// Build the page body into the shell's second container.
pub fn build_inventory_page(
    page: &mut ChildSpawnerCommands,
    asset_server: &AssetServer,
    fonts: &FontAssets,
    s: f32,
) {
    // The sunken panel behind the lattice, drawn as its own 8-piece ring.
    let (bx, by, bw, bh) = INNER_BOX_RECT;
    for ((x, y, w, h), piece) in super::setup::ring(bw, bh, INNER_BOX_PIECE) {
        page.spawn((
            abs_node((bx + x, by + y, w, h), s),
            ImageNode {
                image: asset_server.load(format!("{INNER_BOX_DIR}{piece}.ddj")),
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            Pickable::IGNORE,
        ));
    }

    // The pet's given name, which is the only label this page carries.
    page.spawn((
        CosBagName,
        Text::new(""),
        TextFont {
            font: fonts.two.clone().into(),
            font_size: FontSize::Px(8.0 * s),
            ..default()
        },
        TextColor(Color::WHITE),
        abs_node(NAME_RECT, s),
        Pickable::IGNORE,
    ));

    page.spawn((
        Node {
            display: Display::Grid,
            grid_template_columns: RepeatedGridTrack::px(GRID_COLS as u16, CELL * s),
            grid_template_rows: RepeatedGridTrack::px(GRID_ROWS as u16, CELL * s),
            ..abs_node(LATTICE_RECT, s)
        },
        Pickable::IGNORE,
    ))
    .with_children(|grid| {
        for index in 0..COS_INVENTORY_PAGE_SLOTS {
            let (row, col) = (index / GRID_COLS, index % GRID_COLS);
            // The lattice art tiles as quarters: the left_up tile carries the
            // inter-cell separator on its right/bottom edges, and the
            // right_*/_down variants terminate the grid — the same rule the
            // player's inventory follows, only at a different extent.
            let quarter = match (row == GRID_ROWS - 1, col == GRID_COLS - 1) {
                (false, false) => "left_up",
                (false, true) => "right_up",
                (true, false) => "left_down",
                (true, true) => "right_down",
            };
            grid.spawn((
                CosGridCell { index },
                Node {
                    width: Val::Px(CELL * s),
                    height: Val::Px(CELL * s),
                    ..default()
                },
                ImageNode {
                    image: asset_server.load(format!("{LATTICE_DIR}{quarter}.ddj")),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
            ))
            .with_children(|cell| {
                cell.spawn((
                    CosSlotIcon,
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(ICON_INSET * s),
                        top: Val::Px(ICON_INSET * s),
                        width: Val::Px(ICON_SIZE * s),
                        height: Val::Px(ICON_SIZE * s),
                        ..default()
                    },
                    ImageNode {
                        image: Handle::default(),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    Visibility::Hidden,
                    Pickable::IGNORE,
                ));
                cell.spawn((
                    CosSlotCount,
                    Text::new(""),
                    TextFont {
                        font: fonts.nine.clone().into(),
                        font_size: FontSize::Px(6.0 * s),
                        ..default()
                    },
                    TextColor(Color::WHITE),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(2.0 * s),
                        top: Val::Px(1.0 * s),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                cell.spawn((
                    CosSlotBlocked,
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(0.0),
                        top: Val::Px(0.0),
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    BackgroundColor(BEYOND_CAPACITY_TINT),
                    Visibility::Hidden,
                    Pickable::IGNORE,
                ));
            });
        }
    });

    // The spinner's own art is not transcribed yet — the rect is reserved so
    // the page's geometry stays whole and a later pass has somewhere to put
    // it. Multi-page bags (capacity > 28) are otherwise unreachable.
    let _ = SPINNER_RECT;
}

/// Mirror the pick pet's bag onto the grid.
///
/// Cells past `inventory_size` are dimmed rather than hidden: the lattice is a
/// fixed 7×4 page in the art, so a 12-slot bag still draws 28 wells and marks
/// the 16 that do not exist.
pub fn refresh_cos_inventory(
    state: Res<CosState>,
    page: Res<CosBagPage>,
    item_data: Res<crate::plugins::textdata::ClientItemData>,
    asset_server: Res<AssetServer>,
    cells: Query<(&CosGridCell, &Children)>,
    mut icons: Query<(&mut ImageNode, &mut Visibility), With<CosSlotIcon>>,
    mut counts: Query<&mut Text, With<CosSlotCount>>,
    mut blocked: Query<&mut Visibility, (With<CosSlotBlocked>, Without<CosSlotIcon>)>,
    mut names: Query<&mut Text, (With<CosBagName>, Without<CosSlotCount>)>,
) {
    if !state.is_changed() && !page.is_changed() {
        return;
    }
    let pet = state.first_of_kind(CosKind::GrabPet);
    let capacity = pet.map_or(0, |pet| pet.body.inventory_size);

    for mut name in names.iter_mut() {
        **name = pet
            .and_then(|pet| pet.name())
            .unwrap_or_default()
            .to_string();
    }

    for (cell, children) in cells.iter() {
        let slot = page.slot_of(cell.index);
        let item = pet.and_then(|pet| pet.bag_get(slot));
        let beyond = slot >= capacity;
        for child in children.iter() {
            if let Ok((mut icon, mut visibility)) = icons.get_mut(child) {
                match item {
                    Some(item) => {
                        if let Some(path) = item_data
                            .get(&(item.ref_id as i32))
                            .and_then(|row| row.icon_path())
                        {
                            icon.image = asset_server.load(path);
                            *visibility = Visibility::Inherited;
                        } else {
                            *visibility = Visibility::Hidden;
                        }
                    }
                    None => *visibility = Visibility::Hidden,
                }
            }
            if let Ok(mut text) = counts.get_mut(child) {
                // Only stackables carry a count, and a count of 1 is not
                // drawn — the same rule the player's own grid follows.
                **text = match item.map(|item| &item.data) {
                    Some(ItemTypeData::Expendable { stack_count, .. }) if *stack_count > 1 => {
                        stack_count.to_string()
                    }
                    _ => String::new(),
                };
            }
            if let Ok(mut visibility) = blocked.get_mut(child) {
                *visibility = if beyond {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                };
            }
        }
    }
}

/// Where a pet-bag transfer starts and ends. The three ops the pick pet
/// supports are exactly the three combinations of these two containers, minus
/// inventory→inventory (which is the ordinary 0x7034 move).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BagSide {
    Pet,
    Inventory,
}

/// Request a pet-bag transfer.
///
/// **Whole slots only**: ops 26/27 carry no quantity, so there is no split to
/// offer and none is sent. Op 25 does echo one, and it is filled from the
/// moved stack for symmetry with the ordinary move — the server relocates the
/// whole slot either way (`docs/re/systems/pet-pick-cos.md` §3).
///
/// The request bodies are `[S]` — see [`InventoryOperationRequest::PetToPet`].
pub fn pet_bag_transfer(
    cos_unique_id: u32,
    from: (BagSide, u8),
    to: (BagSide, u8),
    amount: u16,
) -> Option<InventoryOperationRequest> {
    match (from, to) {
        ((BagSide::Pet, source), (BagSide::Pet, target)) => {
            (source != target).then_some(InventoryOperationRequest::PetToPet {
                cos_unique_id,
                source,
                target,
                amount,
            })
        }
        ((BagSide::Pet, pet_slot), (BagSide::Inventory, inventory_slot)) => {
            Some(InventoryOperationRequest::PetToInventory {
                cos_unique_id,
                pet_slot,
                inventory_slot,
            })
        }
        ((BagSide::Inventory, inventory_slot), (BagSide::Pet, pet_slot)) => {
            Some(InventoryOperationRequest::InventoryToPet {
                cos_unique_id,
                inventory_slot,
                pet_slot,
            })
        }
        // Inventory to inventory is not a pet op at all.
        ((BagSide::Inventory, _), (BagSide::Inventory, _)) => None,
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The transcribed rect must divide into whole cells at the shared 36px
    /// size — that is what makes 7×4 the right reading of the lattice, and it
    /// is exactly the original's own page size.
    #[test]
    fn the_lattice_rect_is_exactly_seven_by_four_shared_cells() {
        let (_, _, w, h) = LATTICE_RECT;
        assert_eq!(w / CELL, GRID_COLS as f32);
        assert_eq!(h / CELL, GRID_ROWS as f32);
        assert_eq!(GRID_COLS * GRID_ROWS, COS_INVENTORY_PAGE_SLOTS);
    }

    /// The grid fits inside the panel it is drawn in, which fits the page.
    #[test]
    fn the_lattice_sits_inside_the_inner_box() {
        let (bx, by, bw, bh) = INNER_BOX_RECT;
        let (lx, ly, lw, lh) = LATTICE_RECT;
        assert!(lx >= bx && ly >= by);
        assert!(lx + lw <= bx + bw);
        assert!(ly + lh <= by + bh);
    }

    /// Each container pair maps to exactly one op, and the two that cross the
    /// boundary carry no quantity — ops 26/27 have no such field.
    #[test]
    fn each_container_pair_maps_to_its_own_op() {
        use BagSide::{Inventory, Pet};

        assert_eq!(
            pet_bag_transfer(11, (Pet, 1), (Pet, 2), 5),
            Some(InventoryOperationRequest::PetToPet {
                cos_unique_id: 11,
                source: 1,
                target: 2,
                amount: 5,
            })
        );
        assert_eq!(
            pet_bag_transfer(11, (Pet, 1), (Inventory, 13), 5),
            Some(InventoryOperationRequest::PetToInventory {
                cos_unique_id: 11,
                pet_slot: 1,
                inventory_slot: 13,
            })
        );
        assert_eq!(
            pet_bag_transfer(11, (Inventory, 13), (Pet, 1), 5),
            Some(InventoryOperationRequest::InventoryToPet {
                cos_unique_id: 11,
                inventory_slot: 13,
                pet_slot: 1,
            })
        );
        // Not pet ops: a plain inventory move, and a no-op onto itself.
        assert_eq!(
            pet_bag_transfer(11, (Inventory, 1), (Inventory, 2), 1),
            None
        );
        assert_eq!(pet_bag_transfer(11, (Pet, 1), (Pet, 1), 1), None);
    }

    /// Capacity is the wire's byte, not the page size: a 140-slot ability pet
    /// pages, a 0-slot growth pet still draws one empty page.
    #[test]
    fn pages_follow_capacity_not_the_grid() {
        assert_eq!(CosBagPage::page_count(0), 1);
        assert_eq!(CosBagPage::page_count(28), 1);
        assert_eq!(CosBagPage::page_count(29), 2);
        assert_eq!(CosBagPage::page_count(140), 5);

        let page = CosBagPage(2);
        assert_eq!(page.slot_of(0), 56);
        assert_eq!(page.slot_of(27), 83);
    }
}
