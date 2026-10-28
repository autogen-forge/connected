// Keeps a console window from opening behind the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    connected_desktop::run();
}
