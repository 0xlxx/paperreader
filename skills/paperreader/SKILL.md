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
paperreader --file "paper.pdf" --toc              # Extract TOC (outlines first, then heuristics)
paperreader --file "paper.pdf" --toc --json       # Structured JSON: source, total_pages, entries
paperreader --toc-modes                          # List available detection algorithms
paperreader --file "paper.pdf" --toc --toc-mode layout  # Force one algorithm (see below)
paperreader --file "paper.pdf" --toc --toc-heuristic   # Heuristics only, skip embedded outlines
```

When the default `--toc` result is poor, inspect the PDF and force the algorithm that fits:

```
paperreader --toc-modes
  auto       Default: embedded outlines -> plain scan -> layout scan -> heading scan
  outlines   Embedded PDF outlines only (/Outlines)
  heuristic  Heuristic chain only (plain -> layout -> heading), skipping outlines
  plain      Plain-text scan only (dot leaders / right-aligned numbers / multi-column)
  layout     Layout-aware scan only (word geometry)
  heading    Heading scan only (sampled pages)
```

Forced modes (`plain`/`layout`/`heading`) run **only** that algorithm and return an empty
TOC when it finds nothing — e.g. garbled-ToUnicode textbooks need `--toc-mode layout`,
tech books whose TOC page-number column is not in the text layer need `--toc-mode plain`,
and papers without any TOC structure need `--toc-mode heading`.

Extraction priority (aligned with ISO 32000-1):

1. **PDF embedded outlines** (`/Outlines`, §12.3.3) — the exact tree PDF readers render in the sidebar; fast and zero false positives. Handles UTF-16/UTF-8/PDFDocEncoding titles, `/Dest`, GoTo actions, and named destinations.
2. **Printed TOC pages** — detects the 目录/Contents page(s), then parses entries. A fast plain-text pass (dot leaders, right-aligned page numbers, wrapped-title continuations, multi-column grid TOCs) covers most textbooks in ~0.02s; when plain text is garbled (broken ToUnicode CMaps, interleaved page numbers), a layout-aware pass uses word geometry (right-aligned page runs, indentation, font size) on only the marker page ±2 pages.
3. **Heading scan** — last resort: scan (sampled) pages for strong chapter/section heading patterns.

Page numbers are reported as **physical 1-indexed pages** (usable with `--extract-page`/`--extract-range`): each title is located in the body (via the on-disk index cache when available, else a sampled scan), which derives the median front-matter offset between printed and physical page numbers and corrects every entry. Indexed documents scan the whole body in ~0.1s; unindexed documents sample pages, so pages may be approximate (±10 on very large books).

**For exact pages, index first:**

```bash
paperreader --file "book.pdf" --index   # one-time full-text cache (~2s per 854-page book)
paperreader --file "book.pdf" --toc     # now resolves every page exactly, ~0.05s
```

`--index` extracts every page's text once and caches it on disk (keyed by path hash; invalidated when the file changes). `--toc` uses that cache for the body-wide title search; without it, `--toc` still works but page numbers come from a sampled scan.

Some tech books (e.g. CRC/Taylor & Francis titles like *Game Engine Architecture* Vol 2) put the printed TOC's **page-number column outside the text layer** (rendered as non-text). `--toc` still extracts the clean chapter/section tree from the TOC and resolves physical pages by locating each title in the body — with `--index` the pages are exact.

The whole extraction opens the PDF **once** (page count + outline + heuristics share the same document).

JSON output: `source` is `"outlines" | "printed_toc" | "heading_scan"`; each entry has `page`, `title`, `level` (`0` = part/unit, `1` = chapter, `2` = section, `3` = subsection).

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
