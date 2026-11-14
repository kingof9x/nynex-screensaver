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
}

/// Pixels the mouse has to travel before the screensaver closes.
const MOVE_TO_QUIT: f64 = 24.0;

pub fn run(mode: Mode) {
    let cfg = Config::load();
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App { mode, cfg, wins: Vec::new(), started: Instant::now(), next_frame: Instant::now(), armed_at: None, cursor_home: None };
    let _ = event_loop.run_app(&mut app);
}

impl App {
    fn add_window(&mut self, el: &ActiveEventLoop, attrs: winit::window::WindowAttributes, index: usize) {
        let window = match el.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(_) => return,
        };
        if matches!(self.mode, Mode::Full) {
            window.set_cursor_visible(false);
        }
        let ctx = Context::new(window.clone()).expect("softbuffer context");
        let surface = Surface::new(&ctx, window.clone()).expect("softbuffer surface");
        let sz = window.inner_size();
        let (w, h) = (sz.width.max(1), sz.height.max(1));
        let seed = 0x5eed_u32.wrapping_add(index as u32 * 7919).wrapping_add(self.started.elapsed().subsec_nanos());
        self.wins.push(Win { window, surface, player: None, index, seed, size: (w, h), last: Instant::now(), first_cursor: None });
    }

    fn quit_on_input(&self) -> bool {
        matches!(self.mode, Mode::Full)
    }

    /// True once input should close the screensaver.
    fn armed(&self) -> bool {
        self.armed_at.map(|t| t.elapsed() > Duration::from_millis(800)).unwrap_or(false)
    }
}

#[cfg(windows)]
fn cursor_pos() -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
}

#[cfg(not(windows))]
fn cursor_pos() -> Option<(i32, i32)> {
    None
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
                    let attrs = Window::default_attributes()
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
                let attrs = Window::default_attributes()
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
                        let offset = if self.cfg.different_per_monitor { w.index } else { 0 };
                        w.player = Some(Player::new(self.cfg.clone(), ww as usize, hh as usize, offset, w.seed));
                        w.last = Instant::now();
                        w.window.request_redraw();
                        return;
                    }
                    if let Ok(mut buf) = w.surface.buffer_mut() {
                        if buf.len() == (ww * hh) as usize {
                            if let Some(p) = &mut w.player {
                                p.update(dt, &mut buf);
                            }
                        }
                        let _ = buf.present();
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
                w.window.request_redraw();
            }
            self.next_frame += frame;
            if self.next_frame < now {
                self.next_frame = now + frame;
            }
        }
        el.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
    }
}
