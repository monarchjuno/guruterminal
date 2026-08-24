use std::io;

#[cfg(target_os = "macos")]
use std::process::Command;

/// Opens an already-authorized HTTP(S) URL in the system browser.
///
/// Callers own URL policy. Keeping the OS launch in one native boundary makes
/// OAuth and ordinary external links behave identically.
pub(crate) fn open(url: &str) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let status = macos_open_command(url).status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "macOS open exited with status {status}"
            )));
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        open::that(url)
    }
}

#[cfg(target_os = "macos")]
fn macos_open_command(url: &str) -> Command {
    let mut command = Command::new("/usr/bin/open");
    // A hidden automation-owned browser process can otherwise absorb the
    // LaunchServices request without presenting a window. A fresh application
    // instance makes link activation visible and also recovers cleanly after a
    // user closes the browser during OAuth.
    command.arg("-n").arg("--").arg(url);
    command
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn macos_links_request_a_fresh_browser_instance() {
        let command = macos_open_command("https://example.com/path?value=one");
        assert_eq!(command.get_program(), OsStr::new("/usr/bin/open"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                OsStr::new("-n"),
                OsStr::new("--"),
                OsStr::new("https://example.com/path?value=one"),
            ]
        );
    }
}
