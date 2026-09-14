# Minimap data (Media.pk2)

Everything the in-game minimap consumes, verified against v1.188 data
(`client/src/plugins/hud/minimap.rs`).

## Terrain tiles — `Media.pk2/minimap/{x}x{y}.ddj`

One 256×256 DDJ per world region, named by the region's sector
coordinates in decimal: `x` = low byte of the region id (region x), `y` = high
byte (region z). Example: `168x97.ddj` = region id 0x61A8 (Jangan). Tiles only
exist for regions that have terrain; missing files simply 404 out of the
archive. North is at the top of the image; one tile spans 1920 world units.

**The corpus is not uniformly DXT1** — a loader that assumes it will fail on 54
files. Of the 4,482 tiles, 4,428 are DXT1 (`ddspf.dwFlags=0x4` DDPF_FOURCC,
`"DXT1"`, 32,916 B) and **54 are uncompressed A8R8G8B8** (`dwFlags=0x41`
DDPF_RGB\|DDPF_ALPHAPIXELS, `dwRGBBitCount=32`, masks
A`ff000000` R`00ff0000` G`0000ff00` B`000000ff`, 262,292 B). Both are 256×256
with `mipCount=0`. The 54: `68x97`, `104x84`, `105x100`, `105x101`, `105x102`,
`211x109`, plus a solid block `225x121`…`232x126` (x 225..232 × z 121..126).
Branch on the DDS pixel-format block, not on the file name. Why they ship
uncompressed is UNKNOWN.

## Window layout — `Media.pk2/resinfo/ifminimap.txt`

Standard resinfo "Interface Text" file. Elements and their `Rect="x,y,w,h"`
(window space; the window frame art `interface/minimap/mm_window.ddj` is
140×184):

| Element | Type | Rect | Notes |
|---|---|---|---|
| GDR_MINIMAP_ALPHA | CIFStatic | 14,57,105,105 | map viewport; `mm_alpha.ddj` is a 104×104 black disc used as blend mask |
| GDR_MINIMAP_TEXT_AREANAME | CIFStatic | 12,9,104,12 | FontColor ARGB 255,239,218,164 |
| GDR_MINIMAP_TEXT_POS_X | CIFStatic | 8,32,56,11 | |
| GDR_MINIMAP_TEXT_POS_Y | CIFStatic | 67,32,56,11 | |
| GDR_MINIMAP_BTN_TOG_MAP | CIFButton | 99,47,24,24 | world-map toggle |
| GDR_MINIMAP_ZOOMIN | CIFButton | 107,136,20,20 | |
| GDR_MINIMAP_ZOOMOUT | CIFButton | 90,152,20,20 | |
| GDR_MINIMAP_DUNGEON_FLOOR_INFO | CIFStatic | 1,42,32,32 | dungeon floor indicator |

`mm_window.ddj` (A1R5G5B5, 140×184) carries a transparent circular hole over
the viewport rect, so drawing it over a square, clipped map area produces the
round minimap without any masking. It is **not** otherwise opaque: 13,811 of
its 25,760 px are transparent (53.6%), the outer silhouette included. Whether
that shortcut is pixel-identical to the original's `mm_alpha.ddj` + frame
composite is UNKNOWN — the two artefacts' transparent-pixel counts differ
(`mm_alpha` has 7,999 alpha=1 px; `mm_window` has 8,209 inside the same rect),
so they are complementary in function but not numerically identical.

Buttons come with `_focus` / `_press` texture variants. Sign textures
(`interface/minimap/mm_sign_*.ddj`) with their decoded fill colours:
monster 8×8 red (255,0,0), npc 8×8 blue (65,131,255), otherplayer 8×8
yellow-green (189,230,0), unique 12×12 purple (156,0,255), character arrow
16×16 (points east at zero rotation).

## Displayed coordinates

The in-game X/Y readout is `world_units / 10`, offset so 0 sits at region
x=135 / z=92: `disp_x = gx/10 − 135·192`, `disp_y = gz/10 − 92·192` (each
region is 192 display units wide).

## Area names — `server_dep/silkroad/textdata/textzonename.txt`

UTF-16LE, tab-separated: `service \t region-id \t <language columns…>`. The
region id is the decimal `u16` (dungeon ids appear negative, i.e. high bit
set); the display name is the last non-empty column (English in this data).
Example: `1 \t 25000 \t … \t Jangan` (25000 = 0x61A8 = region 168×97).

## Monster rarity — characterdata column 15

RefObjCommon column 15 (`…, DecayTime(13), Country(14), Rarity(15), …`) is the
rarity class: 0 normal, 1 champion, 3 unique, 4 giant. Verified:
MOB_CH_TIGERWOMAN and MOB_OA_URUCHI carry 3, plain mobs 0. (The group-spawn
packet also carries a per-spawn rarity byte after the monster record, still
unverified — see `client/src/net/entity_spawn.rs`.)

## Dungeon tiles (`minimap_d`) — decoded (openroad, 2026-08-12)

Overworld corpus: `Media/minimap/{x}x{z}.ddj`, **4,482 files**, x ∈ 26..252,
z ∈ 35..126, every one `JMXVDDJ 1000` → DDS at +20, 256×256 and mipCount 0 —
but **4,428 DXT1 @32,916 B and 54 A8R8G8B8 @262,292 B**, see the terrain-tile
section above. Coverage and the `mapinfo.mfo` region bitmask disagree in *both*
directions: 1,134 tiles exist for regions with no `.m` block, and 1,301 `.m`
regions have no tile (64 of those are mfo-active).

Dungeon convention, corpus-proven against `worldmap_mapinfo.txt`
`#section Dungeonmap` (**2,120 files** across 6 group dirs):

```
Media/minimap_d/<group>/<floor_string_lowercased>_<x>x<z>.ddj    256×256 DXT1
```

- `<floor_string>` = Dungeonmap **col 17** (`층수 스트링`, header at file
  line 38). The same token appears in the `.dof` string table (e.g.
  `DH_A01_FLOOR01` at offset `0x3a999` in
  `Data/dungeon/wchina/dunhwang_cv.dof`), tying tiles to `FloorIndex`.
- `<x>`,`<z>` = absolute region-grid coords, the dungeon's own space centred on
  region **(128,128)**; rectangle = Dungeonmap cols 10..13
  (`left(X) top(Y) right(X) bottom(Y)`), tile counts = cols 6,7.
- Dungeon region id = Dungeonmap col 5 (`code`, 32769..32790 =
  `0x8000 | dungeoninfo id`); col 16 = the `Data/dungeon/dungeoninfo.txt` id.

Declared-vs-present spot checks: `donwhang/dh_a01_floor01..04` 36 files (exact),
`jinsi/qt_a01_floor02..05` 220/576/440/380 (exact), `fort_dungeon01` 9 (exact);
partial: `qt_a01_floor01` 32/48, `qt_a01_floor06` 100/120,
`flame_dungeon01` 210/224, `egypt/rn_sd_egypt1_01` 65/66; undeclared extras:
`donwhang_event/dhe_a01_floor01` (6) and `egypt/rn_sd_egypt01_02..06` (26,
legacy); Jupiter dungeons (ids 32787-32790) have table rows but **no**
`minimap_d` directory. **Missing tiles are normal — fail soft.**

The `<group>` folder segment is in no PK2 table and is not derivable from the
`.dof` path (`wchina/dunhwang_cv.dof` → `donwhang`) — **operationally resolved
(openroad, 2026-08-12)** by discovering it from the archive layout itself:
`plugins/hud/minimap.rs::build_dungeon_groups` walks
`minimap_d/<group>/<floor>_<x>x<z>.ddj` once (via the shared `MediaArchive`
handle) and indexes lowercased floor-string → group. This corpus pins 11 group
directories (`Arabia boss_dungeon demon donwhang donwhang_event egypt
flame_dungeon fort_dungeon jinsi jupiter secret`). What the original exe does
(hardcoded table vs the same discovery) still needs the string-xref read, but
rendering no longer depends on it. Implementation:
`sync_minimap_dungeon_context` + dungeon branches in the minimap tile/area
systems; floor selection = the player's current block's `.dof` floor label; the
floor badge uses `mm_dungeonfloor.ddj` at the `GDR_MINIMAP_DUNGEON_FLOOR_INFO`
rect.