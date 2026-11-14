//! "Set as my screensaver": point Windows at this .scr for the current user.

#[cfg(windows)]
pub fn set_as_screensaver() -> String {
    use std::process::Command;
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return format!("Could not find this program: {e}"),
    };
    // Windows only treats files ending in .scr as screensavers. If we were started as an .exe,
    // keep a .scr copy next to the settings so the screensaver list can find it.
    let scr = if exe.extension().map(|e| e.eq_ignore_ascii_case("scr")).unwrap_or(false) {
        exe
    } else {
        let dst = crate::config::app_dir().join("nynex.scr");
        if let Err(e) = std::fs::create_dir_all(crate::config::app_dir()).and_then(|_| std::fs::copy(&exe, &dst).map(|_| ())) {
            return format!("Could not copy the screensaver: {e}");
        }
        dst
    };
    let key = r"HKCU\Control Panel\Desktop";
    let ok = |args: &[&str]| Command::new("reg").args(args).output().map(|o| o.status.success()).unwrap_or(false);
    let scr_s = scr.display().to_string();
    if ok(&["add", key, "/v", "SCRNSAVE.EXE", "/t", "REG_SZ", "/d", &scr_s, "/f"])
        && ok(&["add", key, "/v", "ScreenSaveActive", "/t", "REG_SZ", "/d", "1", "/f"])
    {
        "Set as your screensaver. The wait time is in Windows' Screen Saver Settings.".into()
    } else {
        "Windows refused the change. Right-click nynex.scr and choose Install instead.".into()
    }
}
