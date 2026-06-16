---
name: paperreader
description: Search and read scientific documents (PDFs, EPUBs, TXTs) locally using the paperreader CLI. Use when you need to research literature, extract specific page text, or query key concepts across text indices.
---

# Paperreader CLI

`paperreader` is a high-performance, single-binary Rust CLI tool tailored for LLM agents to search, catalog, and read local academic papers (PDF), books (EPUB), and notes (TXT). It outputs clean, structured JSON, preserves exact page/chapter divisions, and resolves CJK (Chinese) text extraction natively.

## Quick Start

Run the commands directly from your terminal workspace:

```bash
# 1. Build or refresh the text index of the current directory (rayon-accelerated)
paperreader --index

# 2. Search for a keyword (instant results from the index)
paperreader "鸦片战争"

# 3. Search for a regex pattern
paperreader "秦.*统一" -r

# 4. Search and output as structured JSON for programming/scripting
paperreader "neural networks" --json

# 5. Extract a specific page of a PDF for reading/digesting
paperreader --file "/path/to/paper.pdf" --extract-page 15

# 6. Extract a specific chapter of an EPUB book
paperreader --file "/path/to/book.epub" --extract-page 3
```

---

## Workflows & Best Practices for Agents

### 1. JSON First, No Guesswork
When scripting or parsing matches, always use the `--json` flag. The CLI outputs raw JSON to `stdout` and status logs/progress to `stderr`.
- The stdout JSON structure:
  ```json
  {
    "query": "search query",
    "elapsed_ms": 15.4,
    "indexed": true,
    "total_matches": 12,
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
        "context_before": ["context line 1", "context line 2"],
        "context_after": ["context line 3"]
      }
    ]
  }
  ```
- Use `jq` or built-in json parsers to easily read the matches list.

### 2. Deep Reading Strategy
If you hit a match on page `N`, **DO NOT** guess the surrounding context. 
1. Use the `--extract-page N` flag specifying the file path:
   ```bash
   paperreader --file "/path/to/file.pdf" --extract-page 15
   ```
2. Feed the extracted clean plain text directly into your LLM context window to digest the full explanation, formula, or tables on that page.

### 3. Check Library Contents
To understand what materials are in a workspace folder, list them using:
```bash
paperreader --list --json
```
This returns a JSON list of all available PDFs and EPUBs in the folder, detailing their filenames, paths, size in MB, and total pages/chapters.

---

## Command Reference

- `[query]`: Positional query search string.
- `-d, --dir <DIR>`: Sets directory to search (defaults to `.`).
- `-r, --regex`: Interprets the query as a regular expression.
- `-c, --context <N>`: Requests `N` lines of context before and after match hits.
- `--case-sensitive`: Turns on case sensitivity (defaults to case-insensitive).
- `--files <FILTER>`: Filters scanned documents by filename (substring matching).
- `-f, --file <PATH>`: Direct file search (skips directory discovery).
- `--json`: Outputs results in machine-readable JSON.
- `-I, --interactive`: Starts `fzf` terminal UI. Allows navigation, preview, and double-click opening at the matched page in Skim/Sioyek.
- `--extract-page <NUM>`: Extracts page `<NUM>` (or EPUB chapter `<NUM>`) text.
- `--list`: Catalogs files and outputs metadata without searching.
- `--index`: Builds or refreshes the directory index.
- `--reindex`: Forces a full index rebuild, ignoring freshness checks.
- `--no-index`: Forces search directly on files, skipping local indices.
