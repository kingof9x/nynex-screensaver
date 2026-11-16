//! Animated wallpaper: the nynex windows are put behind the desktop icons (inside the
//! desktop's WorkerW window, the same trick live-wallpaper apps use), one per monitor.

use crate::config::app_dir;
use std::path::PathBuf;

/// A running wallpaper stops when this file appears.
pub fn stop_file() -> PathBuf {
    app_dir().join("wallpaper.stop")
}

/// Copy of the program that the wallpaper (and "start with Windows") runs, so it keeps
/// working if the downloaded file is moved.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn exe_copy() -> PathBuf {
    app_dir().join("nynex.exe")
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe extern "system" fn find_worker(top: HWND, out: LPARAM) -> BOOL {
        let defview = wide("SHELLDLL_DefView");
        let worker = wide("WorkerW");
        if !FindWindowExW(top, std::ptr::null_mut(), defview.as_ptr(), std::ptr::null()).is_null() {
            // the WorkerW behind the icons is the next top-level WorkerW after this one
            let w = FindWindowExW(std::ptr::null_mut(), top, worker.as_ptr(), std::ptr::null());
            if !w.is_null() {
                *(out as *mut HWND) = w;
            }
        }
        1
    }

    /// The window that sits between the desktop background and the icons.
    pub fn desktop_layer() -> Option<HWND> {
        unsafe {
            let progman = FindWindowW(wide("Progman").as_ptr(), std::ptr::null());
            if progman.is_null() {
                return None;
            }
            // ask Explorer to create the WorkerW layer
            let mut res = 0usize;
            SendMessageTimeoutW(progman, 0x052C, 0xD, 0x1, SMTO_NORMAL, 1000, &mut res);
            SendMessageTimeoutW(progman, 0x052C, 0, 0, SMTO_NORMAL, 1000, &mut res);
            // Windows 11 24H2 and later: the layer is a child of Progman
            let child = FindWindowExW(progman, std::ptr::null_mut(), wide("WorkerW").as_ptr(), std::ptr::null());
            if !child.is_null() {
                return Some(child);
            }
            // older Windows: a separate top-level WorkerW behind the icons
            let mut found: HWND = std::ptr::null_mut();
            EnumWindows(Some(find_worker), &mut found as *mut HWND as LPARAM);
            if found.is_null() {
                None
            } else {
                Some(found)
            }
        }
    }

    /// Re-parent `hwnd` into the desktop layer at screen position (x, y) with size (w, h).
    pub fn attach(hwnd: isize, x: i32, y: i32, w: i32, h: i32) -> bool {
        let Some(layer) = desktop_layer() else { return false };
        unsafe {
            let hwnd = hwnd as *mut c_void;
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            let style = (style & !((WS_POPUP | WS_CAPTION | WS_THICKFRAME | WS_SYSMENU) as isize)) | (WS_CHILD | WS_VISIBLE) as isize;
            SetWindowLongPtrW(hwnd, GWL_STYLE, style);
            if SetParent(hwnd, layer).is_null() {
                return false;
            }
            // child coordinates are relative to the virtual screen's top-left corner
            let (vx, vy) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
            SetWindowPos(hwnd, std::ptr::null_mut(), x - vx, y - vy, w, h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
        true
    }

    /// Repaint the normal wallpaper (after the animated one goes away).
    pub fn restore_desktop() {
        unsafe {
            let mut buf = vec![0u16; 1024];
            if SystemParametersInfoW(SPI_GETDESKWALLPAPER, buf.len() as u32, buf.as_mut_ptr().cast(), 0) != 0 {
                SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, buf.as_mut_ptr().cast(), SPIF_SENDCHANGE);
            }
        }
    }
}

#[cfg(windows)]
pub use win::{attach, restore_desktop};

#[cfg(not(windows))]
pub fn attach(_hwnd: isize, _x: i32, _y: i32, _w: i32, _h: i32) -> bool {
    false
}
#[cfg(not(windows))]
pub fn restore_desktop() {}

/// Stop any running wallpaper, then start a new one.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn start() -> Result<(), String> {
    stop();
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let _ = std::fs::remove_file(stop_file());
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let copy = exe_copy();
    let same = std::fs::canonicalize(&exe).ok() == std::fs::canonicalize(&copy).ok();
    if !same {
        std::fs::create_dir_all(app_dir()).map_err(|e| e.to_string())?;
        // the copy can be busy if an old wallpaper is still closing; fall back to this file
        if std::fs::copy(&exe, &copy).is_err() && !copy.exists() {
            return std::process::Command::new(exe).arg("--wallpaper").spawn().map(|_| ()).map_err(|e| e.to_string());
        }
    }
    std::process::Command::new(copy).arg("--wallpaper").spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Ask a running wallpaper to close.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn stop() {
    let _ = std::fs::create_dir_all(app_dir());
    let _ = std::fs::write(stop_file(), b"stop");
}

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const RUN_NAME: &str = "nynex wallpaper";

/// Whether the wallpaper starts when you sign in to Windows.
#[cfg(windows)]
pub fn starts_with_windows() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let (k, v): (Vec<u16>, Vec<u16>) = (RUN_KEY.encode_utf16().chain([0]).collect(), RUN_NAME.encode_utf16().chain([0]).collect());
    let mut size = 0u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) == 0 }
}

#[cfg(windows)]
pub fn set_start_with_windows(on: bool) -> bool {
    use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    let k: Vec<u16> = RUN_KEY.encode_utf16().chain([0]).collect();
    let v: Vec<u16> = RUN_NAME.encode_utf16().chain([0]).collect();
    unsafe {
        if on {
            if let Ok(exe) = std::env::current_exe() {
                let copy = exe_copy();
                if std::fs::canonicalize(&exe).ok() != std::fs::canonicalize(&copy).ok() {
                    let _ = std::fs::create_dir_all(app_dir());
                    let _ = std::fs::copy(&exe, &copy);
                }
            }
            let cmd: Vec<u16> = format!("\"{}\" --wallpaper", exe_copy().display()).encode_utf16().chain([0]).collect();
            RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), REG_SZ, cmd.as_ptr().cast(), (cmd.len() * 2) as u32) == 0
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr());
            true
        }
    }
}
