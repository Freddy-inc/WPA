# Thot Android (debug WebView shell)

Sideloadable **debug APK** for phone feedback. The Rust backend is **not** embedded — run it on a PC with `THOT_BIND=0.0.0.0` and point the app at `http://<LAN-IP>:8787`.

If you see a **blank white screen**, the server URL is almost always unreachable — install **0.1.1+** (error panel) and follow the checklist in store `docs/thot-android-apk.md`.

```bash
export ANDROID_HOME=$HOME/Android/Sdk
echo "sdk.dir=$ANDROID_HOME" > local.properties
./gradlew assembleDebug
# → app/build/outputs/apk/debug/app-debug.apk  (versionName 0.1.1-debug)
```
