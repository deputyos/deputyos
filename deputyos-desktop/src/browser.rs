//! Default-browser opener.
//!
//! Wraps the `webbrowser` crate so the launcher can fail gracefully when
//! the OS has no default browser (headless Linux box, CI). On failure we
//! print the URL and instruct the user to open it manually — never abort
//! the start sequence.

use anyhow::Result;

/// Open `url` in the host's default browser. On failure, print to stderr
/// and return Ok — the user can copy the URL themselves. Set
/// `DEPUTYOS_DESKTOP_NO_BROWSER=1` to skip the open call entirely (used in
/// tests + headless dev).
pub fn open_url(url: &str) -> Result<()> {
    if std::env::var("DEPUTYOS_DESKTOP_NO_BROWSER").ok().as_deref() == Some("1") {
        eprintln!("==> browser open skipped (DEPUTYOS_DESKTOP_NO_BROWSER=1); URL: {url}");
        return Ok(());
    }
    match webbrowser::open(url) {
        Ok(()) => {
            eprintln!("==> opened {url} in default browser");
            Ok(())
        }
        Err(e) => {
            eprintln!("==> could not open browser ({e}); please visit: {url}");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_via_env_var() {
        let _g = crate::config::test_env_lock()
            .lock()
            .expect("test_env_lock poisoned");
        let prev = std::env::var("DEPUTYOS_DESKTOP_NO_BROWSER").ok();
        std::env::set_var("DEPUTYOS_DESKTOP_NO_BROWSER", "1");
        // No panic, no actual browser launch.
        open_url("http://localhost:8088").expect("ok");
        match prev {
            Some(v) => std::env::set_var("DEPUTYOS_DESKTOP_NO_BROWSER", v),
            None => std::env::remove_var("DEPUTYOS_DESKTOP_NO_BROWSER"),
        }
    }
}
