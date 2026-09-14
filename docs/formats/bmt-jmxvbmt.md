# Material (*.bmt) - JMXVBMT

A set of materials: colours, a flag word, and a diffuse-map path.

Layout derived from openroad's parser (`client/src/assets/bmt/material.rs`).
Upstream reference: `SilkroadDoc.wiki/JMXVBMT` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVBMT

## Layout

| offset | size | type | field | notes |
|---|---|---|---|---|
| 0 | 12 | char[12] | `Signature` | `JMXVBMT 0102` |
| 12 | 4 | u32 LE | `MaterialCount` | |

Then `MaterialCount` records. Colours are four `f32` (RGBA); strings are a `u32`
byte length followed by CP949 bytes.

| size | type | field | notes |
|---|---|---|---|
| 4 + n | u32 + bytes | `Name` | |
| 16 | f32 × 4 | `Diffuse` | |
| 16 | f32 × 4 | `Ambient` | |
| 16 | f32 × 4 | `Specular` | |
| 16 | f32 × 4 | `Emissive` | |
| 4 | f32 LE | `Power` | D3D specular power; only meaningful with flag `0x4` |
| 4 | u32 LE | `Flag` | see the verified flag table below |
| 4 + n | u32 + bytes | `DiffuseMap.Path` | |
| 4 | f32 LE | `DiffuseMap.Float` | UNKNOWN — often mirrors a colour component |
| 1 | u8 | `DiffuseMap.Unk0` | UNKNOWN |
| 1 | u8 | `DiffuseMap.Unk1` | UNKNOWN |
| 1 | u8 (bool) | `DiffuseMap.IsRelative` | whether the path is relative or absolute in the PK2 |

A normal-map block gated on flag `0x2000` is documented upstream but **never
occurs in v1.188** — that bit is not set in any of the 12,227 corpus materials,
so the parser does not read it.

The first material's `Flag` is the `u32` at offset `88 + name_len`.

## Flags: verified findings (openroad, census of all 12,227 materials in Data.pk2, 2026-07-09)
Only 6 bits are ever set; nothing above 0x200 is used. The wiki is wrong on two:

| Hex | Freq | Meaning |
|--|--|--|
| `0x0001` | 26.3% | two-sided / no backface cull (hair, foliage, flags, ropes, caps, talismans); the material apply fn additionally copies diffuse→ambient into the D3DMATERIAL9 when set |
| `0x0004` | 0.2% | classic D3D sun specular — verified in sro_client.exe: the material apply fn (~0xc82300) does `SetRenderState(D3DRS_SPECULARENABLE, TRUE)` for the draw (restore fn ~0xc82450 sets it FALSE) and fills a D3DMATERIAL9 from the BMT (Diffuse ← +0x4c, Specular ← +0x6c, Power ← +0x8c → D3DMATERIAL9.Power at +0x40). 19 materials total: Jangan palace set, cj_portal, stone lions `cj3_lion*`, `oas_tarim_skull_horse`, `cj_table_chair`, one power=0 flower; specular colors like (0.5,0.25,0)/(0.5,0.5,0.5)/(1,1,1), power 7–170 |
| `0x0008` | 0.9% | emissive / self-illuminated (window lights, jewels; the flag, not the emissive color field, is the render signal). Exe: also does `SetRenderState(D3DRS_LIGHTING, FALSE)` for the draw |
| `0x0040` | 100% | mandatory format marker — the wiki's "ColorTint?" is wrong, it is always set |
| `0x0100` | 96.4% | has diffuse map |
| `0x0200` | 36.4% | has alpha channel — what the alpha *means* is per-.bsr (EnvMap ModData ⇒ sheen mask, see `bsr-jmxvres.md`) |

`0x2000` "BumpMap" is never set in this build. Parse the first material's
flag as the u32 at offset `88 + name_len`.

**Exe flag-apply ground truth** (sro_client.exe @0xc42a95 and the twin
@0xc6c740, verified 2026-07-30): `0x1` → `CULLMODE = NONE`; `0x4` →
`SPECULARENABLE = TRUE`; `0x0200` (unless `0x20000000` is also set, never
in this build) → `ALPHATESTENABLE = TRUE`, `ALPHAREF = 0x80`
(+ `ALPHAFUNC = GREATEREQUAL` in the twin) — the engine-default alpha
handling for alpha-carrying materials is a **ref-128 alpha test, no
blending**; `0x8` → `LIGHTING = FALSE`. A .bsr's mods then override per
draw: an EnvMap mod disables the test (or re-enables it at ref 1 with its
alpha-test bit, see `bsr-jmxvres.md`), a Material mod applies its own
test/blend state bytes.
