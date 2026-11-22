//! Settings app (opened by Windows' "Settings" button, or by running the .scr directly):
//! manage the landscape list, timing, transitions, the look sliders and the logo, with a
//! live preview of the result.

use crate::config::{import_image, Config, TransitionKind, IMAGE_EXTENSIONS};
use crate::player::Player;
use crate::scene::pixel_size;
use eframe::egui;
use std::time::{Duration, Instant};

const PW: usize = 960;
const PH: usize = 540;

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("nynex")
            .with_app_id("nynex")
            .with_inner_size([1440.0, 880.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native("nynex", options, Box::new(|_cc| Ok(Box::new(SettingsApp::new()))))
}

struct SettingsApp {
    cfg: Config,
    saved: String,
    selected: Option<usize>,
    preview: Option<Player>,
    preview_key: String,
    dirty_since: Option<Instant>,
    frame: Vec<u32>,
    tex: Option<egui::TextureHandle>,
    last: Instant,
    status: String,
    /// Minutes of idle before Windows starts the screensaver.
    #[allow(dead_code)]
    wait_minutes: u32,
    #[allow(dead_code)]
    is_screensaver: bool,
    #[allow(dead_code)]
    wallpaper_autostart: bool,
    /// Linux: Omarchy's idle wait in seconds, when it can be changed from here.
    #[allow(dead_code)]
    idle_secs: Option<u32>,
}

impl SettingsApp {
    fn new() -> Self {
        let mut cfg = Config::load();
        // an image with its own look always carries its own logo settings too
        let shared_logo = cfg.logo.clone();
        for l in &mut cfg.landscapes {
            if l.look.is_some() && l.logo.is_none() {
                l.logo = Some(shared_logo.clone());
            }
        }
        let saved = serde_json::to_string(&cfg).unwrap_or_default();
        #[allow(unused_mut)]
        let mut app = SettingsApp {
            selected: if cfg.landscapes.is_empty() { None } else { Some(0) },
            cfg,
            saved,
            preview: None,
            preview_key: String::new(),
            dirty_since: None,
            frame: vec![0; PW * PH],
            tex: None,
            last: Instant::now(),
            status: String::new(),
            wait_minutes: 10,
            is_screensaver: false,
            wallpaper_autostart: false,
            idle_secs: None,
        };
        #[cfg(target_os = "linux")]
        {
            app.is_screensaver = crate::omarchy::screensaver_on();
            app.wallpaper_autostart = crate::omarchy::starts_at_login();
            app.idle_secs = crate::omarchy::idle_seconds();
            if let Some(s) = app.idle_secs {
                app.wait_minutes = (s / 60).max(1);
            }
        }
        #[cfg(windows)]
        {
            let (on, mins) = crate::install::status();
            app.is_screensaver = on;
            app.wallpaper_autostart = crate::wallpaper::starts_with_windows();
            app.wait_minutes = mins;
        }
        app
    }

    fn launch(&mut self, arg: &str) {
        match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg(arg).spawn()) {
            Ok(_) => {}
            Err(e) => self.status = format!("Could not start it: {e}"),
        }
    }

    /// Screensaver and wallpaper controls for Linux (Omarchy / Hyprland).
    #[cfg(target_os = "linux")]
    fn linux_controls(&mut self, ui: &mut egui::Ui) {
        use crate::omarchy;
        ui.separator();
        if omarchy::hook_installed() {
            let mut on = self.is_screensaver;
            if ui.checkbox(&mut on, "My screensaver").on_hover_text("Omarchy starts nynex instead of its text screensaver when the computer is idle.").changed() {
                self.save();
                self.status = omarchy::set_screensaver(on);
                self.is_screensaver = omarchy::screensaver_on();
            }
            if self.idle_secs.is_some() {
                ui.label("after");
                let r = ui.add(egui::DragValue::new(&mut self.wait_minutes).range(1..=180).suffix(" min"));
                if r.drag_stopped() || r.lost_focus() {
                    self.status = omarchy::set_idle_seconds(self.wait_minutes * 60);
                    self.idle_secs = omarchy::idle_seconds();
                }
            }
        } else {
            ui.label("Screensaver: run the Omarchy installer to hook it up").on_hover_text(format!("The installer puts {} in place.", omarchy::HOOK));
        }
        ui.separator();
        ui.label("Wallpaper");
        if ui.button("Start").on_hover_text("Saves, then plays nynex under your windows on every monitor.").clicked() {
            self.save();
            self.status = match crate::wallpaper::start() {
                Ok(()) => "Wallpaper started.".into(),
                Err(e) => format!("Could not start the wallpaper: {e}"),
            };
        }
        if ui.button("Stop").on_hover_text("Stops the animated wallpaper and shows your normal background again.").clicked() {
            crate::wallpaper::stop();
            self.status = "Wallpaper stopped.".into();
        }
        let mut auto = self.wallpaper_autostart;
        if ui.checkbox(&mut auto, "Start at login").on_hover_text("Adds nynex to your Hyprland autostart.").changed() && omarchy::set_start_at_login(auto) {
            self.wallpaper_autostart = auto;
        }
    }

    fn unsaved(&self) -> bool {
        serde_json::to_string(&self.cfg).unwrap_or_default() != self.saved
    }

    fn save(&mut self) {
        match self.cfg.save() {
            Ok(()) => {
                self.saved = serde_json::to_string(&self.cfg).unwrap_or_default();
                self.status = "Saved.".into();
            }
            Err(e) => self.status = format!("Could not save: {e}"),
        }
    }

    fn add_files(&mut self, files: Vec<std::path::PathBuf>) {
        let mut added = 0;
        for f in files {
            let ok = f.extension().map(|e| IMAGE_EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str())).unwrap_or(false);
            if !ok {
                continue;
            }
            match import_image(&f) {
                Ok(p) => {
                    self.cfg.landscapes.push(crate::config::Landscape { path: p, enabled: true, duration: None, look: None, logo: None });
                    added += 1;
                }
                Err(e) => self.status = format!("Could not copy {}: {e}", f.display()),
            }
        }
        if added > 0 {
            self.selected = Some(self.cfg.landscapes.len() - 1);
            self.status = format!("Added {added} landscape{}.", if added == 1 { "" } else { "s" });
        }
    }

    /// Index of the selected landscape among the enabled ones (what the preview plays).
    fn selected_in_enabled(&self) -> Option<usize> {
        let sel = self.selected?;
        let l = self.cfg.landscapes.get(sel)?;
        if !l.enabled {
            return None;
        }
        Some(self.cfg.landscapes[..sel].iter().filter(|l| l.enabled && l.path.exists()).count())
    }

    fn refresh_preview(&mut self) {
        // rebuild the preview shortly after the settings stop changing
        let key = serde_json::to_string(&(&self.cfg.landscapes, &self.cfg.look, &self.cfg.logo, self.cfg.shuffle)).unwrap_or_default();
        if key != self.preview_key {
            self.preview_key = key;
            self.dirty_since = Some(Instant::now());
        }
        if let Some(t) = self.dirty_since {
            if t.elapsed() > Duration::from_millis(180) || self.preview.is_none() {
                self.dirty_since = None;
                let mut cfg = self.cfg.clone();
                cfg.shuffle = false;
                let mut p = Player::new(cfg, PW, PH, 0, 1);
                if let Some(i) = self.selected_in_enabled() {
                    p.jump_to(i);
                }
                self.preview = Some(p);
            }
        }
        if let Some(p) = &mut self.preview {
            // timing and transition changes apply without a rebuild
            p.cfg.default_duration = self.cfg.default_duration;
            p.cfg.transition = self.cfg.transition;
            p.cfg.transition_secs = self.cfg.transition_secs;
            for (a, b) in p.cfg.landscapes.iter_mut().zip(self.cfg.landscapes.iter()) {
                a.duration = b.duration;
            }
        }
    }

    fn landscape_list(&mut self, ui: &mut egui::Ui) {
        ui.heading("Landscapes");
        ui.horizontal(|ui| {
            if ui.button("Add images…").clicked() {
                if let Some(files) = rfd::FileDialog::new().add_filter("Images", &IMAGE_EXTENSIONS).pick_files() {
                    self.add_files(files);
                }
            }
            if ui.button("Add folder…").clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    let mut files: Vec<_> = std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
                    files.sort();
                    self.add_files(files);
                }
            }
        });
        ui.label("Images are copied into the screensaver's own folder, so moving the originals is safe.");
        ui.add_space(6.0);
        let mut action: Option<(usize, i32)> = None; // (index, -1 up, +1 down, 0 remove)
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let n = self.cfg.landscapes.len();
            for i in 0..n {
                let missing = !self.cfg.landscapes[i].path.exists();
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.cfg.landscapes[i].enabled, "");
                    let mut name = self.cfg.landscapes[i].name();
                    if missing {
                        name.push_str(" (missing)");
                    }
                    if self.cfg.landscapes[i].look.is_some() {
                        name.push_str("  · own look");
                    }
                    if ui.selectable_label(self.selected == Some(i), name).clicked() {
                        self.selected = Some(i);
                        let e = self.selected_in_enabled_for(i);
                        if let (Some(p), Some(e)) = (&mut self.preview, e) {
                            p.jump_to(e);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(26.0);
                    let l = &mut self.cfg.landscapes[i];
                    let mut custom = l.duration.is_some();
                    if ui.checkbox(&mut custom, "own time").changed() {
                        l.duration = if custom { Some(self.cfg.default_duration) } else { None };
                    }
                    if let Some(d) = &mut l.duration {
                        ui.add(egui::DragValue::new(d).range(2.0..=7200.0).speed(1.0).suffix(" s"));
                    } else {
                        ui.weak(format!("{} s", self.cfg.default_duration.round()));
                    }
                    if ui.small_button("Up").on_hover_text("Move up").clicked() {
                        action = Some((i, -1));
                    }
                    if ui.small_button("Down").on_hover_text("Move down").clicked() {
                        action = Some((i, 1));
                    }
                    if ui.small_button("Remove").on_hover_text("Remove from the list").clicked() {
                        action = Some((i, 0));
                    }
                });
                ui.separator();
            }
            if n == 0 {
                ui.weak("No landscapes yet. Add some images to get started.");
            }
        });
        if let Some((i, dir)) = action {
            let n = self.cfg.landscapes.len();
            match dir {
                0 => {
                    self.cfg.landscapes.remove(i);
                    self.selected = if self.cfg.landscapes.is_empty() { None } else { Some(i.min(self.cfg.landscapes.len() - 1)) };
                }
                -1 if i > 0 => {
                    self.cfg.landscapes.swap(i, i - 1);
                    self.selected = Some(i - 1);
                }
                1 if i + 1 < n => {
                    self.cfg.landscapes.swap(i, i + 1);
                    self.selected = Some(i + 1);
                }
                _ => {}
            }
        }
    }

    /// Select the previous (-1) or next (+1) enabled landscape and show it in the preview.
    fn step_selection(&mut self, d: i32) {
        let n = self.cfg.landscapes.len();
        if n == 0 {
            return;
        }
        let mut i = self.selected.unwrap_or(0).min(n - 1);
        for _ in 0..n {
            i = (i as i32 + d).rem_euclid(n as i32) as usize;
            if let Some(e) = self.selected_in_enabled_for(i) {
                self.selected = Some(i);
                if let Some(p) = &mut self.preview {
                    p.jump_to(e);
                }
                return;
            }
        }
    }

    fn selected_in_enabled_for(&self, sel: usize) -> Option<usize> {
        let l = self.cfg.landscapes.get(sel)?;
        if !l.enabled || !l.path.exists() {
            return None;
        }
        Some(self.cfg.landscapes[..sel].iter().filter(|l| l.enabled && l.path.exists()).count())
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().slider_width = 190.0;
        let c = &mut self.cfg;
        egui::Grid::new("controls").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.strong("Display");
            ui.end_row();
            ui.label("Time per landscape");
            ui.add(egui::Slider::new(&mut c.default_duration, 2.0..=1800.0).logarithmic(true).suffix(" s"));
            ui.end_row();
            ui.label("Order");
            ui.horizontal(|ui| {
                ui.radio_value(&mut c.shuffle, false, "List order").on_hover_text("Plays top to bottom. Use Up and Down in the list to arrange them.");
                ui.radio_value(&mut c.shuffle, true, "Random");
            });
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut c.different_per_monitor, "Different landscape on each monitor");
            ui.end_row();

            ui.strong("Transition");
            ui.end_row();
            ui.label("Kind");
            egui::ComboBox::from_id_salt("transition").selected_text(c.transition.label()).show_ui(ui, |ui| {
                for k in TransitionKind::ALL {
                    ui.selectable_value(&mut c.transition, k, k.label());
                }
            });
            ui.end_row();
            ui.label("Length");
            ui.add(egui::Slider::new(&mut c.transition_secs, 0.2..=10.0).suffix(" s"));
            ui.end_row();

            ui.label("Frame rate");
            ui.add(egui::Slider::new(&mut c.fps, 10..=60).suffix(" fps"));
            ui.end_row();
        });

        // whose look the sliders below change: the selected image's own, or the shared one
        ui.add_space(8.0);
        ui.separator();
        let sel = self.selected.filter(|&i| i < self.cfg.landscapes.len());
        if let Some(i) = sel {
            let name = self.cfg.landscapes[i].name();
            let mut own = self.cfg.landscapes[i].look.is_some();
            if ui
                .checkbox(&mut own, format!("Give \"{name}\" its own look"))
                .on_hover_text("Its own pixelation, colours, motion and logo. The time per image is set in the list.")
                .changed()
            {
                let (look, logo) = (self.cfg.look.clone(), self.cfg.logo.clone());
                let l = &mut self.cfg.landscapes[i];
                if own {
                    l.look = Some(look);
                    l.logo = Some(logo);
                } else {
                    l.look = None;
                    l.logo = None;
                }
            }
            if own {
                ui.horizontal(|ui| {
                    ui.weak("The sliders below change only this image.");
                    if ui.small_button("Use for all images").on_hover_text("Copies this look to the shared settings.").clicked() {
                        let l = &self.cfg.landscapes[i];
                        if let Some(look) = &l.look {
                            self.cfg.look = look.clone();
                        }
                        if let Some(logo) = &l.logo {
                            self.cfg.logo = logo.clone();
                        }
                    }
                });
            } else {
                ui.weak("The sliders below change every image that doesn't have its own look.");
            }
        } else {
            ui.weak("Shared look for every image.");
        }
        ui.add_space(4.0);
        let shared_logo = self.cfg.logo.clone();
        let (look, logo) = match sel {
            Some(i) if self.cfg.landscapes[i].look.is_some() => {
                let l = &mut self.cfg.landscapes[i];
                (l.look.as_mut().unwrap(), l.logo.get_or_insert(shared_logo))
            }
            _ => (&mut self.cfg.look, &mut self.cfg.logo),
        };
        look_controls(ui, look, logo);
    }
}

fn look_controls(ui: &mut egui::Ui, look: &mut crate::config::Look, logo: &mut crate::config::Logo) {
    egui::Grid::new("look_controls").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.strong("Look");
            ui.end_row();
            ui.label("Pixelation");
            let px = pixel_size(look.pixelation);
            ui.add(egui::Slider::new(&mut look.pixelation, 0.0..=1.0).show_value(false))
                .on_hover_text("Left: full resolution. Right: big chunky pixels.");
            ui.end_row();
            ui.label("");
            ui.weak(if px < 1.5 { "full resolution".to_string() } else { format!("{px:.0} px blocks at 1080p") });
            ui.end_row();
            ui.label("ASCII");
            ui.add(egui::Slider::new(&mut look.ascii, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)))
                .on_hover_text("How much of the screen is drawn as text characters.");
            ui.end_row();
            ui.label("Full colour <-> Stylized");
            ui.add(egui::Slider::new(&mut look.stylize, 0.0..=1.0).show_value(false))
                .on_hover_text("Left: the photo's own colours. Right: warm, limited, dithered palette in the Claude FM style.");
            ui.end_row();
            ui.label("Palette colours");
            ui.add(egui::Slider::new(&mut look.colors, 4..=48));
            ui.end_row();
            ui.label("Dither");
            ui.add(egui::Slider::new(&mut look.dither, 0.0..=1.5));
            ui.end_row();

            ui.strong("Motion");
            ui.end_row();
            ui.label("Clouds");
            ui.add(egui::Slider::new(&mut look.clouds, 0.0..=1.0));
            ui.end_row();
            ui.label("Birds");
            ui.add(egui::Slider::new(&mut look.birds, 0..=12));
            ui.end_row();
            ui.label("Dust / fireflies");
            ui.add(egui::Slider::new(&mut look.motes, 0..=80));
            ui.end_row();
            ui.label("Sparkles");
            ui.add(egui::Slider::new(&mut look.sparkles, 0..=60));
            ui.end_row();
            ui.label("Wind");
            ui.add(egui::Slider::new(&mut look.wind, 0.0..=1.0));
            ui.end_row();
            ui.label("ASCII glitches");
            ui.add(egui::Slider::new(&mut look.glitch, 0.0..=1.0));
            ui.end_row();
            ui.label("Code breaking through");
            ui.add(egui::Slider::new(&mut look.breakthrough, 0.0..=1.0));
            ui.end_row();
            ui.label("");
            ui.horizontal(|ui| {
                ui.checkbox(&mut look.water, "Ripple lakes");
                ui.checkbox(&mut look.stars, "Stars at night");
            });
            ui.end_row();

            ui.strong("Logo");
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut logo.enabled, "Show a centred logo");
            ui.end_row();
            ui.label("Image");
            ui.horizontal(|ui| {
                if ui.button("Choose…").clicked() {
                    if let Some(f) = rfd::FileDialog::new().add_filter("Images", &IMAGE_EXTENSIONS).pick_file() {
                        logo.path = import_image(&f).ok().or(Some(f));
                        logo.enabled = true;
                    }
                }
                ui.weak(logo.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "none".into()));
            });
            ui.end_row();
            ui.label("Size");
            ui.add(egui::Slider::new(&mut logo.size, 0.05..=0.9).custom_formatter(|v, _| format!("{:.0}% of height", v * 100.0)));
            ui.end_row();
            ui.label("Opacity");
            ui.add(egui::Slider::new(&mut logo.opacity, 0.05..=1.0));
            ui.end_row();
            ui.label("Breathing");
            ui.add(egui::Slider::new(&mut logo.pulse, 0.0..=0.4));
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut logo.outline, "Darker rim (helps on bright skies)");
            ui.end_row();
        });
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let dropped: Vec<_> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if !dropped.is_empty() {
            self.add_files(dropped);
        }
        self.refresh_preview();
        // Left / Right step through the enabled landscapes, unless a slider or text box has
        // the keyboard
        if ctx.memory(|m| m.focused().is_none()) {
            let d = ctx.input(|i| (i.key_pressed(egui::Key::ArrowRight) as i32) - (i.key_pressed(egui::Key::ArrowLeft) as i32));
            if d != 0 {
                self.step_selection(d);
            }
        }

        egui::Panel::top("play").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("nynex");
                ui.separator();
                if ui.button("Play in a window").on_hover_text("Saves, then plays the animation in a normal window.").clicked() {
                    self.save();
                    self.launch("--window");
                }
                if ui.button("Play full screen").on_hover_text("Saves, then plays it full screen. Move the mouse or press a key to stop.").clicked() {
                    self.save();
                    self.launch("/s");
                }
                #[cfg(windows)]
                {
                    ui.separator();
                    ui.label("Start after");
                    ui.add(egui::DragValue::new(&mut self.wait_minutes).range(1..=180).suffix(" min"));
                    let label = if self.is_screensaver { "Update screensaver" } else { "Use as my screensaver" };
                    if ui.button(label).on_hover_text("Saves, then makes Windows start nynex when the PC is idle.").clicked() {
                        self.save();
                        self.status = crate::install::set_as_screensaver(self.wait_minutes);
                        self.is_screensaver = crate::install::status().0;
                    }
                    if self.is_screensaver && ui.button("Turn off").on_hover_text("Stops Windows from starting a screensaver.").clicked() {
                        self.status = crate::install::turn_off();
                        self.is_screensaver = crate::install::status().0;
                    }
                }
                #[cfg(windows)]
                {
                    ui.separator();
                    ui.label("Wallpaper");
                    if ui.button("Start").on_hover_text("Saves, then plays nynex behind your desktop icons on every monitor.").clicked() {
                        self.save();
                        self.status = match crate::wallpaper::start() {
                            Ok(()) => "Wallpaper started.".into(),
                            Err(e) => format!("Could not start the wallpaper: {e}"),
                        };
                    }
                    if ui.button("Stop").on_hover_text("Stops the animated wallpaper and brings back your normal one.").clicked() {
                        crate::wallpaper::stop();
                        self.status = "Wallpaper stopped.".into();
                    }
                    let mut auto = self.wallpaper_autostart;
                    if ui.checkbox(&mut auto, "Start with Windows").on_hover_text("Starts the animated wallpaper when you sign in.").changed() {
                        if crate::wallpaper::set_start_with_windows(auto) {
                            self.wallpaper_autostart = auto;
                        }
                    }
                    ui.separator();
                }
                #[cfg(target_os = "linux")]
                self.linux_controls(ui);
                #[cfg(windows)]
                ui.label(if self.is_screensaver {
                    format!("Your screensaver, after {} min idle", self.wait_minutes)
                } else {
                    "Not your screensaver yet".to_string()
                });
            });
            ui.add_space(6.0);
        });

        egui::Panel::bottom("buttons").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let label = if self.unsaved() { "Save *" } else { "Save" };
                if ui.button(label).clicked() {
                    self.save();
                }
                if ui.button("Revert").clicked() {
                    self.cfg = Config::load();
                    self.status = "Reverted to the saved settings.".into();
                }
                ui.label(&self.status);
            });
            ui.add_space(6.0);
        });

        egui::Panel::left("list").resizable(true).default_size(330.0).show(ui, |ui| {
            self.landscape_list(ui);
        });

        egui::Panel::right("controls").resizable(true).default_size(430.0).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.controls(ui));
        });

        egui::CentralPanel::default().show(ui, |ui| {
            // live preview
            let now = Instant::now();
            let dt = (now - self.last).as_secs_f32().min(0.1);
            self.last = now;
            if let Some(p) = &mut self.preview {
                p.update(dt, &mut self.frame);
            }
            let mut rgb = Vec::with_capacity(PW * PH * 3);
            for &px in &self.frame {
                rgb.extend_from_slice(&[(px >> 16) as u8, (px >> 8) as u8, px as u8]);
            }
            let img = egui::ColorImage::from_rgb([PW, PH], &rgb);
            match &mut self.tex {
                Some(t) => t.set(img, egui::TextureOptions::NEAREST),
                None => self.tex = Some(ctx.load_texture("preview", img, egui::TextureOptions::NEAREST)),
            }
            let avail_w = ui.available_width();
            let avail_h = (ui.available_height() - 40.0).max(100.0);
            let w = avail_w.min(avail_h * PW as f32 / PH as f32);
            if let Some(t) = &self.tex {
                ui.add(egui::Image::new(t).fit_to_exact_size(egui::vec2(w, w * PH as f32 / PW as f32)));
            }
            ui.horizontal(|ui| {
                if ui.button("Next landscape >").on_hover_text("Plays the transition in the preview").clicked() {
                    if let Some(p) = &mut self.preview {
                        p.advance();
                    }
                }
                ui.weak("Preview at reduced size. Drop image files anywhere on this window to add them.");
            });
        });

        // the preview is small, so 30 frames a second is plenty and keeps this window light
        ctx.request_repaint_after(Duration::from_secs_f32(1.0 / self.cfg.fps.clamp(10, 30) as f32));
    }
}
