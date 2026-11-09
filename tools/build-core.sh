#!/usr/bin/env bash
# Builds the Rust core for Android and generates its Kotlin bindings.
# Needs cargo-ndk and ANDROID_NDK_HOME.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
module=$root/android/core/src/main

cd "$root/core"
rm -rf "$module/jniLibs" "$module/java/uniffi"

cargo ndk --platform 26 -t arm64-v8a -t armeabi-v7a -t x86_64 -o "$module/jniLibs" \
    build --release --locked -p tui-ffi

# The bindings are read out of a host build of the same library.
cargo build --locked -p tui-ffi --features bindgen
cargo run --locked -p tui-ffi --features bindgen --bin uniffi-bindgen -- \
    generate --library target/debug/libtui_core.so --language kotlin --no-format --out-dir "$module/java"

find "$module/jniLibs" "$module/java/uniffi" -type f -exec ls -l {} +
