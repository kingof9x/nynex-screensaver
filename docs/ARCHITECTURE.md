# How nynex works

This is the map of the code for someone who wants to read or review it. The README covers how to
use the app; this page covers how it is built. Every source file also starts with a comment that
explains its part, and the tricky functions have comments of their own.

## The short version

nynex is one Rust program (`nynex.exe` on Windows, `nynex` on Linux) of about 5,000 lines. It does
four jobs, picked by its command line:

| Started as | Job | Code |
|---|---|---|
| `nynex` (no arguments), `/c` | Settings app with a live preview | `settings.rs` |
| `/s`, `--screensaver`, `--window`, `/p <hwnd>` | The animation in one or more windows | `saver.rs` |
| `--wallpaper` | The animation behind the desktop | `wallpaper.rs` + `saver.rs` (Windows), `layer.rs` (Linux) |
| `--render in.jpg out.png` | One frame to a file (for tests and screenshots) | `main.rs` |

Whatever the job, the picture comes from the same two files:

- `scene.rs` turns one photo into an animated scene and draws its frames.
- `player.rs` runs the slideshow: which landscape is on, loading the next, transitions.

Everything is drawn by the CPU into a plain array of pixels. There is no GPU code, no shaders and
no game engine. The settings app is the one exception: its user interface (egui) draws with OpenGL,
but its preview picture is still made by the CPU renderer.

```
                 config.json
                     │
   ┌─────────────────┼──────────────────┐
   │                 ▼                  │
   │   settings.rs ──► Player ◄── saver.rs / layer.rs (one Player per screen)
   │                    │
   │                    ├── Scene (on screen)   ── render_damage(t) ──► pixels ──► window
   │                    └── Scene (next, built on a loader thread)
   │
   │   install.rs, wallpaper.rs, omarchy.rs: hooking into Windows / Omarchy
   └────────────────────────────────────────────────────────────
```

## Files

| File | Lines | What it does |
|---|---|---|
| `main.rs` | ~180 | Reads the command line, sets up the crash log, the thread pool and (Linux) the memory allocator, and starts the right job. Also the `--render` tool. |
| `config.rs` | ~400 | The settings (`Config`, `Look`, `Logo`), saving and loading `config.json`, clamping bad values, copying added images into the library. |
| `scene.rs` | ~2,000 | The renderer. Analysis of the photo (sky, water, palette, sun), and every frame's effects. |
| `player.rs` | ~400 | The slideshow and the seven transitions. |
| `saver.rs` | ~500 | Windows (winit) and pixel buffers (softbuffer), frame timing, "any input quits", the Windows preview box. |
| `layer.rs` | ~400 | Linux wallpaper: a Wayland layer-shell surface per monitor. |
| `wallpaper.rs` | ~200 | Windows wallpaper: putting our windows behind the desktop icons; "start with Windows". |
| `install.rs` | ~100 | Windows: register `nynex.scr` as the screensaver in the registry. |
| `omarchy.rs` | ~130 | Linux: the Omarchy screensaver hook, idle time, start at login. |
| `settings.rs` | ~650 | The settings app (egui). |
| `stress.rs` | ~290 | Stress tests (only built for `cargo test`). |

## One frame, step by step

A `Scene` is built once per landscape and screen size (`Scene::new`, tens of milliseconds), then
drawn 30 times a second (`Scene::render_damage`, a few milliseconds).

### Building a scene (once)

1. **Decode** the file (`load_source`). Big photos are shrunk to 2,560 px; GIFs keep their frames
   within a memory budget.
2. **Crop** to the screen's shape, like "fill" for a desktop background.
3. **Pick the grid size** from the pixelation slider. The grid is the picture in big pixels; at the
   default setting a 1080p screen gets a grid about 270 cells wide, so each cell is a 7x7 block.
   All the effects are drawn on the grid, which is why they are cheap.
4. **Analyse** a 320-wide copy:
   - *Sky*: a flood fill from the top edge through smooth, similar colours (`find_sky`).
   - *Water*: is the bottom half a mirror image of the top? (`find_water_line`).
   - *Night*: is the sky dark?
   - *Sun*: is there a clearly brighter spot in the sky? It gets a breathing glow.
   - *Palette*: k-means clustering picks the 16 (by default) colours that best represent the picture.
5. **Grade** the grid to the warm "Claude FM" colours and build a 32x32x32 lookup table from any
   colour to its nearest palette colour.
6. **Place** stars, birds, motes, water glints and the logo, using a seed so that the same photo
   always looks the same on the same screen.

### Drawing a frame (30 times a second)

1. Start from the graded grid (or the current GIF frame).
2. Add the moving parts on the grid: water rows slide on sine waves, clouds drift (fractal noise),
   the sun glow breathes, glints and stars twinkle, the logo breathes, birds flap across, motes
   float up, wind streaks race past.
3. **Stylize**: snap each cell toward the nearest palette colour, with an ordered (Bayer) dither so
   the steps between colours become a fine cross-hatch.
4. Sparkles are drawn on top, after the palette, so they stay crisp.
5. **To the screen**: either scale the grid up with hard edges (`upscale`), or draw text cells
   (`ascii`) whose characters follow the brightness underneath.
6. **Breakthroughs**: now and then a band tears open showing nynex's own source code, a hex dump
   or a fake crash log (the program's source is built into the program for this).

Nothing in a frame uses a random number generator. Every "random" choice is a hash of position,
time and seed (`hash2`), so any row can be drawn on any thread and the result is the same.

## Making it cheap

A screensaver that heats the laptop is a bad screensaver. The main tricks, roughly in order of
how much they save:

- **Work on the grid, not the screen.** 270x152 cells is 41,000 cells; a 1080p screen is 2 million
  pixels.
- **Still frames are cached.** If nothing in a scene moves (no clouds, water, birds...), the grid
  is drawn once and reused, and only the sun glow's rectangle is redrawn.
- **Damage rectangles.** `render_damage` returns the screen rectangles that changed, and the window
  code sends only those to the compositor (`present_with_damage` / `wl_surface.damage_buffer`).
- **Lookup tables** for the palette (32 KB per scene) and the screen-column-to-grid-column map.
- **Row copying.** A screen row that shows the same grid row as the row above is a `memcpy`.
- **Few threads.** The rayon pool is capped at 4 threads. More threads finished each frame a
  little sooner but used more CPU overall, because idle workers spin before they sleep.
- **No frame catch-up.** If a frame is late the schedule restarts from now instead of rushing.
- **Hidden means stopped.** The Linux wallpaper only draws when the compositor asks for a frame;
  the Windows wallpaper stops drawing on a screen covered by a maximised window.

## Threads

| Thread | Lifetime | Work |
|---|---|---|
| Main | whole program | Event loop, frame timing, presenting frames |
| rayon pool (up to 4) | whole program | Rows of each frame, in parallel |
| Loader | one per landscape change, a fraction of a second | Decode the next image and build its `Scene` |

The loader hands the finished scene back over a channel (`std::sync::mpsc`). It owns copies of
everything it needs, so there is no shared mutable state and no locks. If the player has moved on
by the time it finishes, its result is dropped.

## Memory

Per screen, roughly:

| What | Size at 1080p |
|---|---|
| The pixel buffer the window shows | 8 MB (owned by softbuffer / the compositor) |
| Current scene (grid, palette table, masks, caches) | 2 to 10 MB; a still ASCII scene also caches a full screen (8 MB) |
| Next scene, waiting | the same again |
| Transition frames | 16 MB, **only during a transition** |
| Decoded photo | up to 20 MB, **only while the loader builds a scene** |

On Linux, glibc's allocator keeps freed memory around (and gives every thread its own heap), so a
program that decodes a 4K photo every minute looks bigger than it is. `main.rs` limits the number
of heaps and `player.rs` asks glibc to hand freed memory back after each landscape change. See
[BENCHMARKS.md](BENCHMARKS.md) for measurements.

## Settings and files

Everything lives in one folder: `%APPDATA%\nynex-screensaver` on Windows, `~/.config/nynex-screensaver`
on Linux.

| File | Written by | Notes |
|---|---|---|
| `config.json` | settings app | Written to a temporary file and renamed, so it is never half-written. A file that does not parse is kept as `config.json.bad`. Every value is clamped on load. |
| `landscapes/` | settings app | Copies of added images (stills over 3,840 px are stored downscaled). |
| `nynex.scr` | Windows, "Use as my screensaver" | A copy of the program for Windows to run. |
| `nynex.exe` | Windows wallpaper / start with Windows | Likewise. |
| `crash.log` | any | Panics, with time and message. |
| `wallpaper.log` | Linux wallpaper | What it attached to, and errors. |
| `wallpaper.stop` | settings app | A running wallpaper (Windows or Linux) exits when this appears, and deletes it. |
| `omarchy-screensaver-off` | settings app | Tells the Omarchy hook to run Omarchy's own screensaver. |

## Platform glue

**Windows screensaver.** Windows runs the program in `HKCU\Control Panel\Desktop\SCRNSAVE.EXE`
with `/s` (show), `/p <hwnd>` (draw inside the little preview box) or `/c` (settings). nynex copies
itself to `nynex.scr`, writes that registry value and sets the timeout with
`SystemParametersInfoW`, so the user never needs Windows' own Screen Saver Settings dialog.

**Windows wallpaper.** The classic "live wallpaper" trick: ask Explorer (message `0x052C` to
`Progman`) to create the layer that holds the wallpaper, then make our windows children of it, below
the desktop icons. Windows 11 24H2 changed this layout; `wallpaper.rs` handles both.

**Linux wallpaper.** The `wlr-layer-shell` Wayland protocol lets a program put a surface on a layer
of the desktop. nynex puts one on the "bottom" layer of every monitor (under windows, over the
desktop's own background), takes no input, and draws through shared memory.

**Omarchy screensaver.** Omarchy's idle service runs `omarchy-launch-screensaver`. The installer in
the [omarchy-pixelbook-go](https://github.com/kingof9x/omarchy-pixelbook-go/tree/main/nynex) repo puts
a replacement earlier on the `PATH` that starts `nynex --screensaver --app-id org.omarchy.screensaver`
on every monitor. Using Omarchy's own app id means Omarchy's window rules and lock timer keep
working unchanged.

## Tests

```
cargo test                                         # unit tests and quick stress tests (debug, a few minutes)
cargo test --release -- --ignored --nocapture      # the long stress tests and the benchmark
```

| Test | Checks |
|---|---|
| `still_frames_match_full_render`, `partial_frames_match_full_render` | The caching and damage shortcuts draw exactly the same pixels as a full redraw. |
| `odd_sizes_and_random_settings_never_panic` | Screens from 1x1 to 1920x1080, including 15x600 and 600x15, with random settings; every pixel is written. |
| `every_transition_at_odd_sizes` | All seven transitions at odd sizes. |
| `broken_images_are_skipped` | Empty, truncated and fake image files in the playlist are skipped, the slideshow keeps going. |
| `nothing_loads` | With only broken images, a message is shown instead of a crash or a blank screen. |
| `stepping_back_and_forth` | Arrow keys during transitions and loads. |
| `bad_config_is_kept_and_values_are_clamped` | A corrupt config is kept aside; absurd values are clamped. |
| `stress_big_gif` (ignored) | A 600-frame 1280x720 GIF keeps its timing and stays under 200 MB. |
| `stress_long_run` (ignored) | A long slideshow with fast transitions does not keep growing in memory. |
| `stress_fuzz` (ignored) | 300 random scenes, sizes and settings. |
| `bench_render` (ignored) | Frame time and CPU time for several looks. |
