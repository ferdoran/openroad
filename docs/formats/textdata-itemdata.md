# textdata: itemdata_*.txt (RefObjCommon + RefObjItem) & magicoption.txt

Location: `Media.pk2/server_dep/silkroad/textdata/itemdata_<base>.txt` (split by
ID range; `itemdata.txt` lists the split files). UTF-16LE, tab-separated, one
row per item; column 0 (`Service`) is `1` when live. Keyed by column 1 (`ID`,
the item `ref_id`). Columns verified against real v1.188 data (0-based indices,
as used by `ItemdataFields` / `StatRange` in
`client/src/assets/textdata/itemdata.rs`).

**Field count: 160, not "~161" (round 2, gamedata.md G13).** The server's `ItemData_*.txt` has
**160** fields and no trailing tab; the client's `itemdata_*.txt` has the same 160 fields **plus a
trailing tab**, which naive tab-splitting reports as 161. Split to 160 and ignore the empty tail.
(The same trailing-tab artefact inflates characterdata to 105 and skilldata to 119; the real widths
are 104 and 118.)

**Two shipping columns this doc does not list.** `PhyAbsorb`/`MagAbsorb` at cols **71/72** and
**79/80** *are* parsed (`client/src/assets/textdata/itemdata.rs:64,86`) and rendered — re-grepped
at HEAD, still absent from the table below. A doc gap on a live path.

## Scalar columns

| col | field | notes |
|-----|-------|-------|
| 0 | Service | 1 = active row |
| 1 | ID | key (`ref_id`) |
| 2 | CodeName | `ITEM_CH_SWORD_01_A`; `*_RARE` suffix = Seal-grade ("rare") item |
| 5 | NameStrID | `SN_*` key → textdataname (localized display name) |
| 9–12 | TypeID1–4 | `3/1/tid3/tid4` = equipment. tid3: 1/2/3 armor, 4 shield, 5 accessory, 6 weapon. `3/3/tid3/tid4` = expendable, of which `3/3/4/*` is **ammunition** (tid4 1 = arrow, 2 = bolt) — the one expendable that is also equipped, wearing the same secondary slot as the shield. Corpus-verified: the `3/3/4/*` bucket is exactly 8 rows across all 10 `itemdata*.txt`, all ammo |
| 14 | Country | 0 = Chinese, 1 = European |
| 17 | CanSell | 1 = sellable to NPCs (0 on mall/quest items) — gates the sell modal |
| 26 | Price | NPC **buy** price; `refpricepolicyofitem` mirrors it for shop packages (sword 890) |
| 27/28 | CostRepair / CostRevive | documented, not parsed (revive = 1.5× repair on every sampled row) |
| 31 | SellPrice | authored per-unit NPC **sell** value (sword 890→427, HP potion 60→21; ratio floors at 0.35 by degree) |
| 33 | ReqLevel1 | level required to equip |
| 52/53/54 | Resource / Drop / Icon path | `.bsr` / `.bsr` / `.ddj` |
| 57 | MaxStack | 1 for unstackables |
| 58 | **Sex** | **0 = Woman, 1 = Man, 2 = Universal** (weapons & shields are always 2) |
| 61 | ItemClass | degree = `ceil(class/3)`. **`[BIN]`**, no longer an inherited label: `FUN_006a3d50` asserts `m_btItemClass > 0` (`ReferenceData.cpp:484`), and the corpus is 11,923 / 11,923 in `{1..12}` |
| 62 | **SetID** | 0 = not a set item; else 1…42 → `refsetitemgroup` col 1 (set-bonus ladder). Corpus-verified across all 10 `itemdata*.txt` (12 079 rows): nonzero values are exactly 1–42, none out of range. **Not parsed today** |
| 94 | Range | weapon attack reach in **world units**, measured past the bodies rather than centre-to-centre. Corpus-verified, one value per TypeID4: dagger 3; sword/blade/axe/rod/staff/**harp** 6; spear/glaive/2h-sword 18; **bow and crossbow 180**. `ItemDataRow::attack_reach` returns it verbatim (what combat's approach consumes, floored by `EquippedWeapon::engagement_reach`); `attack_distance` divides by 10 for the tooltip's "m" (a display unit is 10 world units, `worldmap.md`) |

## White-stat columns (variance-rolled ranges)

Each stat is a `(lower, upper)` pair; the concrete value on an instance is
`lower + (upper-lower) * bits/31`, where `bits` is a 5-bit slot of the item's
`variance` field (0x3013). `bits/31` is also the tooltip's `(+NN%)` enhancement
(+100% = max roll). **The stat columns differ by category** — armor/shield leave
the weapon columns zero and vice-versa:

| stat | weapon cols | armor/shield cols |
|------|-------------|-------------------|
| Durability | 63/64 | 63/64 |
| Physical defense (PD) | — | 65/66 |
| Block rate (shield) | — | 74/75 |
| Physical attack | 95/96 … 97/98 (min…max range) | — |
| Magical attack | 100/101 … 102/103 | — |
| Attack rate | 113/114 | — |
| Critical | 116/117 | — |
| Physical reinforce (÷10 = %) | 105/106 … 107/108 (range) | 82/83 (single) |
| Magical reinforce (÷10 = %) | 109/110 … 111/112 (range) | 84/85 (single) |

Verification: `ITEM_CH_M_HEAVY_01` has the highest phys-reinforce (82/83) and
`ITEM_CH_M_CLOTHES_01` the highest mag-reinforce (84/85) among garment types —
the defining signature of the two reinforcement columns. `ITEM_CH_SHIELD_01`
carries block rate 10~20 at 74/75.

The per-category `variance` 5-bit slot order lives in `*_WHITE_SLOTS` in
`client/src/plugins/hud/inventory/tooltip.rs`; the armor slot order
(durability 0, defense 1, phys-reinforce 2, mag-reinforce 3, block-rate 4) is
best-effort and is the single knob to calibrate if a rolled value or `%`
disagrees with the vanilla client.

There is no per-item rarity/Seal field in the 0x3013 stream; Seal-of-X items are
separate itemdata entries distinguished by the `_RARE` code-name suffix.

## magicoption.txt (item "blue" options)

Single file (no split), UTF-16LE, tab-separated, keyed by column 1 (`ID`). An
equipment item's `MagicParam.kind` (0x3013) is this ID; its `value` is the
rolled magnitude.

| col | field | notes |
|-----|-------|-------|
| 1 | ID | referenced by `MagicParam.kind` |
| 2 | CodeName | `MATTR_*` (e.g. `MATTR_STR`, `MATTR_DUR`, `MATTR_ATHANASIA`) |
| 3 | Operator | display sign: `+`, `-`, `-@` |
| 4 | Level | option tier (id ranges encode level, e.g. `MATTR_INT` +1..+6 = ids 5–10) |
| 29+ | applicability | `weapon`/`armor`/`shield`/`accessory` + `1` flags |

The table has no localized `SN_*` key, so `MATTR_*` → English is mapped inline in
`client/src/assets/textdata/magicoption.rs` (`mattr_display`). Flag-like options
(`MATTR_ATHANASIA` = Immortal, `MATTR_SOLID` = Steady, `MATTR_LUCK` = Lucky,
`MATTR_ASTRAL` = Astral) are shown without a magnitude. The related
`magicoptionassign.txt` (item-type → allowed `MATTR_*` list) is not loaded — it
is a server-side validation table, not needed for display.

Consumers: `ItemDataRow::{gender, is_rare, stat_range}`,
`ClientMagicOptions`, and the inventory tooltip.
