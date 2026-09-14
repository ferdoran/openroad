# textdata: characterdata_*.txt (RefObjCommon + RefObjChar)

Location: `Media.pk2/server_dep/silkroad/textdata/characterdata_<base>.txt`
(split by ID range: `_5000` covers 5000–9999, etc.; `characterdata.txt` lists
the split files). UTF-16LE, tab-separated, one row per character object; a row
is live when column 0 (`Service`) is `1`.

Each row is the RefObjCommon columns followed by the RefObjChar tail — the same
RefObjCommon schema itemdata uses (see `itemdata` handling in
`client/src/assets/textdata/`). Columns verified against real v1.188 data
(0-based indices, as used by `ChardataFields` in
`client/src/assets/textdata/characterdata.rs`):

| col | field | notes |
|-----|-------|-------|
| 0 | Service | 1 = active row |
| 1 | ID | key used by spawn packets (`ref_id`) |
| 2 | CodeName | `MOB_CH_MANGNYANG`, `NPC_CH_SMITH`, `CHAR_CH_MAN1`, … |
| 5 | NameStrID | `SN_*` key → textdataname (localized display name) |
| 9–12 | TypeID1–4 | 1/1/_/_ player, 1/2/1/_ monster, other 1/2/…: NPC/COS |
| 15 | Rarity | 0 normal, 1 champion, 3 unique, 4 giant; corpus also has 6, 7 (elite-type event/quest mobs), 8 (unique-type, e.g. `MOB_TQ_TOMBGENERAL`) |
| 52 | ResourcePath | `.bsr` path under `data://res/` |
| 57 | Lvl | RefObjChar tail; 0 for NPCs |
| 58 | CharGender | 2 on monsters (neutral) |
| 59 | MaxHP | 0 for NPCs |

Verification samples (Media.pk2, v1.188): `MOB_CH_MANGNYANG` id 1933, lvl 1,
MaxHP 54; `MOB_CH_TIGERWOMAN` id 1954, lvl 20, MaxHP 598720, rarity 3;
`NPC_CH_SMITH` id 2003, lvl 0, MaxHP 0. Higher ID ranges contain event
re-issues of the same code names with rescaled stats (e.g. a second
`MOB_CH_MANGNYANG` at id 9383, lvl 90), so lookups must go by ID, not name.

Rarity census across all `MOB_*` rows (all split files): 0 ×6924, 1 ×77,
3 ×627, 4 ×1, 6 ×105, 7 ×91, 8 ×86. The 6/7/8 → `tw_icon_*` badge mapping in
the target window is provisional; the spawn packet also carries a per-instance
rarity byte (see `client/src/net/entity_spawn.rs`), which go-sro fills from
this column.

Consumers: `CharacterDataRow::{name_key, type_ids, rarity, resource_path,
level, max_hp}`. Level and MaxHP drive the target window's level-gap gem and
monster HP bar (`client/src/plugins/hud/target_window.rs`) — current HP then
follows `EntityBarsUpdate` (0x3057).
