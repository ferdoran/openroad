# `regioninfo.txt` + `effectenvsnd.txt` — the zone sound tables

Two CP949 textdata files that only make sense together: `regioninfo.txt` says
**which sectors make up a zone**, `effectenvsnd.txt` says **what that zone
sounds like**. Joined on the zone name they answer the one question a
soundscape needs — *given where the player stands, what should be playing?*

Parser: `client/src/assets/textdata/zonesound.rs`, exposed as the
`ClientZoneSounds` resource (`client/src/plugins/textdata.rs`). Both files are
CP949 with **no BOM**, decoded through `assets/textdata/decode.rs`.

All numbers below were measured in the user's own `Media.pk2` on 2026-08-16.

## `server_dep/silkroad/textdata/regioninfo.txt`

2,675 lines, 43 zone blocks (10 `#TOWN`, 33 `#FIELD`), 2,573 sector rows. Tab
separated, `\r\n`, padded to seven columns with empty fields.

```
#TOWN	장안
167	96	RECT	0	1600	1920	1920
167	97	ALL
...
#FIELD	돈황던젼	donwhang
1	128	ALL
```

| Row | Col 1 | Col 2 | Col 3 | Col 4-7 |
|---|---|---|---|---|
| header | `#TOWN` / `#FIELD` | zone name (the join key) | optional short code (`donwhang`, `jinsi`) — present in 8 of 43 | — |
| sector | X sector | Z sector | `ALL` or `RECT` | `RECT` bounds |

* The zone name is unique across the 43 blocks; the short code is not
  (`jinsi` names three blocks), so the **name** is the identity.
* `ALL` 2,498 rows, `RECT` 75 rows.
* **Sector Z = 128 means dungeon, not overworld.** 17 rows carry Z=128, one
  past the 0..=127 overworld grid. Packed the ordinary way (`z << 8 | x`) they
  land on `0x8000 | x` — exactly the dungeon region ids of
  `Data.pk2:dungeon/dungeoninfo.txt`, and the names agree row for row: x=1
  `Dunhwang_Cv` = 돈황던젼, x=2..4 `jinsi_floor06..04` = 진시황릉456, x=5
  `jinsi_floor03` = 진시황릉3, x=6..7 `jinsi_floor02..01` = 진시황릉12,
  x=10..16 the Egypt caves = 신전, x=17 `fortress_dungeon`, x=18 `flame`.
  So the packing of `hud::region_banner::overworld_region_id` covers dungeon
  zones too and the parser needs no special case.

### `RECT` — overlapping claims and how they resolve

66 sectors are claimed by more than one zone. In **every** one of them exactly
one claimant is `ALL` and the rest are `RECT`: the `RECT` is a specific carve-out
of the `ALL` background. Sector (49,90) is the clearest instance — Alexandria
owns the whole sector, the Delta region takes five stepped rectangles out of it:

```
알렉산드리아  49 90 ALL
델타지역      49 90 RECT  480    0  1920   640
델타지역      49 90 RECT  960  640  1920   960
델타지역      49 90 RECT 1140  960  1920  1280
델타지역      49 90 RECT 1320 1280  1920  1600
델타지역      49 90 RECT 1500 1600  1920  1920
```

`ZoneSoundTable::zone_at` therefore tries the `RECT` claims first and falls back
to the `ALL` claim; `zone_for_region` (region id only, no offset) returns the
`ALL` background.

**The four bounds are two corners, in sector-local SRO units 0..=1920**
(`REGION_SIZE`): columns 2 and 4 march monotonically upward across those five
rows, which is what identifies them as one axis' min and max. Which pair is X
and which is Z is **UNKNOWN** from the file; we read them `x_min z_min x_max
z_max` because columns 1/2 of every sector row are already X/Z in that order
(ADR-0009: a stated reading, not an invented number). The observation that
would settle it: walk the (49,90) boundary in Alexandria and see whether the
zone flips to 델타지역 along X or along Z.

One quirk left as-is: sector (75,72) is listed twice, both times `ALL` and both
times by 왕가의계곡 — a duplicate row, not an ambiguity.

## `server_dep/silkroad/textdata/effectenvsnd.txt`

834 lines, 51 zones, 511 ambient rows, 29 distinct BGM tracks. A depth-tagged
grammar (`<1>` / `<2>` / `<3>`) rather than a column table:

```
<1>	장안
	"Jangan_Town.ogg"
	<2>	낮
			<3>	"night_wind.wav"	0~0
			<3>	"donhwang_wind04.wav"	15~40
	<2>	밤
			<3>	"day_wind.wav"	0~0
```

* `<1>` — zone name, the join key back into `regioninfo.txt`.
* The bare quoted line under it is the **BGM** `.ogg`. All 51 zones have one;
  three of them (`Jupiter_A.ogg`, `Jupiter_A_Boss.ogg`, `Jupiter_Field.ogg`)
  are **not in the user's `Music.pk2`** — and those same Jupiter zones have no
  `regioninfo.txt` block either, so they are unreachable from a region id.
* `<2>` — a section, 낮 (day) or 밤 (night). All 51 zones have exactly both.
* `<3>` — an ambient `.wav` and a repeat interval `a~b` in seconds. `0~0` is
  the continuous bed (100 of the 102 sections have exactly one); a non-zero
  range is a one-shot that fires again after a random delay in `a..=b`.
  Names may carry trailing spaces inside the quotes (`"dd_wind_01.wav"  `).

**Do not "fix" the day/night lists.** In the shipped table the two sections
routinely disagree with their own filenames — 도적마을's 낮/day list opens with
`night_wind.wav`, its 밤/night list with `day_wind.wav`. The section header is
what the client reads; the filename is not authoritative. We reproduce the
file's labelling.

## The join

Exact name match, 41 of 43 `regioninfo` zones resolve. The leftovers are
reported (`ZoneSoundTable::unmatched_zones`, one debug line at load) rather
than silently dropped:

| Side | Name | Note |
|---|---|---|
| `regioninfo` only | 보상인던 (`fort_dungeon`) | no `effectenvsnd` block; plays no zone BGM |
| `regioninfo` only | `잊혀진세계_ 화염산` | **near miss**: `effectenvsnd` spells it `잊혀진세계_화염산`, without the space. Not normalized away — the original joins on the literal name, so this dungeon is silent in the shipped data too. |
| `effectenvsnd` only | 9 zones (거울의차원, 소아시아해변, 요새전보상인던, 5x 유피테르신전, 잊혀진세계_화염산) | no sectors, so no region id can reach them |

`docs/formats/envi-jmxvenvi.md:21-23` records that `JMXVENVI`'s own
`DayBGM`/`NightBGM` strings are obsolete and "handled by regioninfo.txt &
effectenvsnd.txt" — these two files, not the environment profiles, are the
source of zone music.
