---
name: paperreader
description: Index, search, extract pages, and detect TOC in PDF papers with paperreader. Use when searching papers, extracting pdf pages, scanning chapter headings, or listing the paper library.
---

# Paperreader

Search, catalog, and read local papers (PDF) and notes (TXT). Structured JSON output with page/chapter divisions. Native CJK text extraction.

## Research workflow

Three strategies, ordered by token efficiency:

### A. TOC → targeted extract (most efficient)

```bash
# 1. Survey: ~63 entries, ~1K tokens
paperreader --file "book.pdf" --toc --json

# 2. Identify relevant section pages. Extract only those pages, not the whole chapter.
#    "Lock-Free Concurrency" on p285 → extract p285-290 (~5 pages, ~2.5K tokens)
paperreader --file "book.pdf" --extract-range 285-290
```

Total: ~3.5K tokens for targeted reading. No search overhead.

### B. TOC → chapter extract (good for deep reading)

```bash
# 1. TOC to find the chapter
# 2. Extract the whole chapter (~30 pages, ~15K tokens)
paperreader --file "book.pdf" --extract-range 204-235
```

### C. Search (use when you don't know where to look)

```bash
paperreader --file "book.pdf" "specific term" --json
# 97 matches → ~8K tokens. Snippets lack context — use --extract-page to pull
# full pages for matches that look relevant.
```

**Anti-pattern**: `paperreader -d /papers "broad topic" --json` → 700+ matches across all documents, ~50K tokens before you've read anything.

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

`--index` prints page/word stats and warns on unindexable pages. `--no-index` prints a time estimate before searching. Zero-result searches include index diagnostics to distinguish "index failed" from "term absent".

## Table of contents

```bash
paperreader --file "paper.pdf" --toc        # Extract TOC (PDF outlines first, fallback to heuristics)
paperreader --file "paper.pdf" --toc --json # Structured JSON with page numbers and hierarchy
```

Extraction priority: (1) PDF embedded outlines — the same tree PDF readers use for the sidebar, fast and zero false positives; (2) heuristic detection from printed TOC pages. When the document is indexed, all pages are scanned via disk cache (~0.1s); otherwise samples ~25 pages.

JSON output includes hierarchy levels: `0` = part/title, `1` = chapter, `2` = section, `3` = subsection.

## List & inspect

```bash
paperreader --list --json              # Catalog with pages, size, indexed_pages, indexed_words
paperreader --file "paper.pdf" --check # CJK character coverage check (samples 3 pages)
```

`--list` shows per-document index quality (`indexed_pages`/`indexed_words`) when available. `--check` samples 3 pages and reports CJK character ratio.

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
