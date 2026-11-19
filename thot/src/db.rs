//! SQLite persistence: course, glossary cache, fiches, quiz.

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
            // Ensure parent dir exists for file DB.
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
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_languages(&self, l1: &str, l2: &str) -> Result<()> {
        // Upsert languages onto singleton course if present; otherwise store via placeholder.
        let existing = self.get_course().await?;
        let now = now_iso();
        if let Some(c) = existing {
            sqlx::query("UPDATE course SET l1 = ?, l2 = ? WHERE id = 1")
                .bind(l1)
                .bind(l2)
                .execute(&self.pool)
                .await?;
            let _ = c;
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

    pub async fn upsert_course(&self, filename: &str, text: &str) -> Result<CourseRow> {
        let now = now_iso();
        let langs = self.get_course().await?;
        let (l1, l2) = langs
            .map(|c| (c.l1, c.l2))
            .unwrap_or((None, None));

        sqlx::query("DELETE FROM course")
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM glossary")
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM fiches")
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM quiz")
            .execute(&self.pool)
            .await?;

        sqlx::query(
            "INSERT INTO course (id, filename, text, l1, l2, created_at) VALUES (1, ?, ?, ?, ?, ?)",
        )
        .bind(filename)
        .bind(text)
        .bind(&l1)
        .bind(&l2)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        self.get_course()
            .await?
            .ok_or_else(|| anyhow::anyhow!("course missing after upsert"))
    }

    pub async fn get_course(&self) -> Result<Option<CourseRow>> {
        let row = sqlx::query_as::<_, CourseRow>(
            "SELECT id, filename, text, l1, l2, created_at FROM course WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
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

    pub async fn update_fiche(&self, id: i64, front: &str, back: &str) -> Result<Option<FicheRow>> {
        let res = sqlx::query("UPDATE fiches SET front = ?, back = ?, edited = 1 WHERE id = ?")
            .bind(front)
            .bind(back)
            .bind(id)
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Ok(None);
        }
        let row = sqlx::query_as::<_, FicheRow>(
            "SELECT id, front, back, edited, created_at FROM fiches WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
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
}

fn now_iso() -> String {
    let t: DateTime<Utc> = Utc::now();
    t.to_rfc3339()
}
