#!/usr/bin/env python3
"""Build docs/code.html from template.html, pasting in exact excerpts of the Rust source.

Usage: python3 build.py <repo> <template> <out>
In the template, {{name}} is replaced by the excerpt called `name` below, as an escaped
<figure> with a caption naming the file, function and line numbers.
"""
import html
import re
import sys

repo, template, out = sys.argv[1:4]


def lines(f):
    return open(f"{repo}/src/{f}").read().split("\n")


def find(ls, marker, after=0):
    for i in range(after, len(ls)):
        if marker in ls[i]:
            return i
    raise SystemExit(f"marker not found: {marker!r}")


def block(f, *m):
    """From the line containing the last marker to the closing brace at the same indentation.
    With two markers, the first is a line to search after."""
    ls = lines(f)
    a = find(ls, m[-1], find(ls, m[0]) if len(m) == 2 else 0)
    ind = len(ls[a]) - len(ls[a].lstrip())
    for b in range(a + 1, len(ls)):
        if ls[b].rstrip() == " " * ind + "}" or ls[b].rstrip() == " " * ind + "});":
            return a, b
    raise SystemExit(f"no end for {marker!r}")


def span(f, *m):
    """From the line containing `start` to the next line containing `end` (inclusive).
    With three markers, the first is a line to search after: (after, start, end)."""
    ls = lines(f)
    after = find(ls, m[0]) if len(m) == 3 else 0
    start, end = m[-2:]
    a = find(ls, start, after)
    return a, find(ls, end, a)


# name: (file, how, args, caption)
EX = {
    "dispatch": ("main.rs", block, ('match flag.as_str()',), "main.rs · main(): picking the job"),
    "startup": ("main.rs", span, ("// glibc gives every thread", "build_global();"), "main.rs · main(): allocator and thread pool"),
    "trim": ("main.rs", block, ("pub fn trim_heap()",), "main.rs · trim_heap"),
    "load_source": ("scene.rs", block, ("pub fn load_source(",), "scene.rs · load_source"),
    "grid": ("scene.rs", span, ("let sw = sw.max(8);", "let water_a ="), "scene.rs · Scene::new: grid size and analysis"),
    "pixel_size": ("scene.rs", block, ("pub fn pixel_size(",), "scene.rs · pixel_size"),
    "find_sky": ("scene.rs", block, ("fn find_sky(",), "scene.rs · find_sky"),
    "find_water": ("scene.rs", block, ("fn find_water_line(",), "scene.rs · find_water_line"),
    "kmeans": ("scene.rs", block, ("fn kmeans_palette(",), "scene.rs · kmeans_palette"),
    "lut": ("scene.rs", span, ("let mut lut = vec![0u8; 32 * 32 * 32];", "let dither_step ="), "scene.rs · Scene::new: palette lookup table and dither strength"),
    "rng": ("scene.rs", span, ("// a small xorshift generator", "let birds ="), "scene.rs · Scene::new: placing stars, motes and birds"),
    "hash2": ("scene.rs", block, ("pub fn hash2(",), "scene.rs · hash2"),
    "wnoise": ("scene.rs", block, ("fn wnoise(",), "scene.rs · wnoise"),
    "wfbm": ("scene.rs", block, ("fn wfbm(",), "scene.rs · wfbm"),
    "still": ("scene.rs", block, ("fn grid_is_still(",), "scene.rs · grid_is_still"),
    "water": ("scene.rs", block, ("if let Some(wf) = self.water_from {",), "scene.rs · build_grid: water"),
    "bayer": ("scene.rs", block, ("fn bayer(",), "scene.rs · bayer"),
    "stylize": ("scene.rs", block, ("fn stylize_px(",), "scene.rs · stylize_px"),
    "upscale": ("scene.rs", block, ("fn upscale(",), "scene.rs · upscale"),
    "code_lines": ("scene.rs", block, ("fn code_lines()",), "scene.rs · code_lines"),
    "player_new": ("player.rs", span, ("if !p.list.is_empty() {", "        p\n"), "player.rs · Player::new: the first scene"),
    "request": ("player.rs", block, ("fn request(&mut self",), "player.rs · request"),
    "recv": ("player.rs", block, ("pub fn update_damage(", "if let Some(rx) = &self.loading {"), "player.rs · update_damage: collecting the loader's result"),
    "trans_end": ("player.rs", block, ("if elapsed >= len {",), "player.rs · update_damage: the end of a transition"),
    "timing": ("saver.rs", span, ("let frame = Duration::from_secs_f32", "el.set_control_flow"), "saver.rs · about_to_wait: frame timing"),
    "frame_cb": ("layer.rs", block, ("fn frame(&mut self",), "layer.rs · the compositor's frame callback"),
    "wl_damage": ("layer.rs", span, ("// tell the compositor only what changed", "o.layer.commit();"), "layer.rs · draw: sending damage to the compositor"),
    "load_from": ("config.rs", block, ("pub fn load_from(",), "config.rs · Config::load_from"),
    "save": ("config.rs", block, ("pub fn save(&self)",), "config.rs · Config::save"),
}

COMMENT = re.compile(r"(//.*)$")


def render(name):
    f, how, args, cap = EX[name]
    if how is span and args[1].endswith("\n"):
        # end marker is a whole line
        ls = lines(f)
        a = find(ls, args[0])
        want = args[1].rstrip("\n")
        b = next(i for i in range(a + 1, len(ls)) if ls[i] == want)
    else:
        a, b = how(f, *args)
    ls = lines(f)[a:b + 1]
    ind = min(len(l) - len(l.lstrip()) for l in ls if l.strip())
    body = []
    for l in ls:
        l = l[ind:] if l.strip() else ""
        m = COMMENT.search(l)
        if m and '"' not in l[: m.start()]:
            body.append(html.escape(l[: m.start()]) + '<span class="c">' + html.escape(m.group(1)) + "</span>")
        else:
            body.append(html.escape(l))
    url = f"https://github.com/kingof9x/nynex-screensaver/blob/main/src/{f}#L{a + 1}-L{b + 1}"
    return (
        f'<figure class="src"><figcaption><a href="{url}">{html.escape(cap)}</a>'
        f'<span>lines {a + 1}–{b + 1}</span></figcaption><pre><code>' + "\n".join(body) + "</code></pre></figure>"
    )


t = open(template).read()
used = set()


def sub(m):
    used.add(m.group(1))
    return render(m.group(1))


t = re.sub(r"\{\{(\w+)\}\}", sub, t)
missing = set(EX) - used
if missing:
    print("unused excerpts:", missing, file=sys.stderr)
open(out, "w").write(t)
print("wrote", out, len(t), "bytes;", len(used), "excerpts")
