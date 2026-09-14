# resinfo — the classic UI grammar (`Media/resinfo/*.txt`)

The 247 `resinfo/*.txt` files are the classic (pre-4th-gen) UI's own layout
data: every window, every control, its rect, art path, colours and string key.
This page documents **how to read them**, because the naive reading is wrong in
a way that bites exactly where it matters.

## Grammar

```
Interface Text                      <- fixed header line
Section = Create,"0","0"            <- a named section
{
    GDR_BTN_CLOSE:CIFButton         <- a block: Name:Class
    {
        ClientRect=RECT,"0,0,0,0"   <- Key=TYPE,"value" lines
        …
    }
}
```

Value types: `INTEGER`, `STRING`, `POINT` (2 numbers), `RECT` (x,y,w,h),
`COLOR` (a,r,g,b — **alpha first**).

## Read blocks by key name, never by line offset

Keys inside a block are emitted **alphabetically**, and the key *set* varies, so
a key's line position varies with it. A census over all **3740** blocks:

| `Rect=` offset from the block header | blocks |
|---|---:|
| +9 | 3403 |
| +10 | 16 |
| +11 | 305 |
| +13 | 15 |
| +17 | 1 |

There are seven key-sets (base ×3259, `CommandID`+`HelpString` ×302,
`StretchType` ×169, the animation set ×6, `ADDID` ×2, two malformed). Both
`CommandID` (sorts before `DDJ`) and `HelpString` (before `ID`) push `Rect`
down — so **the 337 exceptions are precisely the interactive controls**:
buttons, slots, anything carrying a help string. A `lines[n + 9]` reader is
wrong on every one of them, silently.

Use `client/src/assets/resinfo/interface_text::parse_sections`, which is
key-driven, and ask a block for `entries.get("Rect")`.

## Three traps the corpus actually contains

1. **`grep -a` semantics.** `ifoption_quit.txt` is plain CRLF text that grep's
   binary heuristic rejects; without `-a` its 4 blocks silently vanish from any
   census.
2. **A stray trailing period.** `ifallianceguild.txt:78` is
   `SubSection=STRING,"".` — a regex anchored on `"$` does not match it at all.
3. **Literal `\n` sequences.** 1160 occurrences across 28 files store multi-line
   strings as the two characters `\` and `n`; read verbatim, every one collapses
   into a run-on line.

Malformed lines are skipped with a warning rather than being fatal: this is
user-supplied PK2 data, and the corpus is known to contain malformed blocks.

## Status in openroad

`InterfaceTextLoader` is **not registered** (`client/src/assets/mod.rs:135-136`).
That is deliberate: it declares `extensions() = ["txt", "yaml"]`, and
`TextDataLoader` (`client/src/assets/textdata/mod.rs:376-377`) already claims
`"txt"` for the `server_dep/silkroad/textdata` tables. Registering both makes
the extension mapping ambiguous, so enabling this loader needs a discriminator
(a path prefix or an explicit loader handle) first — see #477.
