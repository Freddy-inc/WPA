//! HTTP API + AppState for the Thot vertical slice.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::Db;
use crate::generate::{self, material_sufficient_for_quiz, GenerationResult};
use crate::glossary::{self, GlossaryResult};
use crate::llm::LlmClient;
use crate::pdf;
use crate::web_verify::WebVerifier;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub llm: LlmClient,
    pub verifier: WebVerifier,
    /// On-disk copy of the active course PDF (for the page viewer).
    pub pdf_path: PathBuf,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/status", get(status))
        .route("/api/languages", put(set_languages))
        .route("/api/import", post(import_pdf))
        .route("/api/course/pdf", get(course_pdf))
        .route("/api/glossary", post(define_glossary))
        .route("/api/generate", post(generate_content))
        .route("/api/fiches", get(list_fiches))
        .route("/api/fiches/{id}", get(get_fiche).put(update_fiche))
        .route("/api/quiz", get(get_quiz))
        .route("/api/quiz/attempt", post(quiz_attempt))
        .route("/api/pages/open", post(open_page))
        .route("/api/progression", get(progression))
        .with_state(Arc::new(state))
}

#[derive(Serialize)]
struct Health {
    ok: bool,
    provider: String,
    groq_key_present: bool,
    stub: bool,
}

async fn health(State(st): State<Arc<AppState>>) -> Json<Health> {
    Json(Health {
        ok: true,
        provider: st.llm.provider_label().into(),
        groq_key_present: st.llm.has_groq_key(),
        stub: st.llm.is_stub(),
    })
}

async fn status(State(st): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
    let course = st.db.get_course().await.map_err(ApiError::from)?;
    let fiches = st.db.list_fiches().await.map_err(ApiError::from)?;
    let quiz = st.db.get_quiz().await.map_err(ApiError::from)?;
    let quiz_count = quiz
        .as_ref()
        .and_then(|q| serde_json::from_str::<Vec<serde_json::Value>>(&q.questions_json).ok())
        .map(|v| v.len())
        .unwrap_or(0);

    let has_pdf = st.pdf_path.is_file();
    Ok(Json(json!({
        "provider": st.llm.provider_label(),
        "groq_key_present": st.llm.has_groq_key(),
        "course": course.as_ref().map(|c| json!({
            "filename": c.filename,
            "char_count": c.text.chars().count(),
            "l1": c.l1,
            "l2": c.l2,
            "preview": c.text.chars().take(280).collect::<String>(),
            "has_text": !c.text.trim().is_empty(),
            "has_pdf": has_pdf && !c.filename.is_empty(),
            "pdf_url": if has_pdf && !c.filename.is_empty() { "/api/course/pdf" } else { "" },
        })),
        "fiches_count": fiches.len(),
        "quiz_questions": quiz_count,
        "progression": st.db.progression_snapshot().await.map_err(ApiError::from)?,
    })))
}

#[derive(Deserialize)]
struct LanguagesBody {
    l1: String,
    l2: String,
}

async fn set_languages(
    State(st): State<Arc<AppState>>,
    Json(body): Json<LanguagesBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let l1 = body.l1.trim();
    let l2 = body.l2.trim();
    if l1.is_empty() || l2.is_empty() {
        return Err(ApiError::bad("L1 et L2 requis"));
    }
    if l1.eq_ignore_ascii_case(l2) {
        return Err(ApiError::bad("Choisissez deux langues différentes"));
    }
    st.db.set_languages(l1, l2).await.map_err(ApiError::from)?;
    Ok(Json(json!({ "l1": l1, "l2": l2 })))
}

async fn import_pdf(
    State(st): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut filename = String::from("cours.pdf");
    let mut bytes: Option<bytes::Bytes> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| ApiError::bad(e.to_string()))? {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" || name == "pdf" {
            if let Some(fname) = field.file_name().map(|s| s.to_string()) {
                filename = fname;
            }
            bytes = Some(
                field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::bad(e.to_string()))?,
            );
        }
    }

    let bytes = bytes.ok_or_else(|| ApiError::bad("fichier PDF manquant (champ file)"))?;
    if bytes.len() > 15 * 1024 * 1024 {
        return Err(ApiError::bad("PDF trop lourd (max 15 Mo)"));
    }

    let extracted = pdf::extract_text(&filename, &bytes).map_err(|e| ApiError::bad(e.to_string()))?;
    if let Some(parent) = st.pdf_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
    }
    std::fs::write(&st.pdf_path, &bytes).map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;

    let course = st
        .db
        .upsert_course(&extracted.filename, &extracted.text)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(json!({
        "ok": true,
        "filename": course.filename,
        "char_count": extracted.char_count,
        "preview": extracted.text.chars().take(400).collect::<String>(),
        "l1": course.l1,
        "l2": course.l2,
        "has_pdf": true,
        "pdf_url": "/api/course/pdf",
    })))
}

async fn course_pdf(State(st): State<Arc<AppState>>) -> Result<Response, ApiError> {
    let course = st
        .db
        .get_course()
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("Aucun cours"))?;
    if course.filename.is_empty() || course.text.trim().is_empty() {
        return Err(ApiError::not_found("Aucun PDF importé"));
    }
    let bytes = std::fs::read(&st.pdf_path).map_err(|_| {
        ApiError::not_found("Fichier PDF absent — réimportez le cours")
    })?;
    let disposition = format!(
        "inline; filename=\"{}\"",
        course.filename.replace('"', "")
    );
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/pdf")
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .unwrap())
}

#[derive(Deserialize)]
struct GlossaryBody {
    term: String,
    #[serde(default)]
    context: Option<String>,
}

async fn define_glossary(
    State(st): State<Arc<AppState>>,
    Json(body): Json<GlossaryBody>,
) -> Result<Json<GlossaryResult>, ApiError> {
    let course = st
        .db
        .get_course()
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::bad("Aucun cours — importez un PDF d'abord"))?;

    let l1 = course
        .l1
        .clone()
        .ok_or_else(|| ApiError::bad("Déclarez L1/L2 avant le glossaire"))?;
    let l2 = course
        .l2
        .clone()
        .ok_or_else(|| ApiError::bad("Déclarez L1/L2 avant le glossaire"))?;

    let context = body
        .context
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(course.text.clone());

    let result = glossary::define_term(
        &st.db,
        &st.llm,
        &st.verifier,
        &body.term,
        &context,
        &l1,
        &l2,
    )
    .await
    .map_err(|e| ApiError::bad(e.to_string()))?;

    Ok(Json(result))
}

#[derive(Deserialize)]
struct GenerateBody {
    /// "fiches" or "quiz"
    kind: String,
}

async fn generate_content(
    State(st): State<Arc<AppState>>,
    Json(body): Json<GenerateBody>,
) -> Result<Json<GenerationResult>, ApiError> {
    let result = generate::generate(&st.db, &st.llm, &body.kind)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(result))
}

async fn list_fiches(
    State(st): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let fiches = st.db.list_fiches().await.map_err(ApiError::from)?;
    Ok(Json(json!({
        "fiches": fiches,
        "count": fiches.len(),
    })))
}

async fn get_fiche(
    State(st): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row = st
        .db
        .get_fiche(id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("fiche introuvable"))?;
    Ok(Json(json!({ "fiche": row })))
}

#[derive(Deserialize)]
struct FicheUpdate {
    front: String,
    back: String,
}

async fn update_fiche(
    State(st): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<FicheUpdate>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row = st
        .db
        .update_fiche(id, &body.front, &body.back)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("fiche introuvable"))?;
    let snap = st.db.progression_snapshot().await.map_err(ApiError::from)?;
    Ok(Json(json!({ "fiche": row, "progression": snap })))
}

async fn get_quiz(State(st): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
    let course = st.db.get_course().await.map_err(ApiError::from)?;
    let has_course = course
        .as_ref()
        .map(|c| !c.text.trim().is_empty())
        .unwrap_or(false);
    let sufficient = course
        .as_ref()
        .map(|c| material_sufficient_for_quiz(&c.text))
        .unwrap_or(false);
    let (_, last) = st.db.quiz_attempt_stats().await.map_err(ApiError::from)?;
    let last_score = last.map(|(s, t)| json!({ "score": s, "total": t }));

    match st.db.get_quiz().await.map_err(ApiError::from)? {
        Some(q) => {
            let questions: Vec<serde_json::Value> =
                serde_json::from_str(&q.questions_json).unwrap_or_default();
            Ok(Json(json!({
                "ready": !questions.is_empty(),
                "blocked": !sufficient,
                "has_course": has_course,
                "question_count": questions.len(),
                "questions": questions,
                "created_at": q.created_at,
                "last_score": last_score,
            })))
        }
        None => Ok(Json(json!({
            "ready": false,
            "blocked": has_course && !sufficient,
            "has_course": has_course,
            "question_count": 0,
            "questions": [],
            "last_score": last_score,
        }))),
    }
}

#[derive(Deserialize)]
struct OpenPageBody {
    page: i64,
    #[serde(default)]
    total_pages: Option<i64>,
}

async fn open_page(
    State(st): State<Arc<AppState>>,
    Json(body): Json<OpenPageBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.page < 1 {
        return Err(ApiError::bad("page doit être ≥ 1"));
    }
    st.db
        .mark_page_opened(body.page, body.total_pages)
        .await
        .map_err(ApiError::from)?;
    let snap = st.db.progression_snapshot().await.map_err(ApiError::from)?;
    Ok(Json(json!({ "ok": true, "progression": snap })))
}

#[derive(Deserialize)]
struct QuizAttemptBody {
    score: i64,
    total: i64,
}

async fn quiz_attempt(
    State(st): State<Arc<AppState>>,
    Json(body): Json<QuizAttemptBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.total < 1 || body.score < 0 || body.score > body.total {
        return Err(ApiError::bad("score/total invalides"));
    }
    st.db
        .record_quiz_attempt(body.score, body.total)
        .await
        .map_err(ApiError::from)?;
    let snap = st.db.progression_snapshot().await.map_err(ApiError::from)?;
    Ok(Json(json!({ "ok": true, "progression": snap })))
}

async fn progression(
    State(st): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = st.db.progression_snapshot().await.map_err(ApiError::from)?;
    Ok(Json(json!(snap)))
}

/* ---------- errors ---------- */

pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }
    fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(value: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: value.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({ "error": self.message }));
        (self.status, body).into_response()
    }
}
