# Skeleton (*.bsk) - JMXVBSK

The bone hierarchy: per bone, three transforms (relative to parent, to world
origin, and to the local armature) plus its children's names.

Layout derived from openroad's parser (`client/src/assets/bsk.rs`). Upstream
reference: `SilkroadDoc.wiki/JMXVBSK` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVBSK

## Layout

Strings are a `u32` byte length followed by CP949 bytes.

| size | type | field | notes |
|---|---|---|---|
| 12 | char[12] | `Signature` | |
| 4 | u32 LE | `BoneCount` | |

Then `BoneCount` bone records:

| size | type | field | notes |
|---|---|---|---|
| 1 | u8 | `BoneType` | `0` bone, `1` dummy |
| 4 + n | u32 + bytes | `BoneName` | |
| 4 + n | u32 + bytes | `ParentBoneName` | |
| 16 | f32 × 4 | `RotationToParent` | quaternion |
| 12 | f32 × 3 | `TranslationToParent` | |
| 16 | f32 × 4 | `RotationToOrigin` | quaternion |
| 12 | f32 × 3 | `TranslationToOrigin` | |
| 16 | f32 × 4 | `RotationToLocal` | quaternion |
| 12 | f32 × 3 | `TranslationToLocal` | |
| 4 | u32 LE | `ChildBoneCount` | |
| (4 + n) × count | u32 + bytes | `ChildBoneName[]` | |

Two trailing words close the file:

| size | type | field | notes |
|---|---|---|---|
| 4 | u32 LE | `Unknown0` | UNKNOWN |
| 4 | u32 LE | `Unknown1` | UNKNOWN |

## Corpus verification (openroad, 2026-08-12)

Layout above verified EOF-exact on every parseable `.bsk` in the user's PK2s.
**Corpus-count correction (2026-08-12):** the original probe reported 952 files;
a case-insensitive re-walk finds **1,014** — the earlier count was case-sensitive
and silently dropped the **62 uppercase `.BSK`** names. (BMS and BAN counts were
unaffected.) The signature/variant split is unchanged: `JMXVBSK 0101` plus 4
zero-byte files and 1 legacy `BSK ` variant. Always match extensions
case-insensitively when probing this corpus.
JMX-File-Editor has no JMXVBSK serializer; ground truth is this wiki page +
`srodevs-docs/file-formats/jmxvbsk.md:11-47`, corpus-confirmed.

- Bone counts 1–257 (props 3–5, characters ~35–42); `boneType == 0` in all
  **28,312** bones (CPrimDummy = 1 never observed in this build); the trailing
  u32 pair is (0, 0) in 947/947.
- **Transform-set semantics** (refined beyond the wiki wording): set1
  (…ToParent) is the local bind; set2 (…ToOrigin) equals the accumulated set1
  parent chain for 96.2% of bones — a *cache* that is occasionally stale (e.g.
  `Spine_Base` stored as identity); set3 (…ToLocal) equals **inverse(set2)** for
  98.0% — i.e. the precomputed **inverse bind pose**, not an independent
  transform. Deviants cluster on `[root]`/`Spine_Base` bones, some axis-permuted.
  ⇒ **the set1 chain is the only fully trustworthy source**; treat set2/set3 as
  caches.
- **Multi-root skeletons**: 865 files have a single root; **82 files have
  several**, joined by a synthetic `[root]` super-root that links its sub-roots
  **only through its child-name list** (375 such edges) — the sub-roots' parent
  fields are empty. Consumers that walk parent fields alone drop `[root]`'s
  transform (artifact/building/boss models, e.g. `fort_stone_ht.bsk`).
  Root names: `Bip01` ×439, `Bone01` ×413, `[root]` ×82.
- Attachment/prop points are ordinary type-0 bones by naming convention
  (1,015 `*hand*`, 12 `*dummy*`, 11 `*cape*`, 3 `*weapon*`) — there is no
  dedicated attachment section.
- **Legacy variant** (1 file, `Data/Prim/skel/item/common/mob_select.bsk`):
  `"BSK " + u32 101` 8-byte header, otherwise identical body, EOF-exact.
- Errata: `silkroad-docs/docs/asset_formats.md` §7's BSK *byte layout* is a
  field-shift misreading (its "linked names" are the previous bone's child
  list); its per-set naming is correct.

