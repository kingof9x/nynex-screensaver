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
}

pub fn load_source(path: &Path) -> Result<Arc<Source>, String> {
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let img = if img.width() > 2560 || img.height() > 2560 {
        img.resize(2560, 2560, FilterType::Triangle)
    } else {
        img
    };
    Ok(Arc::new(Source { img: img.to_rgb8() }))
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
        let pal = kmeans_palette(&graded_small, look.colors as usize);

        // grid
        let grid_img = image::imageops::resize(&cropped, gw as u32, gh as u32, FilterType::Triangle);
        let photo = to_vec(&grid_img);
        let s = look.stylize.clamp(0.0, 1.0);
        let start: Vec<Rgb> = photo.iter().map(|&c| mix(c, grade(c), s)).collect();
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
            for (j, p) in pal.iter().enumerate() {
                let d = (c[0] - p[0]).powi(2) + (c[1] - p[1]).powi(2) + (c[2] - p[2]).powi(2);
                if d < best.0 {
                    best = (d, j as u8);
                }
            }
            *e = best.1;
        }
        let mut steps: Vec<f32> = pal
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
            let drift = t / 60.0 * period as f32; // one tile per minute
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
            let x = ((t / 40.0 + b.off).fract()) * (gw as f32 + 20.0 * unit) - 10.0 * unit;
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
            let x = (m.x + 3.0 * unit * (tau * (t / 12.0 * m.f + m.ph)).sin()).rem_euclid(gw as f32);
            let v = (tau * (t / 3.0 * m.f + m.ph)).sin();
            let (xi, yi) = (x as usize % gw, y as usize % gh);
            let i = yi * gw + xi;
            if v > -0.2 && !self.sky[i] {
                self.work[i] = mix(self.work[i], [255.0, 226.0, 170.0], 0.55 * (v + 0.2).min(1.0));
            }
        }

        // stylize: snap toward the dithered palette
        let s = self.look.stylize.clamp(0.0, 1.0);
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

        self.upscale(out);
        if self.look.ascii > 0.001 {
            self.ascii(out);
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
    fn ascii(&self, out: &mut [u32]) {
        let (sw, sh) = (self.sw, self.sh);
        let f = ((sh as f32 / 540.0).round() as usize).max(1);
        let cs = 8 * f;
        let k = self.look.ascii.clamp(0.0, 1.0);
        let (grid, gw, gh) = (&self.grid, self.gw, self.gh);
        out.par_chunks_mut(sw * cs).enumerate().for_each(|(cy, band)| {
            let rows = band.len() / sw;
            for cx in 0..sw.div_ceil(cs) {
                let px = (cx * cs + cs / 2).min(sw - 1);
                let py = (cy * cs + cs / 2).min(sh - 1);
                let c = unpack(grid[(py * gh / sh).min(gh - 1) * gw + (px * gw / sw).min(gw - 1)]);
                let l = lum(c) / 255.0;
                let gi = ((l * ASCII_RAMP.len() as f32) as usize).min(ASCII_RAMP.len() - 1);
                let glyph = font8x8::legacy::BASIC_LEGACY[ASCII_RAMP[gi] as usize];
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
