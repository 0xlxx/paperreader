# paperreader

High-performance, LLM-agent-friendly local document search and reading tool for academic papers (PDF), books (EPUB), and notes (TXT).

## ▣ Installation / 安装

Install the CLI tool via Homebrew:

```bash
# 1. Tap the repository
brew tap 0xlxx/tap

# 2. Install paperreader
brew install paperreader
```

Make sure you have `ripgrep` installed (Homebrew will install it automatically). For the interactive fzf viewer mode (`-I`), you will also need `fzf` and `Skim`:

```bash
brew install fzf
brew install --cask skim
```

---

## ▣ Quick Start / 快速上手

```bash
# 1. Build or refresh the text index of the current directory (rayon-accelerated)
paperreader --index

# 2. Search for a keyword (instant results from the index)
paperreader "鸦片战争"

# 3. Search and output as structured JSON for agent consumption
paperreader "neural networks" --json

# 4. Search and show matching lines with context
paperreader "transformer" -c 2

# 5. Extract a specific page of a PDF for reading
paperreader --file "/path/to/paper.pdf" --extract-page 15
```

---

## ▣ Motivation / 动机

When LLM agents need to read local document archives, traditional PDF extraction tools are either slow or mangle layouts. `paperreader` is written in Rust to provide:
- **Instant Full-Text Queries**: Pre-extracts pages and utilizes `ripgrep` for millisecond-level results.
- **Agent Friendliness**: Structured JSON outputs (`--json`) and page-by-page extraction (`--extract-page`) for clean injection into context windows.
- **Multilingual Support**: Correctly processes multi-byte CJK (Chinese) character highlighting boundaries without terminal offset glitches.
