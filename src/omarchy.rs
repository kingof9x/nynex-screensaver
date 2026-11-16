//! Linux desktop integration, aimed at Omarchy (Arch + Hyprland) but harmless elsewhere.
//!
//! * Screensaver: Omarchy's idle service runs `omarchy-launch-screensaver`. The nynex
//!   installer puts a replacement of that command in /usr/local/bin (which comes before
//!   /usr/bin on the PATH). It runs nynex unless the "off" file below exists, in which case
//!   it hands over to Omarchy's own screensaver.
//! * Wait time: Omarchy 4 keeps the idle timeouts in ~/.config/omarchy/shell.json.
//! * Start at login: a line in ~/.config/hypr/autostart.lua (Omarchy 4) or autostart.conf.

use crate::config::app_dir;
use std::path::PathBuf;

/// The launcher that the nynex installer puts in front of Omarchy's own.
pub const HOOK: &str = "/usr/local/bin/omarchy-launch-screensaver";
const AUTOSTART_MARK: &str = "nynex wallpaper";

/// The installer's screensaver hook is in place.
pub fn hook_installed() -> bool {
    std::fs::read_to_string(HOOK).map(|s| s.contains("nynex")).unwrap_or(false)
}

/// When this file exists the hook runs Omarchy's own screensaver instead of nynex.
pub fn off_file() -> PathBuf {
    app_dir().join("omarchy-screensaver-off")
}

pub fn screensaver_on() -> bool {
    hook_installed() && !off_file().exists()
}

pub fn set_screensaver(on: bool) -> String {
    if on {
        let _ = std::fs::remove_file(off_file());
        "nynex is your screensaver.".into()
    } else {
        let _ = std::fs::create_dir_all(app_dir());
        let _ = std::fs::write(off_file(), b"off");
        "Back to Omarchy's own screensaver.".into()
    }
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn shell_json() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| home().join(".config")).join("omarchy").join("shell.json")
}

/// Seconds of idle before Omarchy starts the screensaver, if this Omarchy keeps it in shell.json.
pub fn idle_seconds() -> Option<u32> {
    let text = std::fs::read_to_string(shell_json()).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    match v.get("idle").and_then(|i| i.get("screensaver")) {
        Some(s) => s.as_u64().map(|n| n as u32),
        None => Some(150), // Omarchy's default when the key is missing
    }
}

/// Change the idle wait. Omarchy's shell notices the change by itself.
pub fn set_idle_seconds(secs: u32) -> String {
    let path = shell_json();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return format!("Could not read {}: {e}", path.display()),
    };
    let mut v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return format!("Could not understand {}: {e}", path.display()),
    };
    let Some(root) = v.as_object_mut() else { return "shell.json is not a JSON object.".into() };
    let idle = root.entry("idle").or_insert_with(|| serde_json::json!({}));
    let Some(idle) = idle.as_object_mut() else { return "shell.json's idle setting is not an object.".into() };
    idle.insert("screensaver".into(), serde_json::json!(secs));
    // The lock has to come after the screensaver, or the screensaver never shows.
    if let Some(lock) = idle.get("lock").and_then(|l| l.as_u64()) {
        if lock != 0 && lock <= secs as u64 {
            idle.insert("lock".into(), serde_json::json!(secs + 150));
        }
    }
    let out = serde_json::to_string_pretty(&v).unwrap_or_default() + "\n";
    match std::fs::write(&path, out) {
        Ok(()) => format!("The screensaver starts after {} idle.", human(secs)),
        Err(e) => format!("Could not save {}: {e}", path.display()),
    }
}

pub fn human(secs: u32) -> String {
    if secs % 60 == 0 {
        format!("{} min", secs / 60)
    } else if secs > 60 {
        format!("{} min {} s", secs / 60, secs % 60)
    } else {
        format!("{secs} s")
    }
}

fn hypr_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| home().join(".config")).join("hypr")
}

/// The Hyprland autostart file and the line that starts the wallpaper in it.
fn autostart() -> (PathBuf, String) {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "nynex".into());
    let lua = hypr_dir().join("autostart.lua");
    if lua.exists() || !hypr_dir().join("autostart.conf").exists() {
        (lua, format!("o.launch_on_start(\"{exe} --wallpaper\") -- {AUTOSTART_MARK}"))
    } else {
        (hypr_dir().join("autostart.conf"), format!("exec-once = uwsm-app -- {exe} --wallpaper # {AUTOSTART_MARK}"))
    }
}

pub fn starts_at_login() -> bool {
    let (path, _) = autostart();
    std::fs::read_to_string(path).map(|s| s.lines().any(|l| l.contains(AUTOSTART_MARK))).unwrap_or(false)
}

pub fn set_start_at_login(on: bool) -> bool {
    let (path, line) = autostart();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<&str> = text.lines().filter(|l| !l.contains(AUTOSTART_MARK)).collect();
    if on {
        lines.push(&line);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    let _ = std::fs::create_dir_all(hypr_dir());
    std::fs::write(&path, out).is_ok()
}
