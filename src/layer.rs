//! Animated wallpaper for Wayland desktops (Hyprland / Omarchy, Sway, niri, ...).
//!
//! One wlr-layer-shell surface per monitor, under every window and taking no input. It
//! sits on the "bottom" layer by default, so it covers the desktop's own background
//! (Omarchy's shell draws that on the "background" layer) however the two were started.
//! Frames are only drawn when the compositor asks for one (frame callbacks) and no faster
//! than the configured FPS, so a hidden wallpaper costs next to nothing.

use crate::config::Config;
use crate::player::Player;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
        WaylandSurface,
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm, ShmHandler,
    },
};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
    Connection, QueueHandle,
};

/// Layer-shell namespace, so compositor rules can target it (`layerrule = ..., nynex-wallpaper`).
pub const NAMESPACE: &str = "nynex-wallpaper";

struct Output {
    output: wl_output::WlOutput,
    layer: LayerSurface,
    index: usize,
    seed: u32,
    size: (u32, u32),
    configured: bool,
    /// The compositor asked for a new frame (or we have not drawn one yet).
    wants_frame: bool,
    player: Option<Player>,
    last: Instant,
    buffers: Vec<Buffer>,
    scratch: Vec<u32>,
}

struct State {
    registry: RegistryState,
    outputs_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    cfg: Config,
    layer: Layer,
    outputs: Vec<Output>,
    next_index: usize,
    started: Instant,
    exit: bool,
}

fn log(msg: &str) {
    let path = crate::config::app_dir().join("wallpaper.log");
    let _ = std::fs::create_dir_all(crate::config::app_dir());
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if text.len() > 64 * 1024 {
        text.clear();
    }
    text.push_str(msg);
    text.push('\n');
    let _ = std::fs::write(&path, text);
    eprintln!("{msg}");
}

/// Run the wallpaper until told to stop (stop file, SIGTERM, or the compositor going away).
pub fn run_wallpaper() -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("No Wayland session ({e}). The live wallpaper needs Wayland (Hyprland, Sway, ...)."))?;
    let (globals, mut queue) = registry_queue_init(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh).map_err(|_| "The compositor has no wl_compositor.")?;
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(|_| "This compositor does not support wlr-layer-shell, so nynex can't draw a wallpaper.")?;
    let shm = Shm::bind(&globals, &qh).map_err(|_| "The compositor has no wl_shm.")?;
    let pool = SlotPool::new(1920 * 1080 * 4, &shm).map_err(|e| e.to_string())?;
    let layer = match std::env::var("NYNEX_LAYER").ok().as_deref() {
        Some("background") => Layer::Background,
        _ => Layer::Bottom,
    };
    let _ = std::fs::remove_file(crate::wallpaper::stop_file());
    let mut st = State {
        registry: RegistryState::new(&globals),
        outputs_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool,
        cfg: Config::load(),
        layer,
        outputs: Vec::new(),
        next_index: 0,
        started: Instant::now(),
        exit: false,
    };
    // Learn about the monitors (new_output adds a surface for each).
    queue.roundtrip(&mut st).map_err(|e| e.to_string())?;
    log(&format!("wallpaper: started on {} monitor(s), {:?} layer", st.outputs.len(), st.layer));

    let frame = Duration::from_secs_f32(1.0 / st.cfg.fps.clamp(5, 60) as f32);
    let mut next_frame = Instant::now();
    let mut last_stop_check = Instant::now();
    while !st.exit {
        if last_stop_check.elapsed() > Duration::from_millis(500) {
            last_stop_check = Instant::now();
            if crate::wallpaper::stop_file().exists() {
                let _ = std::fs::remove_file(crate::wallpaper::stop_file());
                break;
            }
        }
        let now = Instant::now();
        if now >= next_frame {
            st.draw_all(&qh);
            next_frame += frame;
            if next_frame < now {
                next_frame = now + frame;
            }
        }
        queue.dispatch_pending(&mut st).map_err(|e| e.to_string())?;
        conn.flush().map_err(|e| e.to_string())?;
        // Wait for Wayland events or the next frame time, whichever comes first.
        if let Some(guard) = queue.prepare_read() {
            let timeout = next_frame.saturating_duration_since(Instant::now()).min(Duration::from_millis(500));
            let mut fds = [libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 }];
            let n = unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout.as_millis() as i32) };
            if n > 0 {
                if let Err(e) = guard.read() {
                    if !matches!(e, wayland_client::backend::WaylandError::Io(ref io) if io.kind() == std::io::ErrorKind::WouldBlock) {
                        return Err(format!("Lost the Wayland connection: {e}"));
                    }
                }
            }
        }
        queue.dispatch_pending(&mut st).map_err(|e| e.to_string())?;
    }
    log("wallpaper: stopped");
    Ok(())
}

impl State {
    fn add_output(&mut self, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(qh, surface, self.layer, Some(NAMESPACE), Some(&output));
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        // An empty input region lets clicks go through to the desktop.
        if let Ok(region) = smithay_client_toolkit::compositor::Region::new(&self.compositor) {
            layer.wl_surface().set_input_region(Some(region.wl_region()));
        }
        layer.set_size(0, 0);
        layer.commit();
        let index = self.next_index;
        self.next_index += 1;
        let seed = 0x5eed_u32.wrapping_add(index as u32 * 7919).wrapping_add(self.started.elapsed().subsec_nanos());
        self.outputs.push(Output {
            output,
            layer,
            index,
            seed,
            size: (0, 0),
            configured: false,
            wants_frame: true,
            player: None,
            last: Instant::now(),
            buffers: Vec::new(),
            scratch: Vec::new(),
        });
    }

    fn draw_all(&mut self, qh: &QueueHandle<Self>) {
        for i in 0..self.outputs.len() {
            self.draw(qh, i);
        }
    }

    fn draw(&mut self, qh: &QueueHandle<Self>, i: usize) {
        let o = &mut self.outputs[i];
        if !o.configured || !o.wants_frame {
            return;
        }
        let (w, h) = o.size;
        if w == 0 || h == 0 {
            return;
        }
        let now = Instant::now();
        let dt = (now - o.last).as_secs_f32().min(0.25);
        o.last = now;
        let n = (w * h) as usize;
        if o.scratch.len() != n {
            o.scratch = vec![0; n];
        }
        match &mut o.player {
            Some(p) => p.update(dt, &mut o.scratch),
            None => {
                // first frame: show black right away, build the scene for the next one
                o.scratch.fill(0);
                let offset = if self.cfg.different_per_monitor { o.index } else { 0 };
                o.player = Some(Player::new(self.cfg.clone(), w as usize, h as usize, offset, o.seed));
                o.last = Instant::now();
            }
        }
        // Reuse a buffer the compositor has released, or make a new one (at most three).
        let stride = w as i32 * 4;
        o.buffers.retain(|b| b.height() == h as i32 && b.stride() == stride);
        let mut slot = None;
        for (k, b) in o.buffers.iter_mut().enumerate() {
            if b.canvas(&mut self.pool).is_some() {
                slot = Some(k);
                break;
            }
        }
        let k = match slot {
            Some(k) => k,
            None if o.buffers.len() < 3 => match self.pool.create_buffer(w as i32, h as i32, stride, wl_shm::Format::Xrgb8888) {
                Ok((b, _)) => {
                    o.buffers.push(b);
                    o.buffers.len() - 1
                }
                Err(e) => {
                    log(&format!("wallpaper: could not make a frame buffer: {e}"));
                    return;
                }
            },
            None => return, // all busy; try again next frame
        };
        let buffer = &mut o.buffers[k];
        if let Some(canvas) = buffer.canvas(&mut self.pool) {
            // wl_shm XRGB8888 is little-endian 0x00RRGGBB, the same layout the renderer uses
            for (dst, src) in canvas.chunks_exact_mut(4).zip(o.scratch.iter()) {
                dst.copy_from_slice(&src.to_le_bytes());
            }
        }
        let surface = o.layer.wl_surface();
        surface.damage_buffer(0, 0, w as i32, h as i32);
        surface.frame(qh, surface.clone());
        if buffer.attach_to(surface).is_ok() {
            o.wants_frame = false;
            o.layer.commit();
        }
    }
}

impl CompositorHandler for State {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        if let Some(o) = self.outputs.iter_mut().find(|o| o.layer.wl_surface() == surface) {
            o.wants_frame = true;
        }
    }
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs_state
    }
    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.add_output(qh, output);
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.outputs.retain(|o| o.output != output);
    }
}

impl LayerShellHandler for State {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        // the compositor took this monitor's surface away (monitor unplugged)
        self.outputs.retain(|o| &o.layer != layer);
        if self.outputs.is_empty() {
            self.exit = true;
        }
    }

    fn configure(&mut self, _: &Connection, qh: &QueueHandle<Self>, layer: &LayerSurface, configure: LayerSurfaceConfigure, _: u32) {
        let Some(i) = self.outputs.iter().position(|o| &o.layer == layer) else { return };
        let o = &mut self.outputs[i];
        let (w, h) = configure.new_size;
        let (w, h) = (w.max(1), h.max(1));
        if (w, h) != o.size {
            // Rendered at the monitor's logical size; on a HiDPI screen the compositor scales
            // it up, which suits pixel art and keeps the CPU cost down.
            o.size = (w, h);
            o.player = None;
            o.buffers.clear();
        }
        let first = !o.configured;
        o.configured = true;
        if first {
            o.wants_frame = true;
            self.draw(qh, i);
        }
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

delegate_compositor!(State);
delegate_output!(State);
delegate_shm!(State);
delegate_layer!(State);
delegate_registry!(State);
