# Compound (*.cpd) - JMXVCPD

Groups several `.bsr` resources into one logical object, optionally with a
collision mesh.

Layout derived from openroad's parser (`client/src/assets/cpd.rs`) and verified
against all 377 corpus files below. Upstream reference:
`SilkroadDoc.wiki/JMXVCPD` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVCPD

## Layout

Fixed header, then two file-controlled offsets pointing at the collision path and
the resource list. Both offsets are **absolute** from the start of the file.

| offset | size | type | field | notes |
|---|---|---|---|---|
| 0 | 12 | char[12] | `Signature` | `JMXVCPD 0101` |
| 12 | 4 | u32 LE | `CollisionOffset` | absolute |
| 16 | 4 | u32 LE | `ResourceOffset` | absolute |
| 20 | 4 | u32 LE | `Unknown0` | 0 in 377/377 |
| 24 | 4 | u32 LE | `Unknown1` | 0 in 377/377 |
| 28 | 4 | u32 LE | `Unknown2` | 0 in 377/377 |
| 32 | 4 | u32 LE | `Unknown3` | 0 in 377/377 |
| 36 | 4 | u32 LE | `Unknown4` | 0 in 377/377 |
| 40 | 2 | i16 LE | `Type` | `0` CompoundCharacter, `2` CompoundObject |
| 42 | 2 | i16 LE | `Category` | with `Type`, forms the `ObjectType` value |
| 44 | 4 + n | u32 len + bytes | `Name` | CP949; byte length, not char count |
| … | 4 | u32 LE | `Unknown5` | `{0: 366, 3: 11}` |
| … | 4 | u32 LE | `Unknown6` | `{0: 364, 2: 12, 1: 1}` |

**Note the `Type`/`Category` split.** Upstream documents one `u32 objInfo.Type`;
it is really two `i16`s. The byte image is identical, so the u32 view is not
wrong — but read as a pair it yields the `ObjectType` values directly.

At `CollisionOffset`:

| size | type | field |
|---|---|---|
| 4 + n | u32 len + bytes | `CollisionResourcePath` — empty, or a `.bsr` |

At `ResourceOffset`:

| size | type | field |
|---|---|---|
| 4 | u32 LE | `ResourceCount` |
| 4 + n | u32 len + bytes | `ResourcePath` × `ResourceCount` |

Strings are a `u32` **byte** length followed by CP949 bytes. The parser bounds
every offset, count and length against the file size before following it — the
file is untrusted input.

## Round-trip + corpus verification (openroad, 2026-08-12)

Verified against the JMX-File-Editor round-trip serializer
(`Silkroad/Data/JMXVCPD/JMXVCPD 0101.cs`, Load `:39-66` / Save `:69-107` — the
write side back-patches both offsets, which proves they are absolute) and all
**377 `.cpd`** in the user's Data.pk2: signature `JMXVCPD 0101` ×377, both
offsets match their computed positions in 377/377, **EOF-exact 377/377**.

- `objInfo.Type` is really **i16 Type + i16 Category**
  (`Common/ObjectGeneralInfo.cs:18-19`); the u32 view above is byte-identical and
  yields the `ObjectType` enum values `CompoundCharacter = 0x30000` (×275) and
  `CompoundObject = 0x30002` (×102) — the only two in the corpus.
- Strings are `u32 **byte** length + CP949` (`IO/BSWriter.cs:36-43`).
- `unk01..unk05` (offsets 20-36) are **0 in 377/377**; `unkInt01 ∈ {0:366, 3:11}`,
  `unkInt02 ∈ {0:364, 2:12, 1:1}`.
- `CollisionResourcePath` is empty in 308 files and a real `.bsr` in **69 — all
  of them Type=2 (CompoundObject)**; Type=0 never carries one. Our loader parses
  this path and still never uses it — object collision does not exist in the
  client yet, so `JMXVCPD.collision_mesh` waits for it (#290).
- `ResourceCount` ranges 2..9 (2,437 entries, **100% `.bsr`**). 14 resource paths
  carry trailing NUL padding inside the length field. `ObjectInfo.Name` matches
  the filename in only 107/377 — it is authoring metadata, not a key.
- 41 of 2,506 referenced paths are dangling (all dev-showroom compounds), so
  missing sub-resources must be tolerated.

**Loader state (#290).** `client/src/assets/cpd.rs` now models `Type` and
`Category` as the two `i16`s they are, validates the 12-byte signature, and has
a real error type: the file's own offsets and its resource count are checked
against the file length before they are followed, and every length-prefixed
string is checked before it is read (`BufExt`'s helpers clamp rather than fail,
which would turn a truncated file into a silently short path). The loader
previously `unwrap`ped the read, discarded the signature and seeked unguarded
into file-controlled offsets, so it could only panic — inside an asset-loader
task. The CP949 half of the old "names go through latin1+unidecode" note is
already fixed in `BufExt::get_fixed_size_string`.

