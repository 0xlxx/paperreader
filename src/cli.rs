use clap::Parser;

/// 命令行参数解析结构体
/// Tailwind and custom configurations for CLI options
#[derive(Parser, Debug)]
#[command(
    name = "paperreader",
    author = "0xlxx",
    version = "0.1.0",
    about = "High-performance full-text search for PDF, EPUB, and TXT files, tailored for LLM agents",
    after_help = "Examples:\n  \
                  paperreader --index                        # build index for instant search\n  \
                  paperreader '鸦片战争'                       # search (uses index if available)\n  \
                  paperreader '鸦片战争' -I                    # search + interactive fzf mode\n  \
                  paperreader '秦.*统一' -r                    # regex search\n  \
                  paperreader '封建' -c 2 --files '必修'       # context lines + name filter\n  \
                  paperreader --file /path/to/doc.pdf --extract-page 15  # extract page from specific file\n  \
                  paperreader --list --json                    # list PDFs as structured JSON"
)]
pub struct Cli {
    /// Search query (plain text or regex with -r)
    pub query: Option<String>,

    /// Directory to search
    #[arg(short, long, default_value = ".")]
    pub dir: String,

    /// Treat query as regex
    #[arg(short, long)]
    pub regex: bool,

    /// Context lines around match
    #[arg(short, long, default_value_t = 0)]
    pub context: usize,

    /// Case-sensitive search (defaults to case-insensitive)
    #[arg(long)]
    pub case_sensitive: bool,

    /// Filter PDF/EPUB/TXT filenames (substring match)
    #[arg(long)]
    pub files: Option<String>,

    /// Direct file path (overrides directory-based discovery)
    #[arg(short, long)]
    pub file: Option<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Interactive mode with fzf: filter, preview, and open PDFs
    #[arg(short = 'I', long)]
    pub interactive: bool,

    /// Extract full text of a page (by page number)
    #[arg(long)]
    pub extract_page: Option<usize>,

    /// Extract text from a range of pages (e.g. "3-10", "67,68,69", "1,3-5,10")
    #[arg(long)]
    pub extract_range: Option<String>,

    /// Extract only the first N lines of each page (default 1 if no value given)
    #[arg(long, num_args = 0..=1)]
    pub head: Option<Option<usize>>,

    /// List all PDFs/EPUBs with metadata, don't search
    #[arg(long)]
    pub list: bool,

    /// Build/update text index for fast search
    #[arg(long)]
    pub index: bool,

    /// Force rebuild text index from scratch
    #[arg(long)]
    pub reindex: bool,

    /// Skip index, search PDFs/EPUBs directly
    #[arg(long)]
    pub no_index: bool,

    /// Extract table of contents (scans first ~20 pages for chapter headings and dot-leader patterns)
    #[arg(long)]
    pub toc: bool,

    /// Check CJK text extraction quality by sampling random pages and reporting character coverage
    #[arg(long)]
    pub check: bool,
}
