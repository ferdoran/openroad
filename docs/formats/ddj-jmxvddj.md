# Texture (*.ddj) - JMXVDDJ

Container wrapping a standard DDS in a 20-byte header.

Layout below is **derived from openroad's own parser** (`client/src/assets/ddj.rs`)
and confirmed against the corpus figures in the next section. Upstream reference for
the format's existence and field naming:
`SilkroadDoc.wiki/JMXVDDJ` — https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVDDJ

## Layout

| offset | size | type | field | notes |
|---|---|---|---|---|
| 0 | 12 | char[12] | `Signature` | `JMXVDDJ 1000` |
| 12 | 4 | i32 LE | `TextureBufferSize` | counts its own 8 header bytes; **unreliable, do not size the payload from it** — see below |
| 16 | 4 | i32 LE | `TextureType` | `D3DRESOURCETYPE`; only `3` (D3DRTYPE_TEXTURE) occurs in the corpus |
| 20 | rest | u8[] | `TextureBuffer` | a complete, standard DDS — magic and `dwSize == 124` |

`DDJ_HEADER_LEN = 20` in the parser; everything from offset 20 to EOF is handed to
the DDS reader unchanged. A file whose signature does not match, or whose resource
type is unknown, is rejected rather than parsed.

## Corpus verification (openroad, 2026-08-12)

**37,552 `.ddj`** in the user's PK2s (Media 19,245 · Data 16,355 · Particles
1,113 · Map 839). Signature `JMXVDDJ 1000` ×37,551; `TextureType == 3`
(D3DRTYPE_TEXTURE) in 37,551/37,551 — no 4/5 anywhere. Everything past offset 20
is a **complete, standard DDS** (magic + `dwSize == 124` in 100%); recomputing
the full mip chain matches the file size exactly for **37,551/37,551** real DDJs
(the 20 palettized files differ by exactly their 1,024-byte palette).

- **`TextureLength` counts its own 8 header bytes** (payload = `TextureLength − 8`,
  i.e. the field equals `filesize − 12`) — **but it is unreliable**: correct in
  37,193/37,551 (99.05%), too large by exactly `0x100` (×257), `0x10000` (×87),
  or `0x10100` (×14) in the rest, while those files' DDS payloads are complete.
  **Never size the payload from this field** — take the rest of the buffer.
- Pixel formats: `DXT1 22,697 · A8R8G8B8 5,312 · DXT3 4,372 · A1R5G5B5 3,286 ·
  DXT2 693 · R5G6B5 567 · X8R8G8B8 380 · A8B8G8R8 194 · DXT5 22 · P8 20 ·
  A4R4G4B4 6 · X1R5G5B5 2`. **687** of the 693 **DXT2** live in
  `Data/Prim/lightmap` (472 `dun`, 215 `bldg`) — not all 693, as previously
  stated: 4 more are in `Data/Prim/mtrl/etc` and 2 in `Data/Prim/mtrl/nature`.
  The 20 palettized are `Data/Prim/mtrl/mob/god/*`.
- **DXT2's premultiplied alpha is corpus-confirmed** (probe, 2026-08-12):
  decoding every block of all 693 files and testing `max(r,g,b) <= a` holds on
  **11,649,911 of 11,650,048 texels (99.999%)**. Only 2 files violate it at all
  (`c_swamp_tree02_01/02.ddj`, 137 texels total) — within DXT colour-interpolation
  rounding. Straight alpha would not produce that invariant, so the format tag is
  honest and these must be composited with `BlendFactor::One`, not `SrcAlpha`.
  bevy maps DXT2 onto `Bc2` (= DXT3, straight alpha), so they currently render
  too bright; the blend-state change is **not** yet made — see the RE doc.
- **77.3% ship no mips** (`dwMipMapCount == 0` ×29,008). `dwCaps2 == 0` in 100%
  ⇒ no cubemaps or volume textures anywhere.
- 589 distinct sizes; **6,462 non-square and 2,834 non-power-of-two** (20×20,
  21×21, 24×24 icons) — square/POT assumptions break.
- By location: `Map/tile2d` 752 files **100% DXT1** · `Media/minimap` DXT1
  ×4,428 · `Media/icon` A8R8G8B8 ×2,183 / A1R5G5B5 ×2,026 ·
  `Particles/textures` DXT3 ×984.
- **`Media/res_ui/nifenchantwnd.ddj` is not a DDJ** — it starts
  `48 00 00 00 "CNIFEnchantWnd"`, i.e. a 2DT/UI window definition mis-named with
  a `.ddj` extension.

## Loader coverage

`client/src/assets/ddj.rs` hand-decodes the uncompressed formats and passes the
DXTn family to the GPU compressed. As of #289 it also decodes **A4R4G4B4**,
**X1R5G5B5** (bevy rejects both outright) and **P8** — whose 1,024-byte palette
(256 × BGRA8) precedes the index plane, exactly as the corpus size arithmetic
`1024 + Σ level w×h` predicts. `tools`' `dds_to_rgba8` covers the same set, so
`bsr2glb` no longer exports those materials untextured.

A file that is not a DDJ, or whose pixel format nothing can decode, is now
**logged and skipped** rather than panicking the process.

