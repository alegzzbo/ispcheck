#![cfg_attr(all(windows, not(debug_assertions), feature = "gui"), windows_subsystem = "windows")]

#[allow(unused_macros)]
macro_rules! logf {
    ($m:expr, $($a:tt)*) => { $crate::state::log($m, format!($($a)*)) };
}

mod checks;
mod config;
mod dns;
mod mtr;
mod state;
mod sys;

#[cfg(not(feature = "gui"))]
mod cli;
#[cfg(feature = "gui")]
mod gui;
#[cfg(feature = "gui")]
mod update;

fn main() {
    // TLS-провайдер ring (без C++/cmake/NASM)
    let _ = rustls::crypto::ring::default_provider().install_default();
    #[cfg(feature = "gui")]
    if let Err(e) = gui::run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
    #[cfg(not(feature = "gui"))]
    cli::run();
}
