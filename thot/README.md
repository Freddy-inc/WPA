# Thot — MVP vertical slice

Rust (`axum`) backend + HTML demo for the Thot study loop:

**Create/switch courses → import PDFs (per course) → read → Sens → fiches *or* quiz.**

Many independent courses. Each course owns its documents, glossary cache, fiches, quiz, and progression. Within a course: many PDFs; glossary / fiches / quiz span that course's corpus only.

Free AI only: **Groq** when `GROQ_API_KEY` is set, else fallback `Etude`, else `Etudes` (WPA runtime secret). Otherwise **Ollama**. Stub mode for demos without either. Health reports which env name was present — never the secret.

This folder is isolated from the existing WPA static site.

## Requirements

- Rust stable (`cargo`) — see `rust-toolchain.toml`
- Optional: [`GROQ_API_KEY`](https://console.groq.com/) for cloud free tier
- Optional fallback: [Ollama](https://ollama.com/) with a chat model (default `qwen2.5`)

```bash
# if using Ollama fallback
ollama pull qwen2.5
ollama serve
```

## Run

```bash
cd thot
export GROQ_API_KEY=...          # optional — without it, Ollama is used
# or: export THOT_LLM_STUB=1     # deterministic [STUB] demo without LLM
cargo run
```

Open **http://127.0.0.1:8787/**

| Env | Default | Meaning |
| --- | --- | --- |
| `GROQ_API_KEY` | _(empty)_ | Groq primary (preferred) |
| `Etude` | _(empty)_ | Groq key fallback if `GROQ_API_KEY` absent |
| `Etudes` | _(empty)_ | Groq key fallback if both above absent (WPA runtime) |
| `THOT_PORT` | `8787` | HTTP port |
| `THOT_DATA_DIR` | `data` | SQLite + `docs/{id}.pdf` |
| `THOT_GROQ_INTERACTIVE_MODEL` | `openai/gpt-oss-20b` | Glossary model |
| `THOT_GROQ_BATCH_MODEL` | `openai/gpt-oss-120b` | Fiches / quiz model |
| `THOT_OLLAMA_BASE` | `http://127.0.0.1:11434/v1` | Ollama OpenAI base |
| `THOT_OLLAMA_MODEL` | `qwen2.5` | Local model id |
| `THOT_LLM_STUB` | _(off)_ | `1` = deterministic stub responses |

If Groq and Ollama are both unavailable, the client **falls back to stub** automatically and labels responses with `[STUB]`.

Sample PDFs: `fixtures/sample-photosynthesis.pdf`, `fixtures/sample-cell-respiration.pdf`.

## Languages (L1 / L2)

- **Global** (once at app start): `PUT /api/languages` — default for every course.
- **Optional per-course override**: `PUT /api/courses/{id}/languages` with `{l1,l2}` or `{clear:true}`.
- Glossary / fiches / quiz use the **effective** pair (override if set, else global).

## Multi-course + multi-document (rules)

| Concern | Rule |
| --- | --- |
| Courses | Many independent courses (`POST /api/courses`, `PUT …/activate`) |
| Languages | Global L1/L2 at app start; optional per-course override (default = global) |
| Isolation | Docs, glossary cache, fiches, quiz, progression are **per course** |
| Documents | Many PDFs under the **active** course (`POST /api/import` adds, never wipes siblings) |
| Reader | Opens the **active** document of the active course |
| Glossary | Cache scoped to active course; corpus = that course's docs |
| Fiches / quiz | Generated from the active course's corpus only |
| Progression | Pages ouvertes **per doc** + **total** for the active course |

## PDF reader (product direction)

Reference UX: Apple **Aperçu / Preview** (iPhone / macOS) — rendered pages, selectable text, selection reveals sense + options.

- Pages rendered with **PDF.js** (canvas) + **text layer** (selectable)
- PDF bytes stored at `data/docs/{id}.pdf` (active also mirrored to `course.pdf` for legacy)
- On text selection: floating menu with primary action **Sens** (glossary L1), plus Copier / Fermer

### Gaps vs full Aperçu / Preview

- Single-page focus with prev/next (not continuous scroll of all pages at once)
- No pinch-zoom / fit-width / Markup tools beyond auto-fit scale
- No native system selection UI (custom menu; primary action is glossary)
- Text layer depends on PDF text objects (scans without OCR still fail import)

## API (demo)

| Method | Path | Body | Role |
| --- | --- | --- | --- |
| `GET` | `/api/health` | — | Provider + key presence |
| `GET` | `/api/status` | — | Courses / active course / docs / counts |
| `GET` | `/api/courses` | — | List courses |
| `POST` | `/api/courses` | `{ "title","l1?","l2?" }` | Create + activate course |
| `PUT` | `/api/courses/{id}/activate` | — | Switch active course |
| `PUT` | `/api/courses/{id}` | `{ "title" }` | Rename |
| `DELETE` | `/api/courses/{id}` | — | Delete course + its data |
| `GET` | `/api/languages` | — | Global + effective L1/L2 |
| `PUT` | `/api/languages` | `{ "l1","l2" }` | Set **global** app languages |
| `PUT` | `/api/courses/{id}/languages` | `{ "l1","l2" }` or `{ "clear":true }` | Optional course override |
| `POST` | `/api/import` | `multipart` field `file` | Add PDF to **active** course |
| `GET` | `/api/documents` | — | List documents |
| `PUT` | `/api/documents/{id}/activate` | — | Switch active reader PDF |
| `DELETE` | `/api/documents/{id}` | — | Remove one document |
| `GET` | `/api/documents/{id}/pdf` | — | Raw PDF for that document |
| `GET` | `/api/course/pdf` | — | Active document PDF (compat) |
| `POST` | `/api/glossary` | `{ "term" }` | L2→L1 + Wiktionary/Wikipedia + cache |
| `POST` | `/api/generate` | `{ "kind":"fiches"\|"quiz" }` | One generation path (full corpus) |
| `GET` | `/api/fiches` | — | List fiches (+ count) |
| `GET` | `/api/fiches/{id}` | — | One fiche |
| `PUT` | `/api/fiches/{id}` | `{ "front","back" }` | Edit recto/verso (marks edited) |
| `GET` | `/api/quiz` | — | Stored quiz |
| `POST` | `/api/quiz/attempt` | `{ "score","total" }` | Record quiz attempt |
| `POST` | `/api/pages/open` | `{ "page","total_pages?","doc_id?" }` | Mark page opened for a doc |
| `GET` | `/api/progression` | — | Per-doc + total lecture / fiches / quiz |

## What works

- Several independent courses: create / list / switch; each isolates docs·glossary·fiches·quiz·progression
- Global L1/L2 at app start; optional per-course language override (default = global)
- Several PDFs under the active course: add / list / switch; corpus scoped to that course
- PDF text extraction (`pdf-extract`) with clear failure for empty/scan PDFs
- PDF.js page rendering + selectable text layer + selection→Sens menu
- **Progression**: pages ouvertes per document + total, fiches count, quiz attempts + last score, CTAs
- `LlmClient` OpenAI-compatible: Groq → Ollama → stub
- Glossary: LLM draft → Wiktionary + Wikipedia → reconcile → SQLite cache + sources badge
- **Mes fiches**: empty state + CTAs, list with aperçu recto, edit recto/verso + save, revise (flip)
- **Quiz**: launch screen, blocked if insufficient material, Q one-by-one + Quit, results + feedback, replay; last score on progression
- Fiche edits persist and feed progression count

## Still out of slice

- Async job queue / quota UX polish for 429s
- Audio, SRS, paid AI providers

## Layout

```
thot/
  Cargo.toml
  README.md
  src/           # pdf, llm, web_verify, glossary, generate, db, api
  static/        # demo UI + vendor/pdfjs
  fixtures/      # sample PDF
  data/          # created at runtime (gitignored)
```
