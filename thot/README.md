# Thot — MVP vertical slice

Rust (`axum`) backend + HTML demo for the Thot study loop:

**Create/switch courses → import PDFs (per course) → read → Sens → fiches *or* quiz → podcast overview.**

Many independent courses. Each course owns its documents, glossary cache, fiches, quiz, podcasts, and progression. Within a course: many PDFs; glossary / fiches / quiz / podcast span that course's corpus only.

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
| `THOT_GROQ_BATCH_MODEL` | `openai/gpt-oss-120b` | Fiches / quiz / podcast script model |
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
- On text selection: floating menu with primary action **Sens** (L1), plus Copier / Fermer
  - **Single word**: glossary L1 + Wiktionary/Wikipedia verify (unchanged)
  - **Phrase / sentence** (2–40 words): whole-phrase sense in **L1** using course/PDF context, **plus** per-word help for content terms

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
| `POST` | `/api/glossary` | `{ "term" }` | Word: L2→L1 + web verify. Phrase (2+ words): `kind:"phrase"`, `phrase_sense_l1`, `words[]` + cache |
| `POST` | `/api/generate` | `{ "kind":"fiches"\|"quiz" }` | One generation path (full corpus) |
| `GET` | `/api/fiches` | — | List fiches (+ count) |
| `POST` | `/api/fiches` | `{ "front","back","origin?" }` | Append one fiche (Sens capture; no wipe) |
| `GET` | `/api/fiches/recent` | — | Newest fiches (Derniers capturés) |
| `GET` | `/api/fiches/export?format=csv\|anki` | — | Download CSV or Anki-import TSV |
| `GET` | `/api/fiches/{id}` | — | One fiche |
| `PUT` | `/api/fiches/{id}` | `{ "front","back" }` | Edit recto/verso (marks edited) |
| `GET` | `/api/vocab-status` | — | Term → `review`\|`known` for PDF highlight |
| `GET` | `/api/quiz` | — | Stored quiz |
| `POST` | `/api/quiz/attempt` | `{ "score","total" }` | Record quiz attempt |
| `POST` | `/api/quiz/remediate` | `{ "misses":[{question,notion,correct_answer,chosen_answer}] }` | Append remédiation fiches for misses (LLM + course context) |
| `GET` | `/api/podcasts` | — | List course podcast overviews |
| `POST` | `/api/podcasts/generate` | — | Groq two-voice script + edge-tts MP3 (or Web Speech fallback) |
| `GET` | `/api/podcasts/{id}` | — | Podcast + parsed script |
| `GET` | `/api/podcasts/{id}/audio` | — | MP3 bytes when `playback_mode=mp3` |
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
- Glossary: LLM draft → Wiktionary + Wikipedia → reconcile → SQLite cache + badge « Vérifié sources web » / « Sens proposé — sources limitées »
- Phrase Sens: whole-selection L1 sense (course-aware) + word-level helps; respects effective L1/L2 (global or course override)
- Error / empty UX (§10): empty courses, import non-extractible, sense unavailable, selection too wide, quiz blocked, Groq 429 (no silent fail)
- **Mes fiches**: empty state + CTAs, list with aperçu recto, edit recto/verso + save, revise (flip)
- **Capturer depuis Sens** (+ Fiche): saves L2 + L1 sense **with course context** into Mes fiches (oqou-style loop)
- Quiz remédiation: **Revoir fiche** opens the new card from results
- **Quiz**: launch screen, blocked if insufficient material, Q one-by-one + Quit, results + feedback, replay; last score on progression
- **Remédiation**: after quiz misses, show wrong answers + generate targeted new fiches (appended, not wipe) via Groq + course PDF context
- Fiches / quiz generation: LLM (Groq) with course excerpt + effective L1/L2 (global or course override)
- Fiche edits persist and feed progression count
- **Podcast**: two-voice overview from course PDF (Groq script) + **edge-tts** MP3 (free) or Web Speech fallback; listed per course; **no mic retention**
- **P1 vocab UX**: word status in PDF (à revoir / connu), « Derniers capturés » strip, Export CSV + Anki TSV

## Still out of slice

- Async job queue / quota UX polish for 429s
- Live mic tutoring, heavy SRS, paid AI providers

## Layout

```
thot/
  Cargo.toml
  README.md
  scripts/       # podcast_tts.py (edge-tts)
  src/           # pdf, llm, web_verify, glossary, generate, podcast, db, api
  static/        # demo UI + vendor/pdfjs
  fixtures/      # sample PDF
  data/          # created at runtime (gitignored) — docs/ + podcasts/
```
