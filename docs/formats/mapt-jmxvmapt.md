# MapTexture (*.t) - JMXVMAPT

A baked terrain lightmap: a per-tile grid plus an embedded DDS.

Layout derived from openroad's parser (`client/src/assets/t.rs`) and
corpus-verified across all 4,669 `.t` files in Map.pk2. Upstream reference:
`SilkroadDoc.wiki/JMXVMAPT` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVMAPT

## Layout

| offset | size | type | field | notes |
|---|---|---|---|---|
| 0 | 12 | char[12] | `Signature` | `JMXVMAPT1001` |
| 12 | 9216 | u8[96x96] | `LightMap` | per-tile lighting; 96 tiles x 20 units = 1,920 |
| 9228 | 4 | i32 LE | `LightMapBufferSize` | bytes |
| 9232 | 4 | i32 LE | `LightMapTextureType` | `D3DRESOURCETYPE`; `3` in 4,669/4,669 |
| 9236 | rest | u8[] | `LightMapBuffer` | a complete DDS — **DXT1** in every corpus file |

The tail from `LightMapBufferSize` on is byte-identical to a `JMXVDDJ` minus its
12-byte signature, so the same DDS path decodes both.

The 96x96 grid's original role was gating whether dynamic objects cast or receive
terrain shadow. openroad parses and retains it but does not yet consume it.

### OpenRoad implementation

Parsed by `client/src/assets/t.rs` (`JMXVMAPT` asset + `TLoader`, registered for the `.t`
extension). Regions load their lightmap alongside the `.m` heightmap (`map://{z}/{x}.t`, see
`plugins/map/terrain/mod.rs`).

- The `lightMapBufferSize` / `lightMapTextureType` / DDS tail is byte-identical to a `JMXVDDJ`
  texture minus its 12-byte signature, so the embedded DDS decodes through the shared
  `assets::ddj::dds_buffer_to_image` path. It is exposed as a labeled `"lightmap"` sub-asset.
- The decoded texture is bound into `TerrainBlockSplatMaterial` (bind-group binding 5) and sampled
  in `assets/shaders/terrain_splat.wgsl` at **region-local UV** (positive-modulo of world position
  over the 1920-unit region; world X is negated because region entities are X-mirrored), then
  through a shared scale/offset (`TerrainRenderParams`, Unity `_ST` style) so the mapping is
  data rather than shader code. It is multiplied into the ground albedo before
  lighting, i.e. treated as a baked static-sun/shadow occlusion layer on top of the dynamic PBR
  sun + `terrain_ambient_ratio`, rather than replacing the dynamic lighting.
- Regions without a `.t` (or a lightmap that fails to decode) fall back to a shared 1×1 white
  texture, making the multiply a no-op.
- **Pixel format (corpus-verified 2026-08-09):** all 4669 `.t` files in Map.pk2 embed a **DXT1**
  DDS with `lightMapTextureType = 3` (`probe_t_lightmap_format_census` in `client/src/assets/t.rs`).
  DXT1 has no alpha channel, so the lightmap **cannot be RGBM-encoded** — the mobile port's
  `rgb * a * hdrScale` decode (see `docs/rendering-mobile-shader-comparison.md`) has no
  counterpart in 1.188 data, and the shader's `.rgb`-only sampling loses nothing. Even DXT1's
  1-bit punch-through alpha is unused: a block-level scan of every mip-0 block in the corpus
  (84.8M blocks) found zero `c0 <= c1` blocks with an index-3 texel — alpha is constant 1.0.
- The 96×96 `lightMap` per-tile grid is parsed and retained (`JMXVMAPT::tile_light`) but **not yet
  consumed** — its original role (gating dynamic object terrain shadows) is future work.
- **Calibration pending:** the V axis (world Z) vs. the DDS row order may need flipping; the wiki
  images above are noted as possibly flipped/rotated. The flip is now a data change: toggle
  `graphics.terrain.lightmap_flip_v` in `config.yaml` (or the render-debug panel live) — it rides
  the shared `TerrainRenderParams` UV scale/offset (binding 6) instead of a shader edit.

## Correction + corpus verification (openroad, 2026-08-12)

**`TextureLength` is inclusive of its own 8 header bytes** — the DDS payload is
`TextureLength - 8` bytes, i.e. `TextureLength == (filesize - 9240) + 8` for all
5,145 valid corpus files. The SilkroadDoc wiki (and the ImHex pattern copied
above) overrun by 8 bytes; srodevs-docs `jmxvmapt.md:16,32` has it right.

Corpus (5,146 `.t` in the user's Map.pk2): `JMXVMAPT1001` ×5,145 —
**EOF-exactness 5,145/5,145 (100%)** under the inclusive rule, **0/5,145** under
the wiki's. `TextureType == 3` (D3DRTYPE_TEXTURE) in 100%; embedded DDS is
**DXT1 512×512 in 100%**; every valid file is exactly 140,436 B
(12 + 9,216 + 8 + 131,200). ShadowMap bytes are strongly quantised — 255, 153,
229, 204, 178, 0 cover >99%, i.e. a handful of discrete shade levels rather than
a continuous field.

**One corpus data bug:** `Map/88/83_13.t` is a **MAPM file with a `.t`
extension** (92,712 B, signature `JMXVMAPM1000`) — our loader already rejects it
cleanly via its signature check. 507 files carry a `_NN` suffix (ids 3, 5, 13,
49, 61, 129, 513, 769, 781, 2061), each coexisting with a plain `.t` and none
having `_NN.m`/`.o`/`.o2` siblings; the object-UID and ObjID explanations were
tested and rejected (see the unit doc's UNKNOWNs). 10 regions have a `.m` but no
plain `.t`, so the white-lightmap fallback is live, not theoretical.

Loader notes: `t.rs` is the family's model loader (signature + length checked,
typed errors, no panics); it ignores `TextureLength` in favour of "rest of
file", which is correct here but cannot detect a truncated tail.