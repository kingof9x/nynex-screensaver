//! Stress tests: odd screen sizes, random settings, broken image files, long runs.
//!
//! The quick ones run with `cargo test`. The slow ones are `#[ignore]`d; run them with
//! `cargo test --release stress -- --ignored --nocapture`.

use crate::config::{Config, Landscape, Logo, Look, TransitionKind};
use crate::player::Player;
use crate::scene::{bench::test_source, Scene};
use std::path::{Path, PathBuf};

/// A tiny deterministic random number generator (xorshift), so failures can be replayed.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn f(&mut self) -> f32 {
        (self.next() % 10_001) as f32 / 10_000.0
    }
    fn n(&mut self, max: u32) -> u32 {
        (self.next() % (max as u64 + 1)) as u32
    }
}

fn random_look(r: &mut Rng) -> Look {
    Look {
        pixelation: r.f(),
        ascii: if r.f() < 0.3 { 0.0 } else { r.f() },
        stylize: r.f(),
        colors: 2 + r.n(62),
        dither: r.f() * 2.0,
        clouds: r.f(),
        water: r.f() < 0.5,
        stars: r.f() < 0.5,
        birds: r.n(12),
        motes: r.n(80),
        glitch: r.f(),
        sparkles: r.n(60),
        wind: r.f(),
        breakthrough: r.f(),
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nynex-stress-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Screen sizes a window or monitor can really have, including silly ones mid-resize.
const SIZES: [(usize, usize); 8] = [(1, 1), (8, 8), (15, 600), (600, 15), (152, 112), (333, 777), (1366, 768), (1920, 1080)];

/// Every size and a spread of random settings must render without panicking, and the
/// frame must be fully written (no stale pixels from a previous size).
#[test]
fn odd_sizes_and_random_settings_never_panic() {
    let src = test_source();
    let mut r = Rng(0x5eed_1234);
    for &(w, h) in &SIZES {
        for k in 0..3 {
            let look = if k == 0 { Look::default() } else { random_look(&mut r) };
            let mut sc = Scene::new(&src, None, w, h, &look, &Logo::default(), k);
            // the scene works at its own (possibly enlarged) size
            let mut out = vec![0xdead_beef_u32; sc.sw * sc.sh];
            for i in 0..3 {
                sc.render(i as f32 * 0.7, &mut out);
            }
            assert!(out.iter().all(|&p| p >> 24 == 0), "{w}x{h}: high byte set, so a pixel was never written");
        }
    }
}

/// Every transition at odd sizes, start to finish.
#[test]
fn every_transition_at_odd_sizes() {
    for &(sw, sh) in &[(8usize, 8usize), (15, 600), (640, 360)] {
        let a: Vec<u32> = (0..sw * sh).map(|i| i as u32 & 0xffffff).collect();
        let b: Vec<u32> = (0..sw * sh).map(|i| !(i as u32) & 0xffffff).collect();
        let mut out = vec![0u32; sw * sh];
        for kind in TransitionKind::ALL {
            for step in 0..=20 {
                crate::player::blend(kind, step as f32 / 20.0, &a, &b, &mut out, sw, sh, 9);
            }
        }
    }
}

/// Writes a set of broken "images" next to two good ones.
fn broken_and_good_images(dir: &Path) -> Vec<PathBuf> {
    let good1 = dir.join("good1.png");
    let good2 = dir.join("good2.png");
    image::RgbImage::from_fn(320, 180, |x, y| image::Rgb([x as u8, y as u8, 90])).save(&good1).unwrap();
    image::RgbImage::from_fn(320, 180, |x, y| image::Rgb([90, x as u8, y as u8])).save(&good2).unwrap();
    let empty = dir.join("empty.jpg");
    std::fs::write(&empty, b"").unwrap();
    let text = dir.join("notes.png");
    std::fs::write(&text, b"this is not a picture").unwrap();
    // the first half of a real PNG
    let half = dir.join("truncated.png");
    let bytes = std::fs::read(&good1).unwrap();
    std::fs::write(&half, &bytes[..bytes.len() / 2]).unwrap();
    let gif = dir.join("garbage.gif");
    std::fs::write(&gif, b"GIF89a\x01\x00\x01\x00garbage").unwrap();
    vec![empty, text, good1, half, gif, good2]
}

fn config_with(paths: &[PathBuf]) -> Config {
    Config {
        landscapes: paths.iter().map(|p| Landscape { path: p.clone(), enabled: true, duration: Some(1.0), look: None, logo: None }).collect(),
        transition_secs: 0.2,
        transition: TransitionKind::Random,
        ..Default::default()
    }
}

/// Broken files in the list are skipped: the slideshow starts on a good image and keeps
/// moving between the good ones instead of getting stuck.
#[test]
fn broken_images_are_skipped() {
    let dir = temp_dir("broken");
    let paths = broken_and_good_images(&dir);
    let cfg = config_with(&paths);
    let (sw, sh) = (160, 90);
    let mut p = Player::new(cfg, sw, sh, 0, 1);
    let mut out = vec![0u32; sw * sh];
    let mut frames_seen = std::collections::HashSet::new();
    // 12 seconds of play at 20 fps with 1 s per image: several transitions must happen
    for _ in 0..240 {
        p.update(0.05, &mut out);
        frames_seen.insert(out[sw * sh / 2] ^ out[sw / 3]);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(p.position() == 2 || p.position() == 5, "showing a broken image: position {}", p.position());
    assert!(p.transitions() >= 3, "only {} transitions: the slideshow got stuck", p.transitions());
    let _ = std::fs::remove_dir_all(dir);
}

/// A list where nothing loads shows the error message instead of crashing or spinning.
#[test]
fn nothing_loads() {
    let dir = temp_dir("nothing");
    let paths: Vec<PathBuf> = broken_and_good_images(&dir).into_iter().filter(|p| !p.to_string_lossy().contains("good")).collect();
    let mut p = Player::new(config_with(&paths), 160, 90, 0, 1);
    let mut out = vec![0u32; 160 * 90];
    for _ in 0..50 {
        p.update(0.05, &mut out);
    }
    assert!(out.contains(&0xf6ecdc), "no message drawn");
    let _ = std::fs::remove_dir_all(dir);
}

/// Arrow keys hammered during transitions and loads.
#[test]
fn stepping_back_and_forth() {
    let dir = temp_dir("step");
    let paths = broken_and_good_images(&dir);
    let mut p = Player::new(config_with(&paths), 160, 90, 0, 1);
    let mut out = vec![0u32; 160 * 90];
    let mut r = Rng(77);
    for i in 0..200 {
        p.update(0.07, &mut out);
        if i % 3 == 0 {
            p.step(if r.f() < 0.5 { -1 } else { 1 });
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Resident memory of this process in MB (Linux), for the long-run checks below.
fn rss_mb() -> f64 {
    let s = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: f64 = s.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    pages * 4096.0 / 1_048_576.0
}

/// An animated GIF with many big frames must load within the memory budget.
/// `cargo test --release stress_big_gif -- --ignored --nocapture`
#[test]
#[ignore]
fn stress_big_gif() {
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame, RgbaImage};
    let dir = temp_dir("gif");
    let path = dir.join("big.gif");
    {
        let file = std::fs::File::create(&path).unwrap();
        let mut enc = GifEncoder::new_with_speed(std::io::BufWriter::new(file), 30);
        enc.set_repeat(Repeat::Infinite).unwrap();
        // 600 frames of 1280x720: 1.6 GB if every frame were kept as RGB
        for i in 0..600u32 {
            let img = RgbaImage::from_fn(1280, 720, |x, y| image::Rgba([((x + i * 4) / 5) as u8, (y / 3) as u8, (i % 256) as u8, 255]));
            enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(40, 1))).unwrap();
        }
    }
    let before = rss_mb();
    let t0 = std::time::Instant::now();
    let src = crate::scene::load_source(&path).unwrap();
    let total: f32 = src.frames.iter().map(|f| f.1).sum();
    println!(
        "big gif: {} frames kept, {:.1} s loop, {:.0} MB held, loaded in {:.1} s, RSS {:.0} -> {:.0} MB",
        src.frames.len(),
        total,
        src.frames.iter().map(|f| f.0.as_raw().len()).sum::<usize>() as f64 / 1_048_576.0,
        t0.elapsed().as_secs_f32(),
        before,
        rss_mb()
    );
    assert!((total - 24.0).abs() < 0.5, "loop length changed: {total}");
    assert!(src.frames.iter().map(|f| f.0.as_raw().len()).sum::<usize>() <= 200 * 1024 * 1024);
    let _ = std::fs::remove_dir_all(dir);
}

/// Thousands of random scenes and frames at random sizes.
/// `cargo test --release stress_fuzz -- --ignored --nocapture`
#[test]
#[ignore]
fn stress_fuzz() {
    let src = test_source();
    let mut r = Rng(0xfeed_beef);
    let t0 = std::time::Instant::now();
    let mut frames = 0;
    for k in 0..300 {
        let (w, h) = (1 + r.n(1919) as usize, 1 + r.n(1199) as usize);
        let look = random_look(&mut r);
        let logo = Logo { enabled: r.f() < 0.3, ..Logo::default() };
        let mut sc = Scene::new(&src, None, w, h, &look, &logo, k);
        let mut out = vec![0u32; sc.sw * sc.sh];
        for i in 0..10 {
            sc.render_damage(r.f() * 3600.0 + i as f32, &mut out, i > 0);
            frames += 1;
        }
    }
    println!("fuzz: 300 scenes, {frames} frames in {:.1} s, RSS {:.0} MB", t0.elapsed().as_secs_f32(), rss_mb());
}

/// A long slideshow with fast transitions: memory must level off, not keep growing.
/// `cargo test --release stress_long_run -- --ignored --nocapture`
/// Defaults to 20 minutes of slideshow at 1920x1080; `NYNEX_STRESS_MINUTES` and
/// `NYNEX_STRESS_SIZE` (e.g. `960x540`) change that.
#[test]
#[ignore]
fn stress_long_run() {
    let dir = temp_dir("long");
    let mut paths = Vec::new();
    for i in 0..12u32 {
        let p = dir.join(format!("img{i}.jpg"));
        image::RgbImage::from_fn(3000, 2000, |x, y| image::Rgb([((x + i * 50) / 12) as u8, (y / 8) as u8, ((x ^ y) / 16) as u8])).save(&p).unwrap();
        paths.push(p);
    }
    let mut cfg = config_with(&paths);
    for l in &mut cfg.landscapes {
        l.duration = Some(1.5);
    }
    let (sw, sh) = std::env::var("NYNEX_STRESS_SIZE")
        .ok()
        .and_then(|v| v.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))))
        .unwrap_or((1920usize, 1080usize));
    let minutes: usize = std::env::var("NYNEX_STRESS_MINUTES").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    let mut p = Player::new(cfg, sw, sh, 0, 3);
    let mut out = vec![0u32; sw * sh];
    let mut samples = Vec::new();
    let t0 = std::time::Instant::now();
    // slideshow time at 30 fps, drawn as fast as possible
    let frames = minutes * 60 * 30;
    for i in 0..frames {
        p.update_damage(1.0 / 30.0, &mut out, i > 0);
        if i % 1800 == 0 {
            samples.push(rss_mb());
        }
    }
    let secs = t0.elapsed().as_secs_f64();
    println!(
        "long run: {frames} frames ({} transitions) in {secs:.0} s = {:.1} ms/frame; RSS samples MB: {:?}",
        p.transitions(),
        secs * 1000.0 / frames as f64,
        samples.iter().map(|v| v.round() as i64).collect::<Vec<_>>()
    );
    let (first_half, second_half) = samples.split_at(samples.len() / 2);
    let max1 = first_half.iter().cloned().fold(0.0, f64::max);
    let max2 = second_half.iter().cloned().fold(0.0, f64::max);
    assert!(max2 < max1 * 1.3 + 20.0, "memory keeps growing: {max1:.0} MB then {max2:.0} MB");
    let _ = std::fs::remove_dir_all(dir);
}
