# Resource (*.bsr) - JMXVRES

The top-level object description: which meshes and materials make it up, its
skeleton and animations, its collision mesh, and a **mod palette** of render
modifiers (particles, sounds, UV animation, blending, cloth).

Layout derived from openroad's parser (`client/src/assets/bsr/`, chiefly
`bsr.rs` and `resource.rs`). Those files are the authoritative field-by-field
reference — the sections below give the structure, and the verified findings
further down are openroad's own. Upstream reference:
`SilkroadDoc.wiki/JMXVRES` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVRES

## Sections

| section | contents |
|---|---|
| Signature | `JMXVRES 0109` |
| Header | absolute offsets to each section below, plus flag words |
| ObjectInfo | type/category pair, name, and object flags |
| Material list | counted list of `.bmt` paths |
| Mesh list | counted list of `.bms` paths |
| Skeleton | optional `.bsk` path plus an attachment bone |
| Animation | animation group table — group name, then per entry a `.ban` path and an animation type |
| Mesh groups | named groups of mesh indices (`PrimitiveGroupData`) |
| Animation groups | named groups of animation entries (`PrimitiveAnimationGroupData`) |
| Collision mesh | a `CollisionMesh` — see `bsr/collision_mesh.rs` |
| Mod palette | the render modifiers below |

Strings are a `u32` byte length plus CP949 bytes, as elsewhere in the JMX family.

## Mod palette

Each mod is tagged and attaches to either the whole object or one material
index. openroad models these entries:

| mod | fields |
|---|---|
| `ParticleMod` (effect) | effect path, optional bone, positional offset, delay (ms), a night-only flag, scale, and an owner |
| `SoundMod` | sound path, key time (ms), animation group name, animation type |
| `TexAniMod` | optional material index, UV scroll speed (`Vec2`) |
| `BlendMod` | optional material index, alpha mode |
| `DyVertexMod` | optional material index — cloth/dynamic vertices |

Two mod **sets** group these: a system set and an animation set.

An `EnvMap` mod is what re-interprets a material's alpha channel as a metallic
sheen mask rather than transparency — see the flag discussion in
[`bmt-jmxvbmt.md`](bmt-jmxvbmt.md) and the verified findings below.

## Mod Palette: verified findings (openroad, from real Data.pk2 bytes + sro_client.exe)
The wiki's ModData payload layouts do NOT match the real bytes; the client
scans for entry tags instead of walking the palette (`client/src/assets/bsr/bsr.rs`).

ModData entry tags are `type << 16`; the exe's creator dispatch (@0xc797f0)
switches on the high word:

| Tag | Type | Notes |
|--|--|--|
| `0x00000000` | Material | raw D3D render states (e.g. tree leaves: srcblend 5, dstblend 6, alphatest GREATEREQUAL 0x80) |
| `0x00010000` | TexAni | continuous UV scroll (waterfalls, canal water — see below) |
| `0x00030000` | Particle | .efp reference + bone/offset anchor, `night_only` byte (lamps), header Float0 = uniform effect scale |
| `0x00040000` | EnvMap | texture alpha = env/sheen mask (see below) |
| `0x00070000` | ProgEquipPow | +N enhancement glow — never present in any .bsr; created at runtime from `resinfo/itemtypenumber.txt` |

Every entry starts `tag u32, Float0 f32` — Float0 is a generic IModData
header field (0.5 on every mod type in the census).

**Verified (openroad, exe RE 2026-07-29, corrected 2026-07-30):** for
Particle mods Float0 is a **uniform scale on the spawned effect geometry**.
The client's effect-wrapper creator (0xc816c0) stores it at wrapper+0xc0
and it reaches the effect's SetScale vfunc (0xcaa0b0), scaling every
plate/instance. The effect-side base scale (+0x264) defaults to 1.0 and no
bone/resource scale enters the path — Float0 is the factor between
effect-local and world units (garment talisman ward glows would render 2×
without it). **The attach offset is consumed unscaled**: the 2026-07-29
reading of the pre-step (0xc81190) as multiplying the local offset by
Float0 is contradicted by world data (2026-07-30 —
`res/artifact/china/jangan/cj_lamp01.bsr` authors its flame at
(0, 16.32, −0.107) on a 19.95-unit-tall lamp: only the unscaled offset
lands in the lantern cage, halved by the ubiquitous Float0=0.5 it hangs
mid-post; likewise `cj_weap_chimn.bsr`'s smoke (−5.73, 57.85, −36.47)
would sit inside the roof).
Two header layouts follow the float. Ambient (system) sets carry a
constant `0x30` block-size u32 (sometimes preceded by one extra u32) and a
`0xFFFFFFFF` unset-sentinel before the entry list; animation-linked sets
(e.g. isyutaru's death smoke) go **straight from the float to the entry
payload** — no 0x30 block. Because the raw tag bytes also occur as data
inside other palette structures, openroad validates a tag match either
*structurally* — it sits at a validated mod-set header's first-mod
position (`header + 16 + nameLen`, the position `scan_mod_set_headers`
already range-checked; covers both layouts) — or, for later mods of a
multi-mod set, via the 0x30 marker in one of the two u32 slots after the
float. Requiring the 0x30 marker alone silently dropped the
animation-layout scales back to 1.0.

**Material** (tag `0x00000000`): raw D3D render states + an animated
material-color gradient. Verified layout (census 2026-07-23: 478 entries
across 463 resources parse cleanly, `parse_material_mods` in
`client/src/assets/bsr/bsr.rs`):

```
u32 tag 0 | f32 Float0 (0.5) | i32 Int0 (1..8) | i32 Int1 | i32 mtrlIdx (-1 = all) |
12 zero bytes | u32 durationMs | u32 flag | u32 | u32 gradientKeyCount |
keys { u32 timeMs, f32[4] color } |
if flag & 4 { u32 curveKeyCount, keys { u32 timeMs, f32 value } } |
u32[4] | u8[12] render states | f32 | u32
```

Render-state byte 0 = `D3DRS_SRCBLEND`, byte 1 = `D3DRS_DESTBLEND`
(D3DBLEND values). Corpus: (5,6) SRCALPHA/INVSRCALPHA ×333, (5,2)
SRCALPHA/ONE additive ×86, rest exotic/rare. Bytes 8/9 = alpha-test
ref + `D3DCMP` func (0x80, 7 = GREATEREQUAL in the corpus) — verified in
the exe's `CRTModMtrl::Apply` (@0xc82214): one payload bool gates
`ALPHATESTENABLE` with `ALPHAREF` ← byte 8, `ALPHAFUNC` ← byte 9, a second
gates `ALPHABLENDENABLE` with `SRCBLEND`/`DESTBLEND` ← bytes 0/1 (bytes
2..7 feed `SetTextureStageState` stage values); bytes 10/11 are constant
(100, 200). **Every TexAni carrier pairs its scroll with such a Material
entry** — waterfall sheets are (5,6) alpha-blended (Jangan cj_waterfall02)
or (5,2) additive (hot springs, air-garden water), NOT alpha-masked;
rendering them masked turns the soft water gradients into hard stringy
cutouts. Openroad maps (5,6) → `AlphaMode::Blend`, (5,2) → `AlphaMode::Add`
on the swapped TexAni material (`SroResource::blend_mods`) and leaves
everything else on the default mask. Known gap: a few *items* carry a
(5,6)+alphatest Material mod without any TexAni (china shield_15/16/17/21,
avatar/event models unreferenced by itemdata) — the original renders those
alpha-blended; openroad currently ignores Material mods on non-TexAni
resources.

**TexAni** (tag `0x00010000`): continuously animates the UVs of the meshes
using the target material. Verified layout (full-corpus scan of all 7,715
res/ .bsr, 2026-08-15: **282 entries across 271 files**, `parse_texani_mods`
in `client/src/assets/bsr/bsr.rs` matches the palette walk exactly — 0 false
positives, 0 false negatives), 116 bytes from the tag:

```
u32   tag             0x00010000
f32   Float0          0.5
i32   Int0            1 or 2
i32   Int1            0x110 or 0x310
i32   JMXVBMT_MtrlIdx -1 = every material of the set, else index into the
                      .bmt material list (observed 0..30)
i32   Int3            0
i32   Int4            0
u8[4] flag bytes      0
u32   UnkUInt06       0 or 1 (corpus 0 x164 / 1 x118) — NOT header padding;
                      1 = the transform drives the MultiTex second stage
u32[4] unkUInt7..10   (1,1,1,1), (1,10,1,1) or (0,1,1,1)
f32[16] AnimationMatrix  row-major 4x4 D3D texture-transform
```

The matrix is the per-second UV transform: in 281 of 282 entries only the
classic D3D UV-translation slots `_31`/`_32` (row-major indices 8/9) are
nonzero — index 8 = U scroll, index 9 = V scroll, in uv/sec. Waterfalls
scroll V negative (down the sheet, −0.3…−2.78; `pokpo` = waterfall),
dungeon canals/pools scroll U (±0.02…0.2); a few mobs (sandman,
redeyeghost) and the nasrun avatar scroll their body textures. The single
exception (`euro_esteuro_fountain01.bsr`) carries a 2×2 linear block
(rotation/shear swirl) with zero translation — unhandled (openroad warns
and skips it). MultiTex (`0x00010001`) / MultiTexRev (`0x00010002`) are
distinct tags (texture frame-flip) and remain unhandled; none appear in
the waterfall corpus.

Openroad runtime: the loader keeps always-on ("ambient" set) entries whose
`UnkUInt06` is 0 (see below) as `SroResource::texani_mods`; spawn resolves them to mesh indices via the
.bmt material names and `plugins/texani.rs` swaps those meshes onto
`SroUvScrollMaterial` (`shaders/sro_uv_scroll.wgsl`, shader-clock UV
offset).

**EnvMap** (tag `0x00040000`, only in the `"ambient"` system set — weapons,
metal armor; never characters): marks the resource's texture alpha as a
per-texel environment/sheen mask instead of transparency, so meshes render
opaque. The payload dword at offset **+24** is a flags field (only values
1 and 3 occur across Data.pk2: 815 vs 635 resources):

- **bit 2 set** ⇒ `CRTModEnvMap::Apply` (@0xc836b0) enables alpha test
  `GREATEREQUAL` ref **1**: texels with alpha exactly 0 are cutouts
  (e.g. degree-4/8 glaive blade shapes punched out of a shared atlas, or
  the large see-through areas of the degree-7/8 china shields — 28.5% /
  15.3% of their textures), everything above stays opaque sheen.
- bit 2 clear ⇒ alpha test disabled, fully opaque.
- bit 1 (always set) gates the env sphere-map stage setup itself.

No payload field carries a usable per-resource sheen *tint/metallicity*
(one blend-alpha-like byte varies within 0x70..0x7c). Full-corpus Float0
census (2026-08-09, `probe_envmap_moddata_census` in `bsr.rs`, 2094
validated entries): Float0 is **0.5 on ~89%** of entries and **exactly 0
on ~11%** — the zeros cluster systematically on `_sa`-suffixed avatar
clothes/heavy sets (`res/item/*/…_sa.bsr`) plus one artifact rock. Whether
Float0 = 0 means "sheen strength zero" on those resources is **UNKNOWN**
(needs an original-client comparison); openroad currently applies a uniform
strength to every EnvMap resource. A further ~13 entries carry denormal
bit patterns, most likely mod-variant misalignment noise, not data.

The EnvMap is *usually* the ambient set's first mod, but **not always**:
avatar/event items put other mods first — china shield_13/14 lead with a
Particle mod (attached .efp), shield_12/15/16/17/21 with a Material mod —
so detection must scan the set's whole mod span (see caveat below).

Openroad runtime note: the cutout is a shader `discard` in
`sro_sheen.wgsl`, and because every camera runs a **depth prepass**, the
cutout variant's base `StandardMaterial` must be `AlphaMode::Mask(1/255)`
(not Opaque) so the stock prepass/shadow shaders discard the same texels —
an opaque base writes prepass depth for the discarded texels, blocking
everything behind them into opaque black holes (the degree-7/8 shield
bug, fixed 2026-07-30).

**Render pipeline** (verified in `CRTModEnvMap::Apply` @0xc83690,
sro_client.exe, 2026-07-14): `D3DRS_TEXTUREFACTOR` = per-material color at
[material+0x1e4] if set, else the global scene ENVIRONMENT color
(@0x1113190); then fixed-function stages

| Stage | Texture | COLOROP |
|--|--|--|
| 0 | sphere map (`prim/mtrl/etc/spheremap_gray.ddj`, coords = view-space normal `xy*0.5+0.5`) | `MODULATE(TEXTURE, TFACTOR)` |
| 1 | diffuse map (model UV) | `MODULATEALPHA_ADDCOLOR(TEXTURE, CURRENT)` = `diffuse.rgb + diffuse.a × stage0` |
| 2 | — | `MODULATE2X(DIFFUSE, CURRENT)` — vertex lighting multiplies last |

i.e. the chrome-probe term joins the **albedo before the lighting
multiply** — it darkens in shadow like any surface (not additive glow).

> **Scope correction (round 2, F12).** This three-stage chain answers the **EnvMap sheen** question
> and only that one. The **terrain lightmap** is a different pipeline with its own vertex-shader
> permutation (`#define VS_UV_%s_LIGHTMAP` in `FUN_00bf67c0`'s `shader\vss0.c`/`vss2.c` builder),
> so the lightmap combine op is **still `[U]`** — do not cite this table for it.

**Detection caveat**: a loose byte scan for the tag false-positives on
`00 00 04 00` patterns inside animation data of large resources
(character bodies!). Validate the `(typ 2, "ambient")` set header around
the name, then scan the set's mod span (up to the next set header — mod
payloads are variable-length) for the tag with the generic IModData
header fields validated: Float0 in 0..=1, two small ints, mtrlIdx in
-1..=63, a zero pad dword (`envmap_mod_alpha_test` in
`client/src/assets/bsr/bsr.rs`). An earlier first-mod-slot-only census
counted **1,344** resources (1,339 `res/item`, rest: 2 dun, 1 each
interface/cos/bldg) — an undercount, since it missed later-mod carriers
like shield_12..14. Buildings, nature, mobs and characters have **no**
EnvMap.

Related pages:
[[ModData]]
## Walk-graph field order: corpus-decisive (openroad, 2026-08-11)

The AniGroup walk-graph order documented above (`walkPointCount`, `walkLength`,
then points — JMX-File-Editor `PrimAniTypeData.cs:29-46` round-trip + wiki) is
confirmed **decisively for this build** by a 25,757-entry probe over 7,714
`.bsr`: length-first gives 100% monotone normalized x with last-x = 1.0 in 99.9%
of entries, and `walkLength > 0` appears only on locomotion types (WALK ×561
6.8–666.4, RUN ×997 3.2–1499.4, WALK_BACK ×33, READY01 ×4). The alternative
points-first reading (what `client/src/assets/bsr/bsr.rs:290-296` currently
does) yields last-x = 1.0 in 0.0% of entries and a flat ≈1.0 "length" — ruled
out.

## Mod palette: full-walk census (openroad, 2026-08-12)

A re-implementation of the JMX-File-Editor ModData serializers
(`Silkroad/Data/JMXVRES/ModData/`, all 12 types via `ModDataFactory.cs:13-24`)
walking `SystemModSets + AniModSets + ResAttachable` consumes **7,714 of 7,715**
`.bsr` byte-exactly to EOF (99.987%). Since any wrong payload size desyncs and
cannot land on EOF, this is round-trip-grade proof for all twelve payload
layouts.

**This refines the "the wiki's payload layouts do NOT match the real bytes"
note above:** the palette *is* walkable and those layouts *are* exact. The
tag-scan strategy remains a valid safety net — nothing in the verified findings
(Material render states, TexAni matrix, EnvMap flags, Particle Float0) is
contradicted; the walk reproduces all of them — but it **undercounted carriers**
(see the TexAni scan note below, fixed 2026-08-15).

Complete tag histogram over the corpus:

| Tag | Type | Entries | Files |
|--|--|--:|--:|
| `0x00000000` | Material | 816 | 796 |
| `0x00010000` | TexAni | 282 | 271 |
| `0x00010001` | MultiTex | 120 | 120 |
| `0x00010002` | MultiTexRev | 10 | 9 |
| `0x00030000` | Particle | 2,305 | 1,302 |
| `0x00040000` | EnvMap | 2,210 | — |
| `0x00040001` | BumpEnv | 25 | 25 |
| `0x00050000` | **Sound** | 23,159 | 615 |
| `0x00060000` | DyVertex | 1,015 | 1,015 |
| `0x00060001` | DyJoint | **0** | 0 |
| `0x00060002` | DyLattice | **0** | 0 |
| `0x00070000` | ProgEquipPow | **0** | 0 |

Generic `IModData` header (28 B after the tag, `IModData.cs:22-34`):
`f32 Float0 | i32 Int0 | i32 Int1 | i32 MtrlIdx (-1 = all) | i32 Int3 |
i32 Int4 | u8[4]`; `Float0 == 0.5` in 100% of entries.

**DyVertex scan (added 2026-08-16, #283).** `parse_dyvertex_mods` reads the
soft-body flag (`0x0006 0000`). It is the one ModData type with **no payload**:
exactly 32 B, the tag plus the generic base, so `MtrlIdx` is everything it
says. With no string to anchor on, the base header *is* the candidate test —
exact tag, `Float0 == 0.5`, `Int0 ∈ {1,2}`, `Int3`/`Int4`/flag bytes zero,
`MtrlIdx ∈ -1..=255`, and above all **`Int1 == 0`, which the census reports as
unique to this type** (TexAni `0x110`/`0x310`, MultiTex `0x100`/`0x300`). The
result is surfaced on `SroResource::dyvertex_mods` and **not simulated**: the
payload it activates is the mesh-side `DyVertexData` block
(`docs/formats/bms-jmxvbms.md:121`) and this tree has no cloth simulation, so
the honest state is "the flag is loaded and reachable", not "cloth works".

**Sound scan (added 2026-08-16, #283).** `parse_sound_mods` scans the palette
for the innermost record of a Sound entry —
`u32 hasValue | u32 pathLen | path(.wav) | i32 keyTimeMs | u32 eventLen | event`
— the same way the Particle scan anchors on `.efp`, so it does not depend on
the still-`[U]` parts of the nested container (the 11 config dwords). Census
over the extracted corpus: **615 files / 45,086 tracks**, against the EOF-exact
walk's 615 / 45,098 (99.97%); the 12 misses carry CP949 (non-ASCII) paths.
Owning set types: 39,783 animation-linked (typ 1), 5,300 external (typ 0), 3
ambient (typ 2). `eventLen == 0` is legal (602 tracks) and `keyTimeMs` maxes at
5,856 ms. Two data notes that cost real sounds if ignored: 105 of the 1,896
distinct paths carry a leading `sound\` component that has no counterpart in
the archive (104 resolve once it is dropped), and the client keeps only the
typ-1 tracks — those are the ones a playing animation can drive.

**TexAni scan (fixed 2026-08-15, #282/#443):** the scan used to reject an entry
when any of `+20/+24/+28/+32` was nonzero, but `+32` is
`ModDataTexAni.UnkUInt06` (`ModDataTexAni.cs:23`), not base padding — corpus
histogram `0 ×164, 1 ×118`. That rejected **117 files / 118 entries** (43% of
real carriers, 0 false positives), the animated robe-trim family
(`avata_*_amalrun_*`, `*_11_set_*_fullset`, `alex_library_01`,
`avatar_electus_m_witch_hat`), the degree-14 weapons, `devil_dark`,
`flame_captain`, three waterfalls and the `dun_cen_s0[123]` canals. The
zero-check now covers `+20/+24/+28` only and `+32` is bounded to `{0,1}`.

`UnkUInt06 == 1` is not a free flag: in **117 of those 118** entries the same
file also carries a **MultiTex** (`0x00010001`) mod, in 117/118 of them on the
*same* `MtrlIdx` (only exception `res/nature/particle/dun_cen_s02.bsr`, TexAni
−1 vs MultiTex 0), while only **1 of the 164** `UnkUInt06 == 0` entries sits in
a MultiTex file. So the flag reads as "the transform applies to the MultiTex
second stage". openroad has no MultiTex consumer, so
`client/src/assets/bsr/loader.rs` drops those entries from `texani_mods`
instead of scrolling the base diffuse in their place; the parser still reports
them (`RawTexAniMod::multi_tex_stage`) for the census. **UNKNOWN:** the exact
`CRTModTexAni`/MultiTex stage semantics — revisit when a MultiTex consumer
exists.

**BumpEnv (`0x00040001`) does not reach the sheen path:** `envmap_mod_alpha_test`
(`bsr.rs:436-505`) matches only `0x0004_0000`, and none of the 25 BumpEnv
resources carries a plain EnvMap entry as fallback — Jangan Imperial statues,
`cj3_lion0[12]`, `interface_idol_*`, `chakji*`, potions lose it.

Signature histogram: `JMXVRES 0109` ×7,711 · `0108` ×3 (walk fine under the 0109
layout) · `0107` ×1 (`res/npc/npc/tt.bsr`, obsolete test asset whose palette
layout differs — recommend log-and-skip). `ResAttachable` is emitted for
**NPC (objtype 1) resources too** (×113 with combo block), which
JMX-File-Editor's own `ResAttachable.cs:23` misses.

## Animation group names: the character census (openroad, 2026-08-14)

`PrimAniGroupOffset`'s `groupName` is the stance selector — the client picks a
group by name and falls back to `default` for any type the group lacks
(`SroResource::find_animation_in_group`). Parsed out of every `Res/Char/**.bsr`
in the user's `Data.pk2`, the full set is:

- **China**: `spear`, `bow`, `sword`, `default`, **`cart`**, `avatar_wing`,
  `avatar_nasrun1..3`
- **Europe**: `onehand_staff`, `bow`, `onehand_sword`, `default`, **`cart`**,
  `twohand_sword`, `dagger`, `dual_axe`, `harf`, `twohand_staff`, `avatar_wing`,
  `avatar_nasrun1..3`

All but `cart` and `avatar_*` are weapon classes (`ItemDataRow::animation_group`
maps itemdata TypeIDs onto them). **`cart` is the mounted/riding stance** — the
only non-weapon, non-avatar group — and holds exactly three entries: type `0` →
`cart_stand01.ban` (breathing idle), type `1` → `cart_walk.ban`, type `7` → the
same walk clip. There is no `horse`/`ride`/`riding` group in the corpus; the
name comes from the trade carts riding shipped with.

Cross-race quirk: EU rigs list the **China** clip paths
(`prim\ani\char\china\man\cart_*.ban`) against `europeman_skel.bsk`. `.ban`
tracks are bone-name keyed, so the sharing is deliberate rather than a data bug;
woman rigs point at `char\china\woman\cart_*.ban`.

Other well-known `default`-group ids seen on `chinaman_Adventurer.bsr`, none of
them wired in our client: `13/14/15` the ground-sit triad
(`sitground`/`sitbreath`/`sitstand`), `50..57` `cm_emot_act_*` emotes, `60`
`man_pose`, `61` `standcity02`, `62..66` the knockdown chain, `79` stun, `80`
vendor.
