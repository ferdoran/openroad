# Animation (*.ban) - JMXVBAN

Skeletal animation: keyframe timings plus, per animated bone, a rotation and
translation at each keyframe.

Layout derived from openroad's parser (`client/src/assets/ban.rs`). Upstream
reference: `SilkroadDoc.wiki/JMXVBAN` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVBAN

## Layout

Strings are a `u32` byte length followed by CP949 bytes (`BufExt::get_double_len_string`).

| size | type | field | notes |
|---|---|---|---|
| 12 | char[12] | `Signature` | `JMXVBAN 0102` |
| 4 | i32 LE | `Int0` | UNKNOWN — 0 in every corpus file |
| 4 | i32 LE | `Int1` | UNKNOWN — 0 in every corpus file |
| 4 + n | u32 + bytes | `Name` | |
| 4 | i32 LE | `Duration` | milliseconds |
| 4 | i32 LE | `FramesPerSecond` | |
| 4 | i32 LE | `Type` | `0` OneShot, `1` Cyclic |
| 4 | u32 LE | `KeyframeTimeCount` | |
| 4 × count | u32 LE | `KeyframeTime[]` | milliseconds |
| 4 | u32 LE | `AnimatedBoneCount` | bones whose pose differs from the bind pose |

Then `AnimatedBoneCount` records:

| size | type | field |
|---|---|---|
| 4 + n | u32 + bytes | `BoneName` |
| 4 | u32 LE | `KeyframeCount` |
| 28 × count | see below | `Keyframe[]` |

Each keyframe is 28 bytes — a rotation then a translation, together giving the
bone's transform relative to its parent:

| size | type | field |
|---|---|---|
| 16 | f32 × 4 | `Rotation` (quaternion, x y z w) |
| 12 | f32 × 3 | `Translation` |

File ends after the last keyframe.

## Corpus + cross-source verification (openroad, 2026-08-11)

Layout above corpus-verified over all 4711 `.ban` files in the user's PK2s —
4708 × `JMXVBAN 0102` parse to exact EOF. Invariants: `Int0 = Int1 = 0` in
4708/4708; per-bone keyframeCount == the global keyframe-time count in every
file; times strictly ascending; `Duration` equals the last keyframe time
(±1 ms authoring jitter in 261 files); FPS ∈ {30 ×4694, 20 ×13, 24 ×1};
`Type` ∈ {0 OneShot ×2717, 1 Cyclic ×1991}. Names are **CP949** (3 files carry
Korean). Note: JMX-File-Editor has **no** JMXVBAN serializer (checked
`Silkroad/Data/` + full-source grep) — cross-refs are
`srodevs-docs/file-formats/jmxvban.md` and `silkroad-docs/docs/asset_formats.md`
(their "Duration = ticks / Type = unknown" wording is contradicted by the
corpus: Duration is ms and equals the last keyframe time).

**Legacy variants (not in any reference doc, all three BSR-orphans):**
- `JMXVBAN 0101` (×2): no Int0/Int1, no global time array; per-bone keyframes
  are 36 B `u32 time_ms, u32 time_dup (== time_ms), f32[4] quat, f32[3] trans`.
- `"BAN "` + u32 version 101 (×1, `Data/Prim/ani/item/common/mob_select.ban`):
  0101 body with NUL-terminated strings.

ResourceAnimationType id table: `JMX-File-
Editor/.../JMXVRES/PrimAnimationType.cs:3-209`.