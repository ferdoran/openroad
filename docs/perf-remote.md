# Remote performance insights via the Bevy Remote Protocol

How to inspect and drive a *running* client for performance work, from the
shell — no MCP server, no debugger attach. The client hosts a BRP JSON-RPC
server over HTTP; the `brp_perf` CLI (`tools/src/bin/brp_perf.rs`) wraps the
useful calls, and everything is also reachable with plain `curl`.

## Enabling

1. `config.yaml`: set `diagnostics: true`. That adds `RemotePlugin` + the HTTP
   transport from `bevy_brp_extras`, `RenderDiagnosticsPlugin`, the per-phase
   draw counters and the render-asset counters — see `client/src/main.rs`.

   **Leave `dev_tools: false` while measuring.** `dev_tools` implies
   `diagnostics`, but it also adds the egui world inspector (which reflects
   every entity into egui each frame) and the navmesh debug draws, which run
   whether or not the dev windows are visible. Together they cost double-digit
   FPS, so a reading taken through `dev_tools` describes a build nobody plays.

   `make perf set` and `make perf attribute` work at this tier too: they reach
   `RenderDebugSettings` by reflection over BRP, and `RenderControlsPlugin`
   registers that resource unconditionally (three shipping culling systems read
   it as a plain `Res<_>`). Only the egui panel that edits it is dev-gated, and
   the CLI does not need the panel.
2. Run the client (`make run world`). The server listens on
   `http://127.0.0.1:15702/`; override the port with the `BRP_EXTRAS_PORT`
   env var on the client side. `brp_perf` honors `--port`, `$BRP_PORT`, and
   `$BRP_EXTRAS_PORT` (in that order).

Numbers are only meaningful once past loading (`GameState::Game`).

### Measuring a Windows build from WSL

On a WSL2 machine the client has to run on Windows — a Linux build there has no
GPU (no `/dev/dri`, only `/dev/dxg`), so wgpu falls back to a software
rasterizer and every number describes llvmpipe rather than the card. But
`make perf` cannot then be driven from the WSL shell, for two independent
reasons:

- Bevy's `RemoteHttpPlugin` binds `127.0.0.1` only (`DEFAULT_ADDR`), so the
  server is not listening on any interface WSL can route to.
- WSL2's default networking is NAT, not mirrored: `localhost` inside WSL is
  WSL's own loopback, and the Windows host is a *different* machine at the
  default gateway (`ip route show default`). Windows-to-WSL localhost
  forwarding is automatic; WSL-to-Windows is not.

So run the CLI on the Windows side too. `make build windows` cross-compiles
`brp_perf.exe` next to `client.exe` for exactly this reason — no Rust toolchain
is needed on Windows:

```powershell
# PowerShell, in the repo; client.exe already running via `make run wsl release`
.\scripts\perf-capture.ps1
```

That script is the whole workflow in one run: it takes the baseline (fps, draw
calls per phase, per-pass GPU times, world/cache counts), then toggles terrain,
terrain lighting, shadows, objects and effects off one at a time and re-measures
after each, restores every setting it touched in a `finally` block, re-checks
the baseline for drift, and writes the lot to `perf-<timestamp>.txt`.

It reads each setting's original value rather than assuming a default -- which
matters for `enable_shadows`, seeded from `graphics.shadows.enabled` -- and it
warns when `-Settle` is shorter than the 120 frames the diagnostic averages
over, the mistake that silently mixes pre-change frames into every number.

`-NoExperiments` gives a read-only capture that changes nothing; `-Settle`
raises the wait per measurement (default 8 s, enough down to 15 fps).

The individual commands, if you want one in isolation:

```powershell
$brp = "target\x86_64-pc-windows-gnu\release\brp_perf.exe"
& $brp fps --settle-secs 8
& $brp snapshot --prefix render_phase/
& $brp snapshot --prefix render/
& $brp snapshot --prefix world_counts/
```

Binding the server to `0.0.0.0` would let WSL drive it instead, but BRP can
*mutate* the running world (`world.mutate_resources` is what `perf set` uses),
so that would put a game-controlling RPC endpoint on the LAN. It is not exposed
as a config knob for that reason; ask if the convenience is worth it.

Note `make profile chrome` builds and runs a **Linux** client, so on WSL its
frame-time totals are llvmpipe's, not the GPU's. The per-span CPU ranking is
still meaningful (that is main-thread work), but a representative capture needs
a Windows profiling build:
`cargo build --release -p client --target x86_64-pc-windows-gnu --features profile-chrome`.


## Workflow (`make perf ...`)

```bash
make perf snapshot                       # dump all diagnostics as a table
make perf snapshot PREFIX=world_counts/  # only entity counters
make perf snapshot PREFIX=render_phase/  # draw calls per phase (also: cache_counts/, render/)
make perf fps SECS=3                     # settled ~120-frame avg fps / frame time
make perf sample SECS=30 INTERVAL=250 OUT=before.jsonl   # record JSONL for offline diffing
make perf get                            # print RenderDebugSettings
make perf set FIELD=render_effects VALUE=false           # toggle a subsystem remotely
make perf attribute SECS=3               # per-subsystem frame-cost table (see below)
```

`cargo run -p tools --bin brp_perf -- <cmd> --help` shows all flags.

## What the diagnostics dump contains

`openroad/diagnostics` (registered in `main.rs`, handler in
`client/src/plugins/diagnostics.rs`) returns every diagnostic in the
`DiagnosticsStore` as `{path: {value, avg, smoothed}}`:

- `fps`, `frame_time`, `frame_count`, `entity_count` — Bevy's built-ins.
- `world_counts/*` — per-category entity counters (terrain blocks/tiles, map
  objects, mesh parts, effects, particles, bones, …) plus load/gating gauges:
  `loading_compounds`, `loading_resources` (in-flight object loads),
  `terrain_building` (regions parked in the per-frame mesh-build budget),
  `paused_animations`, `paused_effects` (distance-gated subtrees).
- `cache_counts/*` — sizes of the dedup/registry maps (`sro_meshes`,
  `sro_bind_poses`, `sro_materials`, `spawned_map_objects`, `effect_meshes`,
  `effect_materials`). The maps hold weak ids and are swept every 10s, so
  the counts track *live* cached assets: expect a climb while exploring and
  a drop shortly after leaving an area. Growth that never plateaus while
  revisiting the same area indicates a cache leak.
- `render/*/elapsed_gpu`, `render/*/elapsed_cpu` and the pipeline statistics
  (`vertex_shader_invocations`, `clipper_primitives_out`, …) from
  `RenderDiagnosticsPlugin`. Bevy requests every adapter feature
  (`WgpuSettingsPriority::Functionality`), so **on Vulkan and DX12 the
  `elapsed_gpu` rows are real timestamp queries** — no opt-in needed. Metal and
  WebGPU have no timestamp queries, and there only `elapsed_cpu` (the pass's
  command-encode time) is recorded; it still ranks passes, but real GPU numbers
  there need an Xcode GPU frame capture. The on-screen panel walks whatever is
  present and prefers GPU per pass, marking each row `gpu` or `cpu`.
- `render_phase/<phase>/{batch_sets,bins,unbatchable,draws}` — **draw calls per
  render phase**, summed over every view (shadow cascades and the offscreen
  portrait/paper-doll rigs included). `draws` is the actionable total;
  `batch_sets` counts multi-draw-indirect sets, `bins` batchable-but-not-
  multidrawable bins, and `unbatchable` the entities that get a draw each.
  Sorted phases (`transparent_3d`, `transmissive_3d`) publish only `draws` —
  they have no bins, so every item is its own draw. This is the number the
  batching levers in `perf-future-levers.md` are defined in terms of: collapsing
  terrain materials, restoring effect batching and merging object mesh parts all
  mean "make these go down".
- `mesh_allocator_{slabs,slabs_size,allocations}` and
  `render_asset/*` — what the render world actually holds. Meshes sharing a slab
  are what makes a batch possible, so slab count is a batching signal; the
  render-asset counts are where a GPU-side texture or mesh leak shows up, which
  the CPU-side `cache_counts/*` maps cannot see.
- `process/mem_usage` (GB), `process/cpu_usage`, `system/*` — the client's
  own footprint via `SystemInformationDiagnosticsPlugin`. This is the way to
  hunt memory growth remotely: sample it over time and A/B against suspected
  churn sources (sandboxed sessions cannot `ps`/`vmmap` the game).

`avg` is the mean over the diagnostic's ~120-measurement history — about 2 s
of frames at 60 fps but ~8 s at 15 fps. Settle times (`SECS`) must exceed
`120 / expected_fps` or averages still contain pre-change frames. `smoothed`
is the EMA the on-screen FPS overlay shows.

## Attribution mode

`make perf attribute` measures what each subsystem costs per frame: it reads
`RenderDebugSettings`, measures a baseline, then for each of
`render_terrain`, `render_objects`, `render_water`, `render_effects`,
`play_animations`, `enable_fog`, `backface_culling`, `automatic_batching`
toggles the field off
via `world.mutate_resources`, settles, samples `frame_time.avg`, and restores
it (`cost_ms = frame_time_on − frame_time_off`). `enable_shadows` is skipped
(off by default). The baseline is re-measured at the end and a >10 % drift
prints a warning — run it standing still in a fully loaded area; a streaming
world makes the numbers noisy.

## Raw curl (what the CLI sends)

Bevy 0.19 method names are dotted (`world.get_resources`,
`world.mutate_resources`) — not the pre-0.16 `bevy/*` names — and resource
params need the *full* type path.

```bash
curl -s -X POST http://127.0.0.1:15702/ -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"openroad/diagnostics"}'

curl -s -X POST http://127.0.0.1:15702/ -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"world.mutate_resources","params":{
        "resource":"client::plugins::dev::render_debug::RenderDebugSettings",
        "path":"render_effects","value":false}}'

curl -s -X POST http://127.0.0.1:15702/ -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":3,"method":"world.get_resources","params":{
        "resource":"client::plugins::dev::render_debug::RenderDebugSettings"}}'
```

`rpc.discover` lists all available methods (Bevy built-ins,
`brp_extras/*`, and `openroad/diagnostics`). Reflection-based methods
(`world.query`, `world.get_resources`) only see the few types the client
registers with `register_type` — the marker components behind
`world_counts/*` are deliberately *not* registered; that is what the
`openroad/diagnostics` dump is for.

## Per-system CPU profiles (chrome traces)

BRP diagnostics answer *what* is loaded and *how long* frames take; per-system
CPU attribution comes from a chrome trace, which writes `trace-<nanos>.json` to
the working directory each run.

It is a **build flag**, not a source edit — `client/Cargo.toml` declares
`profile-chrome = ["bevy/trace_chrome"]`, so profiling never means modifying
tracked files:

```bash
make profile chrome            # Linux client (see the WSL caveat below)
make profile windows           # cross-compile a profiling client.exe
make profile summary           # rank the newest trace by span self-time
```

The feature stays out of the default set because it instruments every system
span in every build that carries it, which is a cost no shipped artifact should
pay.

On WSL, `make profile chrome` builds and runs a *Linux* client, which has no
GPU (`/dev/dri` is absent) and renders through llvmpipe — its frame times are
the software rasterizer's. The CPU span ranking is still valid, but for
representative numbers use `make profile windows` and run the exe from a local
Windows disk.

Caveats that make traces silently useless:

- **`RUST_LOG` must not filter bevy targets.** System/schedule spans are
  INFO-level with `bevy_ecs::*` targets; a filter like `warn,client=info`
  keeps the client's own log lines (so everything *looks* fine) but drops
  every span — the trace ends up a few KB of log events. The make targets
  unset `RUST_LOG` for exactly this reason.
- **Exit the game cleanly** (close the window, don't kill it): the tracing
  writer buffers and only reliably flushes on shutdown.
- Expect ~5-10 MB per second of gameplay; 10 s standing still is plenty.
- Write the trace to a **local Windows disk**, not across the WSL 9p bridge —
  at tens of MB per second that write distorts the frame times being measured.

Rank it with `make profile summary` (or `cargo run -p tools --bin trace_summary
-- trace-*.json --top 30`), which reports per-span **self** time. Use
`--last-secs 10` to drop the loading phase. Spans nest, so wall time puts the
root schedule on top of every capture and says nothing.

Read `mean us` as per-frame cost for anything running once per frame. Note the
instrumented build runs at roughly half the frame rate, so treat these as
*relative* weights — with one exception: a span's mean is comparable **across**
traces when the thing it works on has not changed.

## GPU timelines (Tracy)

A chrome trace shows the CPU threads. It cannot tell you whether the GPU is
busy for the whole frame or busy for half of it and idle for the rest, and
those imply opposite next moves — the first says reduce GPU work, the second
says find the stall. Tracy answers it directly, because bevy's
`RenderDiagnosticsPlugin` uploads per-pass GPU timestamps as Tracy GPU zones
(`bevy_render/src/diagnostic/internal.rs`), reachable through
`bevy/trace_tracy` → `bevy_internal/trace_tracy` → `bevy_render/tracing-tracy`.
No extra code: the diagnostics tier already registers the plugin.

```bash
make profile windows-tracy release   # cross-compile a Tracy-instrumented client.exe
make profile tracy                   # Linux client; software rendering under WSL
```

Cross-compiling this one needs the mingw **C++** compiler on top of the usual
`gcc-mingw-w64-x86-64` — Tracy's client is C++, and `tracy-client-sys` compiles
`TracyClient.cpp` as part of the build:

```bash
sudo apt install g++-mingw-w64-x86-64
```

`make profile windows-tracy` checks for it up front, because without it the
build fails deep inside a cc-rs build script (`failed to find tool
"x86_64-w64-mingw32-g++"`) rather than saying what is missing.

Linking C++ also means the exe needs the mingw runtime DLLs beside it, which
the normal pure-Rust client does not. Windows reports these one at a time
(`libstdc++-6.dll`, then `libgcc_s_seh-1.dll` which the first one pulls in), so
copy the whole import closure at once rather than chasing the error boxes:

```bash
scripts/mingw-runtime-dlls.sh \
    target/x86_64-pc-windows-gnu/release/client.exe /mnt/c/coding/openroad
```

That script walks the closure with `objdump` and resolves it against the
directory the *building* g++ reports, so it stays correct across toolchain
upgrades and does not confuse the win32 and posix mingw variants.

The Tracy **viewer is a third-party download you fetch yourself** — it is not
vendored here and not linked from the repo. Its release must match the bundled
`tracy-client-sys` (see `Cargo.lock`; the crate's README names the Tracy
version it speaks). A mismatched viewer refuses the connection with a protocol
error rather than misbehaving quietly.

Run the viewer **on Windows**, next to the client: Tracy connects over TCP
8086 and both ends are then host-local, so none of the WSL NAT problem that
forced a cross-compiled `brp_perf.exe` applies. Start the viewer first; the
client connects on launch and streams live, so there is no truncated-file
hazard — save from the viewer once you have enough.

What to read off it:

- **GPU busy time per frame** against the frame time. This is the number the
  `render/` diagnostics cannot give you: they measure *inside* pass spans, so
  they miss both the gaps between passes and the passes bevy never
  instruments — the shadow pass and tonemapping among them.
- **Where `prepare_windows` blocks.** It is a swapchain acquire, so it shows up
  as a long main-thread wait either way; the GPU track says whether the GPU was
  saturated underneath it (fill-bound) or idle (a stall to find).

## Offline analysis of samples

```bash
jq -s 'map(.frame_time.value) | add/length' before.jsonl     # mean frame time
jq -c '{t: .t_ms, particles: ."world_counts/particles".value}' before.jsonl
```
