# Thot — MVP vertical slice

Rust (`axum`) backend + HTML demo for the Thot study loop:

**Import PDF → read rendered pages (selectable text) → selection menu → L1 sense → generate fiches *or* quiz.**

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
| `THOT_DATA_DIR` | `data` | SQLite + `course.pdf` |
| `THOT_GROQ_INTERACTIVE_MODEL` | `openai/gpt-oss-20b` | Glossary model |
| `THOT_GROQ_BATCH_MODEL` | `openai/gpt-oss-120b` | Fiches / quiz model |
| `THOT_OLLAMA_BASE` | `http://127.0.0.1:11434/v1` | Ollama OpenAI base |
| `THOT_OLLAMA_MODEL` | `qwen2.5` | Local model id |
| `THOT_LLM_STUB` | _(off)_ | `1` = deterministic stub responses |

If Groq and Ollama are both unavailable, the client **falls back to stub** automatically and labels responses with `[STUB]`.

Sample PDF: `fixtures/sample-photosynthesis.pdf`.

## PDF reader (product direction)

Reference UX: Apple **Aperçu / Preview** (iPhone / macOS) — rendered pages, selectable text, selection reveals sense + options.

- Pages rendered with **PDF.js** (canvas) + **text layer** (selectable)
- PDF bytes stored at `data/course.pdf` and served via `GET /api/course/pdf`
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
| `POST` | `/api/import` | `multipart` field `file` | Extract + store PDF bytes |
| `GET` | `/api/course/pdf` | — | Raw PDF for the viewer |
| `POST` | `/api/glossary` | `{ "term" }` | L2→L1 + Wiktionary/Wikipedia + cache |
| `POST` | `/api/generate` | `{ "kind":"fiches"\|"quiz" }` | One generation path |
| `GET` | `/api/fiches` | — | List fiches |
| `PUT` | `/api/fiches/{id}` | `{ "front","back" }` | Edit fiche |
| `GET` | `/api/quiz` | — | Stored quiz |

## What works

- PDF text extraction (`pdf-extract`) with clear failure for empty/scan PDFs
- PDF.js page rendering + selectable text layer + selection→Sens menu
- `LlmClient` OpenAI-compatible: Groq → Ollama → stub
- Glossary: LLM draft → Wiktionary + Wikipedia → reconcile → SQLite cache + sources badge
- Generation of **either** fiches **or** quiz from extracted text
- Fiche edit persistence

## Still out of slice

- Pages-opened reading metric / full progression screen
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
