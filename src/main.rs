#![cfg_attr(all(windows, not(debug_assertions), feature = "gui"), windows_subsystem = "windows")]

fn main() {
    ispcheck::run();
}
