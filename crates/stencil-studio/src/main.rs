// Hide the console window on Windows release builds.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

fn main() -> Result<(), slint::PlatformError> {
    // Allow `stencil-studio path/to/image.jpg`.
    stencil_studio::run(std::env::args_os().nth(1).map(std::path::PathBuf::from))
}
