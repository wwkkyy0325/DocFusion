//! Tauri build script: reads `tauri.conf.json` and generates the window context
//! that `tauri::generate_context!` compiles in.

fn main() {
    tauri_build::build()
}
