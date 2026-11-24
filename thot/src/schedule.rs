//! Study prefs, revision methods catalog, and rule-based revision plan.

use anyhow::Result;
use chrono::{Datelike, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::{ClassSlotRow, Db, ExamRow, RevisionSessionRow};

#[derive(Debug, Clone, Serialize)]
pub struct MethodInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub learn: &'static str,
    pub thot_applies: &'static str,
}

pub const METHODS: &[MethodInfo] = &[
    MethodInfo {
        id: "active_recall",
        name: "Rappel actif",
        summary: "Tester sa mémoire avant de relire.",
        learn: "Posez-vous des questions ou faites un quiz sans regarder le cours, puis vérifiez. L’effort de récupération renforce la mémoire plus que la relecture passive.",
        thot_applies: "Dans Étudier, le quiz est mis en avant. Les fiches servent de contrôle après le test.",
    },
    MethodInfo {
        id: "leitner",
        name: "Leitner / espacement",
        summary: "Revoir plus souvent ce qui est fragile.",
        learn: "Les notions « à revoir » reviennent plus tôt ; les « connues » plus tard. L’espacement évite l’illusion de maîtrise.",
        thot_applies: "Thot priorise les fiches marquées à revoir et le vocabulaire fragile avant le reste.",
    },
    MethodInfo {
        id: "pomodoro",
        name: "Pomodoro",
        summary: "Blocs courts (25 min) + pause.",
        learn: "Travaillez 25 minutes sans distraction, pause 5 minutes. Quatre blocs, puis une pause plus longue.",
        thot_applies: "Un minuteur 25 min apparaît dans Étudier ; une session = un bloc fiches ou quiz.",
    },
    MethodInfo {
        id: "feynman",
        name: "Feynman",
        summary: "Expliquer simplement, à voix haute ou à l’écrit.",
        learn: "Reformulez une notion comme si vous l’enseigniez à un débutant. Les trous dans l’explication montrent quoi retravailler.",
        thot_applies: "Sur chaque fiche, Thot vous invite à expliquer le verso avant de le révéler.",
    },
    MethodInfo {
        id: "interleaving",
        name: "Entrelacement",
        summary: "Alterner fiches et quiz plutôt qu’un seul type.",
        learn: "Mélanger les types de tâches améliore le transfert et réduit le surapprentissage d’un format.",
        thot_applies: "Le plan et Étudier alternent sessions fiches et quiz.",
    },
];

pub fn method_by_id(id: &str) -> Option<&'static MethodInfo> {
    METHODS.iter().find(|m| m.id == id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudyPrefs {
    pub method: String,
    pub reminders_enabled: bool,
    pub reminder_hour: u8,
    pub reminder_minute: u8,
    /// "fiches" | "quiz" | "both"
    pub reminder_target: String,
}

impl Default for StudyPrefs {
    fn default() -> Self {
        Self {
            method: "active_recall".into(),
            reminders_enabled: false,
            reminder_hour: 18,
            reminder_minute: 0,
            reminder_target: "both".into(),
        }
    }
}

impl StudyPrefs {
    pub async fn load(db: &Db) -> Result<Self> {
        let raw = db.get_kv("study_prefs").await?;
        if let Some(s) = raw {
            if let Ok(p) = serde_json::from_str::<StudyPrefs>(&s) {
                return Ok(p);
            }
        }
        Ok(Self::default())
    }

    pub async fn save(&self, db: &Db) -> Result<()> {
        let s = serde_json::to_string(self)?;
        db.set_kv("study_prefs", &s).await
    }
}

const DAY_NAMES: &[&str] = &[
    "Lundi",
    "Mardi",
    "Mercredi",
    "Jeudi",
    "Vendredi",
    "Samedi",
    "Dimanche",
];

pub fn day_label(dow: i64) -> &'static str {
    DAY_NAMES
        .get(dow as usize)
        .copied()
        .unwrap_or("?")
}

/// Chrono weekday: Mon=0 … Sun=6 (matches our schema).
fn chrono_to_schema_dow(d: chrono::Weekday) -> i64 {
    d.num_days_from_monday() as i64
}

fn parse_ymd(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

fn has_class_on(slots: &[ClassSlotRow], date: NaiveDate) -> bool {
    let dow = chrono_to_schema_dow(date.weekday());
    slots.iter().any(|s| s.day_of_week == dow)
}

/// Rule-based plan: sessions on free days, denser near exams, shaped by method.
pub async fn generate_plan(db: &Db) -> Result<Vec<RevisionSessionRow>> {
    let prefs = StudyPrefs::load(db).await?;
    let method = if method_by_id(&prefs.method).is_some() {
        prefs.method.clone()
    } else {
        "active_recall".into()
    };
    let slots = db.list_class_slots().await?;
    let exams = db.list_exams().await?;
    let courses = db.list_courses().await?;

    db.clear_future_revision_sessions().await?;

    let today = Utc::now().date_naive();
    let mut out = Vec::new();

    if exams.is_empty() {
        // No exams: light weekly rhythm for 14 days on free evenings.
        for i in 0..14 {
            let d = today + Duration::days(i);
            if has_class_on(&slots, d) && i % 2 == 0 {
                continue;
            }
            let kind = session_kind_for(&method, i as usize);
            let title = format!("{} — {}", method_title(&method), kind_label(kind));
            let course_id = courses.first().map(|c| c.id);
            let row = db
                .add_revision_session(
                    course_id,
                    &d.format("%Y-%m-%d").to_string(),
                    kind,
                    &method,
                    &title,
                )
                .await?;
            out.push(row);
        }
        return Ok(out);
    }

    for exam in &exams {
        let Some(exam_day) = parse_ymd(&exam.exam_date) else {
            continue;
        };
        if exam_day < today {
            continue;
        }
        let course_title = courses
            .iter()
            .find(|c| c.id == exam.course_id)
            .map(|c| c.title.as_str())
            .unwrap_or("Cours");

        // Sessions: 14, 7, 3, 1 days before + day-before evening style
        let offsets = [14i64, 10, 7, 5, 3, 2, 1];
        for (idx, off) in offsets.iter().enumerate() {
            let d = exam_day - Duration::days(*off);
            if d < today {
                continue;
            }
            // Prefer days without class; if conflict, still schedule (student can move).
            let kind = if *off <= 2 {
                "mixte"
            } else {
                session_kind_for(&method, idx)
            };
            let title = format!(
                "{} · {} (J-{})",
                course_title,
                kind_label(kind),
                off
            );
            let row = db
                .add_revision_session(
                    Some(exam.course_id),
                    &d.format("%Y-%m-%d").to_string(),
                    kind,
                    &method,
                    &title,
                )
                .await?;
            out.push(row);
        }
    }

    // Sort by date for response
    out.sort_by(|a, b| a.scheduled_date.cmp(&b.scheduled_date));
    Ok(out)
}

fn method_title(id: &str) -> &str {
    method_by_id(id).map(|m| m.name).unwrap_or("Révision")
}

fn kind_label(kind: &str) -> &str {
    match kind {
        "fiches" => "fiches",
        "quiz" => "quiz",
        _ => "fiches + quiz",
    }
}

fn session_kind_for(method: &str, i: usize) -> &'static str {
    match method {
        "active_recall" => {
            if i % 3 == 0 {
                "quiz"
            } else if i % 3 == 1 {
                "mixte"
            } else {
                "fiches"
            }
        }
        "leitner" => {
            if i % 2 == 0 {
                "fiches"
            } else {
                "quiz"
            }
        }
        "interleaving" => {
            if i % 2 == 0 {
                "fiches"
            } else {
                "quiz"
            }
        }
        "feynman" => "fiches",
        "pomodoro" => {
            if i % 2 == 0 {
                "fiches"
            } else {
                "quiz"
            }
        }
        _ => {
            if i % 2 == 0 {
                "fiches"
            } else {
                "quiz"
            }
        }
    }
}

pub fn methods_json() -> serde_json::Value {
    json!({
        "methods": METHODS.iter().map(|m| json!({
            "id": m.id,
            "name": m.name,
            "summary": m.summary,
            "learn": m.learn,
            "thot_applies": m.thot_applies,
        })).collect::<Vec<_>>(),
    })
}

pub fn slot_json(s: &ClassSlotRow, course_title: Option<&str>) -> serde_json::Value {
    json!({
        "id": s.id,
        "course_id": s.course_id,
        "course_title": course_title,
        "day_of_week": s.day_of_week,
        "day_label": day_label(s.day_of_week),
        "start_time": s.start_time,
        "end_time": s.end_time,
        "label": s.label,
        "created_at": s.created_at,
    })
}

pub fn exam_json(e: &ExamRow, course_title: Option<&str>) -> serde_json::Value {
    json!({
        "id": e.id,
        "course_id": e.course_id,
        "course_title": course_title,
        "title": e.title,
        "exam_date": e.exam_date,
        "created_at": e.created_at,
    })
}

pub fn session_json(s: &RevisionSessionRow, course_title: Option<&str>) -> serde_json::Value {
    json!({
        "id": s.id,
        "course_id": s.course_id,
        "course_title": course_title,
        "scheduled_date": s.scheduled_date,
        "kind": s.kind,
        "method": s.method,
        "title": s.title,
        "done": s.done,
        "created_at": s.created_at,
    })
}
