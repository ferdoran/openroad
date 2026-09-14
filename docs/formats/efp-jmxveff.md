# Particle (*.efp / *.eff) - JMXVEFF

The particle-effect description: a tree of effect nodes, each with controllers,
emitters, a program of timed commands, and the textures/meshes/animations it
draws with.

Layout derived from openroad's parser (`client/src/assets/efp/format.rs`, ~1,100
lines, plus `mod.rs` and `loader.rs`). That parser is the authoritative
field-by-field reference and the only complete description this project relies
on; the structure below is a map of it. Upstream reference:
`SilkroadDoc.wiki/JMXVEFF` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVEFF

## Top level — EFStoredEffect

| field | notes |
|---|---|
| `VersionStr` | version string |
| `RootScale` | f32 applied to the whole effect |
| `V13Ints` | three i32, version 13 only |
| `Nodes` | flat arena of `EFStoredObject`; children referenced by index |
| `Root` | index of the root node |

The tree is stored flat and reconstructed by index, not by nesting.

## Node — EFStoredObject

| field | notes |
|---|---|
| `DataOffset` | absolute offset of the node's data block |
| `Name` | |
| `Controllers` | list of `EFController` (below) |
| `ProgramLen` | node lifetime in effect frames — also the loop modulus, the row count graphs bake to, and what percent-mode command schedules resolve against |
| `GlobalParams` | named `EEParameter` values |
| `Emitters` | `EESourceData` list |
| `Lifetime` | optional `EESourceData` |
| `Programs` | `EESourceData` list — the command timeline |
| `Timeline` | node timing |
| `ViewMode` | `None`, `Billboard`, `VBillboard`, `YBillboard` |
| `Resource` | `EEResource` — texture, mesh and animation paths |

## Value types

| type | shape |
|---|---|
| `EEBlend<T>` | `begin`, `end`, and a list of keys — a value interpolated over the node's life |
| `EEParameter` | one of f32, `Vector` (Vec3) or `Matrix` (Mat4) |
| `AxisVector4` | an axis plus an angle |
| `RotVector` | euler rotation triple |
| `AngleVector1` | a direction with a cone angle |
| `FrameTextureSlide` | UV slide per frame |
| `Argb` | packed colour, used by the diffuse graphs |

## Commands

The program is a sequence of tagged commands. openroad models these, grouped as
upstream groups them:

**Lifetime** — `NeverExtinct`, `NormalTimeExtinct`, `NormalTimeLoop`,
`StaticEmit`, `ProgramUpdate`.

**Position** — `SetPosition`, `SetSpherePos`, `SetConePos`, `SetBanPos`,
`SetVelocity`, `SetConeVel`, `Force`, `ConeForce`, `Attraction`.

**Rotation** — `SetRotation`, `SetRVelocity`, `SetRotationAxis`,
`SetRVelocityAxis`, `SetRotationMat`, `SetRVelocityMat`, `SetBanRot`.

**Decoration** — `TextureSlide`, `SetGraphScale`, `SetGraphRandomScale`,
`SetGraphDiffuse`, `SetShapeRot`, `SetShapeRotVel`.

**View mode** — `ViewNone`, `ViewBillboard`, `ViewVBillboard`, `ViewYBillboard`.

**Render mode** — `RenderNone`, `RenderPlate`, `RenderMesh`, `RenderLinkPipe`,
`RenderLinkDPipe`, `RenderLinkObj`.

A command's parameter shape is a function of its name — see
`command_from_name` in `format.rs`, which is where the name-to-payload mapping
actually lives.

## Known constants

The slack between a node's timeline ints and its view-mode source is **6 bytes**,
measured in **75,450 of 75,450 nodes** across the 3,738-file corpus. An earlier
note claiming other values also occur was refuted — no such file exists in the
corpus.

## Corpus re-verification (openroad, 2026-08-12)

Re-run of our own parser over `<pk2-corpus>/Particles`: **3,738 `.efp`**
(none exist outside Particles.pk2), **3,729 parse (99.76%)**, and since the
parser rejects trailing bytes, all of those are EOF-exact. **The corpus is
larger than the numbers quoted elsewhere in this doc** — 3,738 files /
**75,441 nodes** / **124,890 EEResource records**, not 2,857 / 51,658; every
histogram blockquote above predates this and should be re-measured.

Two spec corrections (our parser already does the right thing):

- **`EFCDiffuseGraph` has NO trailing floats.** The 4 bytes after its second
  blend are `07000000` (×26,713) or `0a000000` (×22,578) — together exactly the
  49,291 DiffuseGraphs — and they decode as the length of the *next* controller
  name (`"Program"` / `"ScaleGraph"`). The wiki's two trailing floats are wrong.
- **`FrameBANRotation`/`FrameBANPosition` carry an extra u32 before the count**
  (the wiki shows count-only); EOF-exactness breaks without it — stripping the
  read costs 460 file-level parses (3,729 → 3,269). It is 0 in **6,998 of
  7,192** command occurrences and heap-pointer-looking otherwise — a dead
  exporter field.

Corpus distributions: controllers (460,209) StaticEmit 75,311 · NormalTimeLife
75,052 · Program 67,097 · LinkMode 66,313 · Shape 49,449 · ScaleGraph 49,361 ·
DiffuseGraph 49,291 · ViewMode 26,278 · BAN 1,798. RenderShape: Mesh 25,822 ·
Plate 20,552 · LinkPipe 2,519 · LinkObj 287 · LinkDPipe 244 · None 25.
**Blend pairs: 15 distinct but 99.87% are two** — (5,2) SRCALPHA/ONE ×72,212 and
(5,6) SRCALPHA/INVSRCALPHA ×52,498. `mesh_count` ∈ {0, 1} only. The timeline
tail pad is **6 in 75,450/75,450 nodes** (the parser's former 7/5/8/4/9
candidates never fired and have been removed), and
`lifeTimeSource`/`viewModeSource`/`renderSource` always have `hasData == 1`.

**`cull_mode` is not a bool**: over the 75,450 node resources the field is
`1 ×39,795 / 3 ×30,418 / 2 ×5,237`, never 0 — exactly the `D3DCULL` values
(NONE/CW/CCW), so a `!= 0` reading would call every node two-sided. It has no
consumer today: `client/src/plugins/effects/material.rs` sets
`descriptor.primitive.cull_mode = None` unconditionally for effect meshes.

**The remaining 8 non-parsing files are data problems, not layout gaps:**
1 zero-byte (`dun/petra_flame_yellow_glow.efp`) and 7 files with version
`"0000"` that are genuinely corrupt (absurd string lengths, cross-file content
bleed, header byte-ramps). The ninth,
`skill/china/water_hide_keep_a.efp` (version `"0010"`), is recovered since
2026-08-15: it parses byte-exact under the 0011 layout.

