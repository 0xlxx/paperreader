---
name: paperreader
description: Index, search, extract pages, and detect TOC in PDF/EPUB papers with paperreader. Use when searching papers, extracting pdf pages, scanning chapter headings, or listing the paper library.
---

# Paperreader

Search, catalog, and read local papers (PDF), books (EPUB), and notes (TXT). Structured JSON output with page/chapter divisions. Native CJK text extraction.

## Deep reading

When a search match lands on page N, **never guess** surrounding context from the snippet. Pull the full page — it carries the actual explanation, formula, or table:

```bash
paperreader --file "/path/to/file.pdf" --extract-page 15      # single page
paperreader --file "/path/to/file.pdf" --extract-range 9,67-69  # multiple pages at once
paperreader --file "/path/to/file.pdf" --head                  # first line of every page
paperreader --file "/path/to/file.pdf" --head 3                # first 3 lines
```

`--head` scans for chapter locations in one call instead of a shell loop.

## Index & search

```bash
paperreader --index                       # Build or refresh text index
paperreader --reindex                      # Force full rebuild
paperreader "search term"                  # Case-insensitive search (uses index if available)
paperreader "query" --json                 # JSON output (prefer for scripting)
paperreader --no-index "query"             # Search files directly, skip index
paperreader -d /path/to/papers "query"     # Search specific directory
paperreader --files "smith" "query"        # Search only filenames matching substring
paperreader -r "秦.*统一"                   # Regex search
paperreader -c 3 "query"                   # 3 lines of context around each match
paperreader --case-sensitive "Query"       # Case-sensitive
```

`--index` prints aggregate page/word stats after completion and warns on unindexable pages. `--no-index` prints a time estimate before searching. Zero-result searches include index diagnostics (`indexed_pages` vs `total_pages`) to distinguish "index failed" from "term absent".

## List & inspect

```bash
paperreader --list --json              # Catalog with pages, size, indexed_pages, indexed_words
paperreader --file "paper.pdf" --toc   # Heuristic TOC from first ~25 pages (dot-leaders, chapter headings, 第X章)
paperreader --file "paper.pdf" --check # CJK character coverage check (samples 3 pages)
```

`--list` shows per-document index quality (`indexed_pages`/`indexed_words`) when available. `--toc --json` returns structured entries with page numbers. `--check` samples at 25%/50%/75% and reports CJK ratio — quick quality check for Chinese/Japanese/Korean PDFs.

## JSON output

Stdout is JSON, stderr is progress/logs.

```json
{
  "query": "search query",
  "elapsed_ms": 15.4,
  "indexed": true,
  "total_matches": 12,
  "index_stats": {
    "documents": 3,
    "total_pages": 312,
    "indexed_pages": 287,
    "total_words": 48200
  },
  "matches": [
    {
      "file": "/absolute/path/to/paper.pdf",
      "filename": "paper.pdf",
      "page": 15,
      "total_pages": 40,
      "line_num": 12,
      "line": "This line matched the query word.",
      "match": "query word",
      "match_start": 20,
      "match_end": 30,
      "context_before": ["line before match"],
      "context_after": ["line after match"]
    }
  ]
}
```

`index_stats` appears when the index was used. `indexed_pages` < `total_pages` signals incomplete extraction — try `--reindex` or `--no-index`. `total_matches: 0` with healthy `indexed_pages` means the term genuinely doesn't appear.

Each match carries file path, page, and line. Use `--extract-page` to pull the full page rather than relying on `context_before`/`context_after` alone.

## Interactive

`paperreader -I` starts an fzf terminal UI. Not for agent use.
