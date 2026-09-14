# World map data (`worldmap_mapinfo.txt`, `worldmap_localinfo.txt`)

How the M-key world map is described in the v1.188 media and rendered by the
client (`client/src/assets/textdata/worldmap.rs`,
`client/src/plugins/hud/world_map/`). Both files are UTF-16LE tab tables.

## worldmap_mapinfo.txt — `#section WLocalmap` (21 rows)

Columns (0-based):

| col | meaning |
|---|---|
| 0 | map id (0 = world map) |
| 1 | kind: 0 = tiled world map, 1 = single image |
| 2 | Korean name (unused) |
| 3 | texture path (kind 1) or tile prefix `interface\worldmap\map\map_world_` (kind 0) |
| 4,5 | texture w,h |
| 6,7 | used w,h — the sub-rect of the texture that carries the art |
| 8,9 | logical map size in pixels |
| 10,11 | region x1, z1 (north-west corner; z1 is the LARGER z) |
| 12,13 | region x2, z2 (south-east corner) |
| 14–17 | per-edge offsets left/top/right/bottom, in display units |
| 18 | instance code (`INS_FORT_JA`…) or `xxx` |
| 19 | display-name key (`UIIT_*` → textuisystem, `SN_*` → textdataname) |
| 20 | enabled (the world-map row itself carries 0) |
| 21 | tiling (`4x4` / `xxx`) |

A second `#section Dungeonmap` describes dungeon floors (parsed since
2026-08-12 into `WorldMapTable.dungeon_maps`). 23 rows, 0-based columns:

| col | meaning | donwhang floor-1 example |
|---|---|---|
| 0 | dungeon-map id | `2001` |
| 1 | Korean name | `돈황석굴` |
| 2 | floor kind `F` (up) / `B` (basement, down) | `F` |
| 3 | floor number | `1` |
| 4 | floor count of the dungeon | `4` |
| 5 | region id (`0x8000 \| dungeoninfo id`) | `32769` |
| 6/7 | tile counts x, z | `3 3` |
| 8/9 | logical map size px (`= tiles · 256 · col14`) | `768 768` |
| 10-13 | region rect left(X)/top(Z)/right(X)/bottom(Z), **inclusive cells** in the 128-centred dungeon sector space (covered edges run `x1..x2+1` / `z2..z1+1`) | `127 128 129 126` |
| 14 | zoom scale (px per native tile px) | `1.0` |
| 15 | world-map tile prefix; tile = `{prefix}{x}x{z}.ddj` | `interface\worldmap\dungeon\map_world_donf01_` |
| 16 | `dungeoninfo.txt` id | `1` |
| 17 | floor string — matches the `.dof` floor label and, lowercased, the `minimap_d` tile stem | `DH_A01_FLOOR01` |
| 18 | enabled | `1` |

Cols 19-22 are the same per-edge offsets as WLocalmap cols 14-17, and they
are **0 in 19 of the 23 rows** — but not in all of them: the four Jupiter
floors `2020`-`2022` carry `192 256 192 256` and `2023` carries `64 128 64 0`.
`DungeonMapDef` ignores those four values today, so those floors project as if
the offsets were 0. **UNKNOWN:** whether the dungeon rows use them with the
WLocalmap sign convention — untested, no in-game reference shot.

Row counts above are **data rows only**. The exclusion predicate, applied to
the UTF-16LE file: drop the `#section` lines, drop `//` line comments (e.g.
`//Map File Info`), and drop the `/* … */` Korean design-notes block that ends
the WLocalmap section (8 lines, none of them a row). Counting raw non-empty
lines instead yields 29 for WLocalmap and is wrong.

Donwhang declares 4 floor rows on one region id; jinsi has one region id
(and one row) per floor. The M-window renders these through the same
projection model as WLocalmap rows (`DungeonMapDef` in
`assets/textdata/worldmap.rs`), with a floor-selector button row when a
region declares several floors.

## Projection (verified against the data)

Display unit (du) = 10 world units; 1 region = 192 du (region = 1920 world
units).

```text
left_du   = x1*192 + off_left        top_du    = z1*192 + off_top
right_du  = x2*192 + off_right       bottom_du = z2*192 + off_bottom
k = logical_w / (right_du - left_du)   # px per du
px = ((gx_du - left_du) * k,  (top_du - gz_du) * k)
```

with `(gx, gz) = (-sro.x, sro.z)` (the minimap's mirrored-X global
convention) and `gx_du = gx/10`. Checks: world map 4224 px / 25344 du = 1/6;
Jangan 1024/768 = 4/3.

**City test**: a kind-1 row whose region rectangle contains the player's
region is the map to show (e.g. Jangan = x 166..170, z 96..99). No other
town concept exists in the client.

## World map tiles

`interface/worldmap/map/map_world_<x>x<z>.ddj`, 128×128 px, one per
4×4-region block, named after the block's top-left region (min x, max z);
grid x = 46,50,…,174, z = 73,77,…,113. At 1/6 px per du a tile is exactly
128 map px. Grid positions without a file simply fail to load (hidden).
Level/race-gated variants (`map/{90lv,…}/{all,china,europe}/…`) and the
dungeon tiles are not used.

## worldmap_localinfo.txt — POIs (~1160 rows)

| col | meaning |
|---|---|
| 0 | service |
| 1 | id |
| 2 | type: 1 = text label, 2 = icon |
| 3 | `SN_ZONE_*` key (type 1) or ddj path (type 2) |
| 4,5,6 | Korean area / label / map name (unused; the map linkage is col 8) |
| 7 | link map id (> 0 on `city_*.ddj` rows = the world map's clickable city buttons; the art's lower ~30% is an empty name plate the city name renders into) |
| 8 | owning map id (0 = world map) |
| 9,10 | region x,z — world-map POIs only; `-1 -1` on city maps |
| 11,12 | du offset inside the region (world map; pz measured from the region's north edge — calibrate on playtest) or raw pixels on the city image |
| 13,14 | icon size (0 for labels) |

## Deferred

`worldmapguidedata*.txt` (area/monster/quest guide overlays),
`worldmap_instanceinfo.txt`, dungeon maps, the big-window size toggle and
zoom levels beyond the world↔city switch. The `MapMarkers` resource is the
plug-in point for party/academy signs once their packets exist
(`wmap_sign_party*.ddj`, `wmap_sign_apprenticeship.ddj`).
