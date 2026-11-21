#!/usr/bin/env python3
"""Synthesize a two-voice podcast MP3 from a JSON script using edge-tts (free, no key).

Usage:
  python3 podcast_tts.py --script turns.json --out out.mp3 --lang fr

Script JSON:
  {"turns":[{"speaker":"A","text":"..."},{"speaker":"B","text":"..."}]}
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
import subprocess
import sys
import tempfile


VOICE_PAIRS = {
    "fr": ("fr-FR-DeniseNeural", "fr-FR-HenriNeural"),
    "en": ("en-US-JennyNeural", "en-US-GuyNeural"),
    "es": ("es-ES-ElviraNeural", "es-ES-AlvaroNeural"),
    "de": ("de-DE-KatjaNeural", "de-DE-ConradNeural"),
}


async def synth_one(text: str, voice: str, path: str) -> None:
    import edge_tts

    communicate = edge_tts.Communicate(text, voice)
    await communicate.save(path)


def concat_mp3(parts: list[str], out_path: str) -> None:
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        raise RuntimeError("ffmpeg not found")
    if len(parts) == 1:
        shutil.copyfile(parts[0], out_path)
        return
    list_path = out_path + ".txt"
    with open(list_path, "w", encoding="utf-8") as f:
        for p in parts:
            f.write(f"file '{p}'\n")
    try:
        # Re-encode instead of stream-copy: more reliable across edge-tts part files.
        subprocess.run(
            [
                ffmpeg,
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
                list_path,
                "-codec:a",
                "libmp3lame",
                "-q:a",
                "5",
                out_path,
            ],
            check=True,
            capture_output=True,
        )
    finally:
        try:
            os.remove(list_path)
        except OSError:
            pass


async def run(script_path: str, out_path: str, lang: str) -> None:
    with open(script_path, encoding="utf-8") as f:
        data = json.load(f)
    turns = data.get("turns") or []
    if not turns:
        raise RuntimeError("empty turns")
    voice_a, voice_b = VOICE_PAIRS.get(lang[:2].lower(), VOICE_PAIRS["en"])
    parts: list[str] = []
    with tempfile.TemporaryDirectory(prefix="thot-pod-") as td:
        for i, turn in enumerate(turns):
            text = (turn.get("text") or "").strip()
            if not text:
                continue
            speaker = (turn.get("speaker") or "A").upper()
            voice = voice_a if speaker.startswith("A") else voice_b
            part = os.path.join(td, f"part-{i:03d}.mp3")
            await synth_one(text, voice, part)
            if os.path.getsize(part) > 0:
                parts.append(part)
        if not parts:
            raise RuntimeError("no audio parts synthesized")
        concat_mp3(parts, out_path)
    if not os.path.isfile(out_path) or os.path.getsize(out_path) < 64:
        raise RuntimeError("output mp3 missing or too small")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--script", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--lang", default="fr")
    args = ap.parse_args()
    try:
        asyncio.run(run(args.script, args.out, args.lang))
    except Exception as e:
        print(f"podcast_tts error: {e}", file=sys.stderr)
        return 1
    print(args.out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
