# MapObjectIndex / Strings / Extensions (*.ifo) - JMXVOBJI

Three sibling text files that name and place the world's objects. **Text, not
binary** — each has a header line, an entry-count line, then one entry per line.

Column layout derived from openroad's parser (`client/src/assets/ifo/object.rs`).
Upstream reference: `SilkroadDoc.wiki/JMXVOBJI` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXVOBJI

## object.ifo

The object catalogue: the id that `.o2` instances reference, resolved to a `.bsr`.

| # | column | format | notes |
|---|---|---|---|
| 0 | `Index` | zero-padded decimal | the `ObjID` an `.o2` record carries |
| 1 | `Flag` | `0x%08X` | collision-related; possibly only an indicator |
| 2 | `Path` | quoted string | the `.bsr` resource |

## objectstring.ifo

Named world positions — gates, spawn markers and similar, keyed by a string id.

| # | column | format | notes |
|---|---|---|---|
| 0 | `MapUniqueID` | `0x%08X` | packs the region id and a per-region unique id |
| 1 | `Flag` | `0x%08X` | |
| 2 | `XSec` | decimal | region x |
| 3 | `YSec` | decimal | region z |
| 4 | `XOffset` | `0x%08X` | f32 bit pattern, region-local |
| 5 | `YOffset` | `0x%08X` | f32 bit pattern |
| 6 | `ZOffset` | `0x%08X` | f32 bit pattern |
| 7 | `Yaw` | `0x%08X` | f32 bit pattern, radians |
| 8 | `String` | quoted string | the name, e.g. a `POS_STRUCTURE_*` id |

The three offsets and the yaw are written as the **hex bit patterns of f32
values**, not as decimals — they must be reinterpreted, not parsed as integers.

`MapUniqueID` is only unique within its own region, which is why the region id is
packed alongside it.

## objext.ifo

Extension records attaching an extra string to an object instance.

| # | column | format | notes |
|---|---|---|---|
| 0 | `MapUniqueID` | `0x%08X` | as above |
| 1 | — | quoted string | usually empty |
| 2 | — | quoted string | the extension value, e.g. `passent01` |
