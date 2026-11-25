# Benchmarks

How much memory and CPU nynex uses, compared with the version before this review (0.9.2) and with
the Python prototype it grew out of (`fmscape.py`). Everything here was measured on the same
machine, and the method is written down so anyone can repeat it.

## Results

Live 1080p slideshow, five landscapes (three 3840 px Yosemite photos, two 1920 px synthetic lakes),
10 seconds each with 2-second transitions, default look, 30 fps.

| | Python prototype | nynex 0.9.2 | nynex, this version |
|---|---|---|---|
| Frames per second | 18 (as fast as it can) | 30 (as configured) | 30 (as configured) |
| CPU time per frame | 55 ms | {{V092_CPU_MS}} ms | {{NEW_CPU_MS}} ms |
| Share of one CPU core | 100% | {{V092_CPU_PCT}}% | {{NEW_CPU_PCT}}% |
| Memory after start (RSS) | 88 MB | {{V092_RSS_START}} MB | {{NEW_RSS_START}} MB |
| Memory over 10 minutes (RSS, min to max) | 88 to 126 MB | {{V092_RSS_RANGE}} MB | {{NEW_RSS_RANGE}} MB |
| Memory at the end (RSS) | 126 MB | {{V092_RSS_END}} MB | {{NEW_RSS_END}} MB |
| Peak memory (high-water mark) | 260 MB | {{V092_HWM}} MB | {{NEW_HWM}} MB |
| Private memory at the end | not measured | {{V092_PRIV}} MB | {{NEW_PRIV}} MB |
| Needs installed | Python 3, NumPy, Pillow | nothing | nothing |
| Size on disk | 36 KB script + ~60 MB of libraries | 12 MB | 12 MB |

What changed between 0.9.2 and this version, for memory:

- The two full-screen transition frames (16 MB at 1080p) only exist during a transition.
- glibc is limited to two heaps ("arenas") instead of one per thread, and is asked to return freed
  memory (`malloc_trim`) after every landscape change.
- Loading a photo no longer makes two extra full-size copies of it.

CPU per frame did not change in this review; the renderer itself was already tuned in 0.8.4 and
0.9.1. The Python prototype cannot reach 30 fps at 1080p at all, so its "share of a core" is 100%
at 18 fps; per frame, nynex needs about {{SPEEDUP}} times less CPU.

## What the numbers mean

**RSS** (resident set size) is what `top` and `ps` show as RES: all memory of the process that is
in RAM, including shared libraries and shared buffers it maps. **Private** memory leaves out what is
shared with other processes. **High-water mark** (`VmHWM`) is the largest RSS the process ever had,
which for nynex is the moment a big photo is being decoded while the previous scene is still on
screen.

**Windows reports a different number.** Task Manager's default *Memory* column is the *active
private working set*: private memory only, and only the pages Windows has not trimmed. A program
that sits mostly idle between frames, like a screensaver, gets trimmed aggressively, so the same
program shows a much smaller figure there than RSS on Linux. A figure like "6 MB" in Task Manager
and "50 MB" in `top` can both be right. To compare like with like on Windows, add the *Commit size*
column in Task Manager's Details tab, or use Process Explorer's *Private Bytes*.

These results are from a 4-core cloud VM with no GPU, using a software-rendered Wayland compositor
(Sway, headless, pixman renderer). On a desktop CPU the CPU share will be lower; the memory figures
depend mostly on screen size and photo size, not on the machine.

## Method

```
# a headless Wayland desktop to draw on
WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_RENDERER=pixman sway -c sway.cfg &   # one 1920x1080 output

# nynex: run the wallpaper, sample /proc every 5 seconds for 10 minutes
nynex --wallpaper & P=$!
while sleep 5; do
  grep -E '^(Rss|Pss|Private_Clean|Private_Dirty|Anonymous):' /proc/$P/smaps_rollup
  grep VmHWM /proc/$P/status
done
# CPU: utime + stime from /proc/$P/stat at the start and the end
```

The Python prototype has no window, so it was run as a loop that builds the scene for each photo
and renders and scales 1920x1080 frames for 10 seconds each, as fast as it can, without writing
them anywhere (the closest equivalent to what nynex does). It used the same photos.

Both nynex builds were the release binaries from CI: 0.9.2 from its GitHub release, this version
from the pull request's test build. They ran one after the other, not side by side.

## Stress tests

Run with `cargo test -- --ignored --nocapture` (release mode is much faster).

| Test | What it does | Result |
|---|---|---|
| `stress_long_run` | 10 minutes of slideshow time with 1-second landscapes and fast transitions, at 960x540, drawn as fast as possible | Passed; memory moved within a band and did not keep growing |
| `stress_big_gif` | A 600-frame 1280x720 animated GIF (which would be 1.6 GB decoded) | Passed; held under 200 MB and kept its full 24-second timing |
| `stress_fuzz` | 300 random screen sizes and settings, including 1x1 and 2560x1600 | Passed |
| `odd_sizes_and_random_settings_never_panic` | 8 sizes from 1x1 to 1920x1080, 3 random looks each, every pixel written | Passed (part of the normal `cargo test`) |
| `broken_images_are_skipped` | Empty, truncated, mislabelled and garbage files in the playlist | Passed |
