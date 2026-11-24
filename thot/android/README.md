# Thot Android — embedded Rust (debug)

The APK starts **axum inside the process** (`libthot.so` via JNI) and loads `http://127.0.0.1:8787` in a WebView.

**No external PC/cloud URL** for basic UI + SQLite. Optional Groq key via the gear icon (device-only).

```bash
# Native libs (NDK) + sync static assets
export ANDROID_NDK_HOME=$HOME/Android/Sdk/ndk/26.1.10909125
./build-native.sh

# APK
export ANDROID_HOME=$HOME/Android/Sdk
echo "sdk.dir=$ANDROID_HOME" > local.properties
./gradlew assembleDebug
# → app/build/outputs/apk/debug/app-debug.apk
```

Full notes: store `docs/thot-android-apk.md`.
