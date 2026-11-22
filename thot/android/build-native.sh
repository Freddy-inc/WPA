#!/usr/bin/env bash
# Cross-compile libthot.so for Android (arm64 + x86_64) into jniLibs/.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NDK="${ANDROID_NDK_HOME:-${ANDROID_HOME:-$HOME/Android/Sdk}/ndk/26.1.10909125}"
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64"
API=26

build_one() {
  local triple="$1" abi="$2" linker_triple="$3"
  export AR="$TOOLCHAIN/bin/llvm-ar"
  export CC="$TOOLCHAIN/bin/${linker_triple}${API}-clang"
  export CXX="$TOOLCHAIN/bin/${linker_triple}${API}-clang++"
  local linker_var
  linker_var="$(echo "CARGO_TARGET_${triple^^}_LINKER" | tr '-' '_')"
  # shellcheck disable=SC2086
  export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TOOLCHAIN/bin/aarch64-linux-android${API}-clang"
  export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$TOOLCHAIN/bin/x86_64-linux-android${API}-clang"
  (cd "$ROOT" && cargo build --release --target "$triple")
  mkdir -p "$ROOT/android/app/src/main/jniLibs/$abi"
  cp -f "$ROOT/target/$triple/release/libthot.so" \
    "$ROOT/android/app/src/main/jniLibs/$abi/libthot.so"
  echo "OK $abi $(wc -c < "$ROOT/android/app/src/main/jniLibs/$abi/libthot.so") bytes"
}

# Sync UI assets into the APK
rm -rf "$ROOT/android/app/src/main/assets/thot-static"
mkdir -p "$ROOT/android/app/src/main/assets"
cp -a "$ROOT/static" "$ROOT/android/app/src/main/assets/thot-static"

build_one aarch64-linux-android arm64-v8a aarch64-linux-android
build_one x86_64-linux-android x86_64 x86_64-linux-android
echo "Native libs ready."
