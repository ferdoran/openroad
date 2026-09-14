# Bitmap glyph image (`Media/fonts/{0,i,y}.dat`) — JMXVIMG

Canonical layout for the three `.dat` files under `Media/fonts/`.

## Layout — [V] EOF-exact on all three files

```text
+0   12      char[12]   signature   "JMXVIMG11000"  (JMXVIMG + version 1.1000)
+12  2       u16 LE     width
+14  2       u16 LE     height
+16  w*h     u8[]       8-bit alpha mask, row-major, top-down
```

Total size is exactly `16 + width * height`, which is what fixes the field
order — width-first reconciles all three files and renders legible glyphs,
height-first does not:

| file | size | w × h | 16 + w·h |
|---|---|---|---|
| `0.dat` | 100 | 7 × 12 | 100 |
| `i.dat` | 88 | 6 × 12 | 88 |
| `y.dat` | 124 | 9 × 12 | 124 |

Sample values are only `0x00` and `0xFF` — a 1-bit coverage mask stored one
byte per pixel, not an antialiased glyph. Each file is named after the single
glyph it contains (`0`, `i`, `y`).

## What it is not — and what that means for our font stack

The 16-byte header has no codepoint, advance, bearing or kerning field, and the
set is exactly three characters, so **this is not a shipped font**: three
glyphs cannot render text. Treat it as a rasterizer/metrics test triple that
was left in the archive.

The consequence is already in the tree, but the older wording of this paragraph
was wrong on its premise: vSRO's `Media.pk2` **does** ship TrueType faces —
`Media/fonts/` holds `기본서체.ttf`, `영문서체.ttf` and `채팅서체.ttf` beside the
three `.dat` stubs. `client/src/assets/mod.rs` (`FontAssets`) loads the UI face
from there (`FontRole::path`), falling back to the bundled OFL Fira face when
the archive has no such file (#637). Any plan phrased as "write a `.dat` font
loader" is still unachievable as written — there is no `.dat` font to load.

**No loader is registered for `.dat`, deliberately.** With three test glyphs and
no metrics there is nothing for a consumer to do with them; writing a loader
would be inventing a consumer. If one is ever wanted (a format-viewer tool, say),
the layout above is complete — 5 lines of parsing, with the usual rule that a
bounds check comes before every read.

## Two corpus rules this file family established

- **Match extensions case-insensitively.** An earlier `.bsk` probe was
  case-sensitive and undercounted by exactly the 62 uppercase `.BSK` names
  (952 vs the true 1,014; `docs/formats/bsk-jmxvbsk.md`). The client itself is
  safe — `bevy_pk2`'s `Archive::normalize` lowercases both the index keys and
  every query (`bevy_pk2/src/pk2/archive.rs:60-70`), by design, because the
  original relies on that too. It is *probes and censuses* that need the care.
- **Zero-byte files exist and must be skipped, not failed.** Seven of them ship
  in the archives. Parsers whose readers clamp or bounds-check (`BufExt`'s
  fixed-size string reader documents the 4 empty `.bsk` as its reason) already
  degrade correctly; a new parser must not assume a signature is present.
