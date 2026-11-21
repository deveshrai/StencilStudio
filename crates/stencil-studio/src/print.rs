//! Hand a PDF to the operating system's printing flow.

use std::path::Path;
use std::process::Command;

/// Open the platform print dialog for `pdf` (or the closest equivalent).
pub fn print_pdf(pdf: &Path) -> Result<(), String> {
    let path = pdf.to_string_lossy().to_string();

    #[cfg(target_os = "macos")]
    {
        // Preview shows the standard macOS print sheet with the document.
        let esc = path.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            "tell application \"Preview\"\nactivate\nopen POSIX file \"{esc}\"\ndelay 0.5\nprint front document with print dialog\nend tell"
        );
        let ok = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Ok(());
        }
        // Fallback: just open it; the user can press ⌘P.
        return Command::new("open")
            .arg(&path)
            .status()
            .map(|_| ())
            .map_err(|e| e.to_string());
    }

    #[cfg(target_os = "windows")]
    {
        let esc = path.replace('\'', "''");
        return Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("Start-Process -FilePath '{esc}' -Verb Print"),
            ])
            .status()
            .map(|_| ())
            .map_err(|e| e.to_string());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // CUPS; fall back to the desktop's default viewer.
        if Command::new("lp")
            .arg(&path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Ok(());
        }
        return Command::new("xdg-open")
            .arg(&path)
            .status()
            .map(|_| ())
            .map_err(|e| e.to_string());
    }

    #[allow(unreachable_code)]
    Err("Printing is not supported on this platform yet".into())
}
