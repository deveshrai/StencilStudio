//! Platform services that differ between desktop and mobile: choosing files,
//! choosing where to save, showing messages and printing.
//!
//! Desktop uses native dialogs (rfd). Android has no file-dialog API reachable from a
//! NativeActivity, so files are listed in an in-app browser and outputs are written
//! straight to `Documents/StencilStudio`.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickKind {
    Image,
    Palette,
}

impl PickKind {
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            PickKind::Image => &["jpg", "jpeg", "png", "webp"],
            PickKind::Palette => &["json"],
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            PickKind::Image => "Open Image",
            PickKind::Palette => "Load Palette",
        }
    }
}

/// Whether this platform shows native file dialogs (otherwise the in-app browser is used).
pub const NATIVE_DIALOGS: bool = cfg!(not(target_os = "android"));

#[cfg(not(target_os = "android"))]
mod imp {
    use super::*;

    pub fn pick_file(kind: PickKind) -> Option<PathBuf> {
        let filter = match kind {
            PickKind::Image => "Images",
            PickKind::Palette => "Palette",
        };
        rfd::FileDialog::new()
            .set_title(kind.title())
            .add_filter(filter, kind.extensions())
            .pick_file()
    }

    pub fn save_path(title: &str, filter: &str, ext: &str, default_name: &str) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title(title)
            .add_filter(filter, &[ext])
            .set_file_name(default_name)
            .save_file()
    }

    pub fn message(title: &str, text: &str, error: bool) {
        rfd::MessageDialog::new()
            .set_level(if error {
                rfd::MessageLevel::Error
            } else {
                rfd::MessageLevel::Info
            })
            .set_title(title)
            .set_description(text)
            .show();
    }

    pub fn print_pdf(pdf: &Path) -> Result<String, String> {
        crate::print::print_pdf(pdf).map(|_| "Sent to the print dialog".to_string())
    }

    pub fn list_files(_kind: PickKind) -> Vec<PathBuf> {
        Vec::new()
    }

    pub fn ensure_media_permission() -> bool {
        true
    }
}

#[cfg(target_os = "android")]
mod imp {
    use super::*;
    use std::sync::OnceLock;

    pub static APP: OnceLock<slint::android::AndroidApp> = OnceLock::new();

    const STORAGE: &str = "/storage/emulated/0";

    /// `Documents/StencilStudio`, created on demand.
    pub fn output_dir() -> PathBuf {
        let d = Path::new(STORAGE).join("Documents").join("StencilStudio");
        let _ = std::fs::create_dir_all(&d);
        d
    }

    pub fn pick_file(_kind: PickKind) -> Option<PathBuf> {
        None // handled by the in-app browser
    }

    /// No save dialog on Android: write into Documents/StencilStudio, never overwriting.
    pub fn save_path(_title: &str, _filter: &str, ext: &str, default_name: &str) -> Option<PathBuf> {
        let dir = output_dir();
        let stem = Path::new(default_name)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "export".into());
        let mut p = dir.join(format!("{stem}.{ext}"));
        let mut n = 2;
        while p.exists() {
            p = dir.join(format!("{stem}-{n}.{ext}"));
            n += 1;
        }
        Some(p)
    }

    pub fn message(_title: &str, _text: &str, _error: bool) {
        // Shown in-app by the caller (see `lib.rs::show_message`).
    }

    pub fn print_pdf(pdf: &Path) -> Result<String, String> {
        let dest = save_path("", "", "pdf", &pdf.file_name().unwrap_or_default().to_string_lossy())
            .ok_or("no output folder")?;
        std::fs::copy(pdf, &dest).map_err(|e| e.to_string())?;
        Ok(format!(
            "Saved to {} — open it in the Files app to print",
            display(&dest)
        ))
    }

    /// Shorten /storage/emulated/0/... to something people recognise.
    pub fn display(p: &Path) -> String {
        p.strip_prefix(STORAGE)
            .map(|r| r.display().to_string())
            .unwrap_or_else(|_| p.display().to_string())
    }

    /// Image / palette files in the usual shared folders, newest first.
    pub fn list_files(kind: PickKind) -> Vec<PathBuf> {
        let roots: &[&str] = match kind {
            PickKind::Image => &["Pictures", "DCIM", "Download", "Documents"],
            PickKind::Palette => &["Documents", "Download"],
        };
        let exts = kind.extensions();
        let mut out = Vec::new();
        fn walk(dir: &Path, depth: u32, exts: &[&str], out: &mut Vec<PathBuf>) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                if p.is_dir() {
                    if depth > 0 {
                        walk(&p, depth - 1, exts, out);
                    }
                } else if p
                    .extension()
                    .map(|x| exts.contains(&x.to_string_lossy().to_ascii_lowercase().as_str()))
                    .unwrap_or(false)
                {
                    out.push(p);
                }
            }
        }
        for r in roots {
            walk(&Path::new(STORAGE).join(r), 3, exts, &mut out);
        }
        out.sort_by_key(|p| std::cmp::Reverse(std::fs::metadata(p).and_then(|m| m.modified()).ok()));
        out.dedup();
        out
    }

    /// Check (and if needed request) permission to read photos. Returns true if granted.
    pub fn ensure_media_permission() -> bool {
        match media_permission() {
            Ok(g) => g,
            Err(e) => {
                log::warn!("permission check failed: {e}");
                false
            }
        }
    }

    fn media_permission() -> jni::errors::Result<bool> {
        use jni::objects::{JObject, JValue};
        let Some(app) = APP.get() else { return Ok(false) };
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM)? };
        let mut env = vm.attach_current_thread()?;
        let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
        let sdk = env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?.i()?;
        let perm_name = if sdk >= 33 {
            "android.permission.READ_MEDIA_IMAGES"
        } else {
            "android.permission.READ_EXTERNAL_STORAGE"
        };
        let perm = env.new_string(perm_name)?;
        let state = env
            .call_method(
                &activity,
                "checkSelfPermission",
                "(Ljava/lang/String;)I",
                &[JValue::Object(&perm)],
            )?
            .i()?;
        if state == 0 {
            return Ok(true);
        }
        let arr = env.new_object_array(1, "java/lang/String", &perm)?;
        env.call_method(
            &activity,
            "requestPermissions",
            "([Ljava/lang/String;I)V",
            &[JValue::Object(&arr), JValue::Int(1)],
        )?;
        Ok(false)
    }
}

pub use imp::*;

/// Human-friendly path for status messages.
pub fn pretty(p: &Path) -> String {
    #[cfg(target_os = "android")]
    {
        imp::display(p)
    }
    #[cfg(not(target_os = "android"))]
    {
        p.display().to_string()
    }
}
