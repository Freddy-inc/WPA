//! Shared axum boot — desktop `main` and Android JNI both call `run`.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::EnvFilter;

use crate::api::{router as api_router, AppState};
use crate::db::Db;
use crate::llm::{LlmClient, LlmConfig};
use crate::web_verify::WebVerifier;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub data_dir: PathBuf,
    pub static_dir: PathBuf,
    pub host: String,
    pub port: u16,
    /// Optional Groq key injected by Android settings (never logged).
    pub groq_api_key: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from(std::env::var("THOT_DATA_DIR").unwrap_or_else(|_| "data".into())),
            static_dir: resolve_static_dir(),
            host: std::env::var("THOT_BIND").unwrap_or_else(|_| "127.0.0.1".into()),
            port: std::env::var("THOT_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(8787),
            groq_api_key: None,
        }
    }
}

/// Block forever serving HTTP (desktop). Returns only on fatal bind/serve error.
pub async fn run(config: ServerConfig) -> Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("thot=info")),
        )
        .try_init();

    boot(config).await
}

async fn boot(config: ServerConfig) -> Result<()> {
    let data_dir = config.data_dir;
    let _ = std::fs::create_dir_all(&data_dir);
    let docs_dir = data_dir.join("docs");
    let _ = std::fs::create_dir_all(&docs_dir);
    let podcasts_dir = data_dir.join("podcasts");
    let _ = std::fs::create_dir_all(&podcasts_dir);

    // Optional on-device key file (Android) — never commit secrets.
    if let Some(key) = &config.groq_api_key {
        if !key.trim().is_empty() {
            std::env::set_var("GROQ_API_KEY", key.trim());
        }
    } else {
        let key_file = data_dir.join("groq.key");
        if key_file.is_file() {
            if let Ok(k) = std::fs::read_to_string(&key_file) {
                let k = k.trim().to_string();
                if !k.is_empty() {
                    std::env::set_var("GROQ_API_KEY", k);
                }
            }
        }
    }

    let db_path = data_dir.join("thot.db");
    let db = Db::connect(db_path.to_str().unwrap_or("data/thot.db")).await?;

    if let Ok(Some(course)) = db.active_course().await {
        if let Ok(Some(active)) = db.active_document(course.id).await {
            let dest = docs_dir.join(format!("{}.pdf", active.id));
            let legacy = data_dir.join("course.pdf");
            if !dest.is_file() && legacy.is_file() {
                let _ = std::fs::copy(&legacy, &dest);
                tracing::info!(id = active.id, "migrated legacy course.pdf into docs/");
            }
        }
    }

    let llm = LlmClient::new(LlmConfig::default());
    tracing::info!(
        provider = llm.provider_label(),
        groq = llm.has_groq_key(),
        groq_key_env = llm.groq_key_env_name().unwrap_or("none"),
        "LLM client ready"
    );

    let state = AppState {
        db,
        llm,
        verifier: WebVerifier::new(),
        data_dir,
    };

    let static_dir = config.static_dir;
    if !static_dir.join("index.html").is_file() {
        return Err(anyhow!(
            "static UI missing at {}/index.html",
            static_dir.display()
        ));
    }
    let index = static_dir.join("index.html");

    let app = Router::new()
        .merge(api_router(state))
        .fallback_service(ServeDir::new(&static_dir).not_found_service(ServeFile::new(&index)))
        .layer(CorsLayer::permissive());

    let ip: std::net::IpAddr = config
        .host
        .parse()
        .map_err(|e| anyhow!("THOT_BIND invalide ({}): {e}", config.host))?;
    let addr = SocketAddr::from((ip, config.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    tracing::info!("Thot listening on http://{bound}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Spawn the server on a background Tokio runtime (Android JNI). Returns the bound port.
pub fn start_background(config: ServerConfig) -> Result<u16> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("thot=info,warn"))
        .try_init();

    let host = config.host.clone();
    let port = config.port;
    let (tx, rx) = std::sync::mpsc::channel::<Result<u16>>();

    std::thread::Builder::new()
        .name("thot-axum".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("thot-worker")
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send(Err(anyhow!("tokio runtime: {e}")));
                    return;
                }
            };
            rt.block_on(async move {
                // Probe bind first so JNI can report the port quickly.
                let ip: std::net::IpAddr = match host.parse() {
                    Ok(ip) => ip,
                    Err(e) => {
                        let _ = tx.send(Err(anyhow!("bad host: {e}")));
                        return;
                    }
                };
                let addr = SocketAddr::from((ip, port));
                let listener = match tokio::net::TcpListener::bind(addr).await {
                    Ok(l) => l,
                    Err(e) => {
                        let _ = tx.send(Err(anyhow!("bind {addr}: {e}")));
                        return;
                    }
                };
                let bound = match listener.local_addr() {
                    Ok(a) => a.port(),
                    Err(e) => {
                        let _ = tx.send(Err(anyhow!("local_addr: {e}")));
                        return;
                    }
                };
                let _ = tx.send(Ok(bound));

                // Rebuild full app using same config — re-bind is awkward; serve this listener.
                if let Err(e) = serve_on_listener(listener, config).await {
                    tracing::error!(error = %e, "thot server exited");
                }
            });
        })
        .map_err(|e| anyhow!("spawn thot thread: {e}"))?;

    rx.recv()
        .map_err(|_| anyhow!("server start channel closed"))?
}

async fn serve_on_listener(
    listener: tokio::net::TcpListener,
    config: ServerConfig,
) -> Result<()> {
    // Duplicate boot without second bind.
    let data_dir = config.data_dir;
    let _ = std::fs::create_dir_all(&data_dir);
    let docs_dir = data_dir.join("docs");
    let _ = std::fs::create_dir_all(&docs_dir);
    let _ = std::fs::create_dir_all(data_dir.join("podcasts"));

    if let Some(key) = &config.groq_api_key {
        if !key.trim().is_empty() {
            std::env::set_var("GROQ_API_KEY", key.trim());
        }
    } else {
        let key_file = data_dir.join("groq.key");
        if key_file.is_file() {
            if let Ok(k) = std::fs::read_to_string(&key_file) {
                let k = k.trim().to_string();
                if !k.is_empty() {
                    std::env::set_var("GROQ_API_KEY", k);
                }
            }
        }
    }

    let db_path = data_dir.join("thot.db");
    let db = Db::connect(db_path.to_str().unwrap_or("data/thot.db")).await?;
    let llm = LlmClient::new(LlmConfig::default());
    tracing::info!(
        provider = llm.provider_label(),
        groq = llm.has_groq_key(),
        "LLM client ready (embedded)"
    );

    let state = AppState {
        db,
        llm,
        verifier: WebVerifier::new(),
        data_dir,
    };
    let static_dir = config.static_dir;
    let index = static_dir.join("index.html");
    let app = Router::new()
        .merge(api_router(state))
        .fallback_service(ServeDir::new(&static_dir).not_found_service(ServeFile::new(&index)))
        .layer(CorsLayer::permissive());

    axum::serve(listener, app).await?;
    Ok(())
}

pub fn resolve_static_dir() -> PathBuf {
    let candidates = [
        PathBuf::from("static"),
        PathBuf::from("thot/static"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static"),
    ];
    for c in candidates {
        if c.join("index.html").exists() {
            return c;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static")
}
