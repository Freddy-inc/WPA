//! JNI bridge — start embedded axum on 127.0.0.1 for the Android WebView.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};

use jni::objects::{JClass, JString};
use jni::sys::jint;
use jni::JNIEnv;

use crate::server::{self, ServerConfig};

static STARTED: AtomicBool = AtomicBool::new(false);
static BOUND_PORT: AtomicU16 = AtomicU16::new(0);

/// `NativeServer.start(dataDir, staticDir, preferredPort, groqKey) -> boundPort` (or negative error).
#[no_mangle]
pub extern "system" fn Java_app_thot_mobile_NativeServer_start<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    data_dir: JString<'local>,
    static_dir: JString<'local>,
    preferred_port: jint,
    groq_key: JString<'local>,
) -> jint {
    let groq = env
        .get_string(&groq_key)
        .ok()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.trim().is_empty());
    // Hot-update env so a later LlmClient::default() / file read can see the key.
    // Full LLM client is created once at boot — cold-restart app after first key paste.
    if let Some(ref k) = groq {
        std::env::set_var("GROQ_API_KEY", k.trim());
    }

    if STARTED.load(Ordering::SeqCst) {
        let p = BOUND_PORT.load(Ordering::SeqCst);
        return if p > 0 { p as jint } else { -1 };
    }

    let data = match env.get_string(&data_dir) {
        Ok(s) => PathBuf::from(s.to_string_lossy().as_ref()),
        Err(_) => return -2,
    };
    let static_path = match env.get_string(&static_dir) {
        Ok(s) => PathBuf::from(s.to_string_lossy().as_ref()),
        Err(_) => return -3,
    };

    let port = if preferred_port > 0 && preferred_port < 65535 {
        preferred_port as u16
    } else {
        8787
    };

    let config = ServerConfig {
        data_dir: data,
        static_dir: static_path,
        host: "127.0.0.1".into(),
        port,
        groq_api_key: groq,
    };

    match server::start_background(config) {
        Ok(bound) => {
            STARTED.store(true, Ordering::SeqCst);
            BOUND_PORT.store(bound, Ordering::SeqCst);
            bound as jint
        }
        Err(err) => {
            eprintln!("thot native start failed: {err:#}");
            -10
        }
    }
}

/// `NativeServer.port() -> boundPort`
#[no_mangle]
pub extern "system" fn Java_app_thot_mobile_NativeServer_port<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jint {
    BOUND_PORT.load(Ordering::SeqCst) as jint
}

/// `NativeServer.isRunning() -> 1|0`
#[no_mangle]
pub extern "system" fn Java_app_thot_mobile_NativeServer_isRunning<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jint {
    if STARTED.load(Ordering::SeqCst) && BOUND_PORT.load(Ordering::SeqCst) > 0 {
        1
    } else {
        0
    }
}
