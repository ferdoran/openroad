# Mesh (*.bms) - JMXVBMS

One renderable mesh: an offset header, then vertex, skinning, index, cloth,
bounding-box, occlusion and navmesh sections.

Layout derived from openroad's parser (`client/src/assets/bms/`, split across
`header.rs`, `vertex.rs`, `mesh.rs`, `skeleton.rs`, `navmesh.rs`, `index.rs`).
Those files are the authoritative field-by-field reference; the tables here give
the section structure. Upstream reference: `SilkroadDoc.wiki/JMXVBMS` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVBMS

## Signature

| size | type | field | notes |
|---|---|---|---|
| 12 | char[12] | `Signature` | `JMXVBMS 0110` |

## Header

Fifteen `u32` offsets and flags (`FIXED_FIELDS = 15 * 4`), then two
length-prefixed strings. Every offset is absolute from the start of the file; an
offset of 0 means the section is absent.

| size | type | field | notes |
|---|---|---|---|
| 4 | u32 LE | `VertexOffset` | |
| 4 | u32 LE | `SkinOffset` | |
| 4 | u32 LE | `FaceOffset` | |
| 4 | u32 LE | `ClothVertexOffset` | |
| 4 | u32 LE | `ClothEdgeOffset` | |
| 4 | u32 LE | `BoundingBoxOffset` | |
| 4 | u32 LE | `OcclusionPortals` | |
| 4 | u32 LE | `NavMeshOffset` | |
| 4 | u32 LE | `SkinnedNavMeshOffset` | |
| 4 | u32 LE | `UnknownOffset` | UNKNOWN |
| 4 | u32 LE | `UnknownUInt` | UNKNOWN |
| 4 | u32 LE | `NavFlag` | |
| 4 | u32 LE | `SubPrimCount` | |
| 4 | u32 LE | `VertexFlag` | selects the vertex layout — see below |
| 4 + n | u32 + bytes | `Name` | |
| 4 + n | u32 + bytes | `Material` | the `.bmt` this mesh draws with |

## VertexBuffer

At `VertexOffset`: a `u32` count, then that many vertices. The record shape is
selected by `VertexFlag` — the `0x400` bit adds a second UV set. Each vertex
carries position, normal and texture coordinates; see `bms/vertex.rs` for the
exact ordering, which is what the loader actually reads.

## SkinningData

At `SkinOffset`: the bone table (names) followed by per-vertex bone bindings —
two bone indices and a weight each, i.e. 2-bone skinning.

## IndexBuffer

At `FaceOffset`: a `u32` triangle count, then that many `u16` index triples.

## Cloth (DyVertexData)

At `ClothVertexOffset` / `ClothEdgeOffset`: the dynamic-cloth vertex and edge
lists. openroad parses past them but does not simulate cloth.

## BoundingBox

At `BoundingBoxOffset`: the axis-aligned min and max as two `f32 x 3`.

## OcclusionCullingData

At `OcclusionPortals`: the portal polygons used for occlusion culling.

## NavMeshObj

At `NavMeshOffset`: the object-local navmesh — vertices, cells, edges and an
`OutlineLookupGrid` giving an origin, width and height.

### OutlineLookupGrid cell size — measured, and not reliably recoverable

The layout gives `Origin`, `Width` and `Height` but no cell size, so using the
grid as a spatial index means inferring one. Measured over all 2103 nav meshes
in Data.pk2 (v1.188):

- `CellCount == Width * Height` in **2103/2103** meshes.
- `Origin` equals the nav mesh's own vertex XZ bounding-box minimum in
  **2103/2103** meshes (note: the *nav mesh* bbox, not the render bbox).
- Cell size is **100 units**: `extent / Width` never exceeds 100.0 and reaches
  it exactly on meshes whose extent is a round multiple.

The count rule, however, does not follow from the extent. Both `ceil(extent/100)`
(2099/2103) and `floor(extent/100) + 1` (2096/2103) fail, and the two rules fail
on *disjoint* sets — meshes with an identical 600.00-unit extent exist with
`Width` 6 (`dun/wchina/donhwang_cv/.../passage01_bottom05.bms`) and with
`Width` 7 (`bldg/china/jangan05/cj5_tem_main_stair.bms`). The grid was therefore
authored from some quantity other than the nav mesh vertex bounds.

Consequence: the client parses the grid but does **not** query it. Deriving the
cell size per mesh as `extent / Width` would tile the bounding box, but the
resulting cell indices would drift from the authored ones wherever the two rules
disagree, and reading the wrong cell's outline list silently drops walls —
a correctness bug in exchange for a marginal speed-up. The broad phase the
client actually uses is `plugins/nav/index.rs`, a runtime grid over object
instances.

## SkinnedNavMesh

At `SkinnedNavMeshOffset`. The client reads this from the visual section rather
than the navigation one. Three counted arrays, all field meanings **UNKNOWN**:

| size | type | field | notes |
|---|---|---|---|
| 4 | i32 LE | `Count0` | |
| 12 x count | f32 x 3 | `Structure0` | UNKNOWN — shaped like a position |
| 4 | i32 LE | `Count1` | |
| 2 x count | u8 x 2 | `Structure1` | UNKNOWN — shaped like a bone-index pair |
| 4 | i32 LE | `Count2` | |
| 6 x count | u16 x 3 | `Structure2` | UNKNOWN — shaped like a triangle |

The file ends after the last array.

## Corpus verification + corrections (openroad, 2026-08-12)

Structurally verified over all **22,872** `.bms` in the user's PK2s by section
adjacency (each section's computed end == the next header offset, chain ending
exactly at EOF): **22,648 / 22,872 = 99.02%** exact. Signatures: `JMXVBMS 0110`
×22,852, `0109` ×20. Universal invariants: `subPrimCount == 1`,
`unkUInt0 == unkUInt2 == 0`, `SkinnedNavMeshOffset == 0`, `Unknown9` count == 0.
VertexFlag histogram: 0x0 ×17,467 · 0x400 ×5,399 · 0x800 ×5 · 0x1000 ×1.
Nav-bearing 2,625 (11.5%) · cloth 949 · occlusion portal 156 · skinned 5,741.
Note: JMX-File-Editor has **no** JMXVBMS serializer (grep 0 hits), so this
corpus adjacency chain — not a round-trip writer — is the layout's proof.

Corrections to the layout above:

- **Skinning stride is version-split.** `0110` = 2 influences, 6 B/vertex
  `(u8 index, u16 weight)×2` (as documented). **`0109` = 4 influences,
  12 B/vertex** — all 15 skinned 0109 files carry exactly `6·vertexCount`
  extra bytes, pattern `(00,FFFF)(FF,0000)(FF,0000)(FF,0000)`. Boneless 0109
  files store a plain `u32 0`.
- **Morph data (VertexFlag 0x800) is 36 bytes per vertex** — the "TODO: need
  sample" is resolved: 5 samples exist (`Data/Prim/mesh/{cos,mob/event}/event_festival_*.bms`)
  and 36 B/vertex is the only size that keeps the adjacency chain exact.
- **NavFlag bit 3 (value 8) exists** (15 files) and is layout-neutral — the
  flag list above (1/2/4) is incomplete. Observed NavFlag values: 0, 4, 5, 6,
  7, 8, 14; each parses EOF-exact only under the documented conditionals.
- `unkUInt3` (the dword after Material) is **not** always zero — nonzero in
  11,709 files, and equals |{v : Int0 ≠ 0xFFFFFFFF}| in 740 of 800 sampled
  files (vertex weld/chain-table hypothesis, see the unit doc).
- Nav content: `cell.flag == 0` in all 50,279 sampled cells; edge flags
  {0,1,3,8,16,128,131,144} (the NVM edge-flag space).
- **224 damaged files** — mostly repack-added avatar/pet content, but also the
  retail `Prim/mesh/char/china/man/man_call.bms`: header offsets from
  FaceOffset on are understated by a per-file constant (max observed 21). The
  evidence says a bone name was *appended* rather than edited —
  `delta == 4 + len(one of the bone names)` in 194 of the 224. Re-deriving
  later offsets from `actual_skin_end − face_off` recovers **221**; 3 resist
  (`avatar_m_ghost_captain_part2.bms`,
  `avatar_w_2012_new_devil_wing_part{1,2}.bms`) because their `vertex_offset`
  is shifted too, leaving the vertex section unreachable from the header.
  These are live content, not dead data: 216 of the 224 are referenced by 85
  `.bsr`, 17 of those through `charactervisualchange.txt`, and
  `BsrLoaderV2::load` fails the entire resource when one mesh fails.
- `silkroad-docs/docs/asset_formats.md` §BMS is wrong on several points
  ("navmesh 0109-only" — 2,622/2,625 nav files are 0110; "44-byte stride both
  versions"; "u32 vertex color" trailer). Do not cite it for BMS.

## Loader status (2026-08-12, #279)

`client/src/assets/bms/` now implements the three corrections above:

- **Version-keyed skinning stride.** The signature's last four bytes select 4
  influences (`0109`) or 2 (`0110`). The separation is exact — 15/15 skinned
  0109 close the section chain only at 12 B/vertex, 5,502/5,502 skinned 0110
  only at 6 B, with no file valid under both — so no fallback heuristic is
  needed. Influences 3 and 4 are `(0xFF, 0)` in 2,965/2,965 sampled vertices,
  so they are consumed for the stride and dropped rather than blended; reading
  them at the 6 B stride previously left every second vertex with
  `index1 == index2 == 0xFF`, whose zero weight sum became NaN joint weights.
- **Morph records are 36 B.** The old code consumed 64 (`copy_to_bytes(32)`
  advances, and it advanced again), which over-ran the section and made all
  five `event_festival_*` meshes fail.
- **Malformed input yields `Err`, never a panic or an abort.** `parse_bms`
  validates the signature, bounds-checks every section offset and string
  length, and rejects any count whose records cannot fit in the remaining
  bytes *before* it reaches `Vec::with_capacity`. That last guard is the one
  that matters most: the corpus contains counts near `u32::MAX` (a ~24 GiB
  face vector, a ~135 GiB vertex vector), and `handle_alloc_error` aborts the
  process — which, unlike a panic, Bevy's per-loader `catch_unwind` cannot
  contain. The offset re-basing described above is applied at the same time.

Corpus outcome (22,872 files): **22,628 correct / 32 silently wrong / 212
panicking** before, **22,869 correct / 0 wrong / 0 panicking** after, with the
3 unrecoverable files rejected cleanly.

**Not done — follow-up.** Ignoring the offset table entirely and reading the
nine sections back to back from the end of the header is EOF-exact on
**22,872/22,872 = 100%**, including those 3. Section order equals header-offset
order in every file, so the chain is unambiguous. It needs cloth-vertex,
cloth-edge, occlusion-portal and `unknown_offset` skip-parsers the loader does
not have yet, so it belongs in its own change.
