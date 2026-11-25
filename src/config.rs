//! Settings shared by the screensaver and the settings app, stored as JSON in
//! `%APPDATA%\nynex-screensaver\config.json` (or the platform equivalent).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransitionKind {
    Crossfade,
    Dissolve,
    Wipe,
    Slide,
    Pixelate,
    FadeThroughBlack,
    Random,
}

impl TransitionKind {
    pub const ALL: [TransitionKind; 7] = [
        TransitionKind::Crossfade,
        TransitionKind::Dissolve,
        TransitionKind::Wipe,
        TransitionKind::Slide,
        TransitionKind::Pixelate,
        TransitionKind::FadeThroughBlack,
        TransitionKind::Random,
    ];
    pub fn label(self) -> &'static str {
        match self {
            TransitionKind::Crossfade => "Crossfade",
            TransitionKind::Dissolve => "Pixel dissolve",
            TransitionKind::Wipe => "Wipe",
            TransitionKind::Slide => "Slide",
            TransitionKind::Pixelate => "Pixelate",
            TransitionKind::FadeThroughBlack => "Fade through black",
            TransitionKind::Random => "Random",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Landscape {
    pub path: PathBuf,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Seconds on screen; `None` uses the global default.
    #[serde(default)]
    pub duration: Option<f32>,
    /// This image's own look and motion settings; `None` uses the shared ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<Look>,
    /// This image's own logo settings (set together with `look`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<Logo>,
}

impl Landscape {
    pub fn name(&self) -> String {
        self.path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    /// 0 = full resolution, 1 = very chunky pixels.
    pub pixelation: f32,
    /// 0 = no ASCII, 1 = every cell drawn as a character.
    pub ascii: f32,
    /// 0 = the photo's own colours, 1 = warm graded, limited palette, dithered (the Claude FM look).
    pub stylize: f32,
    /// Palette size used by the stylized look.
    pub colors: u32,
    /// Ordered-dither strength for the stylized look.
    pub dither: f32,
    pub clouds: f32,
    pub water: bool,
    pub stars: bool,
    pub birds: u32,
    pub motes: u32,
    /// How often small runs of cells flicker into ASCII characters, 0 = never.
    pub glitch: f32,
    /// Number of twinkling sparkles on screen at once.
    pub sparkles: u32,
    /// 0 = still air, 1 = a strong breeze: faster clouds and birds, sideways dust, wind streaks.
    pub wind: f32,
    /// How often the picture tears open to show the program's own code underneath.
    pub breakthrough: f32,
}

/// `v` limited to `lo..=hi`, or `default` when it isn't a finite number.
fn fin(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { default }
}

impl Look {
    fn sanitize(&mut self) {
        let d = Look::default();
        self.pixelation = fin(self.pixelation, 0.0, 1.0, d.pixelation);
        self.ascii = fin(self.ascii, 0.0, 1.0, d.ascii);
        self.stylize = fin(self.stylize, 0.0, 1.0, d.stylize);
        self.colors = self.colors.clamp(2, 64);
        self.dither = fin(self.dither, 0.0, 2.0, d.dither);
        self.clouds = fin(self.clouds, 0.0, 1.0, d.clouds);
        self.birds = self.birds.min(64);
        self.motes = self.motes.min(400);
        self.glitch = fin(self.glitch, 0.0, 1.0, d.glitch);
        self.sparkles = self.sparkles.min(400);
        self.wind = fin(self.wind, 0.0, 1.0, d.wind);
        self.breakthrough = fin(self.breakthrough, 0.0, 1.0, d.breakthrough);
    }
}

impl Logo {
    fn sanitize(&mut self) {
        let d = Logo::default();
        self.size = fin(self.size, 0.01, 1.0, d.size);
        self.opacity = fin(self.opacity, 0.0, 1.0, d.opacity);
        self.pulse = fin(self.pulse, 0.0, 1.0, d.pulse);
    }
}

impl Default for Look {
    fn default() -> Self {
        Look {
            pixelation: 0.55,
            ascii: 0.0,
            stylize: 1.0,
            colors: 16,
            dither: 0.8,
            clouds: 0.5,
            water: true,
            stars: true,
            birds: 3,
            motes: 18,
            glitch: 0.15,
            sparkles: 10,
            wind: 0.3,
            breakthrough: 0.2,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Logo {
    pub enabled: bool,
    pub path: Option<PathBuf>,
    /// Height as a fraction of the screen height.
    pub size: f32,
    pub opacity: f32,
    /// Slow breathing, 0 = still.
    pub pulse: f32,
    /// Darker rim so the logo reads on bright skies.
    pub outline: bool,
}

impl Default for Logo {
    fn default() -> Self {
        Logo { enabled: false, path: None, size: 0.35, opacity: 0.85, pulse: 0.08, outline: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub landscapes: Vec<Landscape>,
    /// Seconds each landscape stays on screen unless it has its own value.
    pub default_duration: f32,
    pub shuffle: bool,
    pub transition: TransitionKind,
    /// Transition length in seconds.
    pub transition_secs: f32,
    pub look: Look,
    pub logo: Logo,
    pub fps: u32,
    /// Show a different landscape on each monitor.
    pub different_per_monitor: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            landscapes: Vec::new(),
            default_duration: 60.0,
            shuffle: false,
            transition: TransitionKind::Crossfade,
            transition_secs: 2.0,
            look: Look::default(),
            logo: Logo::default(),
            fps: 30,
            different_per_monitor: true,
        }
    }
}

fn yes() -> bool {
    true
}

pub fn app_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("nynex-screensaver")
}

/// Folder that holds copies of the landscapes the user added, so they keep working if the
/// originals are moved or deleted.
pub fn library_dir() -> PathBuf {
    app_dir().join("landscapes")
}

pub fn config_path() -> PathBuf {
    app_dir().join("config.json")
}

impl Config {
    pub fn load() -> Config {
        Self::load_from(&config_path())
    }

    /// Read a config file. A missing file gives the defaults. A file that exists but can't be
    /// parsed (hand-edited, truncated by a crash) is copied aside to `config.json.bad` first,
    /// so pressing Save in the app afterwards can't silently wipe the user's landscape list.
    /// Every value is then clamped to a sane range (see `sanitize`).
    pub fn load_from(path: &Path) -> Config {
        let Ok(text) = std::fs::read_to_string(path) else { return Config::default() };
        let mut cfg = match serde_json::from_str::<Config>(&text) {
            Ok(c) => c,
            Err(e) => {
                let bad = path.with_extension("json.bad");
                if !bad.exists() {
                    let _ = std::fs::copy(path, &bad);
                }
                eprintln!("{}: {e}; using the defaults (the file was kept as {})", path.display(), bad.display());
                Config::default()
            }
        };
        cfg.sanitize();
        cfg
    }

    /// Clamp every number to the range the app's sliders allow (with a little headroom), so a
    /// hand-edited or corrupted config can't ask for a billion birds, a zero-length frame or NaN.
    pub fn sanitize(&mut self) {
        self.default_duration = fin(self.default_duration, 1.0, 86_400.0, 60.0);
        self.transition_secs = fin(self.transition_secs, 0.05, 60.0, 2.0);
        self.fps = self.fps.clamp(5, 60);
        self.look.sanitize();
        self.logo.sanitize();
        for l in &mut self.landscapes {
            if let Some(d) = &mut l.duration {
                *d = fin(*d, 1.0, 86_400.0, 60.0);
            }
            if let Some(look) = &mut l.look {
                look.sanitize();
            }
            if let Some(logo) = &mut l.logo {
                logo.sanitize();
            }
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap())?;
        std::fs::rename(tmp, path)
    }

    pub fn enabled(&self) -> Vec<Landscape> {
        self.landscapes.iter().filter(|l| l.enabled && l.path.exists()).cloned().collect()
    }

    pub fn duration_of(&self, l: &Landscape) -> f32 {
        l.duration.unwrap_or(self.default_duration).max(1.0)
    }

    /// The look an image is shown with: its own if it has one, otherwise the shared one.
    pub fn look_of<'a>(&'a self, l: &'a Landscape) -> &'a Look {
        l.look.as_ref().unwrap_or(&self.look)
    }

    pub fn logo_of<'a>(&'a self, l: &'a Landscape) -> &'a Logo {
        match (&l.look, &l.logo) {
            (Some(_), Some(logo)) => logo,
            _ => &self.logo,
        }
    }
}

/// Copy an image into the library folder (unless it is already there) and return the new path.
pub fn import_image(src: &Path) -> std::io::Result<PathBuf> {
    let lib = library_dir();
    if src.starts_with(&lib) {
        return Ok(src.to_path_buf());
    }
    std::fs::create_dir_all(&lib)?;
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "landscape".into());
    let ext = src.extension().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_else(|| "png".into());
    let unique = |ext: &str| {
        let mut dst = lib.join(format!("{stem}.{ext}"));
        let mut n = 2;
        while dst.exists() {
            dst = lib.join(format!("{stem}-{n}.{ext}"));
            n += 1;
        }
        dst
    };
    // Big stills are stored downscaled: nothing on screen needs more than 4K, and huge photos
    // are slow to load (or too big to decode with the default limits). GIFs are kept as they
    // are; their frames are scaled when played.
    if ext != "gif" {
        if let Some(img) = decode_unlimited(src) {
            if img.width().max(img.height()) > MAX_IMPORT_SIDE {
                let small = img.resize(MAX_IMPORT_SIDE, MAX_IMPORT_SIDE, image::imageops::FilterType::Lanczos3);
                let dst = unique("jpg");
                let rgb = image::DynamicImage::ImageRgb8(small.to_rgb8());
                let file = std::fs::File::create(&dst)?;
                let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), 92);
                enc.encode_image(&rgb).map_err(std::io::Error::other)?;
                return Ok(dst);
            }
        }
    }
    let dst = unique(&ext);
    std::fs::copy(src, &dst)?;
    Ok(dst)
}

/// Longest side kept when importing an image.
pub const MAX_IMPORT_SIDE: u32 = 3840;

/// Decode an image without the decoder's default memory limits (very large photos).
pub fn decode_unlimited(path: &Path) -> Option<image::DynamicImage> {
    let mut r = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    r.no_limits();
    r.decode().ok()
}

pub const IMAGE_EXTENSIONS: [&str; 8] = ["jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_config_is_kept_and_values_are_clamped() {
        let dir = std::env::temp_dir().join(format!("nynex-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // broken JSON: defaults, and the original is kept next to it
        let path = dir.join("config.json");
        std::fs::write(&path, "{ \"landscapes\": [ oops").unwrap();
        let cfg = Config::load_from(&path);
        assert!(cfg.landscapes.is_empty());
        assert!(dir.join("config.json.bad").exists());
        // silly values are clamped
        std::fs::write(&path, r#"{"fps": 100000, "default_duration": -5, "look": {"birds": 4000000000, "pixelation": 7.5, "colors": 0}}"#).unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.fps, 60);
        assert_eq!(cfg.default_duration, 1.0);
        assert_eq!(cfg.look.birds, 64);
        assert_eq!(cfg.look.pixelation, 1.0);
        assert_eq!(cfg.look.colors, 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn big_images_are_downscaled_on_import() {
        let dir = std::env::temp_dir().join(format!("nynex-import-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("huge.png");
        image::RgbImage::from_pixel(5000, 2500, image::Rgb([90, 140, 200])).save(&src).unwrap();
        let out = import_image(&src).unwrap();
        let img = image::open(&out).unwrap();
        assert_eq!((img.width(), img.height()), (3840, 1920));
        assert_eq!(out.extension().unwrap(), "jpg");
        let _ = std::fs::remove_file(out);
        let _ = std::fs::remove_dir_all(dir);
    }
}
