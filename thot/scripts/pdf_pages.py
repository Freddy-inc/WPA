#!/usr/bin/env python3
"""Extract plain text per PDF page (pypdf). stdout = JSON list of page strings.

Usage:
  python3 pdf_pages.py --pdf path/to/file.pdf
"""
from __future__ import annotations

import argparse
import json
import sys


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--pdf", required=True)
    args = ap.parse_args()
    try:
        from pypdf import PdfReader
    except ImportError:
        print("pypdf missing — pip install pypdf", file=sys.stderr)
        return 2
    try:
        reader = PdfReader(args.pdf)
        pages = []
        for page in reader.pages:
            raw = page.extract_text() or ""
            cleaned = "\n".join(
                line.strip() for line in raw.splitlines() if line.strip()
            )
            pages.append(cleaned)
        json.dump(pages, sys.stdout, ensure_ascii=False)
        return 0
    except Exception as e:
        print(f"pdf_pages error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
