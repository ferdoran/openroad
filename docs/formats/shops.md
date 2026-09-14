# NPC shop data (`server_dep/silkroad/textdata/`)

How an NPC's store inventory is described in the v1.188 media, and how the
client denormalizes it (`client/src/assets/textdata/shops.rs`, loaded through
the `refshopgroup.txt` entry point which reads its six siblings raw).

## The ref-shop chain (used by the client)

All files are UTF-16LE, tab-separated, first column `1` = enabled. Join keys
verified against the data (example: the Jangan blacksmith). Tab name keys
resolve via **`textuisystem.txt`** for both prefixes — despite its name, `SN_*`
is *not* in textdataname (`grep SN_TAB_WEAPON`: textuisystem 1 hit,
textdataname 0). The resolver tries both tables.

| file | columns (0-based) | example |
|---|---|---|
| refshopgroup.txt | 2 id, 3 group codename, 4 **NPC codename** | `978 GROUP_STORE_CH_SMITH NPC_CH_SMITH` |
| refmappingshopgroup.txt | 2 group, 3 **store** | `GROUP_STORE_CH_SMITH STORE_CH_SMITH` |
| refmappingshopwithtab.txt | 2 store, 3 **tab group** | `STORE_CH_SMITH STORE_CH_SMITH_GROUP1` |
| refshoptab.txt | 2 id (tab order), 3 tab codename, 4 tab group, 5 **name key** | `2402 STORE_CH_SMITH_TAB1 … SN_TAB_WEAPON` |
| refshopgoods.txt | 2 tab codename, 3 **package codename**, 4 slot (0-based, 6-wide grid) | `STORE_CH_SMITH_TAB1 PACKAGE_ITEM_CH_BLADE_01_A 3` |
| refscrapofpackageitem.txt | 2 package, 3 **item codename**, 4 opt level (rest: variance/durability/params) | `PACKAGE_ITEM_CH_BLADE_01_A ITEM_CH_BLADE_01_A 0` |
| refpricepolicyofitem.txt | 2 package, 3 **currency flag**, 5 **price** *(client PK2 — see the index trap below)* | `PACKAGE_ITEM_CH_BLADE_01_A 1 0 890` |

> **Column-index trap (round 2, gamedata.md G19).** Those indices are correct for the **client**
> PK2 copy and wrong for the **server's own** `SR_GameRefData/refpricepolicyofitem.txt`, which is
> one column narrower — price is at **index 4** there, and index 5 is a constant `0`:
> ```
> SERVER (13 cols):  [0]1 [1]15 [2]PACKAGE_… [3]1 [4]890 [5]0 [6]xxx …
> CLIENT (14 cols):  [0]1 [1]15 [2]PACKAGE_… [3]1 [4]0   [5]890 [6]0 …  + trailing tab
> ```
> Anything parsing the server copy with the client indices reads a constant 0 price.

NPC stores have at most 4 tabs (only the item-mall tab groups have 5). A tab's
goods would page at slot / 30, but nothing in this media reaches that: the slot
histogram over all 1,166 goods rows is min 0, **max 29**, with zero rows ≥ 30,
so the chunking always yields exactly one page.

One NPC is mapped by TWO shop groups (`NPC_WC_WAREHOUSE_W`'s two honor shops)
— the assembler MERGES its pages, iterating groups in sorted codename order for
determinism (a plain last-wins insert dropped one shop per process run at
random). `NPC_TD_THIEF_SELL` has a single group; it used to be listed here,
conflated with `NPC_TD_THIEF_BUY`.

`refaccesspermissionofshop.txt` and `reftreatitemofshop.txt` are empty (BOM
only, 2 B each). **`refshopitemstockperiod.txt` is not** — it is 374 B with 2
enabled rows of 7 columns (svc, country, group, package, start datetime, end
datetime, flag), windowing a `GROUP_MALL` item to 2010-12-26 … 2011-01-30. The
client does not read it.

`refshoptabgroup.txt` (74 rows, 6 cols) carries a per-tab-group **page title**
key at index **4** — 5 `UIIT_*` and 69 `SN_*`. Index 5 is an empty trailing
field on all 74 rows, so a `last()`-style read yields `""`. Unused so far: our
page spinner has no label.

## Currencies — `refpricepolicyofitem` column 3

Column 3 is a currency bitflag, and a package carries one row per currency it
is buyable in; 237 of 933 packages have more than one row, always with
different prices. Histogram over the 1,245 enabled rows — **this vocabulary is
build-specific, not universal** (round 2, gamedata.md G19): the live vSRO server's own export uses
**`1/2/4/8/16`** (`1` gold, `2` silk, `8` guild points at `STORE_*_GUILD_TAB*`, `4` and `16` always
price 0 and `[U]`), where this client PK2 uses `1/2/8/32/1024`. Two builds, two flag tables — read
the vocabulary per build rather than porting this one:

| flag | rows | where it appears | reading |
|---|---|---|---|
| 1 | 793 | all normal `STORE_*` tabs, 66 real NPCs | gold |
| 2 | 140 | `MALL_*` tabs only, NPC `xxx` | silk |
| 8 | 11 | `STORE_*_GUILD_TAB*` at `NPC_*_GUILD` | guild points |
| 32 | 32 | `STORE_{CH,KT,WC}_HONOR_TAB1/2` at the warehouses | honour points |
| 1024 | 43 | `STORE_BATTLE_ARENA_CH_TAB2` at `NPC_BATTLE_ARENA_EXCHANGER` | arena coins |
| 256 / 512 | 144 / 72 | `STORE_SD_SET_ARMOR_TAB*` at the two `*_CHANGER` NPCs; 512 only on `_RARE` | UNKNOWN (job goods) |
| 64 / 128 | 7 / 3 | both on `STORE_SE_ACCESSORY_TAB3` (socket stones) | UNKNOWN (two alternate payments for the same goods) |

None of this is spelled out in the data — the readings come from the tabs and
NPCs each flag reaches, so the four unmapped flags stay UNKNOWN and render as a
bare amount rather than a guessed unit.

Two facts that matter for the assembler:

- **140 packages have no gold row at all** (all flag 2). 101 of them are
  reachable only through the 17 `MALL_*` tabs and the other 39 have no
  `refshopgoods` row at all, so following the full chain
  `refshopgoods → refshoptab → refmappingshopwithtab → refmappingshopgroup →
  refshopgroup` every one of them resolves to NPC `xxx`. **None is reachable
  from a real NPC store.**
- Where a gold row exists it is always the file-order first row for that
  package (793 packages, exactly one gold row each, zero exceptions), so the
  old first-row-wins lookup never actually mispriced an NPC good. The
  assembler now prefers the gold row explicitly rather than relying on that.

## The legacy chain (NOT used)

`shopdata.txt` / `shopgroupdata.txt` / `shoptabdata.txt` / `shopitemdata.txt`
describe the same shops in the older id-keyed form (store → numeric tab ids →
itemdata ref ids). Redundant with the ref chain, so the client ignores them.

## Related

- `npcchat.txt`: `service, NPC codename, greeting strid (_BS), talk-page
  strid (_PS)` — the strings live in `textquest_speech&name.txt` (same
  string-table shape as textuisystem: key col 1, English col **9**). The
  English index is 9 in all four shapes (`textuisystem.txt` 10 cols,
  `textquest_speech&name.txt` 10, `textquest_otherstring.txt` 20,
  `textquest_queststring.txt` 21) — matching `names.rs:28-38`, which reads
  `fields.get(9)` with a reverse-scan fallback.
- `teleportdata.txt`: `service, teleporter id, codename, owner characterdata
  ref id, SN_ZONE_* name key, region, x, y, z, …` (114 of 260 rows carry
  owner ref 0 — dungeon/arena gates, indexed by nobody);
  `teleportlink.txt`: 22 cols, `service, source id, dest id`, then **no fee
  anywhere** (cols 3-6 all-zero across all 365 enabled rows); col 7 is a
  link-type code, and only type-1 rows carry a `[8]/[9]` min/max level gate
  (min 0 = ungated). `teleportbuilding.txt` (gate buildings,
  `service, ref id, codename, kr, xxx, SN_NPC_* name key, …`) rows have no
  characterdata — the spawn resolver classifies those refs as structures and
  spawns invisible click anchors (see `docs/net-npc-talk-0x7046.md`).
