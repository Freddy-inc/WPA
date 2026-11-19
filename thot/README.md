# Thot — MVP vertical slice

Rust (`axum`) backend + minimal HTML demo for the Thot study loop:

**Import PDF → define L2 term (L1 meaning + web sources) → generate fiches *or* quiz.**

Free AI only: **Groq** when `GROQ_API_KEY` is set, otherwise **Ollama** (OpenAI-compatible).

This folder is isolated from the existing WPA static site.

## Requirements

- Rust 1.75+ (`cargo`)
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
cargo run
```

Open **http://127.0.0.1:8787/**

| Env | Default | Meaning |
| --- | --- | --- |
| `GROQ_API_KEY` | _(empty)_ | Groq primary; absent → Ollama |
| `THOT_PORT` | `8787` | HTTP port |
| `THOT_DATA_DIR` | `data` | SQLite dir (`thot.db`) |
| `THOT_GROQ_INTERACTIVE_MODEL` | `openai/gpt-oss-20b` | Glossary model |
| `THOT_GROQ_BATCH_MODEL` | `openai/gpt-oss-120b` | Fiches / quiz model |
| `THOT_OLLAMA_BASE` | `http://127.0.0.1:11434/v1` | Ollama OpenAI base |
| `THOT_OLLAMA_MODEL` | `qwen2.5` | Local model id |
| `THOT_LLM_STUB` | _(off)_ | `1` = deterministic stub responses (demo / CI without LLM) |

If Groq and Ollama are both unavailable, the client **falls back to stub** automatically and labels responses with `[STUB]`.

Sample PDF: `fixtures/sample-photosynthesis.pdf`.

## API (demo)

| Method | Path | Body | Role |
| --- | --- | --- | --- |
| `GET` | `/api/health` | — | Provider + key presence |
| `GET` | `/api/status` | — | Course / counts |
| `PUT` | `/api/languages` | `{ "l1","l2" }` | Language pair |
| `POST` | `/api/import` | `multipart` field `file` | PDF extract → SQLite |
| `POST` | `/api/glossary` | `{ "term" }` | L2→L1 + Wiktionary/Wikipedia + cache |
| `POST` | `/api/generate` | `{ "kind":"fiches"\|"quiz" }` | One generation path |
| `GET` | `/api/fiches` | — | List fiches |
| `PUT` | `/api/fiches/{id}` | `{ "front","back" }` | Edit fiche |
| `GET` | `/api/quiz` | — | Stored quiz |

## What works

- PDF text extraction (`pdf-extract`) with clear failure for empty/scan PDFs
- `LlmClient` OpenAI-compatible: Groq primary, Ollama fallback
- Glossary: LLM draft → Wiktionary + Wikipedia → overlap / reconcile → SQLite cache + sources badge
- Generation of **either** fiches **or** quiz from extracted text (same endpoint, `kind`)
- Minimal HTML UI exercising the full loop
- Fiche edit persistence

## Stubbed / out of slice

- Full PDF page renderer (demo shows extracted text preview, not a page viewer)
- Pages-opened reading metric / progression screen
- Async job queue / quota UX polish for 429s
- Multi-course, audio, SRS, paid AI providers

## Layout

```
thot/
  Cargo.toml
  README.md
  src/           # pdf, llm, web_verify, glossary, generate, db, api
  static/        # demo UI
  fixtures/      # sample PDF
  data/          # created at runtime (gitignored)
```
