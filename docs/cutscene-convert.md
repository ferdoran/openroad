# cutscene convert — `script/intro/<name>.txt` → `.intro` camera path

The intro cutscene is the original's data, so openroad ships none and reads
yours instead. **The client converts the camera script out of your own
`Media.pk2` at startup** (`client/src/scenes/intro_v2/mod.rs`, `init_scene_data`)
— nothing has to be pre-converted for the intro to play, which is what makes a
plain release download runnable.

`make cutscene convert` is still here for the cases where you want a *file*: to
pin a camera path that survives an option.txt edit, to hand-tune one, or to
inspect what the converter produces.

```bash
# from your own extracted Media/ — nothing SRO-owned is committed to this repo
make cutscene convert SCRIPT=/path/to/Media/script/intro/egypt.txt
make cutscene convert SCRIPT=.../roc.txt OUT=assets/intros/roc.intro NAME=roc \
     MUSIC=music://maintheme_cut.ogg
```

## How the client picks a cutscene

Three sources, most explicit first (`init_scene_data`):

1. **`assets/intros/<name>.intro` on disk** — a converted or hand-authored path
   always wins. This is what the tool above produces.
2. **The camera script in your `Media.pk2`** — `script/intro/<name>.txt`, named
   either by `scenes.intro_location` in `config.yaml` or, when that is empty, by
   your own `config/option.txt`.
3. **Neither** — the scene reports which of the two to fix and waits.

`config/option.txt` is the stock client's own startup options: `key = "value"`
lines where every alternative is shipped and all but one is disabled with a
leading `//`. Two keys matter here — `IntroName`, a Windows-separated path to
the camera script, and `IntroBGM`, the track filename. `IntroOption` in
`client/src/assets/intro_scene.rs` parses both; the last enabled entry wins, so
uncommenting one below the others does what it looks like it does.

Note the two are independent settings in the original, not one per-cutscene
record: pinning `scenes.intro_location` chooses the script but the track still
comes from `IntroBGM`. Pass `MUSIC=` to the tool, or supply an `.intro`, to pair
them differently.

## What it reads

A camera block is a run of rows

```text
0.0  S_CameraInsert  <frame>  <rx> <rz>  <x> <y> <z>  <rotx> <roty> <rotz>  1
```

inside a larger script. Everything that is not an `S_CameraInsert` row (the
`[CAMERA]` header, sound rows, blank lines) is skipped; a row that *is* a key
but cannot be read is an **error**, not a skip, because silently dropping a
keyframe would shorten a camera path without saying so. The leading `0.0`, the
verb and the trailing `1` are the three fields the `.intro` asset deliberately
drops (`client/src/assets/intro_scene.rs`, `CameraKeyframe`).

The scripts are UTF-16LE with a BOM and go through the shared textdata decoder;
`config/option.txt` is plain ASCII with CRLF and no BOM, which the same decoder
handles unchanged.

How many scripts a given `Media.pk2` carries is a property of that archive
rather than a fixed set — list yours with
`make pk2 list PK2=/path/to/Media.pk2 | grep script/intro`.

## Why derived and not committed

The scripts are the user's own PK2 data. Deriving at startup keeps the
repository free of SRO-owned content (`CLAUDE.md` "Security") while removing the
manual step that used to stand between a fresh install and a playable intro —
a release download has no `tools/` binary to run, so requiring one made the
shipped default unreachable (#569).

## Tests

`client/src/assets/intro_scene.rs` covers the converter's field mapping and the
`option.txt` selection rules against **synthetic** fixtures — the real camera
data belongs to the user's `Media/` and is never committed, so no test needs a
PK2 file.
