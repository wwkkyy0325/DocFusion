//! The desktop window.

// Without this the window is a console application on Windows, which is a black
// rectangle behind the application and a surprise for whoever started it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    docfusion_gui::run();
}
