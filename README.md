# nynex screensaver

A Windows screensaver that turns your landscape photos into slowly animated pixel-art and ASCII
scenes, in the spirit of the Claude FM stream visuals. Clouds drift, lakes ripple and glint, the sun
breathes, stars twinkle at night, birds glide past and motes float up. It cycles through your
landscapes with the transition you pick, and can show a centred logo.

## Install

1. Download `nynex.exe` from the latest [release](../../releases) and open it. This is the nynex
   app. Windows may warn that the file isn't signed; click **More info**, then **Run anyway**.
2. Add landscapes and tune the look. The preview updates as you go.
3. Click **Play in a window** or **Play full screen** to watch it any time.
4. Pick a wait time next to **Start after** and click **Use as my screensaver**. nynex now starts
   whenever the PC is idle that long. **Turn off** stops it. You don't need Windows' own Screen
   Saver Settings.

**Use as my screensaver** keeps a copy of the program at
`%APPDATA%\nynex-screensaver\nynex.scr` for Windows to run. If you change the settings later, they
apply right away; after downloading a new version, click the button again to update that copy.

If something crashes, the details are written to `%APPDATA%\nynex-screensaver\crash.log`.

## Settings

| | |
|---|---|
| **Landscapes** | Add images or a whole folder (or drop files on the window). Animated GIFs play every frame with their own timing (up to 300 frames). Untick an image to skip it, reorder it, or give it its own display time. Added images are copied to `%APPDATA%\nynex-screensaver\landscapes`. |
| **Time per landscape** | The default display time, from 2 seconds to 30 minutes. |
| **Shuffle / different per monitor** | Plays in random order, and shows a different landscape on each screen. |
| **Transition** | Crossfade, pixel dissolve, wipe, slide, pixelate, fade through black, or random, with its length. |
| **Pixelation** | From full resolution to big chunky pixels. |
| **ASCII** | Fades the picture into coloured text characters. |
| **Full colour ↔ Stylized** | From the photo's own colours to the Claude FM look: a high-key, mostly cream-white picture in a small dithered palette, with soft clay, pale blue and pale sage tints. Palette size and dither strength are adjustable. |
| **Motion** | Cloud amount, birds, dust/fireflies, sparkles, wind (faster clouds and birds, sideways dust, streaks across the sky), ASCII glitches (short runs of cells flickering into characters), code breaking through (the picture tears open and nynex's own source code scrolls through the crack), lake ripples, night stars and frame rate. |
| **Logo** | Any image shown in the centre: a transparent PNG, or dark artwork on white. Adjust its size, opacity and breathing, and whether it has a darker rim. |

**Next landscape** under the preview plays the transition.

Settings are stored in `%APPDATA%\nynex-screensaver\config.json`.

## Command line

`nynex.exe` (and the `nynex.scr` copy) accepts the standard screensaver switches: `/s` (run), `/p <hwnd>` (preview box) and
`/c` (settings). It also has two switches for trying it out:

```
nynex.exe --window                                   # run in a normal window
nynex.exe --render photo.jpg frame.png --seconds 5   # write one frame to a PNG
```

## How it works

Each photo is cropped to the screen's shape and shrunk to the pixel grid set by the pixelation
slider. It is then analysed once:

- **Sky:** a flood fill of smooth, gradually changing colour from the top edge, with clouds and the
  sun filled back in.
- **Water:** a check for whether the lower part of the picture mirrors the upper part, which marks a
  lake reflection.
- **Night:** whether the sky is dark.
- **Palette:** a small k-means palette taken from the warm-graded picture.

Every frame adds the moving parts on the grid. It then snaps colours toward the dithered palette by
the stylize amount, scales up with hard edges, and optionally draws ASCII cells on top. Rendering
runs on the CPU and is spread over all cores with rayon. Each monitor gets a borderless window
drawn with softbuffer. The settings app uses egui.

## Build

```
cargo build --release          # target/release/nynex.exe
```

Cross-compiling from Linux works with `rustup target add x86_64-pc-windows-gnu` and MinGW
(`cargo build --release --target x86_64-pc-windows-gnu`).

GitHub Actions builds every push and pull request. To publish a release with `nynex.exe` and a zip,
open **Actions → Build and release → Run workflow**, type a version such as `v0.1.0` and click
**Run workflow**. Pushing a tag does the same:

```
git tag v0.1.0 && git push origin v0.1.0
```
