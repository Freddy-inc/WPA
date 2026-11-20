//! SQLite: many courses; each has docs, glossary, fiches, quiz, progression.

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
    pub title: String,
    pub filename: String,
    pub text: String,
    pub l1: Option<String>,
    pub l2: Option<String>,
    pub active: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DocumentRow {
    pub id: i64,
    pub course_id: i64,
    pub filename: String,
    pub text: String,
    pub total_pages: i64,
    pub active: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct GlossaryRow {
    pub id: i64,
    pub course_id: i64,
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
    pub course_id: i64,
    pub front: String,
    pub back: String,
    pub edited: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct QuizRow {
    pub course_id: i64,
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
            CREATE TABLE IF NOT EXISTS schema_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        let version: Option<(String,)> =
            sqlx::query_as("SELECT value FROM schema_meta WHERE key = 'version'")
                .fetch_optional(&self.pool)
                .await?;
        let ver = version
            .as_ref()
            .and_then(|(v,)| v.parse::<i64>().ok())
            .unwrap_or(0);

        if ver < 2 {
            self.migrate_to_v2().await?;
        }

        // Ensure tables exist (fresh install or after v2).
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS courses (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                filename TEXT NOT NULL DEFAULT '',
                text TEXT NOT NULL DEFAULT '',
                l1 TEXT,
                l2 TEXT,
                active INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS documents (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                course_id INTEGER NOT NULL,
                filename TEXT NOT NULL,
                text TEXT NOT NULL,
                total_pages INTEGER NOT NULL DEFAULT 0,
                active INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS glossary (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                course_id INTEGER NOT NULL,
                term_l2 TEXT NOT NULL COLLATE NOCASE,
                meaning_l1 TEXT NOT NULL,
                context_snippet TEXT,
                sources_json TEXT NOT NULL,
                verified INTEGER NOT NULL DEFAULT 0,
                confidence REAL NOT NULL DEFAULT 0.0,
                created_at TEXT NOT NULL,
                UNIQUE(course_id, term_l2)
            );
            CREATE TABLE IF NOT EXISTS fiches (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                course_id INTEGER NOT NULL,
                front TEXT NOT NULL,
                back TEXT NOT NULL,
                edited INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS quiz (
                course_id INTEGER PRIMARY KEY,
                questions_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS quiz_attempts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                course_id INTEGER NOT NULL,
                score INTEGER NOT NULL,
                total INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS pages_opened (
                doc_id INTEGER NOT NULL,
                page INTEGER NOT NULL CHECK (page >= 1),
                PRIMARY KEY (doc_id, page)
            );
            CREATE TABLE IF NOT EXISTS app_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                l1 TEXT,
                l2 TEXT
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "INSERT INTO schema_meta (key, value) VALUES ('version', '2')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .execute(&self.pool)
        .await?;

        // Seed global languages once from first course that has them (legacy).
        let _ = self.ensure_app_settings().await;
        let (gl1, gl2) = self.get_global_languages().await?;
        if gl1.is_none() || gl2.is_none() {
            if let Some(c) = self.list_courses().await?.into_iter().find(|c| {
                c.l1.as_ref().map(|s| !s.is_empty()).unwrap_or(false)
                    && c.l2.as_ref().map(|s| !s.is_empty()).unwrap_or(false)
            }) {
                let _ = self
                    .set_global_languages(
                        c.l1.as_deref().unwrap_or("fr"),
                        c.l2.as_deref().unwrap_or("en"),
                    )
                    .await;
            }
        }

        // Ensure at least one course exists (no languages on course = use global).
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM courses")
            .fetch_one(&self.pool)
            .await?;
        if count.0 == 0 {
            let _ = self.create_course("Cours 1", None, None).await?;
        } else if self.active_course().await?.is_none() {
            if let Some(c) = self.list_courses().await?.into_iter().next() {
                let _ = self.set_active_course(c.id).await?;
            }
        }
        Ok(())
    }

    async fn ensure_app_settings(&self) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO app_settings (id, l1, l2) VALUES (1, NULL, NULL)",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_global_languages(&self) -> Result<(Option<String>, Option<String>)> {
        self.ensure_app_settings().await?;
        let row: Option<(Option<String>, Option<String>)> =
            sqlx::query_as("SELECT l1, l2 FROM app_settings WHERE id = 1")
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.unwrap_or((None, None)))
    }

    pub async fn set_global_languages(&self, l1: &str, l2: &str) -> Result<()> {
        self.ensure_app_settings().await?;
        sqlx::query(
            "INSERT INTO app_settings (id, l1, l2) VALUES (1, ?, ?)
             ON CONFLICT(id) DO UPDATE SET l1 = excluded.l1, l2 = excluded.l2",
        )
        .bind(l1)
        .bind(l2)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Effective L1/L2 for a course: course override if both set, else global.
    pub async fn effective_languages_for(
        &self,
        course: &CourseRow,
    ) -> Result<(String, String, bool)> {
        let overridden = course
            .l1
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
            && course
                .l2
                .as_ref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
        if overridden {
            return Ok((
                course.l1.clone().unwrap(),
                course.l2.clone().unwrap(),
                true,
            ));
        }
        let (g1, g2) = self.get_global_languages().await?;
        let l1 = g1.filter(|s| !s.trim().is_empty()).ok_or_else(|| {
            anyhow::anyhow!("Déclarez L1/L2 globales au démarrage de l'app")
        })?;
        let l2 = g2.filter(|s| !s.trim().is_empty()).ok_or_else(|| {
            anyhow::anyhow!("Déclarez L1/L2 globales au démarrage de l'app")
        })?;
        Ok((l1, l2, false))
    }

    pub async fn effective_languages_active(&self) -> Result<(String, String, bool)> {
        let course = self
            .get_course()
            .await?
            .ok_or_else(|| anyhow::anyhow!("Aucun cours actif"))?;
        self.effective_languages_for(&course).await
    }

    /// Rebuild from legacy singleton `course` / unscoped tables into multi-course v2.
    async fn migrate_to_v2(&self) -> Result<()> {
        // Snapshot legacy data if present.
        let has_legacy_course: Option<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='course'",
        )
        .fetch_optional(&self.pool)
        .await?;

        let mut legacy_title = "Cours 1".to_string();
        let mut legacy_l1: Option<String> = None;
        let mut legacy_l2: Option<String> = None;
        let mut legacy_filename = String::new();
        let mut legacy_text = String::new();
        let mut legacy_created = now_iso();

        if has_legacy_course.is_some() {
            #[derive(FromRow)]
            struct OldCourse {
                filename: String,
                text: String,
                l1: Option<String>,
                l2: Option<String>,
                created_at: String,
            }
            if let Ok(Some(c)) = sqlx::query_as::<_, OldCourse>(
                "SELECT filename, text, l1, l2, created_at FROM course WHERE id = 1",
            )
            .fetch_optional(&self.pool)
            .await
            {
                legacy_filename = c.filename;
                legacy_text = c.text;
                legacy_l1 = c.l1;
                legacy_l2 = c.l2;
                legacy_created = c.created_at;
                if !legacy_filename.is_empty() {
                    legacy_title = legacy_filename.clone();
                }
            }
        }

        // Docs without course_id
        #[derive(FromRow)]
        struct OldDoc {
            id: i64,
            filename: String,
            text: String,
            total_pages: i64,
            active: bool,
            created_at: String,
        }
        let old_docs: Vec<OldDoc> = {
            let has_docs: Option<(String,)> = sqlx::query_as(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='documents'",
            )
            .fetch_optional(&self.pool)
            .await?;
            if has_docs.is_some() {
                let cols: Vec<(String,)> =
                    sqlx::query_as("SELECT name FROM pragma_table_info('documents')")
                        .fetch_all(&self.pool)
                        .await
                        .unwrap_or_default();
                let names: Vec<_> = cols.into_iter().map(|(n,)| n).collect();
                if names.iter().any(|n| n == "course_id") {
                    Vec::new() // already v2-shaped
                } else {
                    sqlx::query_as::<_, OldDoc>(
                        "SELECT id, filename, text, total_pages, active, created_at FROM documents ORDER BY id",
                    )
                    .fetch_all(&self.pool)
                    .await
                    .unwrap_or_default()
                }
            } else {
                Vec::new()
            }
        };

        #[derive(FromRow)]
        struct OldGloss {
            term_l2: String,
            meaning_l1: String,
            context_snippet: Option<String>,
            sources_json: String,
            verified: bool,
            confidence: f64,
            created_at: String,
        }
        let old_gloss: Vec<OldGloss> = {
            let has: Option<(String,)> = sqlx::query_as(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='glossary'",
            )
            .fetch_optional(&self.pool)
            .await?;
            if has.is_none() {
                Vec::new()
            } else {
                let cols: Vec<(String,)> =
                    sqlx::query_as("SELECT name FROM pragma_table_info('glossary')")
                        .fetch_all(&self.pool)
                        .await
                        .unwrap_or_default();
                if cols.iter().any(|(n,)| n == "course_id") {
                    Vec::new()
                } else {
                    sqlx::query_as(
                        "SELECT term_l2, meaning_l1, context_snippet, sources_json, verified, confidence, created_at FROM glossary",
                    )
                    .fetch_all(&self.pool)
                    .await
                    .unwrap_or_default()
                }
            }
        };

        #[derive(FromRow)]
        struct OldFiche {
            front: String,
            back: String,
            edited: bool,
            created_at: String,
        }
        let old_fiches: Vec<OldFiche> = {
            let has: Option<(String,)> = sqlx::query_as(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='fiches'",
            )
            .fetch_optional(&self.pool)
            .await?;
            if has.is_none() {
                Vec::new()
            } else {
                let cols: Vec<(String,)> =
                    sqlx::query_as("SELECT name FROM pragma_table_info('fiches')")
                        .fetch_all(&self.pool)
                        .await
                        .unwrap_or_default();
                if cols.iter().any(|(n,)| n == "course_id") {
                    Vec::new()
                } else {
                    sqlx::query_as(
                        "SELECT front, back, edited, created_at FROM fiches ORDER BY id",
                    )
                    .fetch_all(&self.pool)
                    .await
                    .unwrap_or_default()
                }
            }
        };

        let old_quiz_json: Option<(String, String)> = {
            let has: Option<(String,)> = sqlx::query_as(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='quiz'",
            )
            .fetch_optional(&self.pool)
            .await?;
            if has.is_none() {
                None
            } else {
                let cols: Vec<(String,)> =
                    sqlx::query_as("SELECT name FROM pragma_table_info('quiz')")
                        .fetch_all(&self.pool)
                        .await
                        .unwrap_or_default();
                if cols.iter().any(|(n,)| n == "course_id") && !cols.iter().any(|(n,)| n == "id") {
                    None
                } else {
                    sqlx::query_as("SELECT questions_json, created_at FROM quiz LIMIT 1")
                        .fetch_optional(&self.pool)
                        .await
                        .ok()
                        .flatten()
                }
            }
        };

        #[derive(FromRow)]
        struct OldAttempt {
            score: i64,
            total: i64,
            created_at: String,
        }
        let old_attempts: Vec<OldAttempt> = {
            let has: Option<(String,)> = sqlx::query_as(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='quiz_attempts'",
            )
            .fetch_optional(&self.pool)
            .await?;
            if has.is_none() {
                Vec::new()
            } else {
                let cols: Vec<(String,)> =
                    sqlx::query_as("SELECT name FROM pragma_table_info('quiz_attempts')")
                        .fetch_all(&self.pool)
                        .await
                        .unwrap_or_default();
                if cols.iter().any(|(n,)| n == "course_id") {
                    Vec::new()
                } else {
                    sqlx::query_as(
                        "SELECT score, total, created_at FROM quiz_attempts ORDER BY id",
                    )
                    .fetch_all(&self.pool)
                    .await
                    .unwrap_or_default()
                }
            }
        };

        #[derive(FromRow)]
        struct OldPage {
            doc_id: i64,
            page: i64,
        }
        let old_pages: Vec<OldPage> = {
            let has: Option<(String,)> = sqlx::query_as(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='pages_opened'",
            )
            .fetch_optional(&self.pool)
            .await?;
            if has.is_none() {
                Vec::new()
            } else {
                let cols: Vec<(String,)> =
                    sqlx::query_as("SELECT name FROM pragma_table_info('pages_opened')")
                        .fetch_all(&self.pool)
                        .await
                        .unwrap_or_default();
                if cols.iter().any(|(n,)| n == "doc_id") {
                    sqlx::query_as("SELECT doc_id, page FROM pages_opened")
                        .fetch_all(&self.pool)
                        .await
                        .unwrap_or_default()
                } else {
                    Vec::new()
                }
            }
        };

        // Drop old tables and recreate v2.
        for t in [
            "pages_opened",
            "quiz_attempts",
            "quiz",
            "fiches",
            "glossary",
            "documents",
            "course",
            "courses",
        ] {
            sqlx::query(&format!("DROP TABLE IF EXISTS {t}"))
                .execute(&self.pool)
                .await?;
        }

        sqlx::query(
            r#"
            CREATE TABLE courses (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                filename TEXT NOT NULL DEFAULT '',
                text TEXT NOT NULL DEFAULT '',
                l1 TEXT,
                l2 TEXT,
                active INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE TABLE documents (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                course_id INTEGER NOT NULL,
                filename TEXT NOT NULL,
                text TEXT NOT NULL,
                total_pages INTEGER NOT NULL DEFAULT 0,
                active INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE TABLE glossary (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                course_id INTEGER NOT NULL,
                term_l2 TEXT NOT NULL COLLATE NOCASE,
                meaning_l1 TEXT NOT NULL,
                context_snippet TEXT,
                sources_json TEXT NOT NULL,
                verified INTEGER NOT NULL DEFAULT 0,
                confidence REAL NOT NULL DEFAULT 0.0,
                created_at TEXT NOT NULL,
                UNIQUE(course_id, term_l2)
            );
            CREATE TABLE fiches (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                course_id INTEGER NOT NULL,
                front TEXT NOT NULL,
                back TEXT NOT NULL,
                edited INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE TABLE quiz (
                course_id INTEGER PRIMARY KEY,
                questions_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE quiz_attempts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                course_id INTEGER NOT NULL,
                score INTEGER NOT NULL,
                total INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE pages_opened (
                doc_id INTEGER NOT NULL,
                page INTEGER NOT NULL CHECK (page >= 1),
                PRIMARY KEY (doc_id, page)
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        let res = sqlx::query(
            "INSERT INTO courses (title, filename, text, l1, l2, active, created_at)
             VALUES (?, ?, ?, ?, ?, 1, ?)",
        )
        .bind(&legacy_title)
        .bind(&legacy_filename)
        .bind(&legacy_text)
        .bind(&legacy_l1)
        .bind(&legacy_l2)
        .bind(&legacy_created)
        .execute(&self.pool)
        .await?;
        let course_id = res.last_insert_rowid();

        let mut id_map: Vec<(i64, i64)> = Vec::new();
        for d in &old_docs {
            let r = sqlx::query(
                "INSERT INTO documents (course_id, filename, text, total_pages, active, created_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(course_id)
            .bind(&d.filename)
            .bind(&d.text)
            .bind(d.total_pages)
            .bind(d.active)
            .bind(&d.created_at)
            .execute(&self.pool)
            .await?;
            id_map.push((d.id, r.last_insert_rowid()));
        }
        // If no docs but course had text, seed one virtual doc row is skipped — corpus stays on course.

        for g in &old_gloss {
            sqlx::query(
                "INSERT OR IGNORE INTO glossary
                 (course_id, term_l2, meaning_l1, context_snippet, sources_json, verified, confidence, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(course_id)
            .bind(&g.term_l2)
            .bind(&g.meaning_l1)
            .bind(&g.context_snippet)
            .bind(&g.sources_json)
            .bind(g.verified)
            .bind(g.confidence)
            .bind(&g.created_at)
            .execute(&self.pool)
            .await?;
        }
        for f in &old_fiches {
            sqlx::query(
                "INSERT INTO fiches (course_id, front, back, edited, created_at) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(course_id)
            .bind(&f.front)
            .bind(&f.back)
            .bind(f.edited)
            .bind(&f.created_at)
            .execute(&self.pool)
            .await?;
        }
        if let Some((qj, ca)) = &old_quiz_json {
            sqlx::query(
                "INSERT INTO quiz (course_id, questions_json, created_at) VALUES (?, ?, ?)",
            )
            .bind(course_id)
            .bind(qj)
            .bind(ca)
            .execute(&self.pool)
            .await?;
        }
        for a in &old_attempts {
            sqlx::query(
                "INSERT INTO quiz_attempts (course_id, score, total, created_at) VALUES (?, ?, ?, ?)",
            )
            .bind(course_id)
            .bind(a.score)
            .bind(a.total)
            .bind(&a.created_at)
            .execute(&self.pool)
            .await?;
        }
        for p in &old_pages {
            if let Some((_, new_id)) = id_map.iter().find(|(old, _)| *old == p.doc_id) {
                sqlx::query("INSERT OR IGNORE INTO pages_opened (doc_id, page) VALUES (?, ?)")
                    .bind(new_id)
                    .bind(p.page)
                    .execute(&self.pool)
                    .await?;
            }
        }

        let _ = self.refresh_course_corpus(course_id).await;
        Ok(())
    }

    /* ---------- courses ---------- */

    pub async fn list_courses(&self) -> Result<Vec<CourseRow>> {
        let rows = sqlx::query_as::<_, CourseRow>(
            "SELECT id, title, filename, text, l1, l2, active, created_at FROM courses ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_course_by_id(&self, id: i64) -> Result<Option<CourseRow>> {
        let row = sqlx::query_as::<_, CourseRow>(
            "SELECT id, title, filename, text, l1, l2, active, created_at FROM courses WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn active_course(&self) -> Result<Option<CourseRow>> {
        let row = sqlx::query_as::<_, CourseRow>(
            "SELECT id, title, filename, text, l1, l2, active, created_at FROM courses WHERE active = 1 LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        if row.is_some() {
            return Ok(row);
        }
        Ok(self.list_courses().await?.into_iter().next())
    }

    pub async fn active_course_id(&self) -> Result<i64> {
        self.active_course()
            .await?
            .map(|c| c.id)
            .ok_or_else(|| anyhow::anyhow!("Aucun cours actif"))
    }

    pub async fn create_course(
        &self,
        title: &str,
        l1: Option<&str>,
        l2: Option<&str>,
    ) -> Result<CourseRow> {
        let title = title.trim();
        if title.is_empty() {
            return Err(anyhow::anyhow!("titre requis"));
        }
        let now = now_iso();
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM courses")
            .fetch_one(&self.pool)
            .await?;
        if count.0 == 0 {
            // first course becomes active
        } else {
            // new course becomes active (switch into it)
            sqlx::query("UPDATE courses SET active = 0")
                .execute(&self.pool)
                .await?;
        }
        let res = sqlx::query(
            "INSERT INTO courses (title, filename, text, l1, l2, active, created_at)
             VALUES (?, '', '', ?, ?, 1, ?)",
        )
        .bind(title)
        .bind(l1)
        .bind(l2)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        let id = res.last_insert_rowid();
        // If this was first and we didn't clear, ensure only one active
        sqlx::query("UPDATE courses SET active = 0 WHERE id != ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("UPDATE courses SET active = 1 WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        self.get_course_by_id(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("course missing after create"))
    }

    pub async fn set_active_course(&self, id: i64) -> Result<CourseRow> {
        if self.get_course_by_id(id).await?.is_none() {
            return Err(anyhow::anyhow!("cours introuvable"));
        }
        sqlx::query("UPDATE courses SET active = 0")
            .execute(&self.pool)
            .await?;
        sqlx::query("UPDATE courses SET active = 1 WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        self.get_course_by_id(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("cours missing after activate"))
    }

    pub async fn delete_course(&self, id: i64) -> Result<()> {
        let docs = self.list_documents(id).await?;
        for d in &docs {
            sqlx::query("DELETE FROM pages_opened WHERE doc_id = ?")
                .bind(d.id)
                .execute(&self.pool)
                .await?;
        }
        sqlx::query("DELETE FROM documents WHERE course_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM glossary WHERE course_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM fiches WHERE course_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM quiz WHERE course_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM quiz_attempts WHERE course_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        let was_active = self
            .get_course_by_id(id)
            .await?
            .map(|c| c.active)
            .unwrap_or(false);
        sqlx::query("DELETE FROM courses WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if was_active {
            if let Some(next) = self.list_courses().await?.into_iter().next() {
                let _ = self.set_active_course(next.id).await?;
            }
        }
        Ok(())
    }

    /// Set global app languages (chosen once at start).
    pub async fn set_languages(&self, l1: &str, l2: &str) -> Result<()> {
        self.set_global_languages(l1, l2).await
    }

    /// Optional per-course override. Pass `None` for both to clear (use global).
    pub async fn set_course_language_override(
        &self,
        course_id: i64,
        l1: Option<&str>,
        l2: Option<&str>,
    ) -> Result<CourseRow> {
        if self.get_course_by_id(course_id).await?.is_none() {
            return Err(anyhow::anyhow!("cours introuvable"));
        }
        match (l1, l2) {
            (None, None) | (Some(""), Some("")) => {
                sqlx::query("UPDATE courses SET l1 = NULL, l2 = NULL WHERE id = ?")
                    .bind(course_id)
                    .execute(&self.pool)
                    .await?;
            }
            (Some(a), Some(b)) if !a.trim().is_empty() && !b.trim().is_empty() => {
                if a.eq_ignore_ascii_case(b) {
                    return Err(anyhow::anyhow!("Choisissez deux langues différentes"));
                }
                sqlx::query("UPDATE courses SET l1 = ?, l2 = ? WHERE id = ?")
                    .bind(a.trim())
                    .bind(b.trim())
                    .bind(course_id)
                    .execute(&self.pool)
                    .await?;
            }
            _ => {
                return Err(anyhow::anyhow!(
                    "Surcharge : L1 et L2 requis ensemble, ou les deux vides pour le défaut global"
                ));
            }
        }
        self.get_course_by_id(course_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("cours introuvable"))
    }

    pub async fn rename_course(&self, id: i64, title: &str) -> Result<CourseRow> {
        let title = title.trim();
        if title.is_empty() {
            return Err(anyhow::anyhow!("titre requis"));
        }
        sqlx::query("UPDATE courses SET title = ? WHERE id = ?")
            .bind(title)
            .bind(id)
            .execute(&self.pool)
            .await?;
        self.get_course_by_id(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("cours introuvable"))
    }

    /// Active course view with refreshed corpus.
    pub async fn get_course(&self) -> Result<Option<CourseRow>> {
        let Some(c) = self.active_course().await? else {
            return Ok(None);
        };
        let _ = self.refresh_course_corpus(c.id).await;
        self.get_course_by_id(c.id).await
    }

    pub async fn refresh_course_corpus(&self, course_id: i64) -> Result<()> {
        let docs = self.list_documents(course_id).await?;
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
        sqlx::query("UPDATE courses SET filename = ?, text = ? WHERE id = ?")
            .bind(&filename)
            .bind(&text)
            .bind(course_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /* ---------- documents (scoped to a course) ---------- */

    pub async fn add_document(
        &self,
        course_id: i64,
        filename: &str,
        text: &str,
    ) -> Result<DocumentRow> {
        let now = now_iso();
        sqlx::query("UPDATE documents SET active = 0 WHERE course_id = ?")
            .bind(course_id)
            .execute(&self.pool)
            .await?;
        let res = sqlx::query(
            "INSERT INTO documents (course_id, filename, text, total_pages, active, created_at)
             VALUES (?, ?, ?, 0, 1, ?)",
        )
        .bind(course_id)
        .bind(filename)
        .bind(text)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        let id = res.last_insert_rowid();
        self.refresh_course_corpus(course_id).await?;
        self.get_document(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("document missing after insert"))
    }

    pub async fn list_documents(&self, course_id: i64) -> Result<Vec<DocumentRow>> {
        let rows = sqlx::query_as::<_, DocumentRow>(
            "SELECT id, course_id, filename, text, total_pages, active, created_at
             FROM documents WHERE course_id = ? ORDER BY id",
        )
        .bind(course_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_document(&self, id: i64) -> Result<Option<DocumentRow>> {
        let row = sqlx::query_as::<_, DocumentRow>(
            "SELECT id, course_id, filename, text, total_pages, active, created_at
             FROM documents WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn active_document(&self, course_id: i64) -> Result<Option<DocumentRow>> {
        let row = sqlx::query_as::<_, DocumentRow>(
            "SELECT id, course_id, filename, text, total_pages, active, created_at
             FROM documents WHERE course_id = ? AND active = 1 LIMIT 1",
        )
        .bind(course_id)
        .fetch_optional(&self.pool)
        .await?;
        if row.is_some() {
            return Ok(row);
        }
        Ok(self.list_documents(course_id).await?.into_iter().next())
    }

    pub async fn set_active_document(&self, id: i64) -> Result<DocumentRow> {
        let doc = self
            .get_document(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("document introuvable"))?;
        sqlx::query("UPDATE documents SET active = 0 WHERE course_id = ?")
            .bind(doc.course_id)
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
        let doc = self
            .get_document(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("document introuvable"))?;
        let was_active = doc.active;
        let course_id = doc.course_id;
        sqlx::query("DELETE FROM pages_opened WHERE doc_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM documents WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if was_active {
            if let Some(next) = self.list_documents(course_id).await?.into_iter().next() {
                let _ = self.set_active_document(next.id).await?;
            }
        }
        self.refresh_course_corpus(course_id).await?;
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

    /* ---------- glossary (active course) ---------- */

    pub async fn get_glossary(&self, term: &str) -> Result<Option<GlossaryRow>> {
        let course_id = self.active_course_id().await?;
        let row = sqlx::query_as::<_, GlossaryRow>(
            r#"SELECT id, course_id, term_l2, meaning_l1, context_snippet, sources_json,
                      verified, confidence, created_at
               FROM glossary WHERE course_id = ? AND term_l2 = ? COLLATE NOCASE"#,
        )
        .bind(course_id)
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
        let course_id = self.active_course_id().await?;
        let now = now_iso();
        sqlx::query(
            r#"INSERT INTO glossary (course_id, term_l2, meaning_l1, context_snippet, sources_json, verified, confidence, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(course_id, term_l2) DO UPDATE SET
                 meaning_l1 = excluded.meaning_l1,
                 context_snippet = excluded.context_snippet,
                 sources_json = excluded.sources_json,
                 verified = excluded.verified,
                 confidence = excluded.confidence,
                 created_at = excluded.created_at"#,
        )
        .bind(course_id)
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

    /* ---------- fiches ---------- */

    pub async fn replace_fiches(&self, fiches: &[(String, String)]) -> Result<Vec<FicheRow>> {
        let course_id = self.active_course_id().await?;
        sqlx::query("DELETE FROM fiches WHERE course_id = ?")
            .bind(course_id)
            .execute(&self.pool)
            .await?;
        let now = now_iso();
        for (front, back) in fiches {
            sqlx::query(
                "INSERT INTO fiches (course_id, front, back, edited, created_at) VALUES (?, ?, ?, 0, ?)",
            )
            .bind(course_id)
            .bind(front)
            .bind(back)
            .bind(&now)
            .execute(&self.pool)
            .await?;
        }
        self.list_fiches().await
    }

    pub async fn list_fiches(&self) -> Result<Vec<FicheRow>> {
        let course_id = self.active_course_id().await?;
        let rows = sqlx::query_as::<_, FicheRow>(
            "SELECT id, course_id, front, back, edited, created_at FROM fiches WHERE course_id = ? ORDER BY id",
        )
        .bind(course_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_fiche(&self, id: i64) -> Result<Option<FicheRow>> {
        let course_id = self.active_course_id().await?;
        let row = sqlx::query_as::<_, FicheRow>(
            "SELECT id, course_id, front, back, edited, created_at FROM fiches WHERE id = ? AND course_id = ?",
        )
        .bind(id)
        .bind(course_id)
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
        let course_id = self.active_course_id().await?;
        let res = sqlx::query(
            "UPDATE fiches SET front = ?, back = ?, edited = 1 WHERE id = ? AND course_id = ?",
        )
        .bind(front)
        .bind(back)
        .bind(id)
        .bind(course_id)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_fiche(id).await
    }

    /* ---------- quiz ---------- */

    pub async fn save_quiz(&self, questions_json: &str) -> Result<QuizRow> {
        let course_id = self.active_course_id().await?;
        let now = now_iso();
        sqlx::query(
            "INSERT INTO quiz (course_id, questions_json, created_at) VALUES (?, ?, ?)
             ON CONFLICT(course_id) DO UPDATE SET questions_json = excluded.questions_json, created_at = excluded.created_at",
        )
        .bind(course_id)
        .bind(questions_json)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        self.get_quiz()
            .await?
            .ok_or_else(|| anyhow::anyhow!("quiz missing after save"))
    }

    pub async fn get_quiz(&self) -> Result<Option<QuizRow>> {
        let course_id = self.active_course_id().await?;
        let row = sqlx::query_as::<_, QuizRow>(
            "SELECT course_id, questions_json, created_at FROM quiz WHERE course_id = ?",
        )
        .bind(course_id)
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
        let course_id = self.active_course_id().await?;
        let now = now_iso();
        sqlx::query(
            "INSERT INTO quiz_attempts (course_id, score, total, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(course_id)
        .bind(score)
        .bind(total)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn quiz_attempt_stats(&self) -> Result<(i64, Option<(i64, i64)>)> {
        let course_id = self.active_course_id().await?;
        let (attempts,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM quiz_attempts WHERE course_id = ?")
                .bind(course_id)
                .fetch_one(&self.pool)
                .await?;
        let last: Option<(i64, i64)> = sqlx::query_as(
            "SELECT score, total FROM quiz_attempts WHERE course_id = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(course_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok((attempts, last))
    }

    pub async fn progression_snapshot(&self) -> Result<ProgressionSnapshot> {
        let course_id = self.active_course_id().await?;
        let docs = self.list_documents(course_id).await?;
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
            course_id,
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
    pub course_id: i64,
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
