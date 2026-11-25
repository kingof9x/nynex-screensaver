# Review guide

This page is for someone reviewing nynex critically: a code reviewer, a security-minded sysadmin,
or a developer deciding whether to trust it on their machine. It says what the program touches,
where the risky parts are, what was already found and fixed, what is known to be imperfect, and how
to check the claims yourself.

## What to read, in what order

1. [ARCHITECTURE.md](ARCHITECTURE.md): the map (20 minutes).
2. `src/main.rs`: the entry points (5 minutes).
3. `src/player.rs`: the slideshow, threads and memory lifetime (15 minutes).
4. `src/scene.rs`: the renderer. Long, but the top of the file has a table of where each step
   lives. `Scene::new` and `Scene::render_damage` are the two functions everything else hangs off.
5. Platform code as needed: `saver.rs`, `layer.rs`, `wallpaper.rs`, `install.rs`, `omarchy.rs`.

## Build and check it yourself

```
cargo build                       # debug build, about a minute
cargo test                        # 9 tests, a few minutes in debug (the stress tests do real work)
cargo clippy --all-targets        # should print no warnings
cargo test --release -- --ignored --nocapture   # long stress tests and the render benchmark
nynex --render photo.jpg out.png --seconds 5     # one frame, no window needed
```

CI (`.github/workflows/build.yml`) builds Windows and Linux on every push, runs the tests on Linux
in release mode, and renders a frame headless as a smoke test. Every pull request gets downloadable
test builds.

## What the program touches

| Area | What | Where |
|---|---|---|
| Files read | Images the user picked; its own `config.json` | `config.rs`, `scene.rs` |
| Files written | Only inside its own folder (`%APPDATA%\nynex-screensaver` or `~/.config/nynex-screensaver`), plus on Linux `~/.config/omarchy/shell.json` (idle time) and `~/.config/hypr/autostart.lua` or `autostart.conf` (start at login), both only when the user changes those settings | `config.rs`, `omarchy.rs` |
| Windows registry | `HKCU\Control Panel\Desktop\SCRNSAVE.EXE`, the screensaver timeout and on/off via `SystemParametersInfoW`, and `HKCU\...\Run\nynex wallpaper`. Current user only; no admin rights | `install.rs`, `wallpaper.rs` |
| Other programs | Sends one undocumented but widely used message (`0x052C`) to Explorer's `Progman` window to create the wallpaper layer; launches itself | `wallpaper.rs`, `settings.rs` |
| Network | None. There is no networking code and no networking dependency | |
| Elevated rights | Never asks for them. The Omarchy installer (in a separate repo) uses `sudo` to put a launcher in `/usr/local/bin`; nynex itself does not | |

## Unsafe code

Rust's `unsafe` marks code the compiler cannot check. nynex has 20 such blocks, all of them calls
into the operating system, and each has a `SAFETY:` comment saying why it is sound.

| Where | Count | What for |
|---|---|---|
| `install.rs` | 6 | Registry reads and writes, screensaver settings, 8.3 short paths |
| `wallpaper.rs` | 6 | Finding Explorer's desktop windows, re-parenting our windows, registry for "start with Windows" |
| `saver.rs` | 5 | Cursor position, foreground window checks, the preview box's parent window |
| `main.rs` | 2 | glibc `mallopt` and `malloc_trim` (Linux) |
| `layer.rs` | 1 | `poll()` on the Wayland socket |

There is no `unsafe` in the renderer, the slideshow or the settings app. The Windows blocks pass
pointers only to locals and NUL-terminated strings that outlive the call, and pass buffer lengths
explicitly. The Windows-only code is compiled by CI but has no automated tests (see gaps below).

## Inputs a hostile or broken file could control

The images and the config are chosen by the user, so this is defence against accidents more than
attackers, but the same paths would matter if someone handed the user a crafted file.

- **Images** are decoded by the `image` crate (pure Rust decoders for JPEG, PNG, WebP, BMP, GIF,
  TIFF). A single decode may allocate at most 2 GB (`config.rs`, `MAX_DECODE_BYTES`); bigger claims
  fail cleanly instead of aborting the process. GIFs keep at most 300 frames and about 192 MB of
  decoded frames (`scene.rs`, `load_gif_frames`). A file that fails to decode is skipped
  (`player.rs`), and a test feeds it empty, truncated, mislabelled and garbage files.
- **config.json** is parsed by `serde_json`. A file that fails to parse is copied to
  `config.json.bad` and defaults are used. Every number is clamped to a sane range, including NaN
  and infinity (`Config::sanitize`). Paths in the config are only ever opened for reading as images.
- **Command line**: `--render` arguments are clamped (`--size` to 8..16384). The Windows `/p <hwnd>`
  handle is checked with `IsWindow` every frame.

## Findings from this review

The review that produced this guide read every file, ran new stress tests, and measured memory and
CPU. These were found and fixed in the same pull request:

| # | Problem | Effect | Fix |
|---|---|---|---|
| 1 | `--render ... --config` with no path after it | Crash (index out of bounds) | Checked |
| 2 | A `config.json` that failed to parse was replaced by defaults in memory | Pressing Save in the app could wipe the landscape list | Kept as `config.json.bad` first |
| 3 | No validation of config numbers | A hand-edited `"birds": 4000000000` or `NaN` could hang or crash | `sanitize()` clamps everything |
| 4 | Window narrower than 16 px | Panic in `clamp` | Fixed bounds |
| 5 | Long, large GIFs | Up to 830 MB while loading; long GIFs were cut at 300 frames | Byte budget; frames thinned evenly with their delays merged |
| 6 | Next image fails to load in the background | Slideshow stopped advancing forever | Skips to the next one; gives up after a full lap |
| 7 | First image broken | Black screen with an error even if others were fine | Starts on the first image that loads |
| 8 | Transition frame buffers | Two full screens (16 MB at 1080p) held permanently | Allocated per transition |
| 9 | Screensaver window resizing | Scene rebuilt (decode + analysis) on every resize event | Waits 200 ms for the size to settle |
| 10 | Wallpaper "black first frame" | Actually sent after the slow first decode | Sent first, then the scene is built |
| 11 | glibc heap growth on Linux | RSS around 100 to 130 MB and climbing | Two arenas, `malloc_trim` after each change |
| 12 | Unlimited image decode | A crafted header could make the process abort on allocation | 2 GB cap |
| 13 | Two extra full-size copies of each photo while loading | Higher peak memory | `into_rgb8`, crop as a view |
| 14 | Scene seed of exactly 0 | Every bird and mote in the same spot (1 in 4 billion) | Guarded |
| 15 | `--render --size 0x0` | Out-of-bounds write | Clamped |

## Known limitations, not fixed

These are real, deliberate or out of scope for now. None of them corrupts data.

- **The settings app does slow work on its UI thread.** Importing a large photo and rebuilding the
  preview after a slider stops moving both run synchronously; with a 50-megapixel photo the window
  can stall for a second.
- **Removing a landscape keeps its copy** in the `landscapes/` folder. Nothing cleans that folder.
- **Abandoned loads finish anyway.** When the user jumps to another landscape, the background load
  already running is not cancelled; its result is dropped when it arrives. Costs CPU, not memory
  over time.
- **Arrow keys can block for a moment.** Stepping forward before the next landscape has loaded
  waits for that load on the main thread.
- **A running screensaver or wallpaper doesn't see setting changes** until it restarts. (The
  settings app's Start button restarts the wallpaper.)
- **Starting the wallpaper from the app pauses the window for about a second** while any old
  wallpaper is told to stop (`wallpaper::start`).
- **Stopping works by a file in the app folder** (`wallpaper.stop`), which the first running
  wallpaper to see it deletes. If two wallpapers were somehow running at once, one Stop stops only
  one of them.
- **Linux renders at the monitor's logical size.** On a 2x HiDPI screen the compositor scales the
  picture up. For pixel art this is invisible; for the ASCII look the characters are a little
  softer.
- **One heap per GIF frame set per monitor.** Several monitors showing different long GIFs each hold
  up to about 150 MB of frames.
- **The Windows-only code has no automated tests.** Registry writes, the wallpaper's window
  re-parenting and the preview box were tested by hand on Windows 11 (including 24H2's new desktop
  layout). CI compiles them but cannot run them.
- **The settings UI has no automated tests.**
- **`cargo fmt` is not enforced**; lines are long in places.

## Claims and how to check them

| Claim | How to check |
|---|---|
| "No network access" | `grep -rn "TcpStream\|reqwest\|hyper\|ureq\|UdpSocket" src/ Cargo.toml` finds nothing; the dependency list has no HTTP client |
| Memory and CPU figures | [BENCHMARKS.md](BENCHMARKS.md) lists the method; `nynex --wallpaper` under `top`, or `cat /proc/$(pgrep -n nynex)/smaps_rollup` |
| The caching shortcuts don't change the picture | `still_frames_match_full_render`, `partial_frames_match_full_render` |
| Broken files are survivable | `broken_images_are_skipped`, `nothing_loads`, `stress_fuzz` |
| No memory growth over time | `stress_long_run` (set `NYNEX_STRESS_MINUTES`) |

## Dependencies

| Crate | Why | Notes |
|---|---|---|
| `image` | Decoding photos | Pure Rust decoders; only the formats listed are enabled |
| `rayon` | Splitting each frame across cores | Global pool capped at 4 threads |
| `winit`, `softbuffer` | Windows and CPU pixel buffers | No GPU use for the animation |
| `eframe` (egui) | The settings app | OpenGL (glow) backend; only the settings window uses the GPU |
| `rfd` | File pickers | Native dialogs |
| `font8x8` | The ASCII look and on-screen messages | A public-domain 8x8 bitmap font |
| `serde`, `serde_json` | The config file | |
| `dirs` | Finding the config folder | |
| `windows-sys` | Windows API | Windows only |
| `smithay-client-toolkit`, `wayland-client` | The Linux wallpaper | Linux only, pure Rust Wayland |
| `libc` | `poll`, `mallopt`, `malloc_trim` | Linux only |

`Cargo.lock` is committed, and CI builds with `--locked`.

## About the screenshots

The screenshots in `docs/screenshots/` were rendered by this version of nynex from photos supplied
by the project owner (the Yosemite set) and from synthetic test pictures (`sunset_lake`,
`day_lake`). The settings app and wallpaper shots were taken on a headless Linux desktop (Xvfb and
Sway), so window decorations look plainer than on a real desktop.
