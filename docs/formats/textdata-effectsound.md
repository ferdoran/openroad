# `effectsound.txt` — the SFX registry

_Measured in the user's own `Media.pk2`, 2026-08-16. Parser:
`client/src/assets/textdata/effectsound.rs`. Issue #773 (EP-22.2)._

`media://server_dep/silkroad/textdata/effectsound.txt` is the client's
sound-effect index: it answers "what does *this situation* sound like".
CP949, no BOM, tab separated, 6,583 lines.

## Columns

Line 1 is the file's own legend:

```
//	object	handle	skill_ID	event1	event2	event3	blank	folder	filename	volume	description1
	ITEM	SND_EQUIP	-	SWORD	-	-	0	ui\	itSword.wav	80	한손검 장착음(중국, 유럽)
```

| # | column | meaning |
|---|---|---|
| 0 | *(marker)* | empty on a data row, `//` on the legend and the section comments |
| 1 | `object` | who makes the sound: `PLAYER`, `UI`, `ITEM`, `MOB_*`, `COS_*`, `PCM_*`, `STRUCTURE_*` |
| 2 | `handle` | what happens: `SND_DMG`, `SND_SWING1`, `VOC_SHOUT1`, `SND_BUTTON_CLICK`, … |
| 3 | `skill_ID` | the skill this row belongs to, or `-` |
| 4-6 | `event1..3` | sub-qualifiers: weapon type (`SWORD`), ground (`FIELD`/`DIRT`), variant |
| 7 | `blank` | 0-23, unused by us |
| 8 | `folder` | subfolder under `Data.pk2:prim/snd/`, back-slashed, sometimes without the trailing `\` |
| 9 | `filename` | the `.wav`, or `-` for a deliberately silent row |
| 10 | `volume` | 0-100, per row; `-` where unstated |
| 11 | `description1` | Korean editor comment |

A sound is therefore addressed by the **tuple** `(object, handle, skill_ID,
event1..3)`, not by an id, and `-` is the wildcard/none marker in every column.

## Shape of the shipped v1.188 table

| measure | value |
|---|---|
| lines / data rows | 6,583 / 5,674 |
| distinct `object` values | 292 (`PLAYER` alone has 733 rows) |
| registered addresses | 5,083 sounding — 390 carry more than one row (`PCF_KANGSI/VOC_AVOID`: 7) |
| rows with no `filename` (`-`) | 27 |
| rows with no `volume` (`-`) | 17 |
| distinct `.wav` paths | 1,861, of which **38 are not in the archive** |
| `volume` distribution | 100 (4,108), 80 (1,509), 60 (21), 70/90/50/40/30/99 (rest) |

A row whose file is missing from the archive is *data*, not a defect: the row is
kept and the asset load simply fails for it.

## Where the files live

`folder` + `filename` are rooted at `prim/snd/` inside **Data.pk2**, e.g.
`ui\` + `uibutton_a.wav` → `data://prim/snd/ui/uibutton_a.wav` — the path the
client hard-coded before this table was parsed. Paths are lowercased on the way
in; `bevy_pk2` normalizes archive lookups the same way
(`bevy_pk2/src/pk2/archive.rs:70`), which matters because the table mixes
`Player\` with `player\`.

## The `UI` block (32 rows)

The UI rows are the ones a client can use before any game world exists:

| handle | file | volume |
|---|---|---|
| `SND_BUTTON_CLICK` | `uibutton_a.wav`, `uibutton_b.wav` | 80 |
| `SND_WINDOW_OPEN` / `SND_WINDOW_CLOSE` | `uiwinopen.wav` / `uiwinclose.wav` | 80 |
| `SND_ERROR` | `Error.wav` | 80 |
| `SND_ELIXIR_USE` | `Elixir_Use.wav` | 50 |
| `SND_ELIXIR_SUCCESS/_FAILURE/_DESTROY` | `Elixir_Suc/Fail/Des.wav` | 100 |
| `SND_REPAIR` | `itRepair_a/b/c.wav` | 80 |

The per-row `volume` is the number a call site cannot invent, and it is why the
table has to be loaded rather than hard-coding paths where sounds are played.

## UNKNOWN

- How the original picks among the variants of a multi-row address
  (`uibutton_a` vs `uibutton_b`, `itRepair_a/b/c`) — random or round-robin.
  We keep them in file order and let the caller choose.
- What `blank` means. It is never referenced by anything we have seen.

## Verified against the running client

`SCENE=intro_v2` boot, 2026-08-16:
`INFO client::plugins::textdata: loaded 5674 effect sound rows (5083 addresses, 27 mute)`.
