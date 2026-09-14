# Openroad

Rust-based, cross-platform **clone** of the Silkroad Online client, built with Bevy.

## Vision and Notes
- **Clone, not replica** (see [ADR 0009](docs/adrs/adr-0009-clone-not-replica.md)): the
  original v1.188 client and your own PK2 data are the default reference and the
  tie-breaker for every value we cannot otherwise derive — but where a modern engine can
  do better (rendering, UX, accessibility, portability, testability), we deliberately do,
  and say so. Wire compatibility with a real vSRO server is not negotiable.
- SRO is a great game. We are big fans. This project is a fan project and does not aim to harm SRO. The goal is a cross-platform version of the game.
- To access the structures without actively reverse engineering ourselves, we build on v1.188. There is enough publicly available material to read PK2 files. Network communication is designed accordingly, based on existing solutions. The goal is not to bypass protections of active SRO server operations.
- The long-term goal is compatibility with other versions as well. v1.188 is only the first plugin.
- This Rust-based client aims to work by simply being placed inside the Silkroad folder.
- Users are responsible for downloading an appropriate SRO client version themselves. We do not provide links or downloads.

## Running a release build

Want to *play* rather than develop? Download the zip for your platform from
Releases, unzip it, and:

1. Let your system run the unsigned binary — on macOS
   `xattr -dr com.apple.quarantine <folder>`, on Windows SmartScreen → *More
   info* → *Run anyway*.
2. Copy your own `Media.pk2`, `Map.pk2`, `Data.pk2`, `Music.pk2` and
   `Particles.pk2` into `assets/` beside the binary.
3. `cp config.example.yaml config.yaml`, then fill in the `pk2:` key and salt —
   openroad does not ship the archive key.
4. Run `./client` **from that folder**, and point
   `network_settings.gateway_address` at a server you have access to.

Full walkthrough, including the Linux runtime libraries and a troubleshooting
table: [`docs/RUNNING.md`](docs/RUNNING.md).

## Status

Early development. Expect rough edges, incomplete scenes, and missing gameplay
features. Login, character select, world streaming, movement, combat and a good part
of the HUD work against a real vSRO server; large subsystems (guild, quests, storage,
alchemy) are partly or not wired.

Protocol coverage is the one number kept honest by a gate:
[`docs/protocol/opcodes.md`](docs/protocol/opcodes.md) lists every wired opcode and CI
fails if it disagrees with the `packets!` macro. What is planned lives in the issue
tracker, not in a document.

## Architecture Snapshot
- Bevy app with scene-based flow and plugin-driven systems.
- Custom PK2 asset reader registered as Bevy AssetSource.
- Packet framing, handshake, and protocol types for client-server communication.
- Workspace split into focused crates for assets, networking, and serialization.

## Repository Layout
- `client/` Bevy client app, scenes, UI, asset loaders, network plugin.
- `bevy_pk2/` PK2 archive reader that implements Bevy AssetReader.
- `packets/` Packet definitions and event plumbing.
- `sro_macro/` Serialization traits and helpers.
- `sro_macro_derive/` Derive macros for packet structs.
- `tools/` Auxiliary tools and experiments.
- `assets/` Local PK2 files and extracted runtime assets, not distributed.
- `docs/` Architecture decision records and project documentation.

> **Cloned before 2026-09-01?** The history was rewritten twice that day, the
> repository was replaced, and every commit hash changed. See
> [`docs/MIGRATION-history-rewrite.md`](docs/MIGRATION-history-rewrite.md) before
> you `git pull`.

## Getting Started
Requirements:
- Rust as pinned by `rust-toolchain.toml` (currently stable).
- A local Silkroad client installation with PK2 files available.
- Linux native build dependencies for Bevy and native crates.

Install Linux native build dependencies:
```bash
make install-deps
```

Verify Linux native build dependencies:
```bash
make check-deps
```

`make install-deps` supports Debian/Ubuntu, Fedora, and Arch. If `pkg-config` reports missing `alsa.pc`, `libudev.pc`, or `wayland-client.pc`, install the native dependencies above. On Debian/Ubuntu, `alsa.pc` is provided by `libasound2-dev`, `libudev.pc` is provided by `libudev-dev`, and `wayland-client.pc` is provided by `libwayland-dev`.

No project install step is required for development. OpenRoad is run from this repository and expects user-supplied PK2 files locally.

Prepare local files:
```bash
make setup
```

`make setup` asks for local environment values and writes them to `.env`:
- `SRO_PATH` path to your local PK2 folder, used by the client and launcher.
- `DISCORD_URL`, `YOUTUBE_URL`, and `NEWS_JSON_URL` optional launcher URLs.

If `SRO_PATH` is not set, the client falls back to `assets/`. Verify the local setup:
```bash
make check-env
```

Quick run:
```bash
make run
```

Auto-reload on changes (requires `cargo-watch`):
```bash
make watch
```

Auto-reload launcher (requires `cargo-watch`):
```bash
make watch launcher
```

Run launcher:
```bash
make run launcher
```

Launcher environment (optional):
- `DISCORD_URL` and `YOUTUBE_URL` for the footer buttons.
- `NEWS_JSON_URL` base HTTP URL where `news.json` and `posts/*.md` live.

Run a specific scene:
```bash
make run world
```

On Windows, the same `make run <scene>` commands work from PowerShell; Make
delegates the run step to `scripts/run.ps1` so the root `.env` is loaded correctly.

Release profile — `make build release` reproduces exactly what the `build-*.yml`
workflows compile, so a performance claim can be measured locally instead of inferred
from a CI artifact:
```bash
make build release            # cargo build --release --package client
make build windows release    # the Windows cross-compile, on the release profile
make run world release        # any `make run` target, on the release profile
```
Adding `release` to any `run` or `build` goal selects the profile (`RELEASE=1` does the
same thing, for scripts). Expect a longer link and only a small runtime gain: the dev
profile already builds every dependency at `opt-level = 3`. Measure the current
scene's bottleneck before attributing a gain to the build profile.

Water and ice use a shared quad. Terrain pixels are released through Bevy's upload
queue after deriving persistent foliage tints. Hand-decoded DDJ textures preserve
authored mip chains; single-level fallbacks generate colour mips in linear light.
The BC1 cutout compatibility fallback remains enabled pending backend and
original-asset validation.

Asset-free BC1 comparison through the masked, blended and sheen materials:

```bash
cargo test -p client --test bc1_gpu -- --ignored --nocapture
```

Run it on each supported graphics backend; it needs an adapter but no PK2 data
or window. A passing synthetic comparison does not replace checking the original
coin/cutout regression assets.

### Developing in WSL2, running on Windows

WSLg's stock Mesa build has no GPU-accelerated Vulkan driver, so a client run
under WSLg falls back to the `llvmpipe` CPU rasterizer — technically works,
too slow to be useful. Cross-compile a native `.exe` from WSL instead and run
it on the Windows side for full GPU acceleration:

```bash
# In WSL — one-time setup, then on every build:
rustup target add x86_64-pc-windows-gnu
sudo apt install gcc-mingw-w64-x86-64   # once
make build windows            # add `release` for the release profile
```

```powershell
# In PowerShell, from this same repo checked out on the Windows side:
make run wsl
```

The profile has to match on both sides: `make build windows release` writes to
`target/x86_64-pc-windows-gnu/release/`, so launch it with `make run wsl release`.
Without the word on the PowerShell side, `run wsl` looks in `debug/` and reports the
build as missing rather than silently starting an older binary.

`run wsl` shells out to `wsl.exe` to resolve the WSL checkout's path (default
distro `Ubuntu`, default path `~/workspaces/private/openroad` — override with
`$env:WSL_DISTRO` / `$env:WSL_REPO_PATH`) and launches the `client.exe` that
`make build windows` produced there, pointed at that checkout's `assets/`.

Available scenes:
- `intro`
- `world` — offline dev **sandbox** (terrain + player + fly cam, no HUD of its
  own). The real in-game scene, with the HUD, is reached through the intro's
  character selection; per-scene UI gating is an openroad convention, the
  original composes its UI once globally.
- `animations`
- `ui_testing`
- `asset_loading`
- `equipments`
- `particles`
- `skills` — offline skill-system test scene: spawn a character (pick race,
  gender, gear, level) and a 10k-HP training dummy via the egui window, learn
  and level skills in the skill window (S key), drag them onto the underbar,
  and cast them with the slot hotkeys (flat 10 damage per hit, dummy HP
  auto-resets at 0). Right-click a learned skill in the window to withdraw a
  level (SP refunded); the egui simulation panel grants SP, raises masteries
  and force-applies stun/freeze to demo cast interruption.
- `dungeons` — offline dungeon test scene: spawns a character inside the
  Donwhang Stone Cave (loaded from your own Data.pk2 `.dof`), with per-block
  lights/fog and portal culling; click-to-move walks the room nav meshes.
  The egui window teleports between all dungeons from `dungeoninfo.txt`.
  In the `world` scene, walk-in dungeon gate circles (Jangan East → Qin-Shi
  Tomb, Donwhang West → Stone Cave) teleport client-side, with return gates
  inside.

## PK2 Unpack Tool
The repository includes a small CLI to list or extract PK2 archives.

List contents:
```bash
cargo run -p tools --bin pk2_unpack -- --pk2 /path/to/Media.pk2 --list
```

Extract all:
```bash
cargo run -p tools --bin pk2_unpack -- --pk2 /path/to/Media.pk2 --out /tmp/media
```

Extract only a prefix:
```bash
cargo run -p tools --bin pk2_unpack -- --pk2 /path/to/Media.pk2 --out /tmp/media --prefix textures/
```

Makefile shortcuts:
```bash
make pk2 list PK2="/path/to/Media.pk2"
make pk2 unpack PK2="/path/to/Media.pk2" OUT="/tmp/media" PREFIX="textures/"
```

## PK2 Corpus Probe
`pk2_probe` walks your PK2s through the client's own parsers and prints a
`value -> count` histogram for one named field, so a claim like "this flag is
only ever 0 or 1" becomes a citable number instead of an impression. It is
read-only: it never writes, never executes what it reads, and never touches the
network.

```bash
cargo run -p tools --bin pk2_probe -- --pk2 /path/to/Data.pk2 --field bms.vertex_flag
cargo run -p tools --bin pk2_probe -- --corpus /path/to/pk2dir --field itemdata.TypeId1 --format json
```

`--field` with no match lists the registry (each field prints the source line
that defines its meaning). `--prefix` restricts the walk to a subtree.

Note which archive holds what: the mesh/animation/texture formats live in
`Data.pk2`, while the `server_dep/silkroad/textdata` tables live in `Media.pk2`.
Textdata fields follow the master-list indirection the client uses, so the
encrypted `*enc.txt` siblings are excluded.

## Configuration
Config is loaded from `config.yaml` in the repository root. It is **machine-local
and not tracked by git** — `make setup` creates it by copying
`config.example.yaml`, and `make check-env` tells you to when it is missing.

> **Upgrading across the commit that untracked it:** `config.yaml` used to be
> committed even though `.gitignore` listed it (git only ignores *untracked*
> paths, so the rule never applied). If your working copy still matches the old
> committed version, `git pull` will **delete** it — back it up first, or just
> run `make setup` afterwards to regenerate from the template. Local edits keep
> the file, but they will also keep it out of the repo from now on, which is the
> point: the committed copy carried one machine's gateway address and had
> drifted from the template's defaults.

Key options include:
- `network_settings.enabled` to toggle networking.
- `network_settings.packet_dump` (default `true`) to append every received packet to `packet_dump/<opcode>.log` (one `<timestamp> <hex> <E|P>` line per packet; the third column is the frame's wire encryption bit) for offline re-analysis.
- `scenes.startup` to select the default scene.
- `window_settings` for size, mode, and monitor. This is the single authority for the window: the in-game Video option can flip windowed/fullscreen for the running session, but it is deliberately not persisted, so every launch starts from `config.yaml`. `mode` is one of `Windowed`, `BorderlessFullscreen` or `Fullscreen`, and `monitor` (`Primary` or `Current`) is the one a fullscreen mode targets — set it to `Current` to land on whichever monitor the window opens on. `RESOLUTION=WxH` overrides the configured size for one run.
- `dev_tools` (default `false`) to enable the egui world/resource inspectors and the Bevy Remote Protocol server (needed by `bevy_brp_mcp`); costs significant FPS.

## Screenshots
`SCREENSHOT=<path>` makes the client capture one frame and exit, so a UI change
can be reviewed against an image instead of an assertion:

```bash
SCREENSHOT=login.png SCENE=intro_v2 cargo run -p client
SCREENSHOT=/tmp/world.png SCREENSHOT_AFTER=25 SCENE=game cargo run -p client
```

- `SCREENSHOT_AFTER=<seconds>` (default `10`) is how long the app runs before
  the grab — the scenes stream terrain and textures out of the PK2s for several
  seconds, so capturing immediately photographs the loading screen.
- The frame is read back from the GPU, not from the desktop, so this works where
  the OS `screencapture` does not (locked display, no screen-recording
  permission, remote session).
- Exit codes are the script contract: `0` written, `3` captured but not
  writable, `4` no frame came back within 30 s, `101` the client panicked.
- **Do not commit the output.** A screenshot of this client is the user's own
  PK2 art. A bare filename lands in the gitignored `screenshots/` directory for
  that reason.

## Assets and Legal
Required PK2 files are expected under `SRO_PATH` or, if unset, `assets/`:
- `Media.pk2`
- `Map.pk2`
- `Data.pk2`
- `Music.pk2`
- `Particles.pk2`

We do not ship or link any proprietary assets. Users are responsible for obtaining their own SRO client data.

Cutscene camera paths are the original's data and are **not** committed either.
The client converts one out of your own `Media.pk2` at startup — by default the
cutscene your own `config/option.txt` names — so nothing has to be prepared.
Set `scenes.intro_location` to pin a specific script, or convert one to a file
to hand-tune it:

```bash
make cutscene convert SCRIPT=/path/to/Media/script/intro/china_wharf.txt
```

An `assets/intros/<name>.intro` on disk overrides the derived path
([`docs/cutscene-convert.md`](docs/cutscene-convert.md)).

The PK2 archive key is likewise **not** shipped: it belongs with the game data it
decrypts, which is yours rather than ours. Supply it in `config.yaml` under `pk2:`
or via `SRO_PK2_KEY`/`SRO_PK2_SALT` — see `config.example.yaml`. The client will not
start without one.

### Trademarks and affiliation

Silkroad Online and Joymax are trademarks of their respective owners (WEMADE MAX).
This project is **not affiliated with, endorsed by, or sponsored by** them. Those
names are used only descriptively, to say what this software interoperates with.

OpenRoad is an independent, non-commercial fan project. It ships no game content,
and it is not a means of obtaining the game or of circumventing any commercial
service. Reverse engineering in this repository is for **interoperability** and is
performed statically on files the user already owns. The evidence behind that work is
kept out of this repository on purpose — see
[`CONTRIBUTING.md`](CONTRIBUTING.md#what-this-repository-does-not-publish).

Third-party code and assets bundled here are recorded in
[`THIRD-PARTY.md`](THIRD-PARTY.md).

## ADRs
Architecture decisions are recorded in [`docs/adrs/`](docs/adrs/); the documentation
index is [`docs/README.md`](docs/README.md).

## Contributing
This is an open source project. Contributions are welcome.

If you plan a large change, open an issue or proposal first so we can align on scope and direction.

This project runs **no automatic CI gate** — no GitHub Actions workflow runs on a push
or a pull request, and no check is required to merge. Quality is gated **locally**
before pushing:

```bash
make ci
```

That runs the full gate: `cargo fmt --all --check`, the Rust warning policy
(`scripts/check_warnings.py` — rejects every compiler warning except unread
struct-field diagnostics retained for parsed but not yet consumed SRO data), the
opcode-ledger check, the `scripts/re/` tool tests, the reference-data column check
(`scripts/re/check_reference_data.py` — asserts our textdata column indices against
the SQL `SELECT` order in `SR_Db2Media/Settings.cs`; skips with exit 0 unless
`SRO_REFS_PATH` or `<refs>` holds that checkout), `cargo test --workspace`, and
the client build. The pieces are also available individually as `make fmt-check`,
`make warnings`, `make opcodes`, `make re-tools`, `make reference-data`,
`make test`, `make build`.

The gate starts with `make check-target-dir`, which refuses to run when
`CARGO_TARGET_DIR` points at a dir a *different* git worktree of this repo already
built into. Cargo cannot tell those two checkouts apart, so it would reuse the other
one's artifacts and the gate's result would say nothing about your diff.

On WSL with build instability, use this checkout's disk-backed `target/` and
one Cargo job to reduce concurrent compiler memory use. Disable incremental
compilation when recovering from corrupt compiler artifacts:

```bash
CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 make ci
```

Check `df -h . /tmp /mnt/c` before putting a build target under `/tmp`: that
directory can be RAM-backed and its build cache disappears on reboot. One Cargo
job limits concurrent builds; it does not cap a single compiler's memory use.
Also check the Windows host drive: free space reported inside WSL's virtual
disk does not guarantee room for that disk to grow on Windows.
This is a conservative recovery workflow, not a diagnosis of WSL crashes.

### Cross-platform builds

Three **manual** workflows build the client for a platform you may not own and upload a
zip of the binary, `assets/` and `config.example.yaml` (you still supply your own PK2
files and a `config.yaml`):

| Workflow | Target |
|---|---|
| `.github/workflows/build-linux.yml` | `x86_64-unknown-linux-gnu` |
| `.github/workflows/build-windows.yml` | `x86_64-pc-windows-gnu` (cross-built with mingw-w64) |
| `.github/workflows/build-macos.yml` | `aarch64-apple-darwin` |

They run on `workflow_dispatch` only — `gh workflow run build-macos.yml` — and gate
nothing. `make ci` remains the gate. A hand-dispatched build is named
`openroad-<version>-g<sha>-<target>.zip`, so it cannot be confused with a release.

### Releases

`.github/workflows/release.yml` cuts a release. Dispatch it with a bump level; it owns
the version end to end:

```bash
gh workflow run release.yml -f bump=minor          # 0.1.0 -> 0.2.0
gh workflow run release.yml -f version=1.0.0       # or an exact version
gh workflow run release.yml -f bump=patch -f dry_run=true   # rehearse, changing nothing
```

It bumps `[workspace.package] version` in `Cargo.toml`, refreshes `Cargo.lock`, commits
that to `main`, builds all three targets **from that commit**, and then tags `vX.Y.Z` and
publishes a GitHub Release with the three `openroad-<version>-<target>.zip` files
attached. The tag is created only after all three builds pass, so a published tag always
points at a tree that compiled everywhere.

Do not hand-edit the workspace version — the workflow owns it.

## License

Copyright (C) 2026 The OpenRoad Authors.

Every crate is licensed **GPL-3.0-or-later** (`LICENSE`). This program is
distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without
even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
See the GNU General Public License for more details.

Unless you state otherwise, a contribution you submit for inclusion is licensed
under the same terms.

Third-party code vendored here keeps its own licence — see
[`THIRD-PARTY.md`](THIRD-PARTY.md) and [`licenses/`](licenses/).
