//! Slideshow: shows each landscape for its display time, loads the next one in the
//! background, and blends between them with the chosen transition.

use crate::config::{Config, Landscape, TransitionKind};
use crate::scene::{draw_text, hash2, load_logo, load_source, LogoArt, Rect, Scene};
use rayon::prelude::*;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

pub struct Player {
    pub cfg: Config,
    pub sw: usize,
    pub sh: usize,
    list: Vec<Landscape>,
    order: Vec<usize>,
    pos: usize,
    cur: Option<Scene>,
    next: Option<Scene>,
    loading: Option<Receiver<Option<Scene>>>,
    /// Position in `order` of the scene in `next` (or the one being loaded).
    next_pos: usize,
    /// Images in a row that failed to load in the background; after a whole lap of failures
    /// the player stops trying and keeps showing what it has.
    failures: usize,
    shown: f32,
    trans: Option<(f32, TransitionKind)>,
    clock: f32,
    logo: Option<Arc<LogoArt>>,
    /// Scratch frames for the two scenes of a transition. Only allocated while a transition
    /// runs (they are two whole screens: 16 MB at 1080p) and freed again when it ends.
    buf_a: Vec<u32>,
    buf_b: Vec<u32>,
    seed: u32,
    error: Option<String>,
    /// The last frame handed out was drawn by `cur` alone (no transition, no message).
    cur_drew_last: bool,
    /// Transitions completed so far (for tests and the stress runs).
    #[cfg_attr(not(test), allow(dead_code))]
    transitions: usize,
}

fn shuffled(n: usize, seed: u32) -> Vec<usize> {
    let mut v: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        let j = (hash2(i as i64, 7, seed) * (i + 1) as f32) as usize % (i + 1);
        v.swap(i, j);
    }
    v
}

impl Player {
    /// `offset` starts this player further along the list (different landscape per monitor).
    pub fn new(cfg: Config, sw: usize, sh: usize, offset: usize, seed: u32) -> Player {
        let list = cfg.enabled();
        let order = if cfg.shuffle { shuffled(list.len(), seed) } else { (0..list.len()).collect() };
        let logo = cfg.logo.path.as_deref().and_then(load_logo);
        let mut p = Player {
            pos: if list.is_empty() { 0 } else { offset % list.len() },
            cfg,
            sw,
            sh,
            list,
            order,
            cur: None,
            next: None,
            loading: None,
            next_pos: 0,
            failures: 0,
            shown: 0.0,
            trans: None,
            clock: 0.0,
            logo,
            buf_a: Vec::new(),
            buf_b: Vec::new(),
            seed,
            error: None,
            cur_drew_last: false,
            transitions: 0,
        };
        if !p.list.is_empty() {
            // start on the first image that loads, so one broken file doesn't blank the screen
            for _ in 0..p.list.len() {
                p.cur = p.build_sync(p.pos);
                if p.cur.is_some() {
                    break;
                }
                p.pos = (p.pos + 1) % p.list.len();
            }
            p.request(p.pos + 1);
        }
        p
    }

    fn landscape_at(&self, pos: usize) -> Landscape {
        self.list[self.order[pos % self.order.len()]].clone()
    }

    /// Logo artwork for an image: the shared one, unless the image's own settings pick another.
    fn logo_art_for(&self, l: &Landscape) -> Option<Arc<LogoArt>> {
        let lc = self.cfg.logo_of(l);
        if !lc.enabled {
            return None;
        }
        if lc.path == self.cfg.logo.path && self.logo.is_some() {
            return self.logo.clone();
        }
        lc.path.as_deref().and_then(load_logo)
    }

    fn build_sync(&mut self, pos: usize) -> Option<Scene> {
        let l = self.landscape_at(pos);
        let art = self.logo_art_for(&l);
        let (look, logo_cfg) = (self.cfg.look_of(&l).clone(), self.cfg.logo_of(&l).clone());
        match load_source(&l.path) {
            Ok(src) => Some(Scene::new(&src, art.as_deref(), self.sw, self.sh, &look, &logo_cfg, self.seed)),
            Err(e) => {
                self.error = Some(e);
                None
            }
        }
    }

    /// Load the landscape at `pos` on a background thread.
    fn request(&mut self, pos: usize) {
        if self.list.len() < 2 {
            return;
        }
        self.next_pos = pos % self.order.len();
        let l = self.landscape_at(pos);
        let (tx, rx) = channel();
        let (look, logo_cfg, logo, sw, sh, seed) = (
            self.cfg.look_of(&l).clone(),
            self.cfg.logo_of(&l).clone(),
            self.logo_art_for(&l),
            self.sw,
            self.sh,
            self.seed.wrapping_add(pos as u32),
        );
        std::thread::spawn(move || {
            let scene = load_source(&l.path).ok().map(|src| Scene::new(&src, logo.as_deref(), sw, sh, &look, &logo_cfg, seed));
            let _ = tx.send(scene);
        });
        self.loading = Some(rx);
    }

    /// Show a particular landscape right away (settings preview).
    pub fn jump_to(&mut self, list_index: usize) {
        if let Some(pos) = self.order.iter().position(|&i| i == list_index) {
            self.pos = pos;
            self.trans = None;
            self.shown = 0.0;
            self.next = None;
            self.loading = None;
            self.cur = self.build_sync(pos);
            self.request(pos + 1);
        }
    }

    /// Show the previous (`d` = -1) or next (`d` = +1) landscape right away (arrow keys).
    /// Going forward uses the already-loaded next landscape when it is ready.
    pub fn step(&mut self, d: isize) {
        let n = self.order.len();
        if n == 0 {
            return;
        }
        if self.trans.is_some() {
            // finish the running transition first; going forward, that's the whole step
            self.trans = None;
            if let Some(next) = self.next.take() {
                self.cur = Some(next);
                self.pos = self.next_pos;
            }
            self.buf_a = Vec::new();
            self.buf_b = Vec::new();
            self.shown = 0.0;
            self.cur_drew_last = false;
            self.request(self.pos + 1);
            if d > 0 {
                return;
            }
        }
        let target = (self.pos as isize + d).rem_euclid(n as isize) as usize;
        if d > 0 && self.next.is_none() && self.next_pos == target {
            if let Some(rx) = &self.loading {
                // wait for the background load rather than loading the same image twice
                if let Ok(scene) = rx.recv() {
                    self.next = scene;
                }
            }
        }
        let scene = match self.next.take() {
            Some(s) if d > 0 && n > 1 && self.next_pos == target => Some(s),
            _ => self.build_sync(target),
        };
        self.pos = target;
        self.cur = scene;
        self.shown = 0.0;
        self.next = None;
        self.loading = None;
        self.cur_drew_last = false;
        crate::trim_heap();
        self.request(target + 1);
    }

    /// Position in the play order of the landscape on screen.
    #[cfg(test)]
    pub fn position(&self) -> usize {
        self.order[self.pos]
    }

    #[cfg(test)]
    pub fn transitions(&self) -> usize {
        self.transitions
    }

    /// Skip to the next landscape with a transition (settings preview button).
    pub fn advance(&mut self) {
        self.shown = f32::MAX / 2.0;
    }

    pub fn update(&mut self, dt: f32, out: &mut [u32]) {
        self.update_damage(dt, out, false);
    }

    /// Draw the next frame into `out`. `out_has_last` says `out` still holds the frame this
    /// player drew last time; then only changed areas may be redrawn, and they are returned.
    /// None means the whole frame is new.
    pub fn update_damage(&mut self, dt: f32, out: &mut [u32], out_has_last: bool) -> Option<Vec<Rect>> {
        let has_last = out_has_last && self.cur_drew_last;
        self.cur_drew_last = false;
        self.clock += dt;
        if self.list.is_empty() || self.cur.is_none() {
            out.fill(0x0d0a10);
            let msg = if self.list.is_empty() {
                "Add landscapes in the nynex screensaver settings"
            } else {
                self.error.as_deref().unwrap_or("Could not load landscape")
            };
            let scale = (self.sh / 360).max(1);
            draw_text(out, self.sw, self.sh, msg, self.sw / 2, self.sh / 2, scale, 0xf6ecdc);
            return None;
        }

        if let Some(rx) = &self.loading {
            match rx.try_recv() {
                Ok(Some(scene)) => {
                    self.next = Some(scene);
                    self.loading = None;
                    self.failures = 0;
                    // the loader's decode buffers were freed on its thread; give them back
                    crate::trim_heap();
                }
                Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // that image didn't load (deleted, corrupt, or the loader panicked):
                    // skip it and try the one after, unless every image has failed
                    self.loading = None;
                    self.failures += 1;
                    if self.failures < self.order.len() {
                        self.request(self.next_pos + 1);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }

        let dur = self.cfg.duration_of(&self.landscape_at(self.pos));
        self.shown += dt;
        if self.trans.is_none() && self.shown >= dur && self.next.is_some() {
            let mut kind = self.cfg.transition;
            if kind == TransitionKind::Random {
                let i = (hash2(self.pos as i64, 3, self.seed) * 6.0) as usize % 6;
                kind = TransitionKind::ALL[i];
            }
            self.trans = Some((0.0, kind));
        }

        let t = self.clock;
        match self.trans {
            None => {
                let d = self.cur.as_mut().unwrap().render_damage(t, out, has_last);
                self.cur_drew_last = true;
                d
            }
            Some((elapsed, kind)) => {
                let len = self.cfg.transition_secs.max(0.05);
                let p = (elapsed / len).clamp(0.0, 1.0);
                let n = self.sw * self.sh;
                if self.buf_a.len() != n {
                    self.buf_a = vec![0; n];
                    self.buf_b = vec![0; n];
                }
                self.cur.as_mut().unwrap().render(t, &mut self.buf_a);
                self.next.as_mut().unwrap().render(t, &mut self.buf_b);
                blend(kind, ease(p), &self.buf_a, &self.buf_b, out, self.sw, self.sh, self.seed);
                let elapsed = elapsed + dt;
                if elapsed >= len {
                    self.cur = self.next.take();
                    self.pos = self.next_pos;
                    self.transitions += 1;
                    self.shown = 0.0;
                    self.trans = None;
                    // give the two transition frames back to the system until the next one
                    self.buf_a = Vec::new();
                    self.buf_b = Vec::new();
                    // the old scene and both frames are gone: return that memory now
                    crate::trim_heap();
                    self.request(self.pos + 1);
                } else {
                    self.trans = Some((elapsed, kind));
                }
                None
            }
        }
    }
}

fn ease(p: f32) -> f32 {
    p * p * (3.0 - 2.0 * p)
}

#[inline]
fn lerp_px(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| -> (f32, f32, f32) { (((s >> 16) & 255) as f32, ((s >> 8) & 255) as f32, (s & 255) as f32) };
    let (ar, ag, ab) = ch(a);
    let (br, bg, bb) = ch(b);
    let r = (ar + (br - ar) * t) as u32;
    let g = (ag + (bg - ag) * t) as u32;
    let bl = (ab + (bb - ab) * t) as u32;
    (r << 16) | (g << 8) | bl
}

/// Combine two finished frames for transition progress `p` (0 = all `a`, 1 = all `b`).
#[allow(clippy::too_many_arguments)]
pub fn blend(kind: TransitionKind, p: f32, a: &[u32], b: &[u32], out: &mut [u32], sw: usize, sh: usize, seed: u32) {
    let block = (sh / 90).max(4);
    out.par_chunks_mut(sw).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * sw + x;
            *o = match kind {
                TransitionKind::Crossfade | TransitionKind::Random => lerp_px(a[i], b[i], p),
                TransitionKind::Dissolve => {
                    let h = hash2((x / block) as i64, (y / block) as i64, seed + 17);
                    if h < p { b[i] } else { a[i] }
                }
                TransitionKind::Wipe => {
                    // a soft, slightly ragged edge sweeping left to right
                    let edge = p * 1.2 - 0.1;
                    let jitter = (hash2((y / block) as i64, 0, seed + 3) - 0.5) * 0.04;
                    let u = x as f32 / sw as f32 + jitter;
                    let k = ((edge - u) / 0.04 + 0.5).clamp(0.0, 1.0);
                    lerp_px(a[i], b[i], k)
                }
                TransitionKind::Slide => {
                    let off = (p * sw as f32) as usize;
                    if x + off < sw { a[y * sw + x + off] } else { b[y * sw + x + off - sw] }
                }
                TransitionKind::Pixelate => {
                    // blocks grow on the old scene, then shrink on the new one
                    let q = if p < 0.5 { p * 2.0 } else { (1.0 - p) * 2.0 };
                    let bs = (1.0 + q * q * (sh as f32 / 12.0)) as usize;
                    let (bx, by) = ((x / bs * bs + bs / 2).min(sw - 1), (y / bs * bs + bs / 2).min(sh - 1));
                    let src = if p < 0.5 { a } else { b };
                    src[by * sw + bx]
                }
                TransitionKind::FadeThroughBlack => {
                    if p < 0.5 { lerp_px(a[i], 0, p * 2.0) } else { lerp_px(0, b[i], (p - 0.5) * 2.0) }
                }
            };
        }
    });
}
