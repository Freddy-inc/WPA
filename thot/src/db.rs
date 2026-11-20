//! SQLite persistence: one course, many docs, glossary, fiches, quiz, reading.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqlitePoolOptions, FromRow, SqlitePool};

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct CourseRow {
    pub id: i64,
    pub filename: String,
    pub text: String,
    pub l1: Option<String>,
    pub l2: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DocumentRow {
    pub id: i64,
    pub filename: String,
    pub text: String,
    pub total_pages: i64,
    pub active: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct GlossaryRow {
    pub id: i64,
    pub term_l2: String,
    pub meaning_l1: String,
    pub context_snippet: Option<String>,
    pub sources_json: String,
    pub verified: bool,
    pub confidence: f64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct FicheRow {
    pub id: i64,
    pub front: String,
    pub back: String,
    pub edited: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct QuizRow {
    pub id: i64,
    pub questions_json: String,
    pub created_at: String,
}

impl Db {
    pub async fn connect(path: &str) -> Result<Self> {
        let url = if path == ":memory:" {
            "sqlite::memory:".to_string()
        } else {
            if let Some(parent) = std::path::Path::new(path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            format!("sqlite://{path}?mode=rwc")
        };
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .with_context(|| format!("connect sqlite {url}"))?;
        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS course (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                filename TEXT NOT NULL,
                text TEXT NOT NULL,
                l1 TEXT,
                l2 TEXT,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS documents (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                filename TEXT NOT NULL,
                text TEXT NOT NULL,
                total_pages INTEGER NOT NULL DEFAULT 0,
                active INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS glossary (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                term_l2 TEXT NOT NULL COLLATE NOCASE,
                meaning_l1 TEXT NOT NULL,
                context_snippet TEXT,
                sources_json TEXT NOT NULL,
                verified INTEGER NOT NULL DEFAULT 0,
                confidence REAL NOT NULL DEFAULT 0.0,
                created_at TEXT NOT NULL,
                UNIQUE(term_l2)
            );
            CREATE TABLE IF NOT EXISTS fiches (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                front TEXT NOT NULL,
                back TEXT NOT NULL,
                edited INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS quiz (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                questions_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS quiz_attempts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                score INTEGER NOT NULL,
                total INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        self.migrate_pages_opened().await?;

        // One-time migrate from legacy single-PDF course row into documents.
        let doc_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM documents")
            .fetch_one(&self.pool)
            .await?;
        if doc_count.0 == 0 {
            if let Some(c) = self.get_course_raw().await? {
                if !c.filename.is_empty() && !c.text.trim().is_empty() {
                    sqlx::query(
                        "INSERT INTO documents (filename, text, total_pages, active, created_at)
                         VALUES (?, ?, 0, 1, ?)",
                    )
                    .bind(&c.filename)
                    .bind(&c.text)
                    .bind(&c.created_at)
                    .execute(&self.pool)
                    .await?;
                }
            }
        }
        // Keep corpus denormalized in sync.
        let _ = self.refresh_course_corpus().await;
        Ok(())
    }

    /// Ensure pages_opened is (doc_id, page). Rebuild if legacy single-column schema.
    async fn migrate_pages_opened(&self) -> Result<()> {
        let exists: Option<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='pages_opened'",
        )
        .fetch_optional(&self.pool)
        .await?;
        if exists.is_none() {
            sqlx::query(
                "CREATE TABLE pages_opened (
                    doc_id INTEGER NOT NULL,
                    page INTEGER NOT NULL CHECK (page >= 1),
                    PRIMARY KEY (doc_id, page)
                )",
            )
            .execute(&self.pool)
            .await?;
            return Ok(());
        }
        let cols: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM pragma_table_info('pages_opened')")
                .fetch_all(&self.pool)
                .await?;
        let names: Vec<String> = cols.into_iter().map(|(n,)| n).collect();
        if names.iter().any(|n| n == "doc_id") {
            return Ok(());
        }
        // Legacy: PRIMARY KEY (page) only — drop; reopen tracking starts fresh per doc.
        sqlx::query("DROP TABLE pages_opened")
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "CREATE TABLE pages_opened (
                doc_id INTEGER NOT NULL,
                page INTEGER NOT NULL CHECK (page >= 1),
                PRIMARY KEY (doc_id, page)
            )",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_course_raw(&self) -> Result<Option<CourseRow>> {
        let row = sqlx::query_as::<_, CourseRow>(
            "SELECT id, filename, text, l1, l2, created_at FROM course WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn set_languages(&self, l1: &str, l2: &str) -> Result<()> {
        let existing = self.get_course_raw().await?;
        let now = now_iso();
        if existing.is_some() {
            sqlx::query("UPDATE course SET l1 = ?, l2 = ? WHERE id = 1")
                .bind(l1)
                .bind(l2)
                .execute(&self.pool)
                .await?;
        } else {
            sqlx::query(
                "INSERT INTO course (id, filename, text, l1, l2, created_at) VALUES (1, '', '', ?, ?, ?)",
            )
            .bind(l1)
            .bind(l2)
            .bind(&now)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    async fn ensure_course_shell(&self) -> Result<()> {
        if self.get_course_raw().await?.is_none() {
            let now = now_iso();
            sqlx::query(
                "INSERT INTO course (id, filename, text, l1, l2, created_at) VALUES (1, '', '', NULL, NULL, ?)",
            )
            .bind(&now)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Refresh denormalized course.filename/text from all documents (corpus for IA).
    pub async fn refresh_course_corpus(&self) -> Result<()> {
        self.ensure_course_shell().await?;
        let docs = self.list_documents().await?;
        let text = docs
            .iter()
            .map(|d| format!("## {}\n{}", d.filename, d.text))
            .collect::<Vec<_>>()
            .join("\n\n");
        let filename = if docs.is_empty() {
            String::new()
        } else if docs.len() == 1 {
            docs[0].filename.clone()
        } else {
            format!("{} documents", docs.len())
        };
        sqlx::query("UPDATE course SET filename = ?, text = ? WHERE id = 1")
            .bind(&filename)
            .bind(&text)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn add_document(&self, filename: &str, text: &str) -> Result<DocumentRow> {
        self.ensure_course_shell().await?;
        let now = now_iso();
        // Newly imported PDF becomes the active reader target (user can switch later).
        sqlx::query("UPDATE documents SET active = 0")
            .execute(&self.pool)
            .await?;
        let res = sqlx::query(
            "INSERT INTO documents (filename, text, total_pages, active, created_at)
             VALUES (?, ?, 0, 1, ?)",
        )
        .bind(filename)
        .bind(text)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        let id = res.last_insert_rowid();
        self.refresh_course_corpus().await?;
        self.get_document(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("document missing after insert"))
    }

    pub async fn list_documents(&self) -> Result<Vec<DocumentRow>> {
        let rows = sqlx::query_as::<_, DocumentRow>(
            "SELECT id, filename, text, total_pages, active, created_at FROM documents ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_document(&self, id: i64) -> Result<Option<DocumentRow>> {
        let row = sqlx::query_as::<_, DocumentRow>(
            "SELECT id, filename, text, total_pages, active, created_at FROM documents WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn active_document(&self) -> Result<Option<DocumentRow>> {
        let row = sqlx::query_as::<_, DocumentRow>(
            "SELECT id, filename, text, total_pages, active, created_at FROM documents WHERE active = 1 LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        if row.is_some() {
            return Ok(row);
        }
        // Fallback: first doc
        let rows = self.list_documents().await?;
        Ok(rows.into_iter().next())
    }

    pub async fn set_active_document(&self, id: i64) -> Result<DocumentRow> {
        let exists = self.get_document(id).await?;
        if exists.is_none() {
            return Err(anyhow::anyhow!("document introuvable"));
        }
        sqlx::query("UPDATE documents SET active = 0")
            .execute(&self.pool)
            .await?;
        sqlx::query("UPDATE documents SET active = 1 WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        self.get_document(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("document missing after activate"))
    }

    pub async fn delete_document(&self, id: i64) -> Result<()> {
        let was_active = self
            .get_document(id)
            .await?
            .map(|d| d.active)
            .unwrap_or(false);
        sqlx::query("DELETE FROM pages_opened WHERE doc_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM documents WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if was_active {
            if let Some(next) = self.list_documents().await?.into_iter().next() {
                let _ = self.set_active_document(next.id).await?;
            }
        }
        self.refresh_course_corpus().await?;
        Ok(())
    }

    pub async fn set_document_total_pages(&self, id: i64, total: i64) -> Result<()> {
        if total < 1 {
            return Ok(());
        }
        sqlx::query(
            "UPDATE documents SET total_pages = MAX(total_pages, ?) WHERE id = ?",
        )
        .bind(total)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Course view: languages + aggregated corpus (all docs).
    pub async fn get_course(&self) -> Result<Option<CourseRow>> {
        self.refresh_course_corpus().await.ok();
        self.get_course_raw().await
    }

    pub async fn get_glossary(&self, term: &str) -> Result<Option<GlossaryRow>> {
        let row = sqlx::query_as::<_, GlossaryRow>(
            r#"SELECT id, term_l2, meaning_l1, context_snippet, sources_json,
                      verified, confidence, created_at
               FROM glossary WHERE term_l2 = ? COLLATE NOCASE"#,
        )
        .bind(term.trim())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn put_glossary(
        &self,
        term_l2: &str,
        meaning_l1: &str,
        context_snippet: Option<&str>,
        sources_json: &str,
        verified: bool,
        confidence: f64,
    ) -> Result<GlossaryRow> {
        let now = now_iso();
        sqlx::query(
            r#"INSERT INTO glossary (term_l2, meaning_l1, context_snippet, sources_json, verified, confidence, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(term_l2) DO UPDATE SET
                 meaning_l1 = excluded.meaning_l1,
                 context_snippet = excluded.context_snippet,
                 sources_json = excluded.sources_json,
                 verified = excluded.verified,
                 confidence = excluded.confidence,
                 created_at = excluded.created_at"#,
        )
        .bind(term_l2)
        .bind(meaning_l1)
        .bind(context_snippet)
        .bind(sources_json)
        .bind(verified)
        .bind(confidence)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        self.get_glossary(term_l2)
            .await?
            .ok_or_else(|| anyhow::anyhow!("glossary missing after put"))
    }

    pub async fn replace_fiches(&self, fiches: &[(String, String)]) -> Result<Vec<FicheRow>> {
        sqlx::query("DELETE FROM fiches").execute(&self.pool).await?;
        let now = now_iso();
        for (front, back) in fiches {
            sqlx::query(
                "INSERT INTO fiches (front, back, edited, created_at) VALUES (?, ?, 0, ?)",
            )
            .bind(front)
            .bind(back)
            .bind(&now)
            .execute(&self.pool)
            .await?;
        }
        self.list_fiches().await
    }

    pub async fn list_fiches(&self) -> Result<Vec<FicheRow>> {
        let rows = sqlx::query_as::<_, FicheRow>(
            "SELECT id, front, back, edited, created_at FROM fiches ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_fiche(&self, id: i64) -> Result<Option<FicheRow>> {
        let row = sqlx::query_as::<_, FicheRow>(
            "SELECT id, front, back, edited, created_at FROM fiches WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn update_fiche(&self, id: i64, front: &str, back: &str) -> Result<Option<FicheRow>> {
        let front = front.trim();
        let back = back.trim();
        if front.is_empty() || back.is_empty() {
            return Err(anyhow::anyhow!("recto et verso requis"));
        }
        let res = sqlx::query("UPDATE fiches SET front = ?, back = ?, edited = 1 WHERE id = ?")
            .bind(front)
            .bind(back)
            .bind(id)
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_fiche(id).await
    }

    pub async fn save_quiz(&self, questions_json: &str) -> Result<QuizRow> {
        let now = now_iso();
        sqlx::query(
            "INSERT INTO quiz (id, questions_json, created_at) VALUES (1, ?, ?)
             ON CONFLICT(id) DO UPDATE SET questions_json = excluded.questions_json, created_at = excluded.created_at",
        )
        .bind(questions_json)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        self.get_quiz()
            .await?
            .ok_or_else(|| anyhow::anyhow!("quiz missing after save"))
    }

    pub async fn get_quiz(&self) -> Result<Option<QuizRow>> {
        let row = sqlx::query_as::<_, QuizRow>(
            "SELECT id, questions_json, created_at FROM quiz WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn mark_page_opened(
        &self,
        doc_id: i64,
        page: i64,
        total_pages: Option<i64>,
    ) -> Result<()> {
        if page < 1 || doc_id < 1 {
            return Ok(());
        }
        sqlx::query("INSERT OR IGNORE INTO pages_opened (doc_id, page) VALUES (?, ?)")
            .bind(doc_id)
            .bind(page)
            .execute(&self.pool)
            .await?;
        if let Some(total) = total_pages.filter(|t| *t > 0) {
            self.set_document_total_pages(doc_id, total).await?;
        }
        Ok(())
    }

    pub async fn record_quiz_attempt(&self, score: i64, total: i64) -> Result<()> {
        let now = now_iso();
        sqlx::query(
            "INSERT INTO quiz_attempts (score, total, created_at) VALUES (?, ?, ?)",
        )
        .bind(score)
        .bind(total)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn quiz_attempt_stats(&self) -> Result<(i64, Option<(i64, i64)>)> {
        let (attempts,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM quiz_attempts")
            .fetch_one(&self.pool)
            .await?;
        let last: Option<(i64, i64)> = sqlx::query_as(
            "SELECT score, total FROM quiz_attempts ORDER BY id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok((attempts, last))
    }

    pub async fn progression_snapshot(&self) -> Result<ProgressionSnapshot> {
        let docs = self.list_documents().await?;
        let mut by_doc = Vec::new();
        let mut pages_opened = 0i64;
        let mut total_pages = 0i64;
        for d in &docs {
            let (opened,): (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM pages_opened WHERE doc_id = ?")
                    .bind(d.id)
                    .fetch_one(&self.pool)
                    .await?;
            pages_opened += opened;
            total_pages += d.total_pages;
            by_doc.push(DocReadingProgress {
                id: d.id,
                filename: d.filename.clone(),
                pages_opened: opened,
                total_pages: d.total_pages,
                active: d.active,
            });
        }
        let fiches = self.list_fiches().await?.len() as i64;
        let (attempts, last) = self.quiz_attempt_stats().await?;
        Ok(ProgressionSnapshot {
            pages_opened,
            total_pages,
            pages: vec![],
            by_doc,
            fiches_count: fiches,
            quiz_attempts: attempts,
            last_score: last.map(|(s, _)| s),
            last_total: last.map(|(_, t)| t),
            docs_count: docs.len() as i64,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DocReadingProgress {
    pub id: i64,
    pub filename: String,
    pub pages_opened: i64,
    pub total_pages: i64,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProgressionSnapshot {
    pub pages_opened: i64,
    pub total_pages: i64,
    pub pages: Vec<i64>,
    pub by_doc: Vec<DocReadingProgress>,
    pub fiches_count: i64,
    pub quiz_attempts: i64,
    pub last_score: Option<i64>,
    pub last_total: Option<i64>,
    pub docs_count: i64,
}

fn now_iso() -> String {
    let t: DateTime<Utc> = Utc::now();
    t.to_rfc3339()
}
