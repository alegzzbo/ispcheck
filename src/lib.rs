#[allow(unused_macros)]
macro_rules! logf {
    ($m:expr, $($a:tt)*) => { $crate::state::log($m, format!($($a)*)) };
}

mod checks;
mod config;
mod dns;
mod mtr;
mod net;
mod state;
mod sys;

#[cfg(all(feature = "cli", not(feature = "gui")))]
mod cli;
#[cfg(feature = "gui")]
mod gui;
// на Android обновление = установка нового APK, модуль нужен только для VERSION
#[cfg(feature = "gui")]
#[cfg_attr(target_os = "android", allow(dead_code))]
mod update;

#[cfg(not(any(feature = "gui", feature = "cli")))]
compile_error!("включите feature `gui` (по умолчанию) или `cli`: cargo build --no-default-features --features cli");

pub fn run() {
    // TLS-провайдер ring (без C++/cmake/NASM)
    let _ = rustls::crypto::ring::default_provider().install_default();
    #[cfg(feature = "gui")]
    if let Err(e) = gui::run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
    #[cfg(all(feature = "cli", not(feature = "gui")))]
    cli::run();
}

/// Точка входа Android (GameActivity загружает libispcheck.so и вызывает android_main).
#[cfg(all(target_os = "android", feature = "gui"))]
#[no_mangle]
fn android_main(app: android_activity::AndroidApp) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    if let Err(e) = gui::run_android(app) {
        eprintln!("{e}");
    }
}
