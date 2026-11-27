use crate::terminal;
use arboard::Clipboard;
#[cfg(target_os = "linux")]
use arboard::{GetExtLinux, LinuxClipboardKind, SetExtLinux};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

pub(crate) enum CopyResult {
    Copied,
    Noop,
}

/// Reads the PRIMARY selection, the one middle-click pastes. Only Linux has
/// one, and on Wayland only when the compositor offers it. Blocks until the
/// app owning the selection answers, so never call it on the UI thread.
#[cfg(target_os = "linux")]
pub(crate) fn read_primary_selection() -> Option<String> {
    let mut clipboard = Clipboard::new().ok()?;
    let text = clipboard
        .get()
        .clipboard(LinuxClipboardKind::Primary)
        .text()
        .ok()?;
    (!text.is_empty()).then_some(text)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn read_primary_selection() -> Option<String> {
    None
}

/// Blocks, so never call it on the UI thread.
pub(crate) fn read_text() -> Option<String> {
    let text = Clipboard::new().ok()?.get_text().ok()?;
    (!text.is_empty()).then_some(text)
}

pub(crate) struct ClipboardState {
    native: Option<Clipboard>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostClipboard {
    WlCopy,
    Xclip,
    Xsel,
}

impl HostClipboard {
    fn command(self) -> &'static str {
        match self {
            Self::WlCopy => "wl-copy",
            Self::Xclip => "xclip",
            Self::Xsel => "xsel",
        }
    }

    fn args(self) -> &'static [&'static str] {
        match self {
            Self::WlCopy => &[],
            Self::Xclip => &["-selection", "clipboard", "-in"],
            Self::Xsel => &["--clipboard", "--input"],
        }
    }
}

fn in_container() -> bool {
    static IN_CONTAINER: OnceLock<bool> = OnceLock::new();
    *IN_CONTAINER.get_or_init(|| {
        std::path::Path::new("/run/.containerenv").exists()
            || std::path::Path::new("/.dockerenv").exists()
            || std::env::var_os("container").is_some()
    })
}

fn host_spawn_path() -> Option<&'static str> {
    static HOST_SPAWN: OnceLock<Option<&'static str>> = OnceLock::new();
    HOST_SPAWN.get_or_init(|| {
        ["/usr/bin/host-spawn", "/var/home/fra/bin/host-spawn"]
            .into_iter()
            .find(|p| std::path::Path::new(p).is_file())
    }).as_deref()
}

fn host_clipboard() -> Option<HostClipboard> {
    static HOST_CLIPBOARD: OnceLock<Option<HostClipboard>> = OnceLock::new();
    *HOST_CLIPBOARD.get_or_init(detect_host_clipboard)
}

fn detect_host_clipboard() -> Option<HostClipboard> {
    if !in_container() {
        return None;
    }
    let host_spawn = host_spawn_path()?;
    for tool in [HostClipboard::WlCopy, HostClipboard::Xclip, HostClipboard::Xsel] {
        let ok = Command::new(host_spawn)
            .args(["sh", "-c", "command -v \"$1\" >/dev/null 2>&1", "sh", tool.command()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return Some(tool);
        }
    }
    None
}

fn copy_via_host(text: &str, host_spawn: &str, tool: HostClipboard) -> Result<(), String> {
    let mut child = Command::new(host_spawn)
        .arg(tool.command())
        .args(tool.args())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn {} via host-spawn: {e}", tool.command()))?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("write to {}: {e}", tool.command()))?;
    }
    drop(child.stdin.take());

    let output = child
        .wait_with_output()
        .map_err(|e| format!("wait for {}: {e}", tool.command()))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            output.status.to_string()
        } else {
            stderr
        };
        Err(format!("{}: {detail}", tool.command()))
    }
}

impl ClipboardState {
    pub(crate) fn new() -> Self {
        Self {
            native: Clipboard::new().ok(),
        }
    }

    /// Copies through the terminal alone, so a test never writes to the
    /// desktop's clipboard.
    #[cfg(test)]
    pub(crate) fn terminal_only() -> Self {
        Self { native: None }
    }

    pub(crate) fn copy_text(&mut self, text: &str) -> Result<CopyResult, String> {
        let host = Self::copy_host;
        Self::copy_text_impl(text, |t| self.copy_native(t), host, terminal::copy_to_clipboard)
    }

    fn copy_text_impl<F, H, G>(text: &str, native: F, host: H, osc52: G) -> Result<CopyResult, String>
    where
        F: FnOnce(&str) -> Result<(), String>,
        H: FnOnce(&str) -> Result<(), String>,
        G: FnOnce(&str) -> Result<(), String>,
    {
        if text.is_empty() {
            return Ok(CopyResult::Noop);
        }
        let native_err = match native(text) {
            Ok(()) => return Ok(CopyResult::Copied),
            Err(e) => e,
        };
        let host_err = match host(text) {
            Ok(()) => return Ok(CopyResult::Copied),
            Err(e) => e,
        };
        match osc52(text) {
            Ok(()) => Ok(CopyResult::Copied),
            Err(osc52_err) => {
                let mut parts = Vec::new();
                if !native_err.is_empty() {
                    parts.push(format!("native ({native_err})"));
                }
                if !host_err.is_empty() {
                    parts.push(format!("host ({host_err})"));
                }
                parts.push(format!("osc52 ({osc52_err})"));
                Err(parts.join("; "))
            }
        }
    }

    fn copy_host(text: &str) -> Result<(), String> {
        let Some(tool) = host_clipboard() else {
            return Err(String::new());
        };
        let Some(host_spawn) = host_spawn_path() else {
            return Err("host-spawn not found".into());
        };
        copy_via_host(text, host_spawn, tool)
    }

    fn copy_native(&mut self, text: &str) -> Result<(), String> {
        let Some(clipboard) = &mut self.native else {
            return Err("native clipboard unavailable".into());
        };

        #[cfg(target_os = "linux")]
        {
            let primary = clipboard
                .set()
                .clipboard(LinuxClipboardKind::Primary)
                .text(text);
            let system = clipboard
                .set()
                .clipboard(LinuxClipboardKind::Clipboard)
                .text(text);
            match (primary, system) {
                (_, Ok(())) | (Ok(()), _) => Ok(()),
                (Err(p), Err(s)) => Err(format!("primary: {p}; clipboard: {s}")),
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            clipboard.set_text(text).map_err(|e| e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn empty_text_is_noop_and_skips_all_backends() {
        let native_called = Cell::new(false);
        let host_called = Cell::new(false);
        let osc52_called = Cell::new(false);
        let result = ClipboardState::copy_text_impl(
            "",
            |_| {
                native_called.set(true);
                Ok(())
            },
            |_| {
                host_called.set(true);
                Ok(())
            },
            |_| {
                osc52_called.set(true);
                Ok(())
            },
        );
        assert!(matches!(result, Ok(CopyResult::Noop)));
        assert!(!native_called.get(), "empty text must not reach native");
        assert!(!host_called.get(), "empty text must not reach host");
        assert!(!osc52_called.get(), "empty text must not reach osc52");
    }

    #[test]
    fn native_success_skips_host_and_osc52() {
        let host_called = Cell::new(false);
        let osc52_called = Cell::new(false);
        let result = ClipboardState::copy_text_impl(
            "hello",
            |_| Ok(()),
            |_| {
                host_called.set(true);
                Ok(())
            },
            |_| {
                osc52_called.set(true);
                Ok(())
            },
        );
        assert!(matches!(result, Ok(CopyResult::Copied)));
        assert!(!host_called.get(), "host must not fire after native succeeds");
        assert!(!osc52_called.get(), "osc52 must not fire after native succeeds");
    }

    #[test]
    fn native_failure_is_recovered_by_host() {
        let osc52_called = Cell::new(false);
        let result = ClipboardState::copy_text_impl(
            "hello",
            |_| Err("unavailable".into()),
            |_| Ok(()),
            |_| {
                osc52_called.set(true);
                Ok(())
            },
        );
        assert!(matches!(result, Ok(CopyResult::Copied)));
        assert!(!osc52_called.get(), "osc52 must not fire after host succeeds");
    }

    #[test]
    fn host_failure_is_recovered_by_osc52() {
        let result = ClipboardState::copy_text_impl(
            "hello",
            |_| Err("no display".into()),
            |_| Err("host-spawn failed".into()),
            |_| Ok(()),
        );
        assert!(matches!(result, Ok(CopyResult::Copied)));
    }

    #[test]
    fn all_failing_composes_errors() {
        let result = ClipboardState::copy_text_impl(
            "hello",
            |_| Err("no display".into()),
            |_| Err("host-spawn failed".into()),
            |_| Err("stdout closed".into()),
        );
        let Err(msg) = result else {
            panic!("expected Err when all backends fail");
        };
        assert_eq!(
            msg,
            "native (no display); host (host-spawn failed); osc52 (stdout closed)"
        );
    }

    #[test]
    fn skipped_host_is_omitted_from_error() {
        let result = ClipboardState::copy_text_impl(
            "hello",
            |_| Err("no display".into()),
            |_| Err(String::new()),
            |_| Err("stdout closed".into()),
        );
        let Err(msg) = result else {
            panic!("expected Err when backends fail");
        };
        assert_eq!(msg, "native (no display); osc52 (stdout closed)");
    }

    #[test]
    fn whitespace_only_text_is_not_noop() {
        let native_called = Cell::new(false);
        let result = ClipboardState::copy_text_impl(
            "   \n\t",
            |_| {
                native_called.set(true);
                Ok(())
            },
            |_| Ok(()),
            |_| Ok(()),
        );
        assert!(matches!(result, Ok(CopyResult::Copied)));
        assert!(
            native_called.get(),
            "whitespace-only text must still be copied"
        );
    }
}
