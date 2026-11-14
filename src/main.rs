//! nynex screensaver: animated pixel-art / ASCII landscapes for Windows.
//!
//! Windows runs a screensaver (.scr) with one of these arguments:
//!   /s           run full screen
//!   /p <hwnd>    draw inside the little preview box of Screen Saver Settings
//!   /c[:<hwnd>]  open the settings
//! With no arguments (double-clicking nynex.exe) it opens the nynex app, which is where
//! everything is set up: landscapes, look, playing it, and making it the screensaver.
//!
//! Extra options for trying it out:
//!   --window                       run in a normal window
//!   --render <image> <out.png> [--seconds N] [--size WxH] [--config file.json]
//!                                  render a single frame to a PNG (no window needed)

#![cfg_attr(windows, windows_subsystem = "windows")]

mod config;
#[cfg(windows)]
mod install;
mod player;
mod saver;
mod scene;
mod settings;

fn main() {
    // A GUI program has no console, so write crashes to a log the user can send us.
    std::panic::set_hook(Box::new(|info| {
        let dir = config::app_dir();
        let _ = std::fs::create_dir_all(&dir);
        let msg = format!("{:?} {info}\n", std::time::SystemTime::now());
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crash.log")).and_then(|mut f| {
            use std::io::Write;
            f.write_all(msg.as_bytes())
        });
        eprintln!("{msg}");
    }));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = args.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    // Windows may pass "/p 1234", "/p:1234", "-p 1234" and so on.
    let (flag, inline) = match first.trim_start_matches(['/', '-']).split_once(':') {
        Some((f, v)) => (f.to_string(), Some(v.to_string())),
        None => (first.trim_start_matches(['/', '-']).to_string(), None),
    };
    match flag.as_str() {
        "s" => saver::run(saver::Mode::Full),
        "p" | "l" => {
            let hwnd = inline.or_else(|| args.get(1).cloned()).and_then(|v| v.trim().parse::<isize>().ok());
            match hwnd {
                Some(h) => saver::run(saver::Mode::Preview(h)),
                None => saver::run(saver::Mode::Windowed),
            }
        }
        "window" => saver::run(saver::Mode::Windowed),
        "render" => render_cli(&args[1..]),
        _ => {
            if let Err(e) = settings::run() {
                eprintln!("settings window failed: {e}");
            }
        }
    }
}

/// Render one frame of one image to a PNG, using the saved settings (or a given config file).
fn render_cli(args: &[String]) {
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else {
        eprintln!("usage: nynex --render <image> <out.png> [--seconds N] [--size WxH] [--config file.json]");
        std::process::exit(2);
    };
    let mut t = 3.0f32;
    let (mut w, mut h) = (1920usize, 1080usize);
    let mut cfg = config::Config::load();
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--seconds" => t = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(t),
            "--size" => {
                if let Some((a, b)) = args.get(i + 1).and_then(|v| v.split_once('x')) {
                    w = a.parse().unwrap_or(w);
                    h = b.parse().unwrap_or(h);
                }
            }
            "--config" => cfg = config::Config::load_from(std::path::Path::new(&args[i + 1])),
            _ => {}
        }
        i += 2;
    }
    let src = match scene::load_source(std::path::Path::new(input)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let logo = if cfg.logo.enabled { cfg.logo.path.as_deref().and_then(scene::load_logo) } else { None };
    let mut sc = scene::Scene::new(&src, logo.as_deref(), w, h, &cfg.look, &cfg.logo, 1);
    let mut buf = vec![0u32; w * h];
    sc.render(t, &mut buf);
    let mut img = image::RgbImage::new(w as u32, h as u32);
    for (p, &v) in img.pixels_mut().zip(buf.iter()) {
        *p = image::Rgb([(v >> 16) as u8, (v >> 8) as u8, v as u8]);
    }
    if let Err(e) = img.save(output) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
