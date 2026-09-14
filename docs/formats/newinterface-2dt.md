# NewInterface (*.2dt) - NewInterface

The UI layout descriptor: one fixed-size record per control, giving its class,
parentage, rectangle, art paths and UV corners.

**No openroad parser reads `.2dt` yet**, so the field table below is upstream's
layout restated as a table and *not* verified against our own code. The
`## openroad notes` section further down **is** corpus-verified and takes
precedence where the two disagree. Upstream reference:
`SilkroadDoc.wiki/NewInterface` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/NewInterface

## Layout

| size | type | field |
|---|---|---|
| 4 | u32 LE | `EntryCount` |

Then `EntryCount` fixed-size records. Strings are fixed-width NUL-padded fields,
not length-prefixed.

| size | type | field | notes |
|---|---|---|---|
| 64 | char[64] | `Name` | |
| 256 | char[256] | `Image` | |
| 256 | char[256] | `Background` | |
| 128 | char[128] | `Text` | a `UI_STRING` key |
| 64 | char[64] | `Description` | |
| 64 | char[64] | `Prototype` | placeholder text |
| 4 | u32 LE | `Type` | see `newinterface-type.md` |
| 4 | u32 LE | `Id` | |
| 4 | u32 LE | `ParentId` | |
| 4 | u32 LE | `GrandParentId` | |
| 4 | u32 LE | `Unk02` | UNKNOWN |
| 4 | u32 LE | `Unk03` | UNKNOWN |
| 4 | u32 LE | `Color` | RGBA8888 |
| 4 | u32 LE | `ClientRectangle.X` | |
| 4 | u32 LE | `ClientRectangle.Y` | |
| 4 | u32 LE | `ClientRectangle.Width` | |
| 4 | u32 LE | `ClientRectangle.Height` | |
| 8 | f32 x 2 | `TopLeft` | UV |
| 8 | f32 x 2 | `TopRight` | UV |
| 8 | f32 x 2 | `BottomRight` | UV |
| 8 | f32 x 2 | `BottomLeft` | UV |
| 4 | u32 LE | `Unk04` | UNKNOWN — possibly a command id |
| 4 | u32 LE | `ContentId` | used by `CNIFTabButton`, points at a frame |
| 4 | u32 LE | `IsRoot` | |
| 4 x 13 | u32 LE | `Unk07` … `Unk19` | UNKNOWN |
| 4 | u32 LE | `Style` | text-alignment bits |

Record size is **976 bytes** (832 of strings + 36 four-byte fields) by arithmetic
on the table above; that total is derived, not measured against a corpus file.

Related: [`newinterface-type.md`](newinterface-type.md).

## openroad notes (corpus-verified, not from the wiki)

Reader: `client/src/assets/twodt.rs` (`JMXV2DT`, alias `SroNewInterface`).
Corpus: the 42 `Media/res_ui/*.2dt` files, 2,312 entries.

**Entry size is exactly 976 bytes.** `(filesize - 4) / entryCount == 976` holds
in all 42 files, so a short tail is a truncated file rather than a layout
variant.

**`Style` is a `[Flags]` bitfield, not an enum** (#294). The dominant corpus
value `0x10100` is `LINE_CENTER | CENTER`; read as an enum over the single
values it matched nothing and collapsed to "none", taking 2,092 of the 2,312
entries with it. Undocumented bits must be surfaced rather than folded into a
known flag — one entry carries `0x20000`.

**The six string fields are CP949**, not latin1 (#294). Transliterating them
turns the corpus' 2,206 valid strings into garbage.

**`ClientRectangle` is absolute, and a child is *not* re-based to its parent.**
Rects live in one flat design space: measured over all 42 files, **9 place at
least one child at an x or y smaller than their root's**, which refutes
"child coords are parent-relative" outright. A loader computes local layout as
`child.xy − root.xy`, which is a no-op only for roots authored at `(0,0)` (for
example `arena_game_result.2dt`, which the client centres at runtime). Getting
this backwards misplaces children by the root's origin, silently, and only on
the windows *not* authored at `(0,0)` (#447).

**`ContentId` on a Type-16 `CNIFTabButton` is the target pane's `Id`** — two
tabs sharing a `ContentId` are two verbs over one pane layout (#476).

**Generation boundary.** The classic `resinfo/*.txt` grammar (see `resinfo.md`)
and these descriptors are disjoint *sets of windows*, not two encodings of one
set — a byte-level scan of all 247 resinfo files for `arena` returns 0 files
while five `arena_*.2dt` descriptors exist (#447). Check both corpora before
calling a window missing.
