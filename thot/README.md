# Thot — MVP vertical slice

Rust (`axum`) backend + HTML demo for the Thot study loop:

**Import PDFs (same course) → switch documents → read rendered pages → selection → L1 sense → fiches *or* quiz on the full corpus.**

One course, many documents. Glossary / fiches / quiz span all PDFs. Progression tracks pages per document and a course total.

Free AI only: **Groq** when `GROQ_API_KEY` is set, otherwise **Ollama** (OpenAI-compatible). Stub mode for demos without either.

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
| `GROQ_API_KEY` | _(empty)_ | Groq primary; absent → Ollama |
| `THOT_PORT` | `8787` | HTTP port |
| `THOT_DATA_DIR` | `data` | SQLite + `docs/{id}.pdf` |
| `THOT_GROQ_INTERACTIVE_MODEL` | `openai/gpt-oss-20b` | Glossary model |
| `THOT_GROQ_BATCH_MODEL` | `openai/gpt-oss-120b` | Fiches / quiz model |
| `THOT_OLLAMA_BASE` | `http://127.0.0.1:11434/v1` | Ollama OpenAI base |
| `THOT_OLLAMA_MODEL` | `qwen2.5` | Local model id |
| `THOT_LLM_STUB` | _(off)_ | `1` = deterministic stub responses |

If Groq and Ollama are both unavailable, the client **falls back to stub** automatically and labels responses with `[STUB]`.

Sample PDFs: `fixtures/sample-photosynthesis.pdf`, `fixtures/sample-cell-respiration.pdf`.

## Multi-document course (rules)

| Concern | Rule |
| --- | --- |
| Course | Singleton — languages L1/L2 shared |
| Documents | Many PDFs under that one course (`POST /api/import` adds, never wipes siblings) |
| Reader | Opens the **active** document (`PUT …/activate` to switch) |
| Glossary | Shared cache; context defaults to concatenated corpus of all docs |
| Fiches / quiz | Generated from the full corpus (all docs) |
| Progression | Pages ouvertes **per doc** + **total**; fiches/quiz counts are course-level |

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
| `GET` | `/api/status` | — | Course / counts / `has_pdf` |
| `PUT` | `/api/languages` | `{ "l1","l2" }` | Language pair |
| `POST` | `/api/import` | `multipart` field `file` | Add PDF to the course (keeps prior docs) |
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

- Several PDFs under one course: add / list / switch; shared glossary·fiches·quiz corpus
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
- Multi-course, audio, SRS, paid AI providers

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
