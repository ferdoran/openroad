# Running openroad — from a downloaded build

This is the path for someone who wants to *run* the client: download a build,
point it at their own Silkroad files, start it. It assumes no git checkout, no
Rust toolchain and no `make`. If you want to build from source instead, see
**Getting Started** in [the README](https://github.com/ferdoran/openroad/blob/main/README.md).

openroad ships **no game content**. You supply your own Silkroad Online v1.188
data files and the key that opens them; everything below is about connecting the
binary to files you already have.

## 1. Download the right build

Grab the zip for your platform from the repository's Releases page:

```text
openroad-<version>-x86_64-unknown-linux-gnu.zip   Linux, 64-bit Intel/AMD
openroad-<version>-x86_64-pc-windows-gnu.zip      Windows, 64-bit
openroad-<version>-aarch64-apple-darwin.zip       macOS, Apple Silicon
```

Each contains the client binary, an `assets/` folder of openroad's own art and
shaders, and `config.example.yaml`. Unzip it somewhere you can write to.

No build for your platform — an Intel Mac, an ARM Linux box — means building
from source; the README's Getting Started covers it.

## 2. Let your system run it

The binaries are unsigned, so both desktop platforms will stop them the first
time.

```bash
xattr -dr com.apple.quarantine openroad-<version>-aarch64-apple-darwin  # macOS
```

Without that, macOS reports the binary as damaged or blocked rather than as
unsigned. On **Windows**, SmartScreen shows a blue "Windows protected your PC"
dialog — *More info* → *Run anyway*. On **Linux**, mark it executable if the zip
lost the bit: `chmod +x client`.

## 3. Linux: the libraries it needs

The binary links ALSA and libudev directly, and opens X11, Wayland, xkbcommon
and Vulkan on demand. A normal desktop install already has all but the last:

```bash
# Debian/Ubuntu — the runtime libraries, not the -dev packages a build needs
sudo apt install libasound2 libudev1 libx11-6 libx11-xcb1 libxcursor1 \
                 libwayland-client0 libxkbcommon0 libxkbcommon-x11-0
sudo apt install libvulkan1 mesa-vulkan-drivers   # the one usually missing
```

openroad renders through Vulkan. Without a working driver it either fails to
start or falls back to CPU rasterization at a few frames per second.

## 4. Your own game files

Copy these five archives from your Silkroad installation into the `assets/`
folder beside the binary:

```text
Media.pk2   Map.pk2   Data.pk2   Music.pk2   Particles.pk2
```

To keep them where they are instead, set `SRO_PK2_PATH` to the folder holding
them. The lookup order is `SRO_PK2_PATH`, then `SRO_PATH`, then `assets/` next
to the binary (`client/src/plugins/assets/sro.rs`).

That has to be a real environment variable: **the client does not read a `.env`
file** — only the repository's `make run` does that.

## 5. The archive key

Those archives are encrypted, and openroad does not ship the key that opens
them: it belongs with the game data it decrypts, which is yours rather than
ours. Copy the example config next to the binary and supply the key there.

```bash
cp config.example.yaml config.yaml
```

Then uncomment the `pk2:` block and fill in both entries — `key` is plain text,
`salt` is hex, and a key without its salt derives a different cipher:

```yaml
pk2:
  key: "..."
  salt: "..."
```

`SRO_PK2_KEY` and `SRO_PK2_SALT` do the same job and take precedence. The values
for a stock v1.188 client are widely documented; this project does not
distribute them. Without a key the client exits before a window appears.

`config.yaml` must sit in the directory you start the client *from* — that is
where it is looked up.

## 6. Run it

```bash
cd openroad-<version>-<target>   # start from the unzipped folder
./client                         # client.exe on Windows
```

Starting it from somewhere else fails even when the PK2 path is right:
openroad's own art loads from `assets/` relative to the working directory, which
is a separate root from your PK2 folder.

You should get the splash, then the intro cutscene, then the login screen. The
cutscene is converted from your own `Media.pk2` at startup, so there is nothing
to prepare — see [`cutscene-convert.md`](https://github.com/ferdoran/openroad/blob/main/docs/cutscene-convert.md) if you want to pin
a specific one.

## 7. Logging in needs a server

openroad is a **client**. No server ships with it, and the project does not
operate one — it connects to a server you already have access to.

```yaml
network_settings:
  gateway_address: "127.0.0.1:15779"   # or comment out to use your Media.pk2's own
```

Commenting the line out falls back to the gateway your own `Media.pk2` names
(`divisioninfo.txt` + `gateport.txt`).

Without a server you can still look around offline. In `config.yaml`:

```yaml
scenes:
  startup: world      # terrain and a free-flying camera, no HUD
```

`dungeons` and `skills` are the other two offline scenes.

## Troubleshooting

| Symptom | Cause |
|---|---|
| Exits immediately, message about a PK2 key | no `pk2:` block in `config.yaml` and no `SRO_PK2_KEY`/`SRO_PK2_SALT` |
| `failed to open archive …` | one of the five PK2 files is missing from the folder it resolved to |
| Loading screen forever, log says `Path not found: …/assets/fonts/…` | started from a directory that has no `assets/` — `cd` into the unzipped folder |
| `internal error: entered unreachable code` | a `config.yaml` from an older version; start again from `config.example.yaml` |
| Starts, but a few frames per second | no working Vulkan driver, so rendering fell back to the CPU |
| Black screen after the splash, log says no intro cutscene | `scenes.intro_location` names a script your `Media.pk2` does not have; leave it empty |
| Login screen rejects everything / never connects | `gateway_address` points at no running server |

The client writes what it resolved to the log on startup — the PK2 directory and
the cutscene it picked are both printed there, which is usually enough to tell a
path problem from a data problem.
