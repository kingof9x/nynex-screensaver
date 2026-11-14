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
    player: Player,
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
}

pub fn run(mode: Mode) {
    let cfg = Config::load();
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App { mode, cfg, wins: Vec::new(), started: Instant::now(), next_frame: Instant::now() };
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
        let offset = if self.cfg.different_per_monitor { index } else { 0 };
        let seed = 0x5eed_u32.wrapping_add(index as u32 * 7919).wrapping_add(self.started.elapsed().subsec_nanos());
        let player = Player::new(self.cfg.clone(), w as usize, h as usize, offset, seed);
        self.wins.push(Win { window, surface, player, size: (w, h), last: Instant::now(), first_cursor: None });
    }

    fn quit_on_input(&self) -> bool {
        matches!(self.mode, Mode::Full)
    }
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
        let quit = self.quit_on_input();
        let grace = self.started.elapsed() < Duration::from_millis(600);
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => el.exit(),
            WindowEvent::KeyboardInput { event, .. } if quit && event.state == ElementState::Pressed => el.exit(),
            WindowEvent::MouseInput { state: ElementState::Pressed, .. } if quit => el.exit(),
            WindowEvent::MouseWheel { .. } if quit => el.exit(),
            WindowEvent::CursorMoved { position, .. } if quit => {
                if let Some(w) = self.wins.iter_mut().find(|w| w.window.id() == id) {
                    match w.first_cursor {
                        None => w.first_cursor = Some(position),
                        Some(p0) => {
                            let d = ((position.x - p0.x).powi(2) + (position.y - p0.y).powi(2)).sqrt();
                            if d > 12.0 && !grace {
                                el.exit();
                            }
                        }
                    }
                }
            }
            WindowEvent::Resized(sz) => {
                if let Some(w) = self.wins.iter_mut().find(|w| w.window.id() == id) {
                    let (nw, nh) = (sz.width.max(1), sz.height.max(1));
                    if (nw, nh) != w.size {
                        w.size = (nw, nh);
                        w.player = Player::new(self.cfg.clone(), nw as usize, nh as usize, 0, 1);
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
                    if let Ok(mut buf) = w.surface.buffer_mut() {
                        if buf.len() == (ww * hh) as usize {
                            w.player.update(dt, &mut buf);
                        }
                        let _ = buf.present();
                    }
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
