# nynex screensaver

A Windows screensaver that turns your landscape photos into slowly animated pixel-art and ASCII
scenes, in the spirit of the Claude FM stream visuals. Clouds drift, lakes ripple and glint, the sun
breathes, stars twinkle at night, birds glide past and motes float up. It cycles through your
landscapes with the transition you pick, and can show a centred logo.

## Install

1. Download `nynex.scr` from the latest [release](../../releases).
2. Right-click it and choose **Install**. Windows opens Screen Saver Settings with nynex selected.
   You can also put it in `C:\Windows\System32` so it always shows up in the list.
3. Click **Settings** to add landscapes and tune the look. Double-clicking `nynex.scr` opens the
   settings too.

The settings app also has **Set as my screensaver**, which does step 2 for you.

## Settings

| | |
|---|---|
| **Landscapes** | Add images or a whole folder (or drop files on the window). Untick an image to skip it, reorder it, or give it its own display time. Added images are copied to `%APPDATA%\nynex-screensaver\landscapes`. |
| **Time per landscape** | The default display time, from 2 seconds to 30 minutes. |
| **Shuffle / different per monitor** | Plays in random order, and shows a different landscape on each screen. |
| **Transition** | Crossfade, pixel dissolve, wipe, slide, pixelate, fade through black, or random, with its length. |
| **Pixelation** | From full resolution to big chunky pixels. |
| **ASCII** | Fades the picture into coloured text characters. |
| **Full colour ↔ Stylized** | From the photo's own colours to a warm, limited, dithered palette (the Claude FM look). Palette size and dither strength are adjustable. |
| **Motion** | Cloud amount, birds, dust/fireflies, lake ripples, night stars and frame rate. |
| **Logo** | Any image shown in the centre: a transparent PNG, or dark artwork on white. Adjust its size, opacity and breathing, and whether it has a darker rim. |

The preview in the settings window updates live. **Next landscape** plays the transition, and
**Test full screen** saves the settings and runs the real screensaver.

Settings are stored in `%APPDATA%\nynex-screensaver\config.json`.

## Command line

`nynex.scr` accepts the standard screensaver switches: `/s` (run), `/p <hwnd>` (preview box) and
`/c` (settings). It also has two switches for trying it out:

```
nynex.scr --window                                   # run in a normal window
nynex.scr --render photo.jpg frame.png --seconds 5   # write one frame to a PNG
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
cargo build --release          # target/release/nynex.exe; rename to nynex.scr
```

Cross-compiling from Linux works with `rustup target add x86_64-pc-windows-gnu` and MinGW
(`cargo build --release --target x86_64-pc-windows-gnu`).

GitHub Actions builds every push and pull request. To publish a release with `nynex.scr` and a zip,
open **Actions → Build and release → Run workflow**, type a version such as `v0.1.0` and click
**Run workflow**. Pushing a tag does the same:

```
git tag v0.1.0 && git push origin v0.1.0
```
