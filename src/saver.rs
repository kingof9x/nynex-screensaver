//! The screensaver itself: one borderless fullscreen window per monitor (or a child window
//! inside the Screen Saver Settings preview box), drawn with a CPU framebuffer.

use crate::config::Config;
use crate::player::Player;
use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Fullscreen, Window, WindowId, WindowLevel};

pub enum Mode {
    /// Full screen on every monitor; any input exits.
    Full,
    /// Inside the preview box of Windows' Screen Saver Settings (parent window handle).
    Preview(isize),
    /// A normal resizable window, for trying it out (`--window`).
    Windowed,
    /// Behind the desktop icons on every monitor, until told to stop (`--wallpaper`).
    /// (Linux uses layer.rs for the wallpaper instead.)
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Wallpaper,
    /// One full-screen window on whichever monitor the window manager puts it; any input
    /// exits (`--screensaver`). Omarchy's screensaver launcher starts one per monitor.
    Single {
        /// Which landscape to start on when "different per monitor" is on.
        index: usize,
        /// Wayland app id / X11 class, so window rules (and Omarchy's idle handling) find it.
        app_id: String,
    },
}

struct Win {
    window: Rc<Window>,
    surface: Surface<Rc<Window>, Rc<Window>>,
    /// Built on the first redraw, after the window has been painted black, so a slow image
    /// load never leaves a blank white window on screen.
    player: Option<Player>,
    index: usize,
    seed: u32,
    size: (u32, u32),
    last: Instant,
    first_cursor: Option<PhysicalPosition<f64>>,
    /// Wallpaper: this window's screen rectangle (left, top, right, bottom).
    screen: (i32, i32, i32, i32),
    /// Wallpaper: hidden behind a maximised or full-screen window, so not worth drawing.
    covered: bool,
    /// Frames since the whole window was last sent; a full frame goes out now and then anyway.
    partial_frames: u32,
    /// When the window last changed size. Building a scene means decoding and analysing the
    /// image, so while a window is being dragged bigger or smaller it is only painted dark,
    /// and the scene is built once the size has stayed put for a moment.
    resized_at: Option<Instant>,
}

struct App {
    mode: Mode,
    cfg: Config,
    wins: Vec<Win>,
    started: Instant,
    next_frame: Instant,
    /// When every window has shown its first real frame. Input only ends the screensaver
    /// from a short while after this, so the mouse "moves" Windows reports while the
    /// fullscreen windows appear don't close it straight away.
    armed_at: Option<Instant>,
    /// Cursor position on screen when the screensaver armed (Windows).
    cursor_home: Option<(i32, i32)>,
    last_stop_check: Instant,
}

/// -1 for the Left arrow, +1 for Right, None for any other key.
fn arrow_step(key: &winit::keyboard::Key) -> Option<isize> {
    use winit::keyboard::{Key, NamedKey};
    match key {
        Key::Named(NamedKey::ArrowLeft) => Some(-1),
        Key::Named(NamedKey::ArrowRight) => Some(1),
        _ => None,
    }
}

/// Pixels the mouse has to travel before the screensaver closes.
const MOVE_TO_QUIT: f64 = 24.0;

/// How long a window's size has to stay the same before its scene is rebuilt.
const RESIZE_SETTLE: Duration = Duration::from_millis(200);

pub fn run(mode: Mode) {
    let cfg = Config::load();
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App { mode, cfg, wins: Vec::new(), started: Instant::now(), next_frame: Instant::now(), armed_at: None, cursor_home: None, last_stop_check: Instant::now() };
    let _ = event_loop.run_app(&mut app);
}

impl App {
    fn add_window(&mut self, el: &ActiveEventLoop, attrs: winit::window::WindowAttributes, index: usize) {
        let window = match el.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(_) => return,
        };
        if self.quit_on_input() {
            window.set_cursor_visible(false);
        }
        let ctx = Context::new(window.clone()).expect("softbuffer context");
        let surface = Surface::new(&ctx, window.clone()).expect("softbuffer surface");
        let sz = window.inner_size();
        let (w, h) = (sz.width.max(1), sz.height.max(1));
        let seed = 0x5eed_u32.wrapping_add(index as u32 * 7919).wrapping_add(self.started.elapsed().subsec_nanos());
        let screen = (0, 0, 0, 0);
        self.wins.push(Win { window, surface, player: None, index, seed, size: (w, h), last: Instant::now(), first_cursor: None, screen, covered: false, partial_frames: 0, resized_at: None });
    }

    fn quit_on_input(&self) -> bool {
        matches!(self.mode, Mode::Full | Mode::Single { .. })
    }

    /// True once input should close the screensaver.
    fn armed(&self) -> bool {
        self.armed_at.map(|t| t.elapsed() > Duration::from_millis(800)).unwrap_or(false)
    }
}

fn raw_hwnd(w: &Window) -> Option<isize> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match w.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

#[cfg(windows)]
fn cursor_pos() -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
}

/// True when the active window hides the screen rectangle `r` (it is maximised on that
/// screen, or covers all of it), so an animated wallpaper there can't be seen.
#[cfg(windows)]
fn covered_by_foreground(r: (i32, i32, i32, i32)) -> bool {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() || IsIconic(fg) != 0 || IsWindowVisible(fg) == 0 {
            return false;
        }
        let mut cls = [0u16; 64];
        let n = GetClassNameW(fg, cls.as_mut_ptr(), cls.len() as i32).max(0) as usize;
        let name = String::from_utf16_lossy(&cls[..n]);
        if matches!(name.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return false;
        }
        let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(fg, &mut wr) == 0 {
            return false;
        }
        let (cx, cy) = ((wr.left + wr.right) / 2, (wr.top + wr.bottom) / 2);
        let on_this_screen = cx >= r.0 && cx < r.2 && cy >= r.1 && cy < r.3;
        let covers = wr.left <= r.0 && wr.top <= r.1 && wr.right >= r.2 && wr.bottom >= r.3;
        covers || (on_this_screen && IsZoomed(fg) != 0)
    }
}

#[cfg(not(windows))]
fn covered_by_foreground(_r: (i32, i32, i32, i32)) -> bool {
    false
}

#[cfg(not(windows))]
fn cursor_pos() -> Option<(i32, i32)> {
    None
}

/// Give the window an app id (Wayland) / class (X11) that window rules can match.
#[cfg(all(unix, not(target_os = "macos")))]
fn with_app_id(attrs: winit::window::WindowAttributes, app_id: &str) -> winit::window::WindowAttributes {
    use winit::platform::wayland::WindowAttributesExtWayland;
    WindowAttributesExtWayland::with_name(attrs, app_id, app_id)
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn with_app_id(attrs: winit::window::WindowAttributes, _app_id: &str) -> winit::window::WindowAttributes {
    attrs
}

#[cfg(windows)]
fn parent_size(hwnd: isize) -> (u32, u32) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(hwnd as _, &mut r) };
    (((r.right - r.left).max(1)) as u32, ((r.bottom - r.top).max(1)) as u32)
}

#[cfg(windows)]
fn parent_alive(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;
    unsafe { IsWindow(hwnd as _) != 0 }
}

#[cfg(not(windows))]
fn parent_size(_hwnd: isize) -> (u32, u32) {
    (152, 112)
}
#[cfg(not(windows))]
fn parent_alive(_hwnd: isize) -> bool {
    true
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if !self.wins.is_empty() {
            return;
        }
        match self.mode {
            Mode::Full => {
                let monitors: Vec<_> = el.available_monitors().collect();
                for (i, m) in monitors.iter().enumerate() {
                    let attrs = with_app_id(Window::default_attributes(), "nynex")
                        .with_title("nynex screensaver")
                        .with_decorations(false)
                        .with_window_level(WindowLevel::AlwaysOnTop)
                        .with_position(m.position())
                        .with_inner_size(m.size())
                        .with_fullscreen(Some(Fullscreen::Borderless(Some(m.clone()))));
                    self.add_window(el, attrs, i);
                }
                if self.wins.is_empty() {
                    let attrs = Window::default_attributes().with_fullscreen(Some(Fullscreen::Borderless(None)));
                    self.add_window(el, attrs, 0);
                }
            }
            Mode::Single { index, ref app_id } => {
                let app_id = app_id.clone();
                let attrs = with_app_id(Window::default_attributes(), &app_id)
                    .with_title("nynex screensaver")
                    .with_decorations(false)
                    .with_fullscreen(Some(Fullscreen::Borderless(None)));
                self.add_window(el, attrs, index);
            }
            Mode::Wallpaper => {
                let _ = std::fs::remove_file(crate::wallpaper::stop_file());
                let monitors: Vec<_> = el.available_monitors().collect();
                for (i, m) in monitors.iter().enumerate() {
                    let attrs = Window::default_attributes()
                        .with_title("nynex wallpaper")
                        .with_decorations(false)
                        .with_position(m.position())
                        .with_inner_size(m.size());
                    self.add_window(el, attrs, i);
                    if let Some(w) = self.wins.last_mut() {
                        let (p, s) = (m.position(), m.size());
                        w.screen = (p.x, p.y, p.x + s.width as i32, p.y + s.height as i32);
                        let result = match raw_hwnd(&w.window) {
                            Some(h) => crate::wallpaper::attach(h, p.x, p.y, s.width as i32, s.height as i32),
                            None => Err("No window handle.".into()),
                        };
                        let log = crate::config::app_dir().join("wallpaper.log");
                        match result {
                            Ok(msg) => {
                                let mut text = if i == 0 { String::new() } else { std::fs::read_to_string(&log).unwrap_or_default() };
                                text.push_str(&format!("monitor {}: {}\n", i + 1, msg));
                                let _ = std::fs::write(&log, text);
                            }
                            Err(why) => {
                                // without the desktop layer we'd just be a window covering the
                                // desktop, so give up and say why
                                let _ = std::fs::write(&log, format!("monitor {}: {} The wallpaper did not start.\n", i + 1, why));
                                el.exit();
                                return;
                            }
                        }
                    }
                }
            }
            Mode::Preview(hwnd) => {
                let (w, h) = parent_size(hwnd);
                #[allow(unused_mut)]
                let mut attrs = Window::default_attributes()
                    .with_decorations(false)
                    .with_position(PhysicalPosition::new(0, 0))
                    .with_inner_size(PhysicalSize::new(w, h));
                #[cfg(windows)]
                {
                    use winit::raw_window_handle::{RawWindowHandle, Win32WindowHandle};
                    if let Some(nz) = std::num::NonZeroIsize::new(hwnd) {
                        let handle = RawWindowHandle::Win32(Win32WindowHandle::new(nz));
                        attrs = unsafe { attrs.with_parent_window(Some(handle)) };
                    }
                }
                self.add_window(el, attrs, 0);
            }
            Mode::Windowed => {
                let attrs = with_app_id(Window::default_attributes(), "nynex")
                    .with_title("nynex screensaver")
                    .with_inner_size(PhysicalSize::new(1280u32, 720u32));
                self.add_window(el, attrs, 0);
            }
        }
        self.next_frame = Instant::now();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let quit = self.quit_on_input() && self.armed();
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => el.exit(),
            // Left / Right step through the landscapes (in the window and in the screensaver);
            // any other key closes the screensaver
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && arrow_step(&event.logical_key).is_some() => {
                if !event.repeat {
                    let d = arrow_step(&event.logical_key).unwrap_or(0);
                    for w in self.wins.iter_mut() {
                        if let Some(p) = &mut w.player {
                            p.step(d);
                        }
                        w.last = Instant::now();
                        w.window.request_redraw();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } if quit && event.state == ElementState::Pressed => el.exit(),
            WindowEvent::MouseInput { state: ElementState::Pressed, .. } if quit => el.exit(),
            WindowEvent::MouseWheel { .. } if quit => el.exit(),
            // On Windows the cursor is checked in screen coordinates in about_to_wait, which
            // isn't fooled by the windows moving into place. Elsewhere, use window events.
            WindowEvent::CursorMoved { position, .. } if !cfg!(windows) && self.quit_on_input() => {
                let armed = self.armed();
                if let Some(w) = self.wins.iter_mut().find(|w| w.window.id() == id) {
                    match w.first_cursor {
                        Some(p0) if armed => {
                            let d = ((position.x - p0.x).powi(2) + (position.y - p0.y).powi(2)).sqrt();
                            if d > MOVE_TO_QUIT {
                                el.exit();
                            }
                        }
                        _ => w.first_cursor = Some(position),
                    }
                }
            }
            WindowEvent::Resized(sz) => {
                if let Some(w) = self.wins.iter_mut().find(|w| w.window.id() == id) {
                    let (nw, nh) = (sz.width.max(1), sz.height.max(1));
                    if (nw, nh) != w.size {
                        w.size = (nw, nh);
                        w.player = None;
                        w.resized_at = Some(Instant::now());
                        w.window.request_redraw();
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(w) = self.wins.iter_mut().find(|w| w.window.id() == id) {
                    let (ww, hh) = w.size;
                    let (Some(nw), Some(nh)) = (NonZeroU32::new(ww), NonZeroU32::new(hh)) else { return };
                    if w.surface.resize(nw, nh).is_err() {
                        return;
                    }
                    let now = Instant::now();
                    let dt = (now - w.last).as_secs_f32().min(0.25);
                    w.last = now;
                    if w.player.is_none() {
                        // paint black first, then build the scene (which can take a moment)
                        if let Ok(mut buf) = w.surface.buffer_mut() {
                            buf.fill(0);
                            let _ = buf.present();
                        }
                        if w.resized_at.is_some_and(|t| t.elapsed() < RESIZE_SETTLE) {
                            // still being resized: wait (about_to_wait keeps asking for frames)
                            return;
                        }
                        w.resized_at = None;
                        let offset = if self.cfg.different_per_monitor { w.index } else { 0 };
                        w.player = Some(Player::new(self.cfg.clone(), ww as usize, hh as usize, offset, w.seed));
                        w.last = Instant::now();
                        w.window.request_redraw();
                        return;
                    }
                    if let Ok(mut buf) = w.surface.buffer_mut() {
                        let mut damage = None;
                        if buf.len() == (ww * hh) as usize {
                            if let Some(p) = &mut w.player {
                                // the buffer still holds our last frame: only redraw what moves
                                let has_last = buf.age() == 1 && w.partial_frames < 300;
                                damage = p.update_damage(dt, &mut buf, has_last);
                            }
                        }
                        match damage {
                            Some(rects) => {
                                w.partial_frames += 1;
                                let mut rects: Vec<softbuffer::Rect> = rects
                                    .into_iter()
                                    .filter_map(|(x, y, rw, rh)| {
                                        Some(softbuffer::Rect { x: x as u32, y: y as u32, width: NonZeroU32::new(rw as u32)?, height: NonZeroU32::new(rh as u32)? })
                                    })
                                    .collect();
                                if rects.is_empty() {
                                    rects.push(softbuffer::Rect { x: 0, y: 0, width: NonZeroU32::MIN, height: NonZeroU32::MIN });
                                }
                                let _ = buf.present_with_damage(&rects);
                            }
                            None => {
                                w.partial_frames = 0;
                                let _ = buf.present();
                            }
                        }
                    }
                }
                if self.armed_at.is_none() && self.wins.iter().all(|w| w.player.is_some()) {
                    self.armed_at = Some(Instant::now());
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if matches!(self.mode, Mode::Wallpaper) && self.last_stop_check.elapsed() > Duration::from_millis(500) {
            self.last_stop_check = Instant::now();
            for w in self.wins.iter_mut() {
                let covered = covered_by_foreground(w.screen);
                if w.covered && !covered {
                    // pick up the animation where it was, not with a jump
                    w.last = Instant::now();
                }
                w.covered = covered;
            }
            if crate::wallpaper::stop_file().exists() {
                let _ = std::fs::remove_file(crate::wallpaper::stop_file());
                self.wins.clear();
                crate::wallpaper::restore_desktop();
                el.exit();
                return;
            }
        }
        if let Mode::Preview(hwnd) = self.mode {
            if !parent_alive(hwnd) {
                el.exit();
                return;
            }
        }
        if self.quit_on_input() && self.armed() {
            if let Some(p) = cursor_pos() {
                match self.cursor_home {
                    None => self.cursor_home = Some(p),
                    Some(h) => {
                        let d = (((p.0 - h.0) as f64).powi(2) + ((p.1 - h.1) as f64).powi(2)).sqrt();
                        if d > MOVE_TO_QUIT {
                            el.exit();
                            return;
                        }
                    }
                }
            }
        }
        let frame = Duration::from_secs_f32(1.0 / self.cfg.fps.clamp(5, 60) as f32);
        let now = Instant::now();
        if now >= self.next_frame {
            for w in &self.wins {
                if !w.covered {
                    w.window.request_redraw();
                }
            }
            self.next_frame += frame;
            if self.next_frame < now {
                self.next_frame = now + frame;
            }
        }
        el.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
    }
}
