//! HTTP API + AppState for the Thot vertical slice.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::Db;
use crate::generate::{self, material_sufficient_for_quiz, GenerationResult, QuizMiss};
use crate::glossary::{self, GlossaryResult};
use crate::llm::LlmClient;
use crate::pdf;
use crate::podcast;
use crate::web_verify::WebVerifier;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub llm: LlmClient,
    pub verifier: WebVerifier,
    /// Data directory: `docs/{id}.pdf` + sqlite.
    pub data_dir: PathBuf,
}

impl AppState {
    fn docs_dir(&self) -> PathBuf {
        self.data_dir.join("docs")
    }

    fn doc_pdf_path(&self, id: i64) -> PathBuf {
        self.docs_dir().join(format!("{id}.pdf"))
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/status", get(status))
        .route("/api/courses", get(list_courses).post(create_course))
        .route("/api/courses/{id}/activate", put(activate_course))
        .route("/api/courses/{id}/languages", put(set_course_languages))
        .route("/api/courses/{id}", delete(delete_course).put(update_course))
        .route("/api/languages", get(get_languages).put(set_languages))
        .route("/api/import", post(import_pdf))
        .route("/api/documents", get(list_documents))
        .route("/api/documents/{id}/activate", put(activate_document))
        .route("/api/documents/{id}", delete(delete_document))
        .route("/api/documents/{id}/pdf", get(document_pdf))
        .route("/api/course/pdf", get(course_pdf))
        .route("/api/glossary", post(define_glossary))
        .route("/api/generate", post(generate_content))
        .route("/api/fiches", get(list_fiches).post(create_fiche))
        .route("/api/fiches/recent", get(recent_fiches))
        .route("/api/fiches/export", get(export_fiches))
        .route("/api/fiches/{id}", get(get_fiche).put(update_fiche))
        .route("/api/vocab-status", get(vocab_status).put(set_vocab_status))
        .route("/api/quiz", get(get_quiz))
        .route("/api/quiz/attempt", post(quiz_attempt))
        .route("/api/quiz/remediate", post(quiz_remediate))
        .route("/api/source/locate", post(source_locate))
        .route("/api/quiz/targeted", post(quiz_targeted))
        .route("/api/podcasts", get(list_podcasts))
        .route("/api/podcasts/generate", post(generate_podcast))
        .route("/api/podcasts/generate-misses", post(generate_misses_podcast))
        .route("/api/podcasts/{id}", get(get_podcast))
        .route("/api/podcasts/{id}/audio", get(podcast_audio))
        .route("/api/pages/open", post(open_page))
        .route("/api/progression", get(progression))
        .with_state(Arc::new(state))
}

#[derive(Serialize)]
struct Health {
    ok: bool,
    provider: String,
    groq_key_present: bool,
    /// Which env name held the key: `GROQ_API_KEY` | `Etude` | `Etudes` | null. Never the secret.
    groq_key_env: Option<String>,
    stub: bool,
}

async fn health(State(st): State<Arc<AppState>>) -> Json<Health> {
    Json(Health {
        ok: true,
        provider: st.llm.provider_label().into(),
        groq_key_present: st.llm.has_groq_key(),
        groq_key_env: st.llm.groq_key_env_name().map(|s| s.to_string()),
        stub: st.llm.is_stub(),
    })
}

async fn course_summary_async(
    st: &AppState,
    c: &crate::db::CourseRow,
) -> Result<serde_json::Value, ApiError> {
    let docs = st.db.list_documents(c.id).await.map_err(ApiError::from)?;
    let (eff_l1, eff_l2, overridden) = st
        .db
        .effective_languages_for(c)
        .await
        .unwrap_or_else(|_| ("?".into(), "?".into(), false));
    let langs = if eff_l1 == "?" {
        "Langues non définies".to_string()
    } else if overridden {
        format!("{eff_l1} ← {eff_l2} (surcharge)")
    } else {
        format!("{eff_l1} ← {eff_l2}")
    };
    let n = docs.len();
    let subtitle = if n == 0 {
        format!("{langs} · aucun PDF")
    } else if n == 1 {
        format!("{langs} · 1 document")
    } else {
        format!("{langs} · {n} documents")
    };
    Ok(json!({
        "id": c.id,
        "title": c.title,
        "subtitle": subtitle,
        "filename": c.filename,
        "char_count": c.text.chars().count(),
        "docs_count": n,
        "l1": c.l1,
        "l2": c.l2,
        "l1_effective": if eff_l1 == "?" { serde_json::Value::Null } else { json!(eff_l1) },
        "l2_effective": if eff_l2 == "?" { serde_json::Value::Null } else { json!(eff_l2) },
        "languages_overridden": overridden,
        "active": c.active,
        "has_text": !c.text.trim().is_empty(),
        "created_at": c.created_at,
    }))
}

fn doc_summary(d: &crate::db::DocumentRow, st: &AppState) -> serde_json::Value {
    let has_pdf = st.doc_pdf_path(d.id).is_file();
    json!({
        "id": d.id,
        "course_id": d.course_id,
        "filename": d.filename,
        "char_count": d.text.chars().count(),
        "total_pages": d.total_pages,
        "active": d.active,
        "has_pdf": has_pdf,
        "pdf_url": if has_pdf { format!("/api/documents/{}/pdf", d.id) } else { String::new() },
        "created_at": d.created_at,
    })
}

async fn status(State(st): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
    let courses = st.db.list_courses().await.map_err(ApiError::from)?;
    let course = st.db.get_course().await.map_err(ApiError::from)?;
    let course_id = course.as_ref().map(|c| c.id);
    let docs = if let Some(cid) = course_id {
        st.db.list_documents(cid).await.map_err(ApiError::from)?
    } else {
        vec![]
    };
    let active = if let Some(cid) = course_id {
        st.db.active_document(cid).await.map_err(ApiError::from)?
    } else {
        None
    };
    // No active course → empty fiches/quiz/progression (do not 500 the home empty state).
    let (fiches_count, quiz_count, progression) = if course_id.is_some() {
        let fiches = st.db.list_fiches().await.map_err(ApiError::from)?;
        let quiz = st.db.get_quiz().await.map_err(ApiError::from)?;
        let quiz_count = quiz
            .as_ref()
            .and_then(|q| serde_json::from_str::<Vec<serde_json::Value>>(&q.questions_json).ok())
            .map(|v| v.len())
            .unwrap_or(0);
        let progression = st.db.progression_snapshot().await.map_err(ApiError::from)?;
        (fiches.len(), quiz_count, Some(progression))
    } else {
        (0, 0, None)
    };

    let docs_json: Vec<_> = docs.iter().map(|d| doc_summary(d, &st)).collect();
    let active_json = active.as_ref().map(|d| doc_summary(d, &st));
    let mut courses_json = Vec::with_capacity(courses.len());
    for c in &courses {
        courses_json.push(course_summary_async(&st, c).await?);
    }
    let active_course_json = match &course {
        Some(c) => Some(course_summary_async(&st, c).await?),
        None => None,
    };
    let (global_l1, global_l2) = st.db.get_global_languages().await.map_err(ApiError::from)?;
    let effective = match &course {
        Some(c) => st.db.effective_languages_for(c).await.ok(),
        None => None,
    };

    Ok(Json(json!({
        "provider": st.llm.provider_label(),
        "groq_key_present": st.llm.has_groq_key(),
        "groq_key_env": st.llm.groq_key_env_name(),
        "languages": {
            "global_l1": global_l1,
            "global_l2": global_l2,
            "effective_l1": effective.as_ref().map(|(a,_,_)| a),
            "effective_l2": effective.as_ref().map(|(_,b,_)| b),
            "course_overridden": effective.as_ref().map(|(_,_,o)| *o).unwrap_or(false),
        },
        "courses": courses_json,
        "active_course": active_course_json,
        "course": course.as_ref().map(|c| json!({
            "id": c.id,
            "title": c.title,
            "filename": c.filename,
            "char_count": c.text.chars().count(),
            "l1": c.l1,
            "l2": c.l2,
            "preview": c.text.chars().take(280).collect::<String>(),
            "has_text": !c.text.trim().is_empty(),
            "docs_count": docs.len(),
            "has_pdf": active.as_ref().map(|d| st.doc_pdf_path(d.id).is_file()).unwrap_or(false),
            "pdf_url": active.as_ref()
                .filter(|d| st.doc_pdf_path(d.id).is_file())
                .map(|d| format!("/api/documents/{}/pdf", d.id))
                .unwrap_or_default(),
            "active_document_id": active.as_ref().map(|d| d.id),
        })),
        "documents": docs_json,
        "active_document": active_json,
        "fiches_count": fiches_count,
        "quiz_questions": quiz_count,
        "progression": progression,
        "rules": {
            "scope": "many_courses_each_many_docs",
            "courses": "independent — own docs, glossary cache, fiches, quiz, progression",
            "glossary": "per active course (cache isolated)",
            "fiches": "per active course, from that course's doc corpus",
            "quiz": "per active course, from that course's doc corpus",
            "progression": "per active course; pages opened per document + course total",
        },
    })))
}

async fn list_courses(
    State(st): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let courses = st.db.list_courses().await.map_err(ApiError::from)?;
    let mut courses_json = Vec::with_capacity(courses.len());
    for c in &courses {
        courses_json.push(course_summary_async(&st, c).await?);
    }
    Ok(Json(json!({
        "courses": courses_json,
        "count": courses.len(),
        "active_course_id": courses.iter().find(|c| c.active).map(|c| c.id),
    })))
}

#[derive(Deserialize)]
struct CreateCourseBody {
    title: String,
    #[serde(default)]
    l1: Option<String>,
    #[serde(default)]
    l2: Option<String>,
}

async fn create_course(
    State(st): State<Arc<AppState>>,
    Json(body): Json<CreateCourseBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let course = st
        .db
        .create_course(
            &body.title,
            body.l1.as_deref().map(str::trim).filter(|s| !s.is_empty()),
            body.l2.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        )
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    let courses = st.db.list_courses().await.map_err(ApiError::from)?;
    let mut courses_json = Vec::with_capacity(courses.len());
    for c in &courses {
        courses_json.push(course_summary_async(&st, c).await?);
    }
    Ok(Json(json!({
        "ok": true,
        "course": course_summary_async(&st, &course).await?,
        "courses": courses_json,
    })))
}

#[derive(Deserialize)]
struct UpdateCourseBody {
    title: String,
}

async fn update_course(
    State(st): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateCourseBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let course = st
        .db
        .rename_course(id, &body.title)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(json!({ "ok": true, "course": course_summary_async(&st, &course).await? })))
}

async fn activate_course(
    State(st): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let course = st
        .db
        .set_active_course(id)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    let docs = st.db.list_documents(id).await.map_err(ApiError::from)?;
    let active = st.db.active_document(id).await.map_err(ApiError::from)?;
    if let Some(ref d) = active {
        let path = st.doc_pdf_path(d.id);
        if path.is_file() {
            let _ = std::fs::copy(&path, st.data_dir.join("course.pdf"));
        }
    }
    Ok(Json(json!({
        "ok": true,
        "course": course_summary_async(&st, &course).await?,
        "documents": docs.iter().map(|d| doc_summary(d, &st)).collect::<Vec<_>>(),
        "active_document": active.as_ref().map(|d| doc_summary(d, &st)),
        "progression": st.db.progression_snapshot().await.map_err(ApiError::from)?,
    })))
}

async fn delete_course(
    State(st): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let docs = st.db.list_documents(id).await.map_err(ApiError::from)?;
    for d in &docs {
        let _ = std::fs::remove_file(st.doc_pdf_path(d.id));
    }
    st.db
        .delete_course(id)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    let courses = st.db.list_courses().await.map_err(ApiError::from)?;
    let active = st.db.active_course().await.map_err(ApiError::from)?;
    let mut courses_json = Vec::with_capacity(courses.len());
    for c in &courses {
        courses_json.push(course_summary_async(&st, c).await?);
    }
    let active_json = match &active {
        Some(c) => Some(course_summary_async(&st, c).await?),
        None => None,
    };
    Ok(Json(json!({
        "ok": true,
        "courses": courses_json,
        "active_course": active_json,
    })))
}

#[derive(Deserialize)]
struct LanguagesBody {
    l1: String,
    l2: String,
}

async fn get_languages(
    State(st): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (global_l1, global_l2) = st.db.get_global_languages().await.map_err(ApiError::from)?;
    let effective = st.db.effective_languages_active().await.ok();
    Ok(Json(json!({
        "global_l1": global_l1,
        "global_l2": global_l2,
        "effective_l1": effective.as_ref().map(|(a,_,_)| a),
        "effective_l2": effective.as_ref().map(|(_,b,_)| b),
        "course_overridden": effective.as_ref().map(|(_,_,o)| *o).unwrap_or(false),
    })))
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
    // Global app languages (default for all courses).
    st.db.set_languages(l1, l2).await.map_err(ApiError::from)?;
    Ok(Json(json!({
        "ok": true,
        "scope": "global",
        "global_l1": l1,
        "global_l2": l2,
    })))
}

#[derive(Deserialize)]
struct CourseLanguagesBody {
    /// When true, clear course override and use global.
    #[serde(default)]
    clear: bool,
    #[serde(default)]
    l1: Option<String>,
    #[serde(default)]
    l2: Option<String>,
}

async fn set_course_languages(
    State(st): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<CourseLanguagesBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let course = if body.clear {
        st.db
            .set_course_language_override(id, None, None)
            .await
            .map_err(|e| ApiError::bad(e.to_string()))?
    } else {
        let l1 = body.l1.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let l2 = body.l2.as_deref().map(str::trim).filter(|s| !s.is_empty());
        st.db
            .set_course_language_override(id, l1, l2)
            .await
            .map_err(|e| ApiError::bad(e.to_string()))?
    };
    Ok(Json(json!({
        "ok": true,
        "course": course_summary_async(&st, &course).await?,
    })))
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
        return Err(ApiError::bad(
            "Échec chargement : PDF trop lourd (max 15 Mo) — réessayez avec un autre fichier",
        ));
    }

    let course_id = st.db.active_course_id().await.map_err(ApiError::from)?;
    let extracted = pdf::extract_text(&filename, &bytes).map_err(|e| ApiError::bad(e.to_string()))?;
    let doc = st
        .db
        .add_document(course_id, &extracted.filename, &extracted.text)
        .await
        .map_err(ApiError::from)?;

    let dir = st.docs_dir();
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
    let path = st.doc_pdf_path(doc.id);
    std::fs::write(&path, &bytes).map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;

    if doc.active {
        let legacy = st.data_dir.join("course.pdf");
        let _ = std::fs::write(&legacy, &bytes);
    }

    let docs = st.db.list_documents(course_id).await.map_err(ApiError::from)?;
    Ok(Json(json!({
        "ok": true,
        "course_id": course_id,
        "document": doc_summary(&doc, &st),
        "documents": docs.iter().map(|d| doc_summary(d, &st)).collect::<Vec<_>>(),
        "filename": doc.filename,
        "char_count": extracted.char_count,
        "preview": extracted.text.chars().take(400).collect::<String>(),
        "has_pdf": true,
        "pdf_url": format!("/api/documents/{}/pdf", doc.id),
        "note": "Document ajouté au cours actif. Glossaire / fiches / quiz de ce cours portent sur son corpus.",
    })))
}

async fn list_documents(
    State(st): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let course_id = st.db.active_course_id().await.map_err(ApiError::from)?;
    let docs = st.db.list_documents(course_id).await.map_err(ApiError::from)?;
    Ok(Json(json!({
        "course_id": course_id,
        "documents": docs.iter().map(|d| doc_summary(d, &st)).collect::<Vec<_>>(),
        "count": docs.len(),
    })))
}

async fn activate_document(
    State(st): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let doc = st
        .db
        .set_active_document(id)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    let path = st.doc_pdf_path(doc.id);
    if path.is_file() {
        let _ = std::fs::copy(&path, st.data_dir.join("course.pdf"));
    }
    Ok(Json(json!({
        "ok": true,
        "document": doc_summary(&doc, &st),
        "pdf_url": if path.is_file() { format!("/api/documents/{}/pdf", doc.id) } else { String::new() },
    })))
}

async fn delete_document(
    State(st): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let path = st.doc_pdf_path(id);
    let course_id = st
        .db
        .get_document(id)
        .await
        .map_err(ApiError::from)?
        .map(|d| d.course_id)
        .ok_or_else(|| ApiError::not_found("Document introuvable"))?;
    st.db
        .delete_document(id)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    let _ = std::fs::remove_file(&path);
    let docs = st.db.list_documents(course_id).await.map_err(ApiError::from)?;
    let active = st.db.active_document(course_id).await.map_err(ApiError::from)?;
    Ok(Json(json!({
        "ok": true,
        "documents": docs.iter().map(|d| doc_summary(d, &st)).collect::<Vec<_>>(),
        "active_document": active.as_ref().map(|d| doc_summary(d, &st)),
    })))
}

async fn serve_doc_pdf(st: &AppState, id: i64) -> Result<Response, ApiError> {
    let doc = st
        .db
        .get_document(id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("Document introuvable"))?;
    let path = st.doc_pdf_path(id);
    let bytes = std::fs::read(&path).map_err(|_| {
        ApiError::not_found("Fichier PDF absent — réimportez ce document")
    })?;
    let disposition = format!(
        "inline; filename=\"{}\"",
        doc.filename.replace('"', "")
    );
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/pdf")
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .unwrap())
}

async fn document_pdf(
    State(st): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Response, ApiError> {
    serve_doc_pdf(&st, id).await
}

async fn course_pdf(State(st): State<Arc<AppState>>) -> Result<Response, ApiError> {
    let course_id = st.db.active_course_id().await.map_err(ApiError::from)?;
    let doc = st
        .db
        .active_document(course_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("Aucun document"))?;
    serve_doc_pdf(&st, doc.id).await
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
        .ok_or_else(|| ApiError::bad("Aucun cours — créez un cours d'abord"))?;

    let (l1, l2, _) = st
        .db
        .effective_languages_for(&course)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;

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
    .map_err(map_llm_err)?;

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
        .map_err(map_llm_err)?;
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

async fn recent_fiches(
    State(st): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let fiches = st
        .db
        .list_fiches_recent(12)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(json!({
        "fiches": fiches,
        "count": fiches.len(),
        "note": "Derniers capturés (Mes fiches, plus récentes d'abord)",
    })))
}

/// Ling/Lute-style status for PDF text highlighting.
/// Priority: explicit mark > short fiche front (review) > verified glossary (known).
async fn vocab_status(
    State(st): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let fiches = st.db.list_fiches().await.map_err(ApiError::from)?;
    let glossary = st.db.list_glossary().await.map_err(ApiError::from)?;
    let marked = st.db.list_term_statuses().await.map_err(ApiError::from)?;

    let mut terms: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    let mut review_count = 0usize;
    let mut known_count = 0usize;
    let mut marked_count = 0usize;

    // 1) Explicit user marks win.
    for row in &marked {
        let key = normalize_vocab_key(&row.term_l2);
        if key.is_empty() || !is_highlightable_term(&key) {
            continue;
        }
        let st_label = if row.status == "known" {
            "known"
        } else {
            "review"
        };
        if terms.insert(key, json!(st_label)).is_none() {
            marked_count += 1;
            if st_label == "known" {
                known_count += 1;
            } else {
                review_count += 1;
            }
        }
    }

    // 2) Short fiche fronts → review (skip long/auto question-like fronts).
    for f in &fiches {
        for key in vocab_keys_from_front(&f.front) {
            if !is_highlightable_term(&key) {
                continue;
            }
            if terms.contains_key(&key) {
                continue;
            }
            terms.insert(key, json!("review"));
            review_count += 1;
        }
    }

    // 3) Verified glossary → known if not already set.
    for g in &glossary {
        if !g.verified {
            continue;
        }
        let key = normalize_vocab_key(&g.term_l2);
        if key.is_empty() || !is_highlightable_term(&key) {
            continue;
        }
        if terms.contains_key(&key) {
            continue;
        }
        terms.insert(key, json!("known"));
        known_count += 1;
    }

    Ok(Json(json!({
        "terms": terms,
        "review_count": review_count,
        "known_count": known_count,
        "marked_count": marked_count,
        "min_token_len": MIN_VOCAB_TOKEN_LEN,
        "max_term_words": MAX_VOCAB_TERM_WORDS,
        "legend": {
            "review": "à revoir — marqué ou fiche courte",
            "known": "connu — marqué ou glossaire vérifié",
        }
    })))
}

#[derive(Deserialize)]
struct VocabStatusBody {
    term: String,
    /// "known" | "review"
    status: String,
}

async fn set_vocab_status(
    State(st): State<Arc<AppState>>,
    Json(body): Json<VocabStatusBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let term = body.term.trim();
    if term.is_empty() {
        return Err(ApiError::bad("terme requis"));
    }
    let row = st
        .db
        .upsert_term_status(term, &body.status)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(json!({
        "ok": true,
        "term": row.term_l2,
        "status": row.status,
        "updated_at": row.updated_at,
    })))
}

#[derive(Deserialize)]
struct ExportQuery {
    /// `csv` (default) or `anki` (tab-separated for Anki import)
    #[serde(default = "default_export_format")]
    format: String,
}

fn default_export_format() -> String {
    "csv".into()
}

async fn export_fiches(
    State(st): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let fiches = st.db.list_fiches().await.map_err(ApiError::from)?;
    let fmt = q.format.to_lowercase();
    if fmt == "anki" || fmt == "tsv" {
        let mut out = String::from("#separator:tab\n#html:false\n");
        for f in &fiches {
            out.push_str(&anki_escape(&f.front));
            out.push('\t');
            out.push_str(&anki_escape(&f.back));
            out.push('\n');
        }
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/tab-separated-values; charset=utf-8")
            .header(
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"thot-fiches-anki.txt\"",
            )
            .body(Body::from(out))
            .unwrap_or_else(|_| Response::new(Body::empty())));
    }
    // CSV
    let mut out = String::from("front,back,created_at,edited\n");
    for f in &fiches {
        out.push_str(&csv_escape(&f.front));
        out.push(',');
        out.push_str(&csv_escape(&f.back));
        out.push(',');
        out.push_str(&csv_escape(&f.created_at));
        out.push(',');
        out.push_str(if f.edited { "1" } else { "0" });
        out.push('\n');
    }
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"thot-fiches.csv\"",
        )
        .body(Body::from(out))
        .unwrap_or_else(|_| Response::new(Body::empty())))
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn anki_escape(s: &str) -> String {
    s.replace('\t', " ").replace('\n', "<br>").replace('\r', "")
}

const MIN_VOCAB_TOKEN_LEN: usize = 4;
const MAX_VOCAB_TERM_WORDS: usize = 4;

fn normalize_vocab_key(raw: &str) -> String {
    let mut s = raw.trim().to_lowercase();
    for prefix in [
        "[sens]",
        "[remédiation]",
        "[remediation]",
        "[stub]",
        "[remediation]",
    ] {
        if s.starts_with(prefix) {
            s = s[prefix.len()..].trim().to_string();
        }
    }
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '\'' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_highlightable_term(key: &str) -> bool {
    if key.is_empty() || key.contains('?') {
        return false;
    }
    let words: Vec<&str> = key.split_whitespace().collect();
    if words.is_empty() || words.len() > MAX_VOCAB_TERM_WORDS {
        return false;
    }
    // Drop question-like / sentence-like fronts from auto fiches.
    const STOP: &[&str] = &[
        "what", "which", "where", "when", "how", "why", "quel", "quelle", "quels", "quelles",
        "qu", "est", "the", "a", "an", "les", "des", "une", "un", "de", "du", "la", "le",
    ];
    if words
        .first()
        .map(|w| STOP.contains(w))
        .unwrap_or(false)
    {
        return false;
    }
    // At least one content token ≥ min length (allow short connectors like "de").
    words.iter().any(|w| w.chars().count() >= MIN_VOCAB_TOKEN_LEN)
}

fn vocab_keys_from_front(front: &str) -> Vec<String> {
    let key = normalize_vocab_key(front);
    if !is_highlightable_term(&key) {
        return vec![];
    }
    // Only the full short term — no first-token fan-out (that caused the orange wall).
    vec![key]
}

#[derive(Deserialize)]
struct FicheCreate {
    front: String,
    back: String,
    /// Optional origin label for UI (e.g. "sens", "lecture").
    #[serde(default)]
    origin: Option<String>,
}

/// Append one fiche from the reader Sens menu (does not wipe existing cards).
async fn create_fiche(
    State(st): State<Arc<AppState>>,
    Json(body): Json<FicheCreate>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut front = body.front.trim().to_string();
    let back = body.back.trim().to_string();
    if front.is_empty() || back.is_empty() {
        return Err(ApiError::bad("recto et verso requis"));
    }
    let origin = body.origin.as_deref().unwrap_or("sens");
    if origin.eq_ignore_ascii_case("sens") && !front.starts_with("[Sens]") {
        front = format!("[Sens] {front}");
    }
    let rows = st
        .db
        .append_fiches(&[(front, back)])
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    let fiche = rows
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::bad("fiche non créée"))?;
    let snap = st.db.progression_snapshot().await.map_err(ApiError::from)?;
    Ok(Json(json!({
        "ok": true,
        "fiche": fiche,
        "progression": snap,
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
    #[serde(default)]
    doc_id: Option<i64>,
}

async fn open_page(
    State(st): State<Arc<AppState>>,
    Json(body): Json<OpenPageBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.page < 1 {
        return Err(ApiError::bad("page doit être ≥ 1"));
    }
    let course_id = st.db.active_course_id().await.map_err(ApiError::from)?;
    let doc_id = match body.doc_id {
        Some(id) if id > 0 => id,
        _ => st
            .db
            .active_document(course_id)
            .await
            .map_err(ApiError::from)?
            .map(|d| d.id)
            .ok_or_else(|| ApiError::bad("Aucun document actif"))?,
    };
    st.db
        .mark_page_opened(doc_id, body.page, body.total_pages)
        .await
        .map_err(ApiError::from)?;
    let snap = st.db.progression_snapshot().await.map_err(ApiError::from)?;
    Ok(Json(json!({ "ok": true, "doc_id": doc_id, "progression": snap })))
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

#[derive(Deserialize)]
struct QuizRemediateBody {
    misses: Vec<QuizMiss>,
}

async fn quiz_remediate(
    State(st): State<Arc<AppState>>,
    Json(body): Json<QuizRemediateBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.misses.is_empty() {
        return Ok(Json(json!({
            "ok": true,
            "fiches": [],
            "count": 0,
            "note": "Aucune erreur — pas de remédiation."
        })));
    }
    let fiches = generate::remediate_misses(&st.db, &st.llm, &body.misses)
        .await
        .map_err(map_llm_err)?;
    let sources = crate::source::locate_misses(&st.db, &st.data_dir, &body.misses)
        .await
        .unwrap_or_default();
    let snap = st.db.progression_snapshot().await.map_err(ApiError::from)?;
    let notions: Vec<String> = body
        .misses
        .iter()
        .map(|m| {
            let n = m.notion.trim();
            if n.is_empty() {
                m.question.clone()
            } else {
                n.to_string()
            }
        })
        .filter(|s| !s.is_empty())
        .collect();
    Ok(Json(json!({
        "ok": true,
        "fiches": fiches,
        "count": fiches.len(),
        "notions": notions,
        "sources": sources,
        "progression": snap,
    })))
}

#[derive(Deserialize)]
struct SourceLocateBody {
    #[serde(default)]
    query: String,
    #[serde(default)]
    notion: String,
    #[serde(default)]
    misses: Vec<QuizMiss>,
}

async fn source_locate(
    State(st): State<Arc<AppState>>,
    Json(body): Json<SourceLocateBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !body.misses.is_empty() {
        let sources = crate::source::locate_misses(&st.db, &st.data_dir, &body.misses)
            .await
            .map_err(|e| ApiError::bad(e.to_string()))?;
        return Ok(Json(json!({ "ok": true, "sources": sources, "count": sources.len() })));
    }
    let notion = if body.notion.trim().is_empty() {
        body.query.trim().to_string()
    } else {
        body.notion.trim().to_string()
    };
    let query = if body.query.trim().is_empty() {
        notion.clone()
    } else {
        body.query.trim().to_string()
    };
    if query.chars().count() < 2 {
        return Err(ApiError::bad("query trop courte"));
    }
    let hit = crate::source::locate_query_str(&st.db, &st.data_dir, &notion, &query)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))?;
    let sources = match &hit {
        Some(h) => vec![h.clone()],
        None => vec![],
    };
    Ok(Json(json!({
        "ok": true,
        "source": hit,
        "sources": sources,
    })))
}

#[derive(Deserialize)]
struct QuizTargetedBody {
    /// Notion labels from missed quiz items.
    notions: Vec<String>,
    #[serde(default)]
    misses: Vec<QuizMiss>,
}

/// Build a short quiz covering only missed notions (filter existing + optional LLM fill).
async fn quiz_targeted(
    State(st): State<Arc<AppState>>,
    Json(body): Json<QuizTargetedBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut notions: Vec<String> = body
        .notions
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for m in &body.misses {
        let n = m.notion.trim();
        if !n.is_empty() {
            notions.push(n.to_string());
        }
    }
    notions.sort();
    notions.dedup();
    if notions.is_empty() {
        return Err(ApiError::bad("notions requises pour un quiz ciblé"));
    }

    let questions = generate::targeted_quiz(&st.db, &st.llm, &notions)
        .await
        .map_err(map_llm_err)?;
    Ok(Json(json!({
        "ok": true,
        "kind": "quiz",
        "targeted": true,
        "notions": notions,
        "questions": questions,
        "count": questions.len(),
    })))
}

async fn progression(
    State(st): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let snap = st.db.progression_snapshot().await.map_err(ApiError::from)?;
    Ok(Json(json!(snap)))
}

async fn list_podcasts(
    State(st): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let podcasts = st.db.list_podcasts().await.map_err(ApiError::from)?;
    Ok(Json(json!({
        "podcasts": podcasts,
        "count": podcasts.len(),
        "note": "Generated overviews of the course PDF corpus. No microphone audio is recorded.",
    })))
}

async fn generate_podcast(
    State(st): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row = podcast::generate_podcast(&st.db, &st.llm, &st.data_dir)
        .await
        .map_err(map_llm_err)?;
    Ok(Json(json!({
        "ok": true,
        "podcast": row,
        "playback_mode": row.playback_mode,
        "kind": "overview",
    })))
}

#[derive(Deserialize, Default)]
struct PodcastMissesBody {
    #[serde(default)]
    notions: Vec<String>,
    #[serde(default)]
    misses: Vec<QuizMiss>,
}

async fn generate_misses_podcast(
    State(st): State<Arc<AppState>>,
    Json(body): Json<PodcastMissesBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.misses.is_empty() && body.notions.is_empty() {
        return Err(ApiError::bad(
            "notions ou misses requis pour un podcast de remédiation",
        ));
    }
    let (row, sources) = podcast::generate_misses_podcast(
        &st.db,
        &st.llm,
        &st.data_dir,
        &body.misses,
        &body.notions,
    )
    .await
    .map_err(map_llm_err)?;
    let script: serde_json::Value =
        serde_json::from_str(&row.script_json).unwrap_or_else(|_| json!({ "turns": [] }));
    Ok(Json(json!({
        "ok": true,
        "podcast": row,
        "script": script,
        "sources": sources,
        "playback_mode": row.playback_mode,
        "kind": "misses",
    })))
}

async fn get_podcast(
    State(st): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row = st
        .db
        .get_podcast(id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("podcast introuvable"))?;
    let script: serde_json::Value =
        serde_json::from_str(&row.script_json).unwrap_or_else(|_| json!({ "turns": [] }));
    Ok(Json(json!({
        "podcast": row,
        "script": script,
    })))
}

async fn podcast_audio(
    State(st): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Response, ApiError> {
    let row = st
        .db
        .get_podcast(id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("podcast introuvable"))?;
    let rel = row
        .audio_relpath
        .as_deref()
        .ok_or_else(|| ApiError::not_found("pas de fichier audio — utilisez la lecture Web Speech"))?;
    let path = st.data_dir.join(rel);
    if !path.is_file() {
        return Err(ApiError::not_found("fichier audio manquant"));
    }
    let bytes = std::fs::read(&path).map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "audio/mpeg")
        .header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename=\"thot-podcast-{id}.mp3\""),
        )
        .body(Body::from(bytes))
        .unwrap_or_else(|_| Response::new(Body::empty())))
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
    fn rate_limited(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: msg.into(),
        }
    }
}

fn map_llm_err(e: anyhow::Error) -> ApiError {
    let msg = e.to_string();
    let lower = msg.to_lowercase();
    if lower.contains("429") || lower.contains("rate limit") || lower.contains("limite groq") {
        ApiError::rate_limited(msg)
    } else {
        ApiError::bad(msg)
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
