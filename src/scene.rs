//! Turning a landscape photo into an animated scene.
//!
//! A `Scene` is built once per landscape and screen size:
//! the photo is cropped to the screen's shape, shrunk to the pixel grid chosen by the
//! pixelation slider, colour-graded, and analysed (where is the sky, is there a lake
//! reflection, is it night). Every frame then adds the moving parts on the grid (drifting
//! clouds, rippling water, glints, stars, a breathing sun glow, birds, motes, the logo),
//! snaps colours toward a small dithered palette by the stylize amount, and scales the grid
//! up to the screen with hard edges. The ASCII slider turns a share of the screen's cells
//! into characters.

use crate::config::{Logo, Look};
use image::{imageops::FilterType, RgbImage};
use rayon::prelude::*;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;

pub type Rgb = [f32; 3];

const LUM: Rgb = [0.299, 0.587, 0.114];
const CREAM: Rgb = [250.0, 244.0, 232.0];
const WARM_WHITE: Rgb = [255.0, 246.0, 228.0];
const BAYER4: [[f32; 4]; 4] = [
    [0.0, 8.0, 2.0, 10.0],
    [12.0, 4.0, 14.0, 6.0],
    [3.0, 11.0, 1.0, 9.0],
    [15.0, 7.0, 13.0, 5.0],
];
const GLITCH_CHARS: &[u8] = b"#%&@$*+=/\\<>[]{}01|~^?!";
/// The program's own source, shown by the "code breaking through" effect.
fn code_lines() -> &'static [String] {
    static LINES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    LINES.get_or_init(|| {
        [include_str!("scene.rs"), include_str!("player.rs"), include_str!("saver.rs")]
            .iter()
            .flat_map(|s| s.lines())
            .map(|l| l.replace('\t', "    "))
            .filter(|l| !l.trim().is_empty())
            .collect()
    })
}

const ASCII_RAMP: &[u8] = b" .:-=+*#%@";

#[inline]
fn bayer(x: usize, y: usize) -> f32 {
    (BAYER4[y & 3][x & 3] + 0.5) / 16.0 - 0.5
}
#[inline]
fn lum(c: Rgb) -> f32 {
    c[0] * LUM[0] + c[1] * LUM[1] + c[2] * LUM[2]
}
#[inline]
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
#[inline]
fn pack(c: Rgb) -> u32 {
    let r = c[0].clamp(0.0, 255.0) as u32;
    let g = c[1].clamp(0.0, 255.0) as u32;
    let b = c[2].clamp(0.0, 255.0) as u32;
    (r << 16) | (g << 8) | b
}
#[inline]
fn unpack(p: u32) -> Rgb {
    [((p >> 16) & 255) as f32, ((p >> 8) & 255) as f32, (p & 255) as f32]
}
#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ------------------------------------------------------------------ noise
#[inline]
pub fn hash2(x: i64, y: i64, seed: u32) -> f32 {
    let mut n = (x.wrapping_mul(374761393) as u64)
        .wrapping_add(y.wrapping_mul(668265263) as u64)
        .wrapping_add((seed as u64).wrapping_mul(2246822519))
        & 0xFFFF_FFFF;
    n = ((n ^ (n >> 13)).wrapping_mul(1274126177)) & 0xFFFF_FFFF;
    ((n ^ (n >> 16)) & 0xFFFF) as f32 / 65535.0
}

/// Value noise that wraps in x every `period` lattice cells.
fn wnoise(x: f32, y: f32, seed: u32, period: i64) -> f32 {
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    let (fx, fy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (ix, iy) = (ix as i64, iy as i64);
    let x0 = ix.rem_euclid(period);
    let x1 = (ix + 1).rem_euclid(period);
    let a = hash2(x0, iy, seed);
    let b = hash2(x1, iy, seed);
    let c = hash2(x0, iy + 1, seed);
    let d = hash2(x1, iy + 1, seed);
    (a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy
}

fn wfbm(x: f32, y: f32, seed: u32, period: i64) -> f32 {
    let (mut v, mut a, mut f, mut tot) = (0.0, 0.5, 1.0f32, 0.0);
    for o in 0..4 {
        v += a * wnoise(x * f, y * f, seed + o * 31, period * f as i64);
        tot += a;
        a *= 0.5;
        f *= 2.0;
    }
    v / tot
}

// ------------------------------------------------------------------ sources
/// A decoded landscape photo, shrunk so the longest side is at most 2560 px.
pub struct Source {
    pub img: RgbImage,
    /// For an animated GIF: every frame and how long it shows, in seconds. Empty for stills.
    pub frames: Vec<(RgbImage, f32)>,
}

/// Most frames kept from an animated GIF (memory stays modest even for long loops).
const MAX_GIF_FRAMES: usize = 300;

fn load_gif_frames(path: &Path) -> Vec<(RgbImage, f32)> {
    use image::AnimationDecoder;
    let Ok(file) = std::fs::File::open(path) else { return Vec::new() };
    let Ok(dec) = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file)) else { return Vec::new() };
    let mut out = Vec::new();
    for f in dec.into_frames().take(MAX_GIF_FRAMES) {
        let Ok(f) = f else { break };
        let (n, d) = f.delay().numer_denom_ms();
        // browsers treat very short delays as 0.1 s; do the same
        let secs = if d == 0 || n as f32 / d as f32 <= 10.0 { 0.1 } else { n as f32 / d as f32 / 1000.0 };
        let img = image::DynamicImage::ImageRgba8(f.into_buffer());
        let img = if img.width() > 1280 || img.height() > 1280 { img.resize(1280, 1280, FilterType::Triangle) } else { img };
        out.push((img.to_rgb8(), secs));
    }
    if out.len() < 2 {
        out.clear();
    }
    out
}

pub fn load_source(path: &Path) -> Result<Arc<Source>, String> {
    let is_gif = path.extension().map(|e| e.eq_ignore_ascii_case("gif")).unwrap_or(false);
    let frames = if is_gif { load_gif_frames(path) } else { Vec::new() };
    if let Some((first, _)) = frames.first() {
        return Ok(Arc::new(Source { img: first.clone(), frames }));
    }
    let img = match crate::config::decode_unlimited(path) {
        Some(i) => i,
        None => image::open(path).map_err(|e| format!("{}: {e}", path.display()))?,
    };
    let img = if img.width() > 2560 || img.height() > 2560 {
        img.resize(2560, 2560, FilterType::Triangle)
    } else {
        img
    };
    Ok(Arc::new(Source { img: img.to_rgb8(), frames: Vec::new() }))
}

/// Logo artwork as coverage 0..1 (transparent PNGs use alpha; opaque art uses darkness).
pub struct LogoArt {
    pub w: usize,
    pub h: usize,
    pub cov: Vec<f32>,
}

pub fn load_logo(path: &Path) -> Option<Arc<LogoArt>> {
    let img = image::open(path).ok()?.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let has_alpha = img.pixels().any(|p| p[3] < 250);
    let cov = img
        .pixels()
        .map(|p| {
            let a = p[3] as f32 / 255.0;
            let dark = 1.0 - (p[0] as f32 + p[1] as f32 + p[2] as f32) / (3.0 * 255.0);
            if has_alpha { a } else { dark }
        })
        .collect();
    Some(Arc::new(LogoArt { w, h, cov }))
}

// ------------------------------------------------------------------ analysis helpers
fn crop_to_aspect(img: &RgbImage, aspect: f32) -> RgbImage {
    let (w, h) = (img.width(), img.height());
    let (cw, ch) = if w as f32 / h as f32 > aspect {
        (((h as f32) * aspect).round() as u32, h)
    } else {
        (w, ((w as f32) / aspect).round() as u32)
    };
    let (x0, y0) = ((w - cw.min(w)) / 2, (h - ch.min(h)) / 2);
    image::imageops::crop_imm(img, x0, y0, cw.min(w).max(1), ch.min(h).max(1)).to_image()
}

fn to_vec(img: &RgbImage) -> Vec<Rgb> {
    img.pixels().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect()
}

/// Separable box blur with clamped edges.
fn box_blur(a: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return a.to_vec();
    }
    let k = (2 * r + 1) as f32;
    let mut tmp = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for d in 0..=2 * r {
                let xx = (x + d).saturating_sub(r).min(w - 1);
                s += a[y * w + xx];
            }
            tmp[y * w + x] = s / k;
        }
    }
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for d in 0..=2 * r {
                let yy = (y + d).saturating_sub(r).min(h - 1);
                s += tmp[yy * w + x];
            }
            out[y * w + x] = s / k;
        }
    }
    out
}

/// Warm split-tone grade (plum shadows, clay mids, cream highlights) with an S-curve and a
/// saturation lift: the colour language of the stylized look.
fn grade(c: Rgb) -> Rgb {
    let x = [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0];
    let l = lum(x);
    let mut y = [0.0; 3];
    for i in 0..3 {
        let v = (l + (x[i] - l) * 1.25).clamp(0.0, 1.0);
        y[i] = v * v * (3.0 - 2.0 * v) * 0.5 + v * 0.5;
    }
    let shadow = [38.0 / 255.0, 26.0 / 255.0, 46.0 / 255.0];
    let mid = [217.0 / 255.0, 119.0 / 255.0, 87.0 / 255.0];
    let high = [246.0 / 255.0, 236.0 / 255.0, 220.0 / 255.0];
    let ws = (1.0 - l * 2.2).clamp(0.0, 1.0).powf(1.5);
    let wh = (l * 1.6 - 0.6).clamp(0.0, 1.0).powf(1.5);
    let wm = (1.0 - (l - 0.5).abs() * 2.5).clamp(0.0, 1.0) * 0.35;
    let wt = ws + wh + wm;
    let mut out = [0.0; 3];
    for i in 0..3 {
        let tone = shadow[i] * ws + high[i] * wh + mid[i] * wm;
        out[i] = ((y[i] * (1.0 - wt * 0.45) + tone * 0.45).clamp(0.0, 1.0)) * 255.0;
    }
    out
}

/// The Claude FM colour language: a high-key, mostly cream-white picture with soft tints.
/// Three ramps, dark to light, that every colour is pulled onto at the stylized end: clay for
/// most things, pale slate blue for skies and water, pale sage for vegetation. All of them end
/// in the same cream paper white.
const RAMP_WARM: [u32; 8] = [0x7a4a3c, 0xa8604b, 0xcf8061, 0xe3a98c, 0xeecbb5, 0xf4e2d4, 0xf8efe5, 0xfbf8f2];
const RAMP_BLUE: [u32; 8] = [0x4f5d6e, 0x768898, 0x9eadba, 0xc0cbd3, 0xdae0e3, 0xe9ecea, 0xf3f3ee, 0xfbf8f2];
const RAMP_GREEN: [u32; 8] = [0x56603f, 0x7a875f, 0xa0aa85, 0xc0c7a8, 0xd9dcc6, 0xe8eadc, 0xf3f2e8, 0xfbf8f2];

fn ramp(r: &[u32; 8], t: f32) -> Rgb {
    let x = t.clamp(0.0, 1.0) * 7.0;
    let i = (x as usize).min(6);
    let f = x - i as f32;
    let c = |v: u32| [((v >> 16) & 255) as f32, ((v >> 8) & 255) as f32, (v & 255) as f32];
    mix(c(r[i]), c(r[i + 1]), f)
}

/// Map a colour onto the FM ramps, keeping the lightness of `c` and choosing the ramp by the
/// hue of `hue_from`.
fn fm_color(c: Rgb, hue_from: Rgb) -> Rgb {
    let (r, g, b) = (hue_from[0] / 255.0, hue_from[1] / 255.0, hue_from[2] / 255.0);
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let sat = if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
    let hue = if mx - mn < 1e-5 {
        0.0
    } else if mx == r {
        60.0 * (((g - b) / (mx - mn)).rem_euclid(6.0))
    } else if mx == g {
        60.0 * ((b - r) / (mx - mn) + 2.0)
    } else {
        60.0 * ((r - g) / (mx - mn) + 4.0)
    };
    let bump = |centre: f32, width: f32| {
        let d = ((hue - centre + 180.0).rem_euclid(360.0) - 180.0).abs();
        (1.0 - d / width).clamp(0.0, 1.0)
    };
    let chroma = (sat * 2.5).clamp(0.0, 1.0);
    let wb = bump(215.0, 75.0) * chroma;
    let wg = bump(95.0, 70.0) * chroma;
    let ww = (1.0 - (wb + wg) * 1.5).max(0.0) + 0.02;
    let t = (wb + wg + ww).max(1e-5);
    // gentle S-curve on lightness so the result has the poster-like contrast of the stream
    let l = lum(c) / 255.0;
    // high key: lift everything toward the cream paper colour, so most of the frame is near
    // white and only the darkest parts keep a soft tint
    let l = 0.1 + 0.9 * l.powf(0.62);
    let (a, bb, cc) = (ramp(&RAMP_WARM, l), ramp(&RAMP_BLUE, l), ramp(&RAMP_GREEN, l));
    [
        (a[0] * ww + bb[0] * wb + cc[0] * wg) / t,
        (a[1] * ww + bb[1] * wb + cc[1] * wg) / t,
        (a[2] * ww + bb[2] * wb + cc[2] * wg) / t,
    ]
}

fn kmeans_palette(px: &[Rgb], k: usize) -> Vec<Rgb> {
    let step = (px.len() / 8000).max(1);
    let data: Vec<Rgb> = px.iter().step_by(step).copied().collect();
    let mut order: Vec<usize> = (0..data.len()).collect();
    order.sort_by(|&a, &b| lum(data[a]).partial_cmp(&lum(data[b])).unwrap());
    let k = k.clamp(2, 64).min(data.len().max(2));
    let mut cent: Vec<Rgb> = (0..k).map(|i| data[order[i * (data.len() - 1) / (k - 1).max(1)]]).collect();
    let mut lab = vec![0usize; data.len()];
    for _ in 0..12 {
        for (i, d) in data.iter().enumerate() {
            let mut best = (f32::MAX, 0);
            for (j, c) in cent.iter().enumerate() {
                let e = (d[0] - c[0]).powi(2) + (d[1] - c[1]).powi(2) + (d[2] - c[2]).powi(2);
                if e < best.0 {
                    best = (e, j);
                }
            }
            lab[i] = best.1;
        }
        let mut sum = vec![[0.0f32; 3]; k];
        let mut cnt = vec![0usize; k];
        for (i, d) in data.iter().enumerate() {
            let s = &mut sum[lab[i]];
            s[0] += d[0];
            s[1] += d[1];
            s[2] += d[2];
            cnt[lab[i]] += 1;
        }
        for j in 0..k {
            if cnt[j] > 0 {
                let n = cnt[j] as f32;
                cent[j] = [sum[j][0] / n, sum[j][1] / n, sum[j][2] / n];
            }
        }
    }
    cent.sort_by(|a, b| lum(*a).partial_cmp(&lum(*b)).unwrap());
    cent.dedup_by(|a, b| (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs() < 3.0);
    cent
}

/// Sky mask: smooth, gradually-changing colour flood-filled from the top edge, with enclosed
/// holes (clouds, sun, birds) filled back in.
fn find_sky(px: &[Rgb], w: usize, h: usize) -> Vec<bool> {
    let mut s = vec![[0.0f32; 3]; w * h];
    for ch in 0..3 {
        let c: Vec<f32> = px.iter().map(|p| p[ch]).collect();
        let b = box_blur(&c, w, h, 1);
        for i in 0..w * h {
            s[i][ch] = b[i];
        }
    }
    let l: Vec<f32> = s.iter().map(|&c| lum(c)).collect();
    let l2: Vec<f32> = l.iter().map(|v| v * v).collect();
    let m = box_blur(&l, w, h, 2);
    let m2 = box_blur(&l2, w, h, 2);
    let tex: Vec<f32> = (0..w * h).map(|i| (m2[i] - m[i] * m[i]).max(0.0).sqrt()).collect();
    let mut top: Vec<f32> = tex[..w * 3.min(h)].to_vec();
    top.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let tex_lim = (top[top.len() / 2] * 3.0 + 3.0).max(6.0);
    let mut sky = vec![false; w * h];
    let mut q = VecDeque::new();
    for x in 0..w {
        if tex[x] < tex_lim {
            sky[x] = true;
            q.push_back((0usize, x));
        }
    }
    while let Some((y, x)) = q.pop_front() {
        let c = s[y * w + x];
        let nb = [(y + 1, x), (y.wrapping_sub(1), x), (y, x + 1), (y, x.wrapping_sub(1))];
        for (ny, nx) in nb {
            if ny < h && nx < w {
                let i = ny * w + nx;
                if !sky[i] && tex[i] < tex_lim {
                    let d = (s[i][0] - c[0]).abs() + (s[i][1] - c[1]).abs() + (s[i][2] - c[2]).abs();
                    if d < 10.0 {
                        sky[i] = true;
                        q.push_back((ny, nx));
                    }
                }
            }
        }
    }
    // fill holes: anything not reachable from the bottom edge without crossing sky
    let mut seen = sky.clone();
    for x in 0..w {
        let i = (h - 1) * w + x;
        if !seen[i] {
            seen[i] = true;
            q.push_back((h - 1, x));
        }
    }
    while let Some((y, x)) = q.pop_front() {
        let nb = [(y + 1, x), (y.wrapping_sub(1), x), (y, x + 1), (y, x.wrapping_sub(1))];
        for (ny, nx) in nb {
            if ny < h && nx < w && !seen[ny * w + nx] {
                seen[ny * w + nx] = true;
                q.push_back((ny, nx));
            }
        }
    }
    (0..w * h).map(|i| sky[i] || !seen[i]).collect()
}

/// Row where a lake reflection starts, if the lower part of the picture mirrors the upper part.
fn find_water_line(px: &[Rgb], w: usize, h: usize) -> Option<usize> {
    let l0: Vec<f32> = px.iter().map(|&c| lum(c)).collect();
    let l = box_blur(&l0, w, h, 1);
    let row_dev = |y: usize| -> Vec<f32> {
        let r = &l[y * w..(y + 1) * w];
        let m = r.iter().sum::<f32>() / w as f32;
        r.iter().map(|v| v - m).collect()
    };
    let rows: Vec<Vec<f32>> = (0..h).map(row_dev).collect();
    let corr = |pairs: &mut dyn Iterator<Item = (usize, usize)>| -> f32 {
        let (mut sab, mut saa, mut sbb) = (0.0f64, 0.0f64, 0.0f64);
        for (a, b) in pairs {
            for x in 0..w {
                let (p, q) = (rows[a][x] as f64, rows[b][x] as f64);
                sab += p * q;
                saa += p * p;
                sbb += q * q;
            }
        }
        (sab / ((saa * sbb).sqrt() + 1e-6)) as f32
    };
    let (mut best, mut axis) = (-1.0f32, None);
    let hf = h as f32;
    for y in (hf * 0.3) as usize..(hf * 0.9) as usize {
        let k = (y as i64 - (hf * 0.1) as i64).min(h as i64 - 1 - y as i64).min((hf * 0.25) as i64);
        if (k as f32) < hf * 0.1 {
            continue;
        }
        let k = k as usize;
        // mirrored: row y-1-i against y+1+i;  plain: row y-k+i against y+1+i
        let mirrored = corr(&mut (0..k).map(|i| (y - 1 - i, y + 1 + i)));
        let plain = corr(&mut (0..k).map(|i| (y - k + i, y + 1 + i)));
        let score = mirrored - plain.max(0.0);
        if score > best {
            best = score;
            axis = Some(y);
        }
    }
    if best > 0.5 { axis.map(|a| a + 1) } else { None }
}

// ------------------------------------------------------------------ scene
struct Star {
    x: usize,
    y: usize,
    f: f32,
    ph: f32,
}
struct Mote {
    x: f32,
    y: f32,
    f: f32,
    ph: f32,
}
struct Bird {
    off: f32,
    y: f32,
    ph: f32,
}

pub struct Scene {
    pub sw: usize,
    pub sh: usize,
    pub gw: usize,
    pub gh: usize,
    start: Vec<Rgb>,
    /// Animated GIF frames on the grid (graded), with the time each one ends; empty for stills.
    anim: Vec<(Vec<[u8; 3]>, f32)>,
    anim_frame: usize,
    sky: Vec<bool>,
    water_from: Option<usize>,
    hz: f32,
    pal: Vec<Rgb>,
    lut: Vec<u8>,
    dither_step: f32,
    glow: Option<Vec<f32>>,
    stars: Vec<Star>,
    motes: Vec<Mote>,
    birds: Vec<Bird>,
    glints: Vec<(usize, f32)>,
    logo: Option<Vec<f32>>,
    logo_rim: Vec<bool>,
    look: Look,
    logo_cfg: Logo,
    seed: u32,
    work: Vec<Rgb>,
    grid: Vec<u32>,
    cloud: Vec<f32>,
    cw: usize,
    ch: usize,
    xmap: Vec<usize>,
}

/// Pixel size on screen for a pixelation slider value, at 1080p.
pub fn pixel_size(pixelation: f32) -> f32 {
    1.0 + 15.0 * pixelation.clamp(0.0, 1.0).powf(1.5)
}

impl Scene {
    pub fn new(src: &Source, logo: Option<&LogoArt>, sw: usize, sh: usize, look: &Look, logo_cfg: &Logo, seed: u32) -> Scene {
        let sw = sw.max(8);
        let sh = sh.max(8);
        let aspect = sw as f32 / sh as f32;
        let px = pixel_size(look.pixelation) * sh as f32 / 1080.0;
        let gw = ((sw as f32 / px.max(1.0)).round() as usize).clamp(16, sw.min(2560));
        let gh = ((gw as f32 / aspect).round() as usize).max(9);
        let cropped = crop_to_aspect(&src.img, aspect);

        // analysis at a fixed small size
        let aw = 320usize;
        let ah = ((aw as f32 / aspect).round() as usize).max(8);
        let small = image::imageops::resize(&cropped, aw as u32, ah as u32, FilterType::Triangle);
        let small_v = to_vec(&small);
        let sky_a = find_sky(&small_v, aw, ah);
        let water_a = if look.water { find_water_line(&small_v, aw, ah) } else { None };
        let graded_small: Vec<Rgb> = small_v.iter().map(|&c| grade(c)).collect();
        let pal_match = kmeans_palette(&graded_small, look.colors as usize);
        // what each palette entry is drawn as: pulled onto the FM ramps by the stylize amount
        // The grade warms everything, so pick each entry's FM ramp from the ungraded colours
        // it stands for (otherwise green grass would land on the warm ramp and turn brown).
        let mut src_sum = vec![[0.0f32; 4]; pal_match.len()];
        for (g, o) in graded_small.iter().zip(small_v.iter()) {
            let mut best = (f32::MAX, 0);
            for (j, p) in pal_match.iter().enumerate() {
                let d = (g[0] - p[0]).powi(2) + (g[1] - p[1]).powi(2) + (g[2] - p[2]).powi(2);
                if d < best.0 {
                    best = (d, j);
                }
            }
            let e = &mut src_sum[best.1];
            e[0] += o[0];
            e[1] += o[1];
            e[2] += o[2];
            e[3] += 1.0;
        }
        // entries are drawn fully in FM colours; the stylize slider blends toward them per pixel
        let pull = 1.0;
        let pal: Vec<Rgb> = pal_match
            .iter()
            .zip(src_sum.iter())
            .map(|(&c, e)| {
                let hue_from = if e[3] > 0.0 { [e[0] / e[3], e[1] / e[3], e[2] / e[3]] } else { c };
                mix(c, fm_color(c, hue_from), pull)
            })
            .collect();

        // grid
        let grid_img = image::imageops::resize(&cropped, gw as u32, gh as u32, FilterType::Triangle);
        let photo = to_vec(&grid_img);
        let s = look.stylize.clamp(0.0, 1.0);
        let start: Vec<Rgb> = photo.iter().map(|&c| mix(c, grade(c), s)).collect();
        // an animated GIF: prepare every frame the same way (the sky and water analysis
        // comes from the first frame)
        let mut anim = Vec::new();
        let mut end = 0.0;
        // keep memory to about 150 MB per screen: long GIFs drop frames evenly (their
        // delays are merged so the timing stays right)
        let budget = (150_000_000 / (gw * gh * 3).max(1)).clamp(2, MAX_GIF_FRAMES);
        let step = src.frames.len().div_ceil(budget).max(1);
        for chunk in src.frames.chunks(step) {
            let (img, _) = &chunk[0];
            let secs: f32 = chunk.iter().map(|f| f.1).sum();
            let g = image::imageops::resize(&crop_to_aspect(img, aspect), gw as u32, gh as u32, FilterType::Triangle);
            let cells = g
                .pixels()
                .map(|p| {
                    let c = [p[0] as f32, p[1] as f32, p[2] as f32];
                    let m = mix(c, grade(c), s);
                    [m[0] as u8, m[1] as u8, m[2] as u8]
                })
                .collect();
            end += secs;
            anim.push((cells, end));
        }
        let sky: Vec<bool> = (0..gw * gh)
            .map(|i| {
                let (x, y) = (i % gw, i / gw);
                sky_a[(y * ah / gh).min(ah - 1) * aw + (x * aw / gw).min(aw - 1)]
            })
            .collect();
        let water_from = water_a.map(|r| r * gh / ah);
        let mut hz_rows: Vec<usize> = (0..gw)
            .map(|x| (0..gh).rev().find(|&y| sky[y * gw + x]).map(|y| y + 1).unwrap_or(0))
            .collect();
        hz_rows.sort_unstable();
        let hz = hz_rows[gw / 2] as f32;

        let sky_l: Vec<f32> = (0..gw * gh).filter(|&i| sky[i]).map(|i| lum(grade(photo[i]))).collect();
        let sky_mean = if sky_l.is_empty() { 128.0 } else { sky_l.iter().sum::<f32>() / sky_l.len() as f32 };
        let night = sky_mean < 95.0;

        // palette lookup table, 5 bits per channel
        let mut lut = vec![0u8; 32 * 32 * 32];
        for (i, e) in lut.iter_mut().enumerate() {
            let c = [((i >> 10) & 31) as f32 * 8.0 + 4.0, ((i >> 5) & 31) as f32 * 8.0 + 4.0, (i & 31) as f32 * 8.0 + 4.0];
            let mut best = (f32::MAX, 0u8);
            for (j, p) in pal_match.iter().enumerate() {
                let d = (c[0] - p[0]).powi(2) + (c[1] - p[1]).powi(2) + (c[2] - p[2]).powi(2);
                if d < best.0 {
                    best = (d, j as u8);
                }
            }
            *e = best.1;
        }
        let mut steps: Vec<f32> = pal_match
            .windows(2)
            .map(|w| (w[1][0] - w[0][0]).abs() + (w[1][1] - w[0][1]).abs() + (w[1][2] - w[0][2]).abs())
            .collect();
        steps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let dither_step = if steps.is_empty() { 32.0 } else { steps[steps.len() / 2] / 2.0 };

        // glow around the brightest light in the sky (not for a plain blue day sky)
        let mut glow = None;
        if !sky_l.is_empty() {
            let mut sum = [0.0f32; 3];
            let mut n = 0.0;
            for i in 0..gw * gh {
                if sky[i] {
                    let g = grade(photo[i]);
                    sum[0] += g[0];
                    sum[1] += g[1];
                    sum[2] += g[2];
                    n += 1.0;
                }
            }
            let blue_day = sum[2] / n > sum[0] / n + 30.0 && !night;
            if !blue_day {
                let lmap: Vec<f32> = (0..aw * ah).map(|i| if sky_a[i] { lum(graded_small[i]) } else { -1.0 }).collect();
                let bl = box_blur(&lmap, aw, ah, 5);
                let (mut bi, mut bv) = (0, f32::MIN);
                for (i, &v) in bl.iter().enumerate() {
                    if sky_a[i] && v > bv {
                        bv = v;
                        bi = i;
                    }
                }
                let mut ls: Vec<f32> = (0..aw * ah).filter(|&i| sky_a[i]).map(|i| lum(graded_small[i])).collect();
                ls.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let p90 = ls[(ls.len() as f32 * 0.9) as usize];
                if bv > p90 + 8.0 {
                    let gx = (bi % aw) as f32 / aw as f32 * gw as f32;
                    let gy = (bi / aw) as f32 / ah as f32 * gh as f32;
                    let r = gw as f32 * 0.09;
                    glow = Some(
                        (0..gw * gh)
                            .map(|i| {
                                let (x, y) = ((i % gw) as f32, (i / gw) as f32);
                                let d = ((x - gx).powi(2) + (y - gy).powi(2)).sqrt() / r;
                                if sky[i] { (1.0 - d).max(0.0).powi(2) } else { 0.0 }
                            })
                            .collect(),
                    );
                }
            }
        }

        let mut rng = seed.wrapping_mul(2654435761);
        let mut rnd = move || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng as f32) / (u32::MAX as f32)
        };
        let mut stars = Vec::new();
        if night && look.stars {
            let n = (gw * gh) as f32 * 0.004;
            for _ in 0..n as usize {
                let (x, y) = ((rnd() * gw as f32) as usize % gw, (rnd() * gh as f32) as usize % gh);
                if sky[y * gw + x] {
                    stars.push(Star { x, y, f: 1.0 + (rnd() * 3.0).floor(), ph: rnd() });
                }
            }
        }
        let motes = (0..look.motes)
            .map(|_| Mote { x: rnd() * gw as f32, y: rnd() * gh as f32, f: 1.0 + (rnd() * 2.0).floor(), ph: rnd() })
            .collect();
        let birds = (0..look.birds).map(|_| Bird { off: rnd() * 0.15, y: gh as f32 * (0.12 + rnd() * 0.2), ph: rnd() }).collect();
        let mut glints = Vec::new();
        if let Some(wf) = water_from {
            for y in wf..gh {
                for x in 0..gw {
                    if hash2((x / 2) as i64, y as i64, seed + 99) > 0.985 {
                        glints.push((y * gw + x, hash2((x / 2) as i64, y as i64, seed + 7)));
                    }
                }
            }
        }

        // logo coverage on the grid, centred
        let mut logo_cov = None;
        let mut logo_rim = Vec::new();
        if let (Some(art), true) = (logo, logo_cfg.enabled) {
            let th = ((gh as f32 * logo_cfg.size) as usize).clamp(4, gh);
            let tw = ((art.w as f32 * th as f32 / art.h as f32) as usize).clamp(4, gw);
            let (x0, y0) = ((gw - tw) / 2, (gh - th) / 2);
            let mut cov = vec![0.0f32; gw * gh];
            for ty in 0..th {
                for tx in 0..tw {
                    // box-average the source pixels that fall in this grid cell
                    let (sx0, sx1) = (tx * art.w / tw, ((tx + 1) * art.w / tw).max(tx * art.w / tw + 1));
                    let (sy0, sy1) = (ty * art.h / th, ((ty + 1) * art.h / th).max(ty * art.h / th + 1));
                    let (mut s, mut n) = (0.0, 0.0);
                    for yy in sy0..sy1.min(art.h) {
                        for xx in sx0..sx1.min(art.w) {
                            s += art.cov[yy * art.w + xx];
                            n += 1.0;
                        }
                    }
                    cov[(y0 + ty) * gw + x0 + tx] = if n > 0.0 { s / n } else { 0.0 };
                }
            }
            logo_rim = vec![false; gw * gh];
            logo_cov = Some(cov);
        }

        let cw = gw.min(240);
        let ch = ((cw as f32 / aspect) as usize).max(4);
        let xmap = (0..sw).map(|x| x * gw / sw).collect();
        Scene {
            sw,
            sh,
            gw,
            gh,
            start,
            anim,
            anim_frame: 0,
            sky,
            water_from,
            hz,
            pal,
            lut,
            dither_step,
            glow,
            stars,
            motes,
            birds,
            glints,
            logo: logo_cov,
            logo_rim,
            look: look.clone(),
            logo_cfg: logo_cfg.clone(),
            seed,
            work: vec![[0.0; 3]; gw * gh],
            grid: vec![0; gw * gh],
            cloud: vec![0.0; cw * ch],
            cw,
            ch,
            xmap,
        }
    }

    /// Render the scene at time `t` (seconds) into a screen-sized 0RGB buffer.
    pub fn render(&mut self, t: f32, out: &mut [u32]) {
        let (gw, gh) = (self.gw, self.gh);
        let tau = std::f32::consts::TAU;
        let unit = gw as f32 / 480.0; // effect sizes are tuned for a 480-wide grid
        // wind speeds things up; 0.3 (the default) is the original pace
        let wind = self.look.wind.clamp(0.0, 1.0);
        let wf = 0.25 + wind * 2.5;
        if let Some(total) = self.anim.last().map(|a| a.1) {
            let at = t.rem_euclid(total.max(0.01));
            let idx = self.anim.iter().position(|a| at < a.1).unwrap_or(0);
            if idx != self.anim_frame {
                self.anim_frame = idx;
                for (d, s) in self.start.iter_mut().zip(self.anim[idx].0.iter()) {
                    *d = [s[0] as f32, s[1] as f32, s[2] as f32];
                }
            }
        }
        self.work.copy_from_slice(&self.start);

        // water: each row slides sideways on slow sine waves, deeper rows move more
        if let Some(wf) = self.water_from {
            let start = &self.start;
            let span = (gh - wf).max(1) as f32;
            self.work[wf * gw..].par_chunks_mut(gw).enumerate().for_each(|(i, row)| {
                let y = wf + i;
                let depth = i as f32 / span;
                let amp = (0.6 + depth * 6.0) * unit;
                let sh = (amp * (tau * t * 0.25 + y as f32 * 0.9 / unit.max(0.3)).sin()
                    + 0.5 * amp * (-tau * t * 0.17 + y as f32 * 0.37 / unit.max(0.3)).sin())
                .round() as i64;
                let src = &start[y * gw..(y + 1) * gw];
                for (x, px) in row.iter_mut().enumerate() {
                    let sx = (x as i64 - sh).rem_euclid(gw as i64) as usize;
                    *px = src[sx];
                }
            });
        }

        // clouds: wrap-around noise drifting slowly, lightening the sky
        if self.look.clouds > 0.0 {
            let (cw, ch) = (self.cw, self.ch);
            let period = 4i64;
            let drift = t * wf / 60.0 * period as f32; // one tile per minute at the default wind
            let hzc = (self.hz / gh as f32 * ch as f32).max(1.0);
            let thr = 1.0 - self.look.clouds * 0.45;
            let seed = self.seed + 5;
            self.cloud.par_chunks_mut(cw).enumerate().for_each(|(y, row)| {
                let band = (1.0 - ((y as f32 / hzc) - 0.35).abs() * 2.2).clamp(0.0, 1.0);
                for (x, v) in row.iter_mut().enumerate() {
                    let n = wfbm(
                        x as f32 / cw as f32 * period as f32 + drift,
                        y as f32 / cw as f32 * period as f32 * 3.2,
                        seed,
                        period,
                    );
                    *v = smoothstep(0.0, 0.18, n * band - thr);
                }
            });
            let cloud = &self.cloud;
            let sky = &self.sky;
            let amount = 0.55;
            self.work.par_chunks_mut(gw).enumerate().for_each(|(y, row)| {
                let cy = (y as f32 + 0.5) / gh as f32 * ch as f32 - 0.5;
                let y0 = (cy.floor().max(0.0) as usize).min(ch - 1);
                let y1 = (y0 + 1).min(ch - 1);
                let fy = (cy - y0 as f32).clamp(0.0, 1.0);
                for (x, px) in row.iter_mut().enumerate() {
                    if !sky[y * gw + x] {
                        continue;
                    }
                    let cx = (x as f32 + 0.5) / gw as f32 * cw as f32 - 0.5;
                    let x0 = (cx.floor().max(0.0) as usize).min(cw - 1);
                    let x1 = (x0 + 1).min(cw - 1);
                    let fx = (cx - x0 as f32).clamp(0.0, 1.0);
                    let a = (cloud[y0 * cw + x0] * (1.0 - fx) + cloud[y0 * cw + x1] * fx) * (1.0 - fy)
                        + (cloud[y1 * cw + x0] * (1.0 - fx) + cloud[y1 * cw + x1] * fx) * fy;
                    if a > 0.0 {
                        *px = mix(*px, mix(*px, WARM_WHITE, 0.45), a * amount);
                    }
                }
            });
        }

        // breathing glow around the sun / brightest light
        if let Some(g) = &self.glow {
            let k = 0.10 + 0.05 * (tau * t / 6.0).sin();
            self.work.par_iter_mut().zip(g.par_iter()).for_each(|(px, &gv)| {
                if gv > 0.0 {
                    *px = mix(*px, [255.0, 236.0, 200.0], gv * k * 2.0);
                }
            });
        }

        // glints on water
        for &(i, ph) in &self.glints {
            if (tau * (t / 3.0 + ph)).sin() > 0.75 {
                self.work[i] = mix(self.work[i], WARM_WHITE, 0.85);
            }
        }

        // stars
        for s in &self.stars {
            let v = 0.5 + 0.5 * (tau * (s.f * t / 8.0 + s.ph)).sin();
            if v > 0.35 {
                let i = s.y * gw + s.x;
                self.work[i] = mix(self.work[i], WARM_WHITE, v);
            }
        }

        // logo
        if let Some(cov) = &self.logo {
            let breathe = 1.0 + self.logo_cfg.pulse * (tau * t / 10.0).sin();
            let op = self.logo_cfg.opacity.clamp(0.0, 1.0) * breathe;
            let stylize = self.look.stylize;
            let rim = &mut self.logo_rim;
            for i in 0..gw * gh {
                let a = cov[i] * op;
                rim[i] = false;
                if a > 0.02 {
                    // in the stylized look the edge is dithered instead of blended
                    let (x, y) = (i % gw, i / gw);
                    let hard = if a + bayer(x, y) * 0.5 > 0.5 { 1.0 } else { 0.0 };
                    let k = (a * (1.0 - stylize) + hard * stylize) * 0.62;
                    self.work[i] = mix(self.work[i], CREAM, k);
                }
            }
            if self.logo_cfg.outline {
                for y in 0..gh {
                    for x in 0..gw {
                        let i = y * gw + x;
                        if cov[i] * op >= 0.5 {
                            continue;
                        }
                        let inside = |xx: usize, yy: usize| cov[yy * gw + xx] * op >= 0.5;
                        let near = (x > 0 && inside(x - 1, y))
                            || (x + 1 < gw && inside(x + 1, y))
                            || (y > 0 && inside(x, y - 1))
                            || (y + 1 < gh && inside(x, y + 1));
                        if near {
                            rim[i] = true;
                        }
                    }
                }
                for i in 0..gw * gh {
                    if rim[i] {
                        let c = self.work[i];
                        self.work[i] = [c[0] * 0.62, c[1] * 0.62, c[2] * 0.62];
                    }
                }
            }
        }

        // birds
        let dark = self.pal[0];
        for b in &self.birds {
            let x = ((t * (0.6 + wf * 0.4) / 40.0 + b.off).fract()) * (gw as f32 + 20.0 * unit) - 10.0 * unit;
            let y = b.y + 2.0 * unit * (tau * (t / 4.0 + b.ph)).sin();
            let flap = (tau * (t * 2.0 + b.ph)).sin() > 0.0;
            let s = unit.max(1.0).round() as i64;
            let pts: [(i64, i64); 5] = if flap {
                [(0, 0), (-1, -1), (1, -1), (-2, -1), (2, -1)]
            } else {
                [(0, 0), (-1, 0), (1, 0), (-2, 1), (2, 1)]
            };
            for (dx, dy) in pts {
                for sy in 0..s {
                    for sx in 0..s {
                        let px = x as i64 + dx * s + sx;
                        let py = y as i64 + dy * s + sy;
                        if px >= 0 && py >= 0 && (px as usize) < gw && (py as usize) < gh {
                            self.work[py as usize * gw + px as usize] = dark;
                        }
                    }
                }
            }
        }

        // motes / fireflies drifting upward with a sway
        for m in &self.motes {
            let y = (m.y - t / 48.0 * gh as f32 * 0.25 * m.f).rem_euclid(gh as f32);
            let gust = (wind - 0.3).max(0.0) * t * gw as f32 / 25.0 * m.f;
            let x = (m.x + gust + 3.0 * unit * (1.0 + wind * 2.0) * (tau * (t / 12.0 * m.f + m.ph)).sin()).rem_euclid(gw as f32);
            let v = (tau * (t / 3.0 * m.f + m.ph)).sin();
            let (xi, yi) = (x as usize % gw, y as usize % gh);
            let i = yi * gw + xi;
            if v > -0.2 && !self.sky[i] {
                self.work[i] = mix(self.work[i], [255.0, 226.0, 170.0], 0.55 * (v + 0.2).min(1.0));
            }
        }

        // wind streaks: thin pale lines racing across the sky in a strong breeze
        let streaks = ((wind - 0.4).max(0.0) * 14.0) as usize;
        for k in 0..streaks {
            let speed = gw as f32 * (0.5 + wind);
            let life = 3.0 + hash2(k as i64, 1, self.seed + 31) * 3.0;
            let cyc = ((t + k as f32 * 1.7) / life).floor();
            let ph = ((t + k as f32 * 1.7) / life).fract();
            let y = (hash2(k as i64, cyc as i64, self.seed + 32) * self.hz.max(4.0)) as usize;
            let len = (8.0 + hash2(k as i64, cyc as i64, self.seed + 33) * 24.0) * unit;
            let x0 = ph * (speed * life / 3.0) - len - hash2(k as i64, cyc as i64, self.seed + 34) * gw as f32 * 0.3;
            let fade = (ph * std::f32::consts::PI).sin();
            for d in 0..len as i64 {
                let x = x0 as i64 + d;
                if x >= 0 && (x as usize) < gw && y < gh {
                    let i = y * gw + x as usize;
                    if self.sky[i] {
                        let a = 0.4 * fade * (d as f32 / len).min(1.0);
                        // pale line on a darker sky, faint grey-blue line on a bright one
                        let col = if lum(self.work[i]) > 200.0 { [168.0, 182.0, 196.0] } else { [250.0, 246.0, 238.0] };
                        self.work[i] = mix(self.work[i], col, a);
                    }
                }
            }
        }

        // stylize: snap toward the dithered FM palette (eased so the middle of the slider is
        // already clearly stylized)
        let s = self.look.stylize.clamp(0.0, 1.0).powf(0.7);
        let dstep = self.dither_step * self.look.dither * 2.0;
        let (pal, lut, work) = (&self.pal, &self.lut, &self.work);
        self.grid.par_chunks_mut(gw).enumerate().for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let c = work[y * gw + x];
                if s <= 0.001 {
                    *o = pack(c);
                    continue;
                }
                let d = bayer(x, y) * dstep;
                let q = |v: f32| (((v + d).clamp(0.0, 255.0)) as usize >> 3).min(31);
                let p = pal[lut[(q(c[0]) << 10) | (q(c[1]) << 5) | q(c[2])] as usize];
                *o = pack(mix(c, p, s));
            }
        });

        // sparkles: little four-point stars that twinkle in and out, drawn after the palette
        // so they stay crisp; dark ink on bright ground, warm light on dark ground
        for k in 0..self.look.sparkles as usize {
            let life = 1.6 + hash2(k as i64, 2, self.seed + 41) * 2.4;
            let tt = t + hash2(k as i64, 3, self.seed + 41) * life;
            let cyc = (tt / life).floor() as i64;
            let b = ((tt / life).fract() * std::f32::consts::PI).sin().powi(3);
            let x = (hash2(k as i64, cyc, self.seed + 42) * gw as f32) as i64;
            let y = (hash2(k as i64, cyc, self.seed + 43) * gh as f32) as i64;
            let arm = ((1.0 + b * 2.0) * unit.max(1.0)).round() as i64;
            let s = unit.max(1.0).round() as i64;
            if x < 0 || y < 0 || x as usize >= gw || y as usize >= gh {
                continue;
            }
            let ground = unpack(self.grid[y as usize * gw + x as usize]);
            let col = if lum(ground) > 190.0 { [217.0, 119.0, 87.0] } else { [255.0, 244.0, 214.0] };
            for d in -arm..=arm {
                let a = if d == 0 { b } else { b * (1.0 - d.abs() as f32 / (arm + 1) as f32) };
                for (px, py) in [(x + d * s, y), (x, y + d * s)] {
                    for sy in 0..s {
                        for sx in 0..s {
                            let (qx, qy) = (px + sx, py + sy);
                            if qx >= 0 && qy >= 0 && (qx as usize) < gw && (qy as usize) < gh {
                                let i = qy as usize * gw + qx as usize;
                                self.grid[i] = pack(mix(unpack(self.grid[i]), col, a));
                            }
                        }
                    }
                }
            }
        }

        self.upscale(out);
        if self.look.ascii > 0.001 || self.look.glitch > 0.001 {
            self.ascii(out, t);
        }
        if self.look.breakthrough > 0.001 {
            self.breakthrough(out, t);
        }
    }

    /// "The code breaking through": every so often the picture tears along a band, rows slide
    /// sideways with the colour channels split, and through a ragged crack you see this
    /// program's own source code scrolling past, before the picture heals again.
    fn breakthrough(&self, out: &mut [u32], t: f32) {
        let (sw, sh) = (self.sw, self.sh);
        let amount = self.look.breakthrough.clamp(0.0, 1.0);
        let f = ((sh as f32 / 540.0).round() as usize).max(1);
        let cs = 8 * f;
        let (cols, rows) = (sw.div_ceil(cs), sh.div_ceil(cs));
        let lines = code_lines();
        let slots = 1 + (amount * 5.0) as usize;
        let seed = self.seed + 61;
        let mut row_buf = vec![0u32; sw];
        for k in 0..slots {
            let life = 0.8 + hash2(k as i64, 1, seed) * 2.2;
            let gap = life * (3.5 - amount * 3.0).max(0.4);
            let period = life + gap;
            let tt = t + hash2(k as i64, 2, seed) * period;
            let cyc = (tt / period).floor() as i64;
            let age = tt - cyc as f32 * period;
            if age > life || hash2(k as i64, cyc, seed + 1) > 0.35 + amount * 0.65 {
                continue;
            }
            let p = age / life;
            // opens fast, holds, closes
            let open = (p / 0.15).min(1.0).min((1.0 - p) / 0.25).clamp(0.0, 1.0);
            let h = |n: i64| hash2(k as i64 * 7 + n, cyc, seed + 2);
            let band_rows = 2 + (h(1) * (3.0 + amount * 9.0)) as usize;
            let r0 = (h(2) * rows.saturating_sub(band_rows) as f32) as usize;
            let c_w = ((0.2 + h(3) * 0.5 + amount * 0.3) * cols as f32) as usize;
            let c0 = (h(4) * cols.saturating_sub(c_w) as f32) as usize;
            let tick = (t * 14.0).floor() as i64;

            // 1. tear: rows around the band slide sideways with an RGB split
            let y0 = r0.saturating_sub(1) * cs;
            let y1 = ((r0 + band_rows + 1) * cs).min(sh);
            for y in y0..y1 {
                let slice = (y / (2 * f)) as i64;
                let off = ((hash2(slice, tick, seed + 3) - 0.5) * 2.0 * open * (12 + (amount * 40.0) as i64) as f32 * f as f32) as i64;
                if off == 0 {
                    continue;
                }
                let row = &mut out[y * sw..(y + 1) * sw];
                row_buf.copy_from_slice(row);
                let split = (2 * f) as i64;
                for x in 0..sw as i64 {
                    let at = |dx: i64| row_buf[(x - off - dx).rem_euclid(sw as i64) as usize];
                    let (r, g, b) = (at(split) >> 16 & 255, at(0) >> 8 & 255, at(-split) & 255);
                    row[x as usize] = (r << 16) | (g << 8) | b;
                }
            }

            // 2. crack: cells inside the band show scrolling source code on near-black
            let start_line = (h(5) * lines.len() as f32) as usize;
            let scroll = (age * 10.0) as usize;
            let ccx = c0 as f32 + c_w as f32 / 2.0;
            let ccy = r0 as f32 + band_rows as f32 / 2.0;
            for r in r0..(r0 + band_rows).min(rows) {
                // the row's text runs on into the following lines rather than repeating
                let indent = (h(6) * 4.0) as usize;
                let mut text: Vec<u8> = Vec::new();
                let mut li = (start_line + scroll + (r - r0) * 3) % lines.len();
                while text.len() < c_w + indent {
                    text.extend_from_slice(lines[li].trim_start().as_bytes());
                    text.extend_from_slice(b"  ");
                    li = (li + 1) % lines.len();
                }
                let line = &text[..];
                for c in c0..(c0 + c_w).min(cols) {
                    // ragged edge: cells near the middle open first
                    let dx = (c as f32 - ccx).abs() / (c_w as f32 / 2.0).max(1.0);
                    let dy = (r as f32 - ccy).abs() / (band_rows as f32 / 2.0).max(1.0);
                    let edge = dx.max(dy * 0.8);
                    let jag = hash2(c as i64, r as i64 + cyc * 97, seed + 4) * 0.35;
                    if edge + jag > open * 1.15 {
                        continue;
                    }
                    let ch = line.get(c - c0 + indent).copied().unwrap_or(b' ');
                    let ch = if ch.is_ascii_graphic() || ch == b' ' { ch } else { b' ' };
                    // flicker a few characters
                    let ch = if hash2(c as i64, r as i64 * 13 + tick, seed + 5) < 0.04 { b'#' } else { ch };
                    let fg = if hash2(r as i64, cyc, seed + 6) < 0.2 { 0xf6ecdc } else { 0xd97757 };
                    let bg = 0x141413;
                    let glyph = font8x8::legacy::BASIC_LEGACY[ch as usize & 127];
                    for gy in 0..cs {
                        let y = r * cs + gy;
                        if y >= sh {
                            break;
                        }
                        let bits = glyph[gy / f];
                        for gx in 0..cs {
                            let x = c * cs + gx;
                            if x >= sw {
                                break;
                            }
                            out[y * sw + x] = if (bits >> (gx / f)) & 1 == 1 { fg } else { bg };
                        }
                    }
                }
            }
        }
    }

    fn upscale(&self, out: &mut [u32]) {
        let (sw, sh, gw, gh) = (self.sw, self.sh, self.gw, self.gh);
        let (grid, xmap) = (&self.grid, &self.xmap);
        out.par_chunks_mut(sw).enumerate().for_each(|(y, row)| {
            let gy = (y * gh / sh).min(gh - 1);
            let src = &grid[gy * gw..(gy + 1) * gw];
            for (x, o) in row.iter_mut().enumerate() {
                *o = src[xmap[x]];
            }
        });
    }

    /// Draw the screen as text cells: each cell gets a character picked by brightness, in the
    /// cell's own colour on a dark ground. The ASCII amount fades from the picture to the text.
    /// Glitches: now and then a short run of cells flickers into random characters for a
    /// fraction of a second, whatever the ASCII amount.
    fn ascii(&self, out: &mut [u32], t: f32) {
        let (sw, sh) = (self.sw, self.sh);
        let f = ((sh as f32 / 540.0).round() as usize).max(1);
        let cs = 8 * f;
        let base = self.look.ascii.clamp(0.0, 1.0);
        let glitch = self.look.glitch.clamp(0.0, 1.0);
        let tick = (t * 9.0).floor() as i64;
        let seed = self.seed;
        let (grid, gw, gh) = (&self.grid, self.gw, self.gh);
        out.par_chunks_mut(sw * cs).enumerate().for_each(|(cy, band)| {
            let rows = band.len() / sw;
            // one glitch decision per row per tick: a run starting somewhere along it
            let g_on = glitch > 0.0 && hash2(cy as i64, tick, seed + 51) < glitch * glitch * 0.18;
            let g_start = (hash2(cy as i64, tick, seed + 52) * sw.div_ceil(cs) as f32) as usize;
            let g_len = 2 + (hash2(cy as i64, tick, seed + 53) * 10.0 * (0.4 + glitch)) as usize;
            for cx in 0..sw.div_ceil(cs) {
                let glitched = g_on && cx >= g_start && cx < g_start + g_len;
                let k = if glitched { 1.0 } else { base };
                if k <= 0.001 {
                    continue;
                }
                let px = (cx * cs + cs / 2).min(sw - 1);
                let py = (cy * cs + cs / 2).min(sh - 1);
                let c = unpack(grid[(py * gh / sh).min(gh - 1) * gw + (px * gw / sw).min(gw - 1)]);
                let l = lum(c) / 255.0;
                let ch = if glitched {
                    GLITCH_CHARS[(hash2(cx as i64, tick * 31 + cy as i64, seed + 54) * GLITCH_CHARS.len() as f32) as usize % GLITCH_CHARS.len()]
                } else {
                    let gi = ((l * ASCII_RAMP.len() as f32) as usize).min(ASCII_RAMP.len() - 1);
                    ASCII_RAMP[gi]
                };
                let glyph = font8x8::legacy::BASIC_LEGACY[ch as usize];
                let bg = [c[0] * 0.14, c[1] * 0.14, c[2] * 0.14];
                let fg = [c[0] * 1.2 + 24.0, c[1] * 1.2 + 24.0, c[2] * 1.2 + 24.0];
                for ry in 0..rows {
                    let bits = glyph[(ry / f).min(7)];
                    let row = &mut band[ry * sw..(ry + 1) * sw];
                    for rx in 0..cs {
                        let x = cx * cs + rx;
                        if x >= sw {
                            break;
                        }
                        let t = if (bits >> (rx / f)) & 1 == 1 { fg } else { bg };
                        row[x] = if k >= 0.999 { pack(t) } else { pack(mix(unpack(row[x]), t, k)) };
                    }
                }
            }
        });
    }
}

// ------------------------------------------------------------------ text (for messages)
pub fn draw_text(out: &mut [u32], sw: usize, sh: usize, text: &str, cx: usize, cy: usize, scale: usize, color: u32) {
    let w = text.len() * 8 * scale;
    let x0 = cx.saturating_sub(w / 2);
    let y0 = cy.saturating_sub(4 * scale);
    for (i, ch) in text.bytes().enumerate() {
        let glyph = font8x8::legacy::BASIC_LEGACY[(ch as usize).min(127)];
        for gy in 0..8 * scale {
            for gx in 0..8 * scale {
                if (glyph[gy / scale] >> (gx / scale)) & 1 == 1 {
                    let (x, y) = (x0 + i * 8 * scale + gx, y0 + gy);
                    if x < sw && y < sh {
                        out[y * sw + x] = color;
                    }
                }
            }
        }
    }
}
