//! Thot MVP vertical slice — axum server.

mod api;
mod db;
mod generate;
mod glossary;
mod llm;
mod pdf;
mod podcast;
mod source;
mod web_verify;

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::EnvFilter;

use crate::api::{router as api_router, AppState};
use crate::db::Db;
use crate::llm::{LlmClient, LlmConfig};
use crate::web_verify::WebVerifier;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("thot=info".parse()?))
        .init();

    let data_dir = PathBuf::from(std::env::var("THOT_DATA_DIR").unwrap_or_else(|_| "data".into()));
    let _ = std::fs::create_dir_all(&data_dir);
    let docs_dir = data_dir.join("docs");
    let _ = std::fs::create_dir_all(&docs_dir);
    let db_path = data_dir.join("thot.db");
    let db = Db::connect(db_path.to_str().unwrap_or("data/thot.db")).await?;

    // Bridge legacy single course.pdf into docs/{id}.pdf when missing.
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

    let static_dir = resolve_static_dir();
    let index = static_dir.join("index.html");

    let app = Router::new()
        .merge(api_router(state))
        .fallback_service(
            ServeDir::new(&static_dir).not_found_service(ServeFile::new(&index)),
        )
        .layer(CorsLayer::permissive());

    let port: u16 = std::env::var("THOT_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8787);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Thot listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn resolve_static_dir() -> PathBuf {
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
