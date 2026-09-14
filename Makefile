.PHONY: help
.PHONY: install-deps setup
.PHONY: check-deps check-env
.PHONY: run watch launcher intro intro_v2 world animations ui_testing asset-loading asset_loading skills dungeons netcheck

.PHONY: build windows release test fmt fmt-check clippy warnings opcodes check-target-dir re-tools reference-data deny ci clean
.PHONY: pk2 pk2-list pk2-unpack list unpack bsr2glb
.PHONY: perf snapshot sample fps get set attribute
.PHONY: profile chrome tracy summary windows
.PHONY: cutscene convert

RUN_TARGET := $(word 2,$(MAKECMDGOALS))
BUILD_TARGET := $(word 2,$(MAKECMDGOALS))
PK2_TARGET := $(word 2,$(MAKECMDGOALS))
PERF_TARGET := $(word 2,$(MAKECMDGOALS))
PROFILE_TARGET := $(word 2,$(MAKECMDGOALS))
CUTSCENE_TARGET := $(word 2,$(MAKECMDGOALS))

# The release profile is selected by the word `release` anywhere in the goal list —
# `make build windows release`, `make run world release` — or equivalently by RELEASE=1.
# It cannot be a *positional* subcommand the way `pk2 list` is, because `run` and `build`
# already spend their second word on the scene or the cross-target; matching it
# positionally would collide. `make build release` is the one exception that reads the
# word positionally, because there the second word is free.
RELEASE_SELECTED := $(or $(RELEASE),$(filter release,$(MAKECMDGOALS)))
CARGO_PROFILE_FLAG := $(if $(RELEASE_SELECTED),--release,)

# Help

help:
	@echo "Targets:"
	@echo ""
	@echo "Setup:"
	@echo "  install-deps    Install Linux native build dependencies"
	@echo "  setup           Create local config/assets placeholders and .env for development"
	@echo ""
	@echo "Checks:"
	@echo "  check-deps      Check Linux native build dependencies"
	@echo "  check-env       Check local config and required user-supplied PK2 files"
	@echo ""
	@echo "Run:"
	@echo "  run             Run client (uses config scenes.startup)"
	@echo "  run launcher    Run launcher (GUI)"
	@echo "  run intro       Run client with SCENE=intro_v2 (the current intro)"
	@echo "  run world       Run client with SCENE=world"
	@echo "  run asset-loading Run client with SCENE=asset_loading"
	@echo "  run skills      Run client with SCENE=skills (offline skill-system test scene)"
	@echo "  run dungeons    Run client with SCENE=dungeons (offline dungeon test scene)"
	@echo "  ... release     Any 'run' above, on the release profile (make run world release)"
	@echo "  watch           Run client on code changes (requires cargo-watch, set SCENE=...)"
	@echo "  watch launcher  Run launcher on code changes (requires cargo-watch)"
	@echo "  run wsl         Run from PowerShell, NOT from WSL: launches the client.exe that"
	@echo "                  'make build windows' cross-built in the WSL checkout"
	@echo ""
	@echo "Build and quality:"
	@echo "  build           Build all workspace crates"
	@echo "  build release   Build the client with --release, exactly as build-*.yml does"
	@echo "  build windows   (WSL/Linux) Cross-compile client.exe + brp_perf.exe for Windows;"
	@echo "                  needs"
	@echo "                  rustup target add x86_64-pc-windows-gnu + apt gcc-mingw-w64-x86-64"
	@echo "  build windows release The same cross-compile, on the release profile"
	@echo "  test            Run workspace tests"
	@echo "  fmt             Format all workspace crates"
	@echo "  clippy          Clippy (all targets, all features)"
	@echo "  warnings        Reject Rust warnings except unread struct fields"
	@echo "  opcodes         Check the opcode ledger against the packets! macro"
	@echo "  reference-data  Check textdata column indices against SR_Db2Media (SRO_REFS_PATH)"
	@echo "  deny            Supply-chain gate: licences/advisories (needs cargo-deny)"
	@echo "  ci              Full local gate: check-target-dir + fmt-check + warnings + opcodes + re-tools + reference-data + deny + test + build"
	@echo "  clean           Cargo clean"
	@echo ""
	@echo "Tools:"
	@echo "  pk2 list        List PK2 contents (set PK2=/path/to/file.pk2)"
	@echo "  pk2 unpack      Extract PK2 contents (set PK2=/path/to/file.pk2, OUT=dir, PREFIX=opt)"
	@echo "  bsr2glb         Convert a .bsr (+ deps) to .glb/.fbx (BSR='res\\...' [PK2=] [OUT=] [PREFIX=] [FORMAT=glb|fbx|both] [RAW=1])"
	@echo "  cutscene convert Convert Media/script/intro/<name>.txt into assets/intros/<name>.intro (SCRIPT= [OUT=] [NAME=] [MUSIC=])"
	@echo "  perf snapshot   Dump diagnostics of the running client via BRP (PREFIX=opt)"
	@echo "  perf sample     Record diagnostics to JSONL (SECS=30 INTERVAL=250 OUT=opt)"
	@echo "  perf fps        Print settled avg fps/frame time (SECS=3)"
	@echo "  perf get/set    Read / set RenderDebugSettings (FIELD=render_effects VALUE=false)"
	@echo "  perf attribute  Per-subsystem frame-cost table via off/on toggles (SECS=3)"
	@echo "  profile chrome  Run with chrome-trace instrumentation (writes trace-<nanos>.json)"
	@echo "  profile windows Cross-compile a chrome-trace client.exe (the one to use on WSL)"
	@echo "  profile tracy   Run with Tracy instrumentation (live CPU + GPU zones)"
	@echo "  profile summary Rank the newest trace by span self time (TRACE=, TOP=)"

# Setup

install-deps:
	@if command -v apt >/dev/null 2>&1; then \
		sudo apt install clang pkg-config libx11-dev libasound2-dev libudev-dev libwayland-dev lld; \
	elif command -v dnf >/dev/null 2>&1; then \
		sudo dnf install clang pkgconf-pkg-config libX11-devel alsa-lib-devel systemd-devel wayland-devel lld; \
	elif command -v pacman >/dev/null 2>&1; then \
		sudo pacman -S clang pkgconf libx11 alsa-lib systemd wayland lld; \
	else \
		echo "Unsupported package manager. Install clang, pkg-config/pkgconf, X11, ALSA, libudev/systemd, Wayland, and lld development packages manually."; \
		exit 1; \
	fi

setup:
	mkdir -p assets
	@if [ ! -f config.yaml ]; then \
		cp config.example.yaml config.yaml; \
		echo "Created config.yaml from config.example.yaml"; \
	else \
		echo "config.yaml already exists"; \
	fi
	@if [ -f .env ]; then \
		printf ".env already exists. Overwrite it? [y/N]: "; \
		read -r overwrite_env; \
		case "$$overwrite_env" in y|Y|yes|YES) ;; *) echo "Keeping existing .env"; exit 0 ;; esac; \
	fi; \
	echo "Configure local environment (.env). Leave optional URLs empty to skip."; \
	printf "SRO PK2 folder path: "; \
	read -r sro_path; \
	printf "Discord URL (optional): "; \
	read -r discord_url; \
	printf "YouTube URL (optional): "; \
	read -r youtube_url; \
	printf "News JSON base URL (optional): "; \
	read -r news_json_url; \
	escape_env() { printf "%s" "$$1" | sed "s/'/'\\\\''/g"; }; \
	{ \
		echo "# Local OpenRoad environment"; \
		echo "# Generated by make setup"; \
		printf "SRO_PATH='"; escape_env "$$sro_path"; printf "'\n"; \
		if [ -n "$$discord_url" ]; then printf "DISCORD_URL='"; escape_env "$$discord_url"; printf "'\n"; fi; \
		if [ -n "$$youtube_url" ]; then printf "YOUTUBE_URL='"; escape_env "$$youtube_url"; printf "'\n"; fi; \
		if [ -n "$$news_json_url" ]; then printf "NEWS_JSON_URL='"; escape_env "$$news_json_url"; printf "'\n"; fi; \
	} > .env; \
	echo "Wrote .env"
	@echo "Set SRO_PATH to your PK2 folder or add your own PK2 files to assets/: Media.pk2 Map.pk2 Data.pk2 Music.pk2 Particles.pk2"
	@echo "Note: make setup writes the root .env for local development. Launcher defaults and public URLs can be copied from launcher/.env.example into launcher/.env if needed."

# Checks

check-deps:
	@missing=0; \
	if ! command -v pkg-config >/dev/null 2>&1; then \
		echo "Missing pkg-config/pkgconf. Run: make install-deps"; \
		missing=1; \
	else \
		for package in alsa libudev x11 wayland-client; do \
			if ! pkg-config --exists "$$package"; then \
				echo "Missing $$package pkg-config metadata. Run: make install-deps"; \
				missing=1; \
			fi; \
		done; \
	fi; \
	if ! command -v clang >/dev/null 2>&1; then \
		echo "Missing clang. Run: make install-deps"; \
		missing=1; \
	fi; \
	if ! command -v lld >/dev/null 2>&1; then \
		echo "Missing lld. Run: make install-deps"; \
		missing=1; \
	fi; \
	if [ "$$missing" -eq 0 ]; then \
		echo "Native dependencies OK"; \
	else \
		exit 1; \
	fi

check-env:
	@test -f config.yaml || (echo "Missing config.yaml. Run: make setup"; exit 1)
	@test -f .env || (echo "Missing .env. Run: make setup"; exit 1)
	@set -a; . ./.env; set +a; \
	pk2_dir="$${SRO_PK2_PATH:-$${SRO_PATH:-assets}}"; \
	missing=0; \
	for file in Media.pk2 Map.pk2 Data.pk2 Music.pk2 Particles.pk2; do \
		if [ ! -f "$$pk2_dir/$$file" ]; then \
			echo "Missing $$pk2_dir/$$file"; \
			missing=1; \
		fi; \
	done; \
	if [ -z "$$SRO_PK2_KEY" ] && ! grep -qE '^[[:space:]]*key:[[:space:]]*["'"'"']?[^"'"'"'[:space:]]' config.yaml; then \
		echo "Missing PK2 key: set SRO_PK2_KEY/SRO_PK2_SALT, or a pk2: block in config.yaml."; \
		echo "The archive key is not shipped with openroad — see config.example.yaml."; \
		missing=1; \
	fi; \
	if [ "$$missing" -eq 0 ]; then \
		echo "Environment OK: $$pk2_dir"; \
	else \
		echo "Set SRO_PATH in .env to your PK2 folder, or add your own PK2 files to assets/."; \
		exit 1; \
	fi

# Run

# Windows GNU Make uses cmd.exe, so delegate the run subcommand to PowerShell;
# this keeps the existing POSIX recipe unchanged on Unix while preserving .env loading.
ifeq ($(OS),Windows_NT)
run:
	powershell -NoProfile -ExecutionPolicy Bypass -File scripts/run.ps1 -RunTarget "$(RUN_TARGET)" $(if $(RELEASE_SELECTED),-Release,)
else
run:
	@case "$(RUN_TARGET)" in \
		"") set -a; [ ! -f .env ] || . ./.env; set +a; cargo run -p client $(CARGO_PROFILE_FLAG) ;; \
		launcher) set -a; [ ! -f .env ] || . ./.env; set +a; SRO_PATH="$${SRO_PATH:-.}" cargo run -p launcher $(CARGO_PROFILE_FLAG) ;; \
		intro|intro_v2) set -a; [ ! -f .env ] || . ./.env; set +a; SCENE=intro_v2 cargo run -p client $(CARGO_PROFILE_FLAG) ;; \
		world|animations|ui_testing|asset_loading|skills|dungeons) set -a; [ ! -f .env ] || . ./.env; set +a; SCENE=$(RUN_TARGET) cargo run -p client $(CARGO_PROFILE_FLAG) ;; \
		asset-loading) set -a; [ ! -f .env ] || . ./.env; set +a; SCENE=asset_loading cargo run -p client $(CARGO_PROFILE_FLAG) ;; \
		wsl) \
			echo "'run wsl' runs on the Windows side, not in WSL: the 'wsl' names the"; \
			echo "checkout the .exe was cross-built *from*, not the shell that launches it."; \
			echo "Windows GNU Make delegates it to scripts/run.ps1, which resolves this"; \
			echo "repo's Windows path and starts target/x86_64-pc-windows-gnu/.../client.exe."; \
			echo; \
			echo "  here, in WSL:   make build windows$(if $(RELEASE_SELECTED), release,)"; \
			echo "  then, in PowerShell, from the same checkout:"; \
			echo "                  make run wsl$(if $(RELEASE_SELECTED), release,)"; \
			exit 2 ;; \
		*) echo "Unknown run target: $(RUN_TARGET)"; exit 2 ;; \
	esac
endif

launcher intro intro_v2 world animations ui_testing asset-loading asset_loading skills dungeons wsl:
	@:

# Headless net-check client: drives the full login->join roundtrip with no
# window and dumps every packet (credentials from config.yaml `dev_fast_login`).
netcheck:
	@set -a; [ ! -f .env ] || . ./.env; set +a; NETCHECK=1 cargo run -p client

watch:
	@case "$(RUN_TARGET)" in \
		"") SCENE=$(SCENE) cargo watch -w client -w bevy_pk2 -w packets -w sro_macro -w sro_macro_derive -w tools -i assets -i target -x "run -p client" ;; \
		launcher) set -a; [ ! -f .env ] || . ./.env; set +a; SRO_PATH="$${SRO_PATH:-.}" cargo watch -w launcher -w bevy_pk2 -w packets -w sro_macro -w sro_macro_derive -w tools -i assets -i target -x "run -p launcher" ;; \
		intro|intro_v2) SCENE=intro_v2 cargo watch -w client -w bevy_pk2 -w packets -w sro_macro -w sro_macro_derive -w tools -i assets -i target -x "run -p client" ;; \
		world|animations|ui_testing|asset_loading|skills|dungeons) SCENE=$(RUN_TARGET) cargo watch -w client -w bevy_pk2 -w packets -w sro_macro -w sro_macro_derive -w tools -i assets -i target -x "run -p client" ;; \
		asset-loading) SCENE=asset_loading cargo watch -w client -w bevy_pk2 -w packets -w sro_macro -w sro_macro_derive -w tools -i assets -i target -x "run -p client" ;; \
		*) echo "Unknown watch target: $(RUN_TARGET)"; exit 2 ;; \
	esac

# Build and quality

# `make build release` reproduces exactly what the build-*.yml workflows compile, so a
# release-profile claim can be measured locally instead of inferred from a CI artifact.
ifeq ($(OS),Windows_NT)
build:
	@case "$(BUILD_TARGET)" in \
		""|windows) cargo build $(CARGO_PROFILE_FLAG) ;; \
		release) cargo build --release --package client ;; \
		*) echo "Unknown build target: $(BUILD_TARGET)"; exit 2 ;; \
	esac
else
build:
	@case "$(BUILD_TARGET)" in \
		"") cargo build $(CARGO_PROFILE_FLAG) ;; \
		release) cargo build --release --package client ;; \
		windows) \
			command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1 || { echo "Missing mingw-w64. Install: sudo apt install gcc-mingw-w64-x86-64"; exit 1; }; \
			rustup target add x86_64-pc-windows-gnu >/dev/null 2>&1 || true; \
			CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc cargo build --package client --target x86_64-pc-windows-gnu $(CARGO_PROFILE_FLAG); \
			echo "==> brp_perf.exe (the Windows client's BRP server binds loopback, so the"; \
			echo "    perf CLI has to run on the Windows side too — docs/perf-remote.md)"; \
			CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc cargo build --package tools --bin brp_perf --target x86_64-pc-windows-gnu $(CARGO_PROFILE_FLAG) ;; \
		*) echo "Unknown build target: $(BUILD_TARGET)"; exit 2 ;; \
	esac
endif

windows release:
	@:

test:
	cargo test

fmt:
	cargo fmt --all

clippy:
	cargo clippy --all-targets --all-features

warnings:
	python3 scripts/check_warnings.py

# The full local quality gate — run this before pushing. There is no CI service:
# this repo deliberately has no GitHub Actions, so these checks are the gate.

ci: check-target-dir fmt-check warnings opcodes re-tools reference-data deny test build

# Supply-chain gate: licences, advisories, wildcard versions, source registries
# (deny.toml). Skips with a notice when cargo-deny is absent, the same way the
# other optional tooling degrades — the gate is local and must stay runnable on
# a fresh checkout.
deny:
	@if command -v cargo-deny >/dev/null 2>&1; then \
		cargo deny check; \
	else \
		echo "cargo-deny not installed - skipping supply-chain gate."; \
		echo "  install with: cargo install cargo-deny"; \
	fi

fmt-check:
	cargo fmt --all --check

opcodes:
	python3 scripts/check_opcode_ledger.py

# Cargo cannot tell two worktrees apart in a shared target dir, so a gate run
# there can pass on another worktree's binary. Refuse instead (#374).
check-target-dir:
	python3 scripts/check_target_dir.py

# Tests for the stdlib-only tools under scripts/ and scripts/re/ (no cargo, no deps).
re-tools:
	python3 scripts/re/test_harvest_db_schema.py
	python3 scripts/re/test_refgrep.py
	python3 scripts/re/test_stringbind.py
	python3 scripts/re/test_check_reference_data.py
	python3 scripts/test_check_opcode_ledger.py
	python3 scripts/test_bump_version.py
	bash scripts/test_safe_analyze.sh

# Textdata column indices vs SR_Db2Media. Skips (exit 0) when the reference
# checkout is absent, so it never fails a build over an optional input.
reference-data:
	python3 scripts/re/check_reference_data.py

clean:
	cargo clean

# Tools

pk2:
	@case "$(PK2_TARGET)" in \
		list) cargo run -p tools --bin pk2_unpack -- --pk2 "$(PK2)" --list ;; \
		unpack) \
			if [ -z "$(OUT)" ]; then \
				echo "OUT must be set: make pk2 unpack PK2=/path/to/file.pk2 OUT=dir [PREFIX=prefix]"; \
				exit 2; \
			fi; \
			cargo run -p tools --bin pk2_unpack -- --pk2 "$(PK2)" --out "$(OUT)" $(if $(PREFIX),--prefix "$(PREFIX)",) ;; \
		"") echo "Usage: make pk2 list PK2=/path/to/file.pk2"; echo "       make pk2 unpack PK2=/path/to/file.pk2 OUT=dir [PREFIX=prefix]" ;; \
		*) echo "Unknown pk2 target: $(PK2_TARGET)"; exit 2 ;; \
	esac

pk2-list:
	cargo run -p tools --bin pk2_unpack -- --pk2 "$(PK2)" --list

pk2-unpack:
	@if [ -z "$(OUT)" ]; then \
		echo "OUT must be set: make pk2-unpack PK2=/path/to/file.pk2 OUT=dir [PREFIX=prefix]"; \
		exit 2; \
	fi
	cargo run -p tools --bin pk2_unpack -- --pk2 "$(PK2)" --out "$(OUT)" $(if $(PREFIX),--prefix "$(PREFIX)",)

list unpack:
	@:

# Convert a .bsr resource (+ meshes/materials/textures/skeleton/animations) to .glb/.fbx
bsr2glb:
	@set -a; [ ! -f .env ] || . ./.env; set +a; \
	if [ -z "$(BSR)" ] && [ -z "$(PREFIX)" ]; then \
		echo "Usage: make bsr2glb BSR='res\\char\\china\\chinaman_fighter.bsr' [PK2=path/Data.pk2] [OUT=file.glb|file.fbx] [FORMAT=glb|fbx|both] [RAW=1]"; \
		echo "       make bsr2glb PREFIX='res\\item' [OUT=dir] [FORMAT=...] (batch mode)"; \
		exit 2; \
	fi; \
	cargo run -p tools --bin bsr2glb -- $(if $(PK2),--pk2 "$(PK2)",) $(if $(BSR),--bsr "$(BSR)",) $(if $(PREFIX),--prefix "$(PREFIX)",) $(if $(OUT),--out "$(OUT)",) $(if $(FORMAT),--format "$(FORMAT)",) $(if $(RAW),--raw,)

# Convert an original cutscene camera script into an .intro camera path (#569).
# The script is the user's own extracted Media data; nothing is committed here.
cutscene:
	@case "$(CUTSCENE_TARGET)" in \
		convert) \
			if [ -z "$(SCRIPT)" ]; then \
				echo "SCRIPT must be set: make cutscene convert SCRIPT=/path/Media/script/intro/egypt.txt [OUT=] [NAME=] [MUSIC=]"; \
				exit 2; \
			fi; \
			cargo run -q -p tools --bin intro_convert -- --script "$(SCRIPT)" $(if $(OUT),--out "$(OUT)",) $(if $(NAME),--name "$(NAME)",) $(if $(MUSIC),--music "$(MUSIC)",) ;; \
		"") echo "Usage: make cutscene convert SCRIPT=/path/Media/script/intro/egypt.txt [OUT=file.intro] [NAME=egypt] [MUSIC=music://x.ogg]"; exit 2 ;; \
		*) echo "Unknown cutscene target: $(CUTSCENE_TARGET)"; exit 2 ;; \
	esac

convert:
	@:

# BRP perf tooling against a running client (docs/perf-remote.md)
perf:
	@case "$(PERF_TARGET)" in \
		snapshot) cargo run -q -p tools --bin brp_perf -- snapshot $(if $(PREFIX),--prefix "$(PREFIX)",) ;; \
		sample) cargo run -q -p tools --bin brp_perf -- sample $(if $(SECS),--secs "$(SECS)",) $(if $(INTERVAL),--interval-ms "$(INTERVAL)",) $(if $(OUT),--out "$(OUT)",) ;; \
		fps) cargo run -q -p tools --bin brp_perf -- fps $(if $(SECS),--settle-secs "$(SECS)",) ;; \
		get) cargo run -q -p tools --bin brp_perf -- get ;; \
		set) \
			if [ -z "$(FIELD)" ] || [ -z "$(VALUE)" ]; then \
				echo "FIELD and VALUE must be set: make perf set FIELD=render_effects VALUE=false"; \
				exit 2; \
			fi; \
			cargo run -q -p tools --bin brp_perf -- set "$(FIELD)" "$(VALUE)" ;; \
		attribute) cargo run -q -p tools --bin brp_perf -- attribute $(if $(SECS),--secs "$(SECS)",) ;; \
		"") echo "Usage: make perf snapshot|sample|fps|get|set|attribute (see docs/perf-remote.md)"; exit 2 ;; \
		*) echo "Unknown perf target: $(PERF_TARGET)"; exit 2 ;; \
	esac

snapshot sample fps get set attribute:
	@:

# Per-system CPU profiling (docs/perf-remote.md). The feature is a build flag
# rather than a commented-out line in client/Cargo.toml, so profiling never
# means editing tracked source — and RUST_LOG is cleared here because a filter
# that keeps our log lines silently drops every bevy_ecs span, which produces a
# trace that looks fine and contains nothing.
profile:
	@case "$(PROFILE_TARGET)" in \
		chrome) set -a; [ ! -f .env ] || . ./.env; set +a; unset RUST_LOG; \
			echo "NOTE: this builds and runs a LINUX client. On WSL that has no GPU"; \
			echo "      (no /dev/dri) and renders in software, so its frame times are"; \
			echo "      llvmpipe's. The CPU span ranking is still valid. For a"; \
			echo "      representative capture use 'make profile windows'."; \
			SCENE=$${SCENE:-world} cargo run -p client --features profile-chrome $(CARGO_PROFILE_FLAG); \
			echo; echo "Wrote trace-<nanos>.json. Rank it with:"; \
			echo "  cargo run -p tools --bin trace_summary -- trace-*.json --top 30" ;; \
		tracy) set -a; [ ! -f .env ] || . ./.env; set +a; unset RUST_LOG; \
			echo "Start the Tracy profiler GUI first; it connects on TCP 8086."; \
			SCENE=$${SCENE:-world} cargo run -p client --features profile-tracy $(CARGO_PROFILE_FLAG) ;; \
		windows|windows-tracy) \
			command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1 || { echo "Missing mingw-w64. Install: sudo apt install gcc-mingw-w64-x86-64"; exit 1; }; \
			case "$(PROFILE_TARGET)" in \
				windows-tracy) FEATURE=profile-tracy ;; \
				*) FEATURE=profile-chrome ;; \
			esac; \
			if [ "$$FEATURE" = profile-tracy ]; then \
				command -v x86_64-w64-mingw32-g++ >/dev/null 2>&1 || { \
					echo "Missing the mingw C++ compiler. Tracy's client is C++ (tracy-client-sys"; \
					echo "compiles TracyClient.cpp), so gcc alone is not enough -- without g++ this"; \
					echo "fails deep in a cc-rs build script instead of here."; \
					echo; \
					echo "  sudo apt install g++-mingw-w64-x86-64"; \
					exit 1; \
				}; \
			fi; \
			echo "Building with LTO off: instrumenting every system span on top of thin"; \
			echo "LTO crashed rustc (SIGSEGV) on this toolchain, and a profiling build has"; \
			echo "no reason to pay for cross-crate inlining anyway."; \
			CARGO_PROFILE_RELEASE_LTO=false \
			CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
			cargo build --package client --target x86_64-pc-windows-gnu --features $$FEATURE $(CARGO_PROFILE_FLAG) \
			&& { \
				echo; \
				echo "Built a PROFILING client.exe ($$FEATURE). It overwrites the normal one at"; \
				echo "the same path, so rebuild without the feature when you are done measuring."; \
				echo; \
				echo "Run it with its working directory on a LOCAL Windows disk, beside a"; \
				echo "config.yaml and an assets/ folder:"; \
				echo; \
				echo "  cp target/x86_64-pc-windows-gnu/$(if $(RELEASE_SELECTED),release,debug)/client.exe /mnt/c/coding/openroad/client-profile.exe"; \
				echo "  # PowerShell:  cd C:/coding/openroad ; ./client-profile.exe"; \
				echo; \
				if [ "$$FEATURE" = profile-tracy ]; then \
					echo "This build links C++ (TracyClient.cpp), so unlike the normal client it"; \
					echo "needs the mingw runtime beside it -- Windows refuses to start the exe"; \
					echo "otherwise, one missing-DLL box at a time. Copy the whole closure with:"; \
					echo; \
					echo "  scripts/mingw-runtime-dlls.sh target/x86_64-pc-windows-gnu/$(if $(RELEASE_SELECTED),release,debug)/client.exe /mnt/c/coding/openroad"; \
					echo; \
					echo "Start the Tracy profiler GUI on WINDOWS before the client. It listens on"; \
					echo "TCP 8086 and both ends are Windows-local, so none of the WSL NAT problem"; \
					echo "that forced brp_perf.exe applies here. The viewer release must match the"; \
					echo "bundled tracy-client-sys - see docs/perf-remote.md."; \
					echo; \
					echo "This streams live rather than writing a file, so there is no truncated-"; \
					echo "trace hazard: save from the viewer once you have enough."; \
				else \
					echo "The local disk matters for chrome traces: they are tens of MB per second,"; \
					echo "and writing that across the WSL 9p bridge distorts the frame times being"; \
					echo "measured."; \
					echo; \
					echo "Exit by closing the window. The writer buffers and only flushes on a"; \
					echo "clean shutdown, so a killed process leaves a truncated trace."; \
					echo "Keep the session short - 10 s standing still is plenty."; \
				fi; \
			} ;; \
		summary) cargo run -q -p tools --bin trace_summary -- $(if $(TRACE),$(TRACE),$$(ls -t trace-*.json 2>/dev/null | head -1)) $(if $(TOP),--top "$(TOP)",) ;; \
		"") echo "Usage: make profile chrome|windows|tracy|windows-tracy|summary (see docs/perf-remote.md)"; exit 2 ;; \
		*) echo "Unknown profile target: $(PROFILE_TARGET)"; exit 2 ;; \
	esac

chrome tracy windows-tracy summary:
	@:
