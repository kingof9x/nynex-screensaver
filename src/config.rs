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

    pub fn load_from(path: &Path) -> Config {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
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
}

/// Copy an image into the library folder (unless it is already there) and return the new path.
pub fn import_image(src: &Path) -> std::io::Result<PathBuf> {
    let lib = library_dir();
    if src.starts_with(&lib) {
        return Ok(src.to_path_buf());
    }
    std::fs::create_dir_all(&lib)?;
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "landscape".into());
    let ext = src.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "png".into());
    let mut dst = lib.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while dst.exists() {
        dst = lib.join(format!("{stem}-{n}.{ext}"));
        n += 1;
    }
    std::fs::copy(src, &dst)?;
    Ok(dst)
}

pub const IMAGE_EXTENSIONS: [&str; 8] = ["jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff"];
