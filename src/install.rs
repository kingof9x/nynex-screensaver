//! Makes nynex the Windows screensaver for the current user, without going through
//! Windows' Screen Saver Settings.

use crate::config::app_dir;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SPI_GETSCREENSAVEACTIVE, SPI_GETSCREENSAVETIMEOUT,
    SPI_SETSCREENSAVEACTIVE, SPI_SETSCREENSAVETIMEOUT,
};

const DESKTOP: &str = r"Control Panel\Desktop";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Where the screensaver copy lives. Windows only runs files ending in .scr as screensavers.
pub fn scr_path() -> PathBuf {
    app_dir().join("nynex.scr")
}

/// Windows' screensaver launcher is unreliable with long paths that contain spaces, so
/// register the 8.3 short form of the path when there is one.
fn short_path(p: &Path) -> String {
    let long = wide(&p.display().to_string());
    let mut buf = vec![0u16; 1024];
    let n = unsafe { GetShortPathNameW(long.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return p.display().to_string();
    }
    String::from_utf16_lossy(&buf[..n])
}

fn reg_get(name: &str) -> Option<String> {
    let (key, val) = (wide(DESKTOP), wide(name));
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), val.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size)
    };
    if r != 0 {
        return None;
    }
    let n = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..n]))
}

fn reg_set(name: &str, value: &str) -> bool {
    let (key, val, data) = (wide(DESKTOP), wide(name), wide(value));
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), val.as_ptr(), REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) == 0 }
}

/// The current state: (nynex is the chosen screensaver and switched on, wait in minutes).
pub fn status() -> (bool, u32) {
    let mut active: i32 = 0;
    let mut secs: u32 = 0;
    unsafe {
        SystemParametersInfoW(SPI_GETSCREENSAVEACTIVE, 0, (&mut active as *mut i32).cast(), 0);
        SystemParametersInfoW(SPI_GETSCREENSAVETIMEOUT, 0, (&mut secs as *mut u32).cast(), 0);
    }
    let ours = reg_get("SCRNSAVE.EXE").map(|v| v.to_lowercase().contains("nynex")).unwrap_or(false);
    (ours && active != 0, (secs / 60).max(1))
}

/// Copy this program to nynex.scr, make it the screensaver, and start it after `minutes` idle.
pub fn set_as_screensaver(minutes: u32) -> String {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return format!("Could not find this program: {e}"),
    };
    let scr = scr_path();
    let same = std::fs::canonicalize(&exe).ok() == std::fs::canonicalize(&scr).ok();
    if !same {
        if let Err(e) = std::fs::create_dir_all(app_dir()).and_then(|_| std::fs::copy(&exe, &scr).map(|_| ())) {
            return format!("Could not copy the screensaver: {e}");
        }
    }
    if !reg_set("SCRNSAVE.EXE", &short_path(&scr)) {
        return "Windows refused the change (a work or school policy may control the screensaver).".into();
    }
    let flags = SPIF_UPDATEINIFILE | SPIF_SENDCHANGE;
    let ok = unsafe {
        SystemParametersInfoW(SPI_SETSCREENSAVETIMEOUT, minutes.max(1) * 60, std::ptr::null_mut(), flags) != 0
            && SystemParametersInfoW(SPI_SETSCREENSAVEACTIVE, 1, std::ptr::null_mut(), flags) != 0
    };
    if ok {
        format!("nynex is your screensaver. It starts after {} min idle.", minutes.max(1))
    } else {
        "Set the screensaver, but Windows did not accept the wait time.".into()
    }
}

/// Switch the screensaver off and forget nynex as the chosen one.
pub fn turn_off() -> String {
    let (key, val) = (wide(DESKTOP), wide("SCRNSAVE.EXE"));
    unsafe {
        RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), val.as_ptr());
        SystemParametersInfoW(SPI_SETSCREENSAVEACTIVE, 0, std::ptr::null_mut(), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE);
    }
    "Screensaver turned off.".into()
}
