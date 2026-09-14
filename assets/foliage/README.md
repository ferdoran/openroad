# Foliage pack cards

Grass billboard cards for the optional pack foliage layer
(`graphics.foliage.mode: pack | both`, see `client/src/plugins/map/foliage/pack.rs`).
This is **not** SRO game data — the native foliage layer uses the grass models
from the user's own PK2 files instead.

Source: **ambientCG Foliage006** ("grass strands" blade atlas,
https://ambientcg.com/view?id=Foliage006), license **CC0 1.0** (public
domain, no attribution required — credited voluntarily).

The cards are *generated*, not hand-cut: each is a composed tuft of several
scanned blades fanned around a common root, produced deterministically by

```
cargo run -p tools --bin gen_foliage_cards -- --input <dir>
```

where `<dir>` contains the unzipped `Foliage006_2K-PNG_Color.png` +
`Foliage006_2K-PNG_Opacity.png` (download `Foliage006_2K-PNG.zip` from
ambientCG). The tool composites the separate opacity map into the color
alpha, isolates blade islands, filters to slender near-vertical blades, and
writes `grass_{a..d}.png` (512×512, alpha-cutout, tuft root at the bottom).

In-game the cards render on cross-quads with `AlphaMode::Mask` and are
tinted toward the underlying terrain tile's average color via vertex colors
(`graphics.foliage.tint.tile_blend_pack`), so they blend with each zone's
palette. Additional cards can be added under the same naming scheme and
listed in `PACK_SPRITES` (`pack.rs`).
