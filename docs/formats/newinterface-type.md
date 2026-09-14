# NewInterface Type (*.2dt)

The control-class discriminator in a 2DT entry: which widget the client
instantiates, and the default art it loads for it.

The table below is the **fact set** — id, class name, default texture — recovered
from the client's own construction switch. Upstream reference (an OllyDbg
disassembly listing of that switch):
`SilkroadDoc.wiki/NewInterfaceType` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/NewInterfaceType

## Types

| id | class | default art |
|---|---|---|
| 0 | `CNIFMainFrame` | `interface\frame\mframe_wnd_` |
| 1 | `CNIFrame` | `interface\inventory\int_window_` |
| 2 | `CNIFNormaltile` | `interface\ifcommon\bg_tile\com_bg_tile_b.ddj` |
| 3 | `CNIFStretch` | `interface\ifcommon\com_blacksquare_` |
| 4 | `CNIFButton` | `interface\ifcommon\com_button.ddj` |
| 5 | `CNIFStatic` | — |
| 6 | `CNIFEdit` | — |
| 7 | `CNIFTextBox` | — |
| 8 | `CNIFSlot` | — |
| 9 | `CNIFLattice` | `interface\ifcommon\lattice_window\com_lattice_` |
| 10 | `CNIFGauge` | `interface\playerminiinfo\pmi_hp.ddj` |
| 11 | `CNIFCheckBox` | `interface\ifcommon\com_checkbutton02_on.ddj` |
| 12 | `CNIFComboBox` | — |
| 13 | `CNIFVirticalScroll` | — |
| 14 | `CNIFPageManager` | — |
| 15 | `CNIFBarWnd` | — |
| 16 | `CNIFTabButton` | `on.ddj` / `off.ddj` / `disable.ddj` |
| 17 | `CNIFBothSidesGauge` | — |
| 18 | `CNIFWnd` | — |
| 19 | `CNIFSlideCtrl` | `interface\recovery\re_selectbar.ddj` |
| 20 | `CNIFSpinButtonCtrl` | — |

The switch covers ids 0..=20; anything outside that range is not a valid type.

**Spelling:** the class name really is `CNIFVirticalScroll` in the binary — the
typo is the original's, and any table keyed on the exact string must reproduce it.

## Composed art

Three types load a *set* of textures rather than one, by suffixing a common
prefix:

- `CNIFStretch` — `left_up`, `right_up`, `left_down`, `right_down`, `left_side`,
  `right_side` (`.ddj` each), the nine-slice pieces of a stretchable panel.
- `CNIFTabButton` — `on`, `off`, `disable`.
- `CNIFMainFrame`, `CNIFrame`, `CNIFLattice` — the trailing `_` in the paths
  above is a prefix completed the same way.
