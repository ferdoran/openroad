<#
.SYNOPSIS
  Capture a full performance baseline from a running client, plus the subsystem
  A/Bs, into one file.

.DESCRIPTION
  Runs on Windows against a Windows client. That is not a preference: on a WSL2
  machine a Linux client has no GPU (no /dev/dri, only /dev/dxg) so wgpu falls
  back to a software rasterizer, and the client's BRP server binds 127.0.0.1 --
  which WSL's loopback is not. See docs/perf-remote.md.

  The client needs `diagnostics: true` in the config.yaml it actually loaded
  (that path is relative to the client's working directory, so `make run wsl`
  reads the WSL checkout's copy). It does NOT need `dev_tools`.

  This drives a LIVE client: the experiments switch terrain, shadows, objects
  and effects off one at a time, so the game visibly flickers while it runs.
  Every setting is read first and restored in a finally block, so an interrupted
  run does not leave the client with terrain switched off. Use -NoExperiments
  for a read-only capture that changes nothing.

.PARAMETER Settle
  Seconds to wait before reading each measurement. Must exceed 120 / fps or the
  ~120-sample average still contains frames from before the change -- at 20 fps
  that is 5.9 s, hence the default of 8. Raise it if the client is slower.

.EXAMPLE
  .\scripts\perf-capture.ps1
  .\scripts\perf-capture.ps1 -Settle 12 -Out jangan-msaa1.txt
#>
[CmdletBinding()]
param(
    [string]$Brp,
    [double]$Settle = 8,
    [string]$Out,
    # Baseline only: skip the four toggle experiments, which take ~10 x Settle.
    [switch]$NoExperiments,
    # Skip just the resolution ladder, keeping the subsystem A/Bs.
    [switch]$NoLadder,
    # Render scales to sweep. The first entry is the reference the fit is
    # reported against, so keep 1.0 first. Four points is enough for a
    # two-unknown fit with slack, and each one costs another Settle.
    [double[]]$Ladder = @(1.0, 0.85, 0.7, 0.55)
)

$ErrorActionPreference = "Stop"

# --- locate brp_perf.exe ----------------------------------------------------
if (-not $Brp) {
    $candidates = @(
        (Join-Path $PSScriptRoot "..\target\x86_64-pc-windows-gnu\release\brp_perf.exe"),
        (Join-Path $PSScriptRoot "..\target\x86_64-pc-windows-gnu\debug\brp_perf.exe"),
        # `make run wsl` starts the client from the WSL checkout, so that is
        # where a `make build windows` put the exe even when this script is run
        # from a Windows-side copy of the repo.
        "\\wsl$\Ubuntu\home\$env:USERNAME\workspaces\private\openroad\target\x86_64-pc-windows-gnu\release\brp_perf.exe",
        "\\wsl.localhost\Ubuntu\home\$env:USERNAME\workspaces\private\openroad\target\x86_64-pc-windows-gnu\release\brp_perf.exe"
    )
    $Brp = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
}
if (-not $Brp -or -not (Test-Path -LiteralPath $Brp)) {
    Write-Error "brp_perf.exe not found. Build it with 'make build windows release' in WSL, or pass -Brp <path>."
    exit 1
}
$Brp = (Resolve-Path -LiteralPath $Brp).Path

if (-not $Out) { $Out = "perf-{0}.txt" -f (Get-Date -Format "yyyyMMdd-HHmmss") }

# --- output plumbing --------------------------------------------------------
$lines = New-Object System.Collections.Generic.List[string]
function Emit([string]$text = "") {
    $lines.Add($text) | Out-Null
    Write-Host $text
}
function Section([string]$title) {
    $script:section = $title
    Emit ""
    Emit ("=" * 72)
    Emit "== $title"
    Emit ("=" * 72)
}

# Runs brp_perf and captures stdout+stderr as text. Never throws: a failed step
# should be recorded and the run should continue to the restore.
function Brp([string[]]$BrpArgs) {
    $text = (& $Brp @BrpArgs 2>&1 | Out-String)
    return $text.TrimEnd()
}
$script:frames = New-Object System.Collections.Generic.List[object]
# NOT $script:ladder: PowerShell variable names are case-insensitive, so that
# name IS the -Ladder parameter, and assigning it here silently replaced the
# sweep list with an empty one. The foreach then ran zero times and the whole
# section completed without emitting anything -- no rows, no error, no clue.
$script:ladderRows = New-Object System.Collections.Generic.List[object]
function Step([string]$title, [string[]]$BrpArgs) {
    Emit ""
    Emit "--- $title"
    $text = Brp $BrpArgs
    $script:lastText = $text
    Emit $text
    # Remember every frame-time reading so the run can judge its own deltas
    # against its own drift at the end, instead of leaving that to whoever
    # reads the file.
    $m = [regex]::Match($text, 'frame_time:\s*([0-9.]+)')
    if ($m.Success) {
        $script:frames.Add([pscustomobject]@{ Label = $script:section; Ms = [double]$m.Groups[1].Value })
    }
}

# --- preflight --------------------------------------------------------------
$settingsRaw = Brp @("get")
try {
    $settings = $settingsRaw | ConvertFrom-Json
} catch {
    Write-Host $settingsRaw
    Write-Error "Could not read RenderDebugSettings -- the client is not reachable. See the message above."
    exit 1
}

Section "Environment"
Emit "date        : $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')"
Emit "brp_perf    : $Brp"
Emit "settle      : $Settle s"
# Every adapter, not the first: a machine with a remote-desktop or virtual
# display enumerates that ahead of the real GPU, and naming the wrong one makes
# the whole reading unattributable.
foreach ($gpu in (Get-CimInstance Win32_VideoController)) {
    $mode = if ($gpu.CurrentHorizontalResolution) { " @ $($gpu.CurrentHorizontalResolution)x$($gpu.CurrentVerticalResolution)" } else { "" }
    Emit "gpu         : $($gpu.Name)$mode"
}
Emit ""
Emit "Stand still in a fully streamed area for the whole run; a moving camera"
Emit "makes the sections inconsistent with each other."

# --- baseline ---------------------------------------------------------------
# Streaming has to finish before any of this means anything: at spawn the client
# is still building terrain meshes and resolving resource loads, and a reading
# taken during that measures the loader, not the frame. Both counters are
# published for exactly this (see plugins/diagnostics.rs), so wait on them
# rather than asking the operator to judge it by eye.
function Wait-ForStreaming([int]$TimeoutSecs = 90) {
    $deadline = (Get-Date).AddSeconds($TimeoutSecs)
    while ((Get-Date) -lt $deadline) {
        $counts = Brp @("snapshot", "--prefix", "world_counts/")
        $building = [regex]::Match($counts, 'terrain_building\s+([0-9.]+)')
        $loading = [regex]::Match($counts, 'loading_resources\s+([0-9.]+)')
        if (-not $building.Success -or -not $loading.Success) {
            return "could not read the streaming counters; measuring anyway"
        }
        $b = [double]$building.Groups[1].Value
        $l = [double]$loading.Groups[1].Value
        if ($b -eq 0 -and $l -eq 0) {
            return "settled (terrain_building 0, loading_resources 0)"
        }
        Write-Host "  waiting for streaming: terrain_building=$b loading_resources=$l"
        Start-Sleep -Seconds 2
    }
    return "TIMED OUT after ${TimeoutSecs}s with streaming still in flight -- the numbers below include loader work"
}

Section "Streaming settle"
Emit (Wait-ForStreaming)

# Discarded on purpose: an idle laptop GPU sits at a low clock and ramps only
# once it has work. Measuring immediately after streaming settles catches that
# ramp, and the ramp is worth more than any lever being measured -- one run
# drifted 6.6% across its own experiments while the machine warmed up, which
# swamped all five of them.
Section "Warm-up (discarded)"
Step "fps"                  @("fps", "--settle-secs", "$Settle")
$script:frames.Clear()

Section "Baseline"
Step "fps"                  @("fps", "--settle-secs", "$Settle")

# The diagnostic averages a fixed ~120 samples, so the settle has to outlast
# that many frames or the mean still contains pre-change frames. Easy to get
# wrong by eye: at 20 fps it is 6 s, not the 3 s the CLI defaults to.
$measuredFps = [regex]::Match(($lines[-1]), 'fps:\s*([0-9.]+)')
if ($measuredFps.Success) {
    $needed = 120.0 / [double]$measuredFps.Groups[1].Value
    if ($Settle -lt $needed) {
        Emit ""
        Emit ("WARNING: -Settle {0}s is shorter than the {1:N1}s that 120 frames take at {2} fps." -f $Settle, $needed, $measuredFps.Groups[1].Value)
        Emit ("         Every average below still contains frames from before its change." )
        Emit ("         Re-run with -Settle {0:N0}." -f [Math]::Ceiling($needed * 1.3))
    }
}
Step "render phases (draw calls)" @("snapshot", "--prefix", "render_phase/")
Step "render passes (GPU/CPU ms)" @("snapshot", "--prefix", "render/")
# Captured here so the verdict can separate the two halves of the frame. GPU
# pass time scales with the pixel count; the wait for a swapchain image does
# not. That distinction is what makes a windowed run comparable with a
# fullscreen one at all -- borderless renders at the monitor's resolution, so
# the two modes differ by ~11% in fragments before anything else is measured.
$script:renderPixels = 0
$script:gpuSumMs = 0.0
$px = [regex]::Match($script:lastText, 'fxaa/fxaa/fragment_shader_invocations\s+([0-9.]+)')
if ($px.Success) { $script:renderPixels = [double]$px.Groups[1].Value }
foreach ($line in ($script:lastText -split "`n")) {
    # Second column is `avg`. `fxaa/fxaa` is the pass nested inside the `fxaa`
    # node, so counting both would double-charge it.
    $m = [regex]::Match($line, '^render/(\S+)/elapsed_gpu\s+\S+\s+([0-9.]+)')
    if ($m.Success -and $m.Groups[1].Value -notlike 'fxaa/fxaa*') {
        $script:gpuSumMs += [double]$m.Groups[2].Value
    }
}
Step "world counts"         @("snapshot", "--prefix", "world_counts/")
Step "cache counts"         @("snapshot", "--prefix", "cache_counts/")
Step "mesh allocator + render assets" @("snapshot", "--prefix", "mesh_allocator")
Step "render assets (GPU-resident)" @("snapshot", "--prefix", "render_asset")
# Process CPU/memory says whether an unexplained gap between frame_time and the
# summed GPU passes is main-thread work or something outside the app entirely.
Step "process + system"     @("snapshot", "--prefix", "process/")
Step "system"               @("snapshot", "--prefix", "system/")
Step "render debug settings" @("get")

# --- experiments ------------------------------------------------------------
# field -> value to test. The originals are restored from `settings` afterwards,
# not from an assumption about defaults: enable_shadows in particular is seeded
# from graphics.shadows.enabled, so its default is whatever config says.
$experiments = @(
    @{ Field = "render_terrain";       Value = "false"; Why = "terrain's share of the opaque pass" },
    @{ Field = "terrain_lighting_mode"; Value = "2";    Why = "splat sampling vs PBR lighting (2 = baked, no N.L)" },
    @{ Field = "enable_shadows";       Value = "false"; Why = "sizes the shadow pass, which bevy 0.19 does not instrument" },
    @{ Field = "render_objects";       Value = "false"; Why = "how much the map objects actually contribute" },
    @{ Field = "render_effects";       Value = "false"; Why = "effects, incl. the per-frame UV material churn" }
)

$touched = @{}
try {
    if (-not $NoExperiments) {
        foreach ($e in $experiments) {
            $field = $e.Field
            $original = $settings.$field
            if ($null -eq $original) {
                Emit ""
                Emit "--- SKIPPED $field (not present in RenderDebugSettings)"
                continue
            }
            $originalJson = ($original | ConvertTo-Json -Compress)
            if ($originalJson -eq $e.Value) {
                Emit ""
                Emit "--- SKIPPED $field (already $originalJson, so toggling measures nothing)"
                continue
            }

            Section "$field = $($e.Value)  --  $($e.Why)"
            Emit "(original: $originalJson)"
            $touched[$field] = $originalJson

            $setResult = Brp @("set", $field, $e.Value)
            if ($setResult) { Emit $setResult }
            Step "fps"           @("fps", "--settle-secs", "$Settle")
            Step "opaque pass"   @("snapshot", "--prefix", "render/main_opaque")
            Step "render phases" @("snapshot", "--prefix", "render_phase/")
            # Per experiment, not just at the baseline: the question these rows
            # answer is whether a change moved the *wait* or the work, and that
            # is only visible if every row carries them.
            Step "pipeline"      @("snapshot", "--prefix", "render/pipeline/")

            Brp @("set", $field, $originalJson) | Out-Null
            $touched.Remove($field)
        }
    }

    # --- resolution ladder --------------------------------------------------
    # The one experiment the A/Bs above cannot run: they each remove a
    # subsystem, so they measure what that subsystem costs, never how the frame
    # as a whole responds to pixels. This sweeps render_scale and fits
    #
    #     frame_ms = fixed + k * megapixels
    #
    # over readings taken in ONE session, which is what makes it trustworthy --
    # comparing a windowed run against a fullscreen one crosses power plan,
    # camera direction and server population all at once, and those are exactly
    # the confounders docs/perf-baselines.md warns about.
    #
    # The slope is what render_scale (and every other per-pixel lever: shadow
    # filtering, tonemapping, MSAA) can buy. The intercept is what none of them
    # can touch -- shadow-map rendering, draw submission, main-thread work --
    # and it is the ceiling on what this whole class of change can achieve.
    if (-not $NoExperiments -and -not $NoLadder) {
        $original = $settings.render_scale
        if ($null -eq $original) {
            Section "Resolution ladder"
            Emit "SKIPPED: this client has no render_scale field (built before it landed)."
        } else {
            $touched["render_scale"] = ($original | ConvertTo-Json -Compress)
            foreach ($scale in $Ladder) {
                # Invariant culture, not "$scale": on a de-DE machine string
                # interpolation of a double emits "0,85", which is not JSON and
                # would be rejected (or worse, silently reparsed) by `set`.
                $scaleText = $scale.ToString([System.Globalization.CultureInfo]::InvariantCulture)
                Section ("render_scale = {0}" -f $scaleText)
                Brp @("set", "render_scale", $scaleText) | Out-Null
                Step "fps" @("fps", "--settle-secs", "$Settle")
                $frameMs = if ($script:frames.Count) { $script:frames[$script:frames.Count - 1].Ms } else { 0 }
                # Read the pixel count rather than computing it: rounding and a
                # window that is not what the OS claims both make the arithmetic
                # wrong, and this pass covers exactly width x height.
                Step "render size" @("snapshot", "--prefix", "render/fxaa/fxaa/fragment")
                $m = [regex]::Match($script:lastText, 'fragment_shader_invocations\s+([0-9.]+)')
                if ($m.Success -and $frameMs -gt 0) {
                    $script:ladderRows.Add([pscustomobject]@{
                        Scale = [double]$scale
                        Px    = [double]$m.Groups[1].Value
                        Ms    = $frameMs
                    }) | Out-Null
                }
            }
            Brp @("set", "render_scale", $touched["render_scale"]) | Out-Null
            $touched.Remove("render_scale")
        }
    }
}
finally {
    # Anything still in $touched means the run was interrupted mid-experiment.
    foreach ($field in @($touched.Keys)) {
        Write-Host "restoring $field = $($touched[$field])"
        & $Brp set $field $touched[$field] 2>&1 | Out-Null
    }

    if (-not $NoExperiments) {
        Section "Baseline re-check (drift)"
        Emit "Compare against the first fps block. A large gap means the world"
        Emit "changed under the measurement -- streaming, weather, a mob wave --"
        Emit "and the experiment deltas above are not comparable."
        Step "fps" @("fps", "--settle-secs", "$Settle")
        Step "render debug settings (should match the baseline block)" @("get")
    }

    if ($script:frames.Count -ge 2) {
        Section "Verdict"
        $baseline = $script:frames[0]
        $recheck = $script:frames[$script:frames.Count - 1]
        $drift = [Math]::Abs($recheck.Ms - $baseline.Ms)
        Emit ("baseline {0:N3} ms, re-check {1:N3} ms -> drift {2:N3} ms" -f $baseline.Ms, $recheck.Ms, $drift)
        if ($script:renderPixels -gt 0) {
            $residual = $baseline.Ms - $script:gpuSumMs
            Emit ""
            Emit ("render size    : {0:N0} px ({1})" -f $script:renderPixels, $(if ($script:renderPixels -ge 2304000) { "monitor-sized; a fullscreen mode" } else { "windowed" }))
            Emit ("GPU passes     : {0:N2} ms  ({1:N3} ns/fragment)" -f $script:gpuSumMs, ($script:gpuSumMs * 1e6 / $script:renderPixels))
            Emit ("residual       : {0:N2} ms  (frame minus GPU passes: CPU work plus every wait)" -f $residual)
            Emit ""
            Emit "Comparing two runs at different render sizes: GPU passes scale with the"
            Emit "pixel count and the residual does not, so compare ns/fragment for the"
            Emit "first and raw ms for the second. A windowed and a borderless run differ"
            Emit "by ~11% in fragments on a 16:10 panel before anything else is measured."
        }
        # Say so out loud when the sweep produced nothing. The first version of
        # this section could complete without emitting a single line, so the
        # report simply had no ladder in it and looked like a run that was never
        # asked for one -- silence has to be impossible here, not merely rare.
        if ($script:ladderRows.Count -lt 2 -and -not $NoLadder -and -not $NoExperiments) {
            Emit ""
            Emit ("Resolution ladder: only {0} usable point(s) -- no fit. Either `perf set" -f $script:ladderRows.Count)
            Emit "render_scale` failed (a client built before the field landed), or the"
            Emit "fxaa fragment count could not be read back at each step."
        }
        if ($script:ladderRows.Count -ge 2) {
            Emit ""
            Emit "Resolution ladder (one session, so these rows ARE comparable):"
            Emit ("  {0,-7} {1,12} {2,10} {3,12}" -f "scale", "pixels", "frame ms", "ns/pixel")
            foreach ($r in $script:ladderRows) {
                Emit ("  {0,-7:N2} {1,12:N0} {2,10:N3} {3,12:N3}" -f $r.Scale, $r.Px, $r.Ms, ($r.Ms * 1e6 / $r.Px))
            }
            # Least squares on (megapixels, ms). Two unknowns, >=2 points.
            $n = $script:ladderRows.Count
            $sx = 0.0; $sy = 0.0; $sxx = 0.0; $sxy = 0.0
            foreach ($r in $script:ladderRows) {
                $x = $r.Px / 1e6; $y = $r.Ms
                $sx += $x; $sy += $y; $sxx += $x * $x; $sxy += $x * $y
            }
            $denom = ($n * $sxx) - ($sx * $sx)
            if ([Math]::Abs($denom) -gt 1e-9) {
                $slope = (($n * $sxy) - ($sx * $sy)) / $denom
                $intercept = ($sy - ($slope * $sx)) / $n
                $atFull = $script:ladderRows[0]
                $fillMs = $slope * ($atFull.Px / 1e6)
                Emit ""
                Emit ("  fit: frame = {0:N3} ms fixed + {1:N3} ms/megapixel" -f $intercept, $slope)
                Emit ("  at {0:N0} px that is {1:N2} ms fill ({2:N0}%) and {3:N2} ms fixed ({4:N0}%)" -f `
                    $atFull.Px, $fillMs, (100 * $fillMs / $atFull.Ms), $intercept, (100 * $intercept / $atFull.Ms))
                Emit ""
                Emit "  The slope is the ceiling on every per-pixel lever (render_scale, shadow"
                Emit "  filtering, tonemapping, MSAA) put together. The intercept is what none of"
                Emit "  them can reach: shadow-map rendering, draw submission, main-thread work."
                if ($intercept -lt 0) {
                    Emit ""
                    Emit "  A negative intercept means the fit is not linear -- most likely the GPU"
                    Emit "  clocked differently across the sweep. Check ns/pixel above: it should"
                    Emit "  RISE as the scale drops (fixed cost spread over fewer pixels). If it"
                    Emit "  falls instead, the machine changed mid-run and the fit says nothing."
                }
            }
        }
        Emit ""
        Emit ("{0,-42} {1,10} {2,9}  {3}" -f "change", "frame ms", "delta", "verdict")
        Emit ("-" * 78)
        foreach ($f in $script:frames) {
            $d = $f.Ms - $baseline.Ms
            $verdict = if ($f -eq $baseline) { "" }
                elseif ([Math]::Abs($d) -le $drift) { "INSIDE DRIFT - not measurable" }
                else { "resolved" }
            Emit ("{0,-42} {1,10:N3} {2,9:N3}  {3}" -f $f.Label, $f.Ms, $d, $verdict)
        }
        Emit ("-" * 78)
        if ($drift -gt 0.4) {
            Emit ""
            Emit "Drift exceeds the deltas this run was trying to resolve, so most rows"
            Emit "above say nothing. On a laptop APU the usual cause is GPU clock ramp."
            Emit "Cross-check render/fxaa/elapsed_gpu against a previous capture: it is a"
            Emit "fixed full-screen workload, so if it moved, the machine did -- not the game."
            Emit "Set the Windows power plan to High performance and re-run."
        }
    }

    $lines | Set-Content -LiteralPath $Out -Encoding UTF8
    Write-Host ""
    Write-Host "Wrote $((Resolve-Path -LiteralPath $Out).Path)"
}
