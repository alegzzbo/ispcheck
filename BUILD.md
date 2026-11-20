# Сборка ispcheck (Rust)

Настройки провайдера: `src/config.rs`. Версия: `version` в `Cargo.toml`.
Иконка (необязательно): `favicon.ico` рядом с `Cargo.toml`. Она попадает в иконку окна/панели задач на всех ОС и в сам exe при сборке на Windows. Файл нужно закоммитить в репозиторий. После добавления или замены иконки пересоберите проект.
Шрифт символов (✔ ✖ ⚠ ▶): `assets/symbols.ttf`, подмножество DejaVu Sans Mono, лицензия в `assets/LICENSE-DejaVu.txt`.

## Windows
1. Скачать и запустить https://rustup.rs (`rustup-init.exe`), везде Enter (профиль по умолчанию, MSVC).
2. Если установщик предложит Visual Studio Build Tools, принять. Нужен компонент
   «Desktop development with C++» (MSVC + Windows SDK). Перезапустить терминал.
3. В папке проекта: `cargo build --release`
4. Готово: `target\release\ispcheck.exe` (один файл, без DLL, без консольного окна).

Вариант без Visual Studio: `rustup default stable-gnu`, поставить MSYS2 и в его оболочке
`pacman -S mingw-w64-x86_64-gcc`, добавить `C:\msys64\mingw64\bin` в PATH, затем `cargo build --release`.

## Linux
1. `sudo apt install build-essential curl` (Fedora: `sudo dnf install gcc`)
2. `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`, затем `source ~/.cargo/env`
3. `cargo build --release` → `target/release/ispcheck`
4. Заголовочные файлы X11/GL не нужны (библиотеки подгружаются при запуске, на десктопе они есть).
5. Для MTR/трассировки нужны права на raw ICMP: `sudo setcap cap_net_raw+ep target/release/ispcheck`
   (либо запуск от root). Без них работает только обычный пинг.

## macOS
1. `xcode-select --install`
2. rustup, как для Linux.
3. `cargo build --release` → `target/release/ispcheck`
4. Универсальный бинарь:
   `rustup target add aarch64-apple-darwin x86_64-apple-darwin`
   `cargo build --release --target aarch64-apple-darwin`
   `cargo build --release --target x86_64-apple-darwin`
   `lipo -create -output ispcheck target/aarch64-apple-darwin/release/ispcheck target/x86_64-apple-darwin/release/ispcheck`
5. Неподписанное приложение блокирует Gatekeeper: ПКМ → «Открыть» или
   `xattr -dr com.apple.quarantine ispcheck`.

## Кросс-сборка Windows из Linux (проверено)
```
rustup target add x86_64-pc-windows-gnu
sudo apt install gcc-mingw-w64-x86-64
cargo build --release --target x86_64-pc-windows-gnu
```
Результат: `target/x86_64-pc-windows-gnu/release/ispcheck.exe`.

## Автообновление
Формат `latest.json` и ключ ed25519 те же, что у Go-версии. Ключи платформ: `windows-amd64`,
`linux-amd64`, `darwin-arm64` и т. д. Подписывается sha256 бинарника (сам digest, не hex).

## Консольная сборка (без GUI)
```
cargo build --release --no-default-features --features cli
```
Не требует OpenGL и графических библиотек, собирается заметно быстрее. Статический вариант для Linux:
```
rustup target add x86_64-unknown-linux-musl
sudo apt install musl-tools
cargo build --release --no-default-features --features cli --target x86_64-unknown-linux-musl
```
Параметры: `ispcheck --help`.

## macOS: приложение и dmg
Скрипт `packaging/macos/package.sh <бинарь> <версия> <папка>` (только на macOS) собирает `ISP Check.app` и `ispcheck-macos-universal.dmg`. Перед использованием замените `BUNDLE_ID` в начале скрипта. Иконка берётся из `favicon.ico` (желательно квадратный, 512 px и больше). В CI это делает задание `macos-universal`.

## Android
Нужны: JDK 17, Android SDK + NDK (`ANDROID_NDK_HOME`), Gradle 8.9+, `cargo install cargo-ndk`,
`rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android`.
```
bash android/build-rust.sh                    # libispcheck.so -> android/app/src/main/jniLibs
gradle -p android assembleRelease             # android/app/build/outputs/apk/release/app-release.apk
```
Подпись релиза: `ANDROID_KEYSTORE_FILE`, `ANDROID_KS_PASS`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASS` (иначе debug-ключ).
В CI: секрет `ANDROID_KEYSTORE_B64` (base64 от .jks) и три остальных. Ключ храните постоянно: APK с другой подписью не обновится поверх.
Иконка: добавьте `mipmap-*/ic_launcher.png` в `android/app/src/main/res` и замените `android:icon` в манифесте.
Rust собирается как cdylib (`src/lib.rs`), десктопный бинарь `src/main.rs` вызывает `ispcheck::run()`.
