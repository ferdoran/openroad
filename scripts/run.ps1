param(
    [string]$RunTarget = "",
    # `make run <scene> RELEASE=1` passes -Release; the Makefile cannot spend the
    # subcommand slot on the profile because the scene already owns it.
    [switch]$Release
)

# Splatted into every cargo invocation below so the profile is chosen in one place.
$cargoArgs = if ($Release) { @("--release") } else { @() }

# Make's Unix run recipe sources .env before launching Cargo. PowerShell performs
# the equivalent here because Windows GNU Make executes recipes through cmd.exe.
if (Test-Path -LiteralPath ".env") {
    Get-Content -LiteralPath ".env" | ForEach-Object {
        $line = $_.Trim()
        if ($line -and -not $line.StartsWith("#") -and $line.Contains("=")) {
            $key, $value = $line.Split("=", 2)
            $value = $value.Trim().Trim("'", '"')
            [Environment]::SetEnvironmentVariable($key.Trim(), $value, "Process")
        }
    }
}

switch ($RunTarget) {
    "" {
        & cargo run -p client @cargoArgs
    }
    "launcher" {
        if (-not $env:SRO_PATH) { $env:SRO_PATH = "." }
        & cargo run -p launcher @cargoArgs
    }
    { $_ -in @("intro", "intro_v2") } {
        $env:SCENE = "intro_v2"
        & cargo run -p client @cargoArgs
    }
    { $_ -in @("world", "animations", "ui_testing", "asset_loading", "skills", "dungeons") } {
        $env:SCENE = $RunTarget
        & cargo run -p client @cargoArgs
    }
    "asset-loading" {
        $env:SCENE = "asset_loading"
        & cargo run -p client @cargoArgs
    }
    "wsl" {
        # Launches a client.exe cross-built from WSL by `make build windows`, so the
        # WSL checkout stays the single source of truth and this side only runs it.
        $distro = if ($env:WSL_DISTRO) { $env:WSL_DISTRO } else { "Ubuntu" }
        $repoPath = if ($env:WSL_REPO_PATH) { $env:WSL_REPO_PATH } else { "~/workspaces/private/openroad" }
        # Single quotes would stop bash from expanding ~, so swap it for a
        # literal $HOME that bash resolves itself inside double quotes instead.
        $bashRepoPath = if ($repoPath.StartsWith("~")) { '$HOME' + $repoPath.Substring(1) } else { $repoPath }
        $wslCmd = "wslpath -w `"$bashRepoPath`""
        $winPath = (& wsl.exe -d $distro -e bash -lc $wslCmd 2>$null | Select-Object -Last 1)
        if (-not $winPath) {
            Write-Error "Could not resolve the WSL repo path via 'wsl.exe -d $distro'. Set `$env:WSL_DISTRO / `$env:WSL_REPO_PATH if your setup differs."
            exit 1
        }
        $winPath = $winPath.Trim()
        $profileDir = if ($Release) { "release" } else { "debug" }
        $exe = Join-Path $winPath "target\x86_64-pc-windows-gnu\$profileDir\client.exe"
        if (-not (Test-Path -LiteralPath $exe)) {
            $buildCmd = if ($Release) { "make build windows release" } else { "make build windows" }
            Write-Error "No Windows build found at $exe. Run '$buildCmd' from WSL first."
            exit 1
        }
        if (-not $env:SRO_PATH) { $env:SRO_PATH = Join-Path $winPath "assets" }
        Push-Location -LiteralPath $winPath
        try { & $exe } finally { Pop-Location }
    }
    default {
        Write-Error "Unknown run target: $RunTarget"
        exit 2
    }
}

exit $LASTEXITCODE
