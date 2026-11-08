// Иконка exe (только при сборке на Windows и если рядом лежит favicon.ico).
fn main() {
    #[cfg(windows)]
    if std::path::Path::new("favicon.ico").exists() {
        let mut r = winresource::WindowsResource::new();
        r.set_icon("favicon.ico");
        r.compile().expect("не удалось встроить иконку");
    }
}
