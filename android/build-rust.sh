#!/usr/bin/env bash
# Собирает libispcheck.so для Android в app/src/main/jniLibs.
# Нужны: rustup targets (aarch64-linux-android armv7-linux-androideabi x86_64-linux-android),
# `cargo install cargo-ndk`, Android NDK (переменная ANDROID_NDK_HOME).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -o android/app/src/main/jniLibs build --profile "${PROFILE:-release}" --lib
