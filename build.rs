// Иконка: если рядом с Cargo.toml лежит favicon.ico, она встраивается в окно (cfg has_icon)
// и, при сборке на Windows, в сам exe.
fn main() {
    println!("cargo:rerun-if-changed=favicon.ico");
    println!("cargo::rustc-check-cfg=cfg(has_icon)");
    if std::path::Path::new("favicon.ico").exists() {
        println!("cargo:rustc-cfg=has_icon");
        #[cfg(windows)]
        {
            let mut r = winresource::WindowsResource::new();
            r.set_icon("favicon.ico");
            if let Err(e) = r.compile() {
                println!("cargo:warning=иконка exe не встроена: {e}");
            }
        }
    }
}
