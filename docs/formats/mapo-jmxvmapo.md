# MapObject (`*.o2`, and the vestigial `*.o`) - JMXVMAPO

Placement of object instances in a world region.

> **Correction (round 2, F2): the v1.188 client never opens a `.o` file — only `.o2`.**
> The complete set of per-region filename literals in the 10,237-row client string
> table contains no `%d\%d.o` literal. This title used to read
> "MapObject (\*.o, \*.o2)", presenting the two as peer runtime formats; `.o` is a
> **pre-v1.188 predecessor**. Treat `.o` as `do-not-wire`.
> See [`../re/round2/formats.md`](../re/round2/formats.md) F2.

Layout derived from openroad's parsers (`client/src/assets/o2.rs`, and `o.rs` for
the vestigial form). Upstream reference: `SilkroadDoc.wiki/JMXVMAPO` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVMAPO

## Layout

| size | type | field | notes |
|---|---|---|---|
| 12 | char[12] | `Signature` | `JMXVMAPO1001` |

Then 36 blocks (6 x 6, `z` outer). In `.o2` each block holds **four LOD groups**;
in `.o` the object list follows the block directly with no LOD level. A higher LOD
level fades out sooner with distance.

Per LOD group (`.o2`) or per block (`.o`):

| size | type | field |
|---|---|---|
| 2 | u16 LE | `ObjectCount` |

Then `ObjectCount` records. Each is **30 bytes** in `.o2` (`RECORD_LEN`), 28 in
`.o` — the difference is the trailing `RegionID`, which `.o` does not carry.

| size | type | field | notes |
|---|---|---|---|
| 4 | u32 LE | `ObjID` | keys into `value.ifo` |
| 12 | f32 x 3 | `LocalPosition` | region-relative |
| 2 | i16 LE | `IsStatic` | `0` no, `0xFFFF` yes |
| 4 | f32 LE | `Yaw` | |
| 2 | i16 LE | `UID` | tracks one object across blocks |
| 2 | i16 LE | `Short0` | UNKNOWN |
| 1 | u8 | `IsBig` | object exceeds region size |
| 1 | u8 | `IsStruct` | has an `objectstring.ifo` entry |
| 2 | u16 LE | `RegionID` | **`.o2` only** — bits 0-7 x, 8-14 z, bit 15 dungeon |

## Correction + corpus verification (openroad, 2026-08-12)

**The `.o` layout above (one `mapObjInfoCount` per block, inherited from the
SilkroadDoc wiki) is WRONG.** `.o` has the same four count-prefixed LoD groups
per block as `.o2`, only with 28-byte entries (no trailing `RegionID`):

```
per block (6x6): per group (0..3): u16 count, count x {
    u32 ObjID | f32 x,y,z | u16 IsStatic | f32 Yaw | u16 UID
    u16 Short0 | u8 IsBig | u8 IsStruct }        // stride 28
```

Proof: under the 1-group model **0 of 4,360** corpus files reach EOF (e.g.
`Map/61/40.o` stops at offset 84 of 300); under the 4-group model
**4,342/4,353** `JMXVMAPO1001` files are byte-exact to EOF.

**Three-group bodies are recognised by LENGTH, not by version.** 18 files have
a 228-byte (12 + 36×3×2) all-zero body: 7 are `JMXVMAPO1000` and **11 are
`JMXVMAPO1001`, so a signature-version rule mis-reads those 11**. `o.rs` drives
the group count off the body length instead; the bodies are entirely zero, so
no object is lost either way.

Corpus (4,360 `.o`): **73,722 objects**, per group `g0 = 0, g1 = 0,
g2 = 50,346, g3 = 23,376` — LoD groups 0 and 1 are empty in **all** 156,708
blocks. `ObjID` 0..3305 · positions x,z ∈ [0,1920), y ∈ [−1970, 6101] ·
`IsBig` 0/1 only (892 set) · `IsStruct` 0/1 only (280 set) · `IsStatic` is
0xFFFF ×65,114 / 0 ×8,199 **plus ~409 other values** · `Short0` is 0 in 93%
with 0xCCCC (MSVC debug fill) among the rest — both look like uninitialised
editor memory. 289 regions have a `.m` but no `.o`.

Corpus (4,506 `.o2`): **4,506/4,506 EOF-exact at 4 groups, 0/4,506 at 3**;
231,261 objects, `g0 = 0, g1 = 0, g2 = 199,688, g3 = 31,573`; 2,562 distinct
`RegionID` (0x3944..0x7FEE); the **dungeon bit is set in 0 objects**; `IsBig`
is set on 21,872 objects (9.5%) which legitimately spill outside their region
(min z = −128.4) — the cross-region culling hint.

Also: line 18 above cites `value.ifo` for `ObjID`; the correct table is
**`object.ifo`** (as the wiki says and as our code actually uses).

**Both loaders implement this model as of #287** (`client/src/assets/o.rs`,
`client/src/assets/o2.rs`). Two notes for anyone touching them again:

- The 12-byte signature must be **skipped explicitly**. The pre-#287 `o2.rs`
  consumed it by accident through a `first_u16 != 0 ⇒ empty block` branch, which
  shifted every block six slots and dropped blocks 30..35 of every region —
  37,599 placements, 6,803 distinct world objects, never spawned. The first
  re-land attempt (PR #360) removed that branch without adding the skip and
  therefore panicked on 4,506 of 4,506 real `.o2` files; its fixture had no
  signature, so the tests passed. Fixtures here now carry one.
- `bytes::Buf`'s getters panic on underflow and these parsers run inside asset
  loader tasks, so every read is bounds-checked and a short file stops early.

