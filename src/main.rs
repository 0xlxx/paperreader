use std::path::{Path, PathBuf};
use std::fs;
use std::time::Instant;
use std::collections::HashSet;
use clap::{Parser, CommandFactory};

mod cli;
mod config;
mod pdf;
mod epub;
mod index;
mod search;
mod formatter;
mod interactive;
mod toc;

use cli::Cli;
use search::{search_txt, find_texts};
use index::{collect_index_map, read_valid_meta};
use pdf::{find_pdfs, extract_page, search_pdf};
use epub::{find_epubs, search_epub};
use interactive::{run_fzf_interactive, handle_open_args, check_page_viewer};
use formatter::format_result;

/// 导出 EPUB 的指定章节（chapter_num 为 1-indexed）并清洗 HTML 格式
pub(crate) fn extract_epub_chapter(path: &Path, chapter_num: usize) -> Option<String> {
    let mut doc = ::epub::doc::EpubDoc::new(path).ok()?;
    if chapter_num < 1 || chapter_num > doc.spine.len() {
        return None;
    }
    doc.set_current_chapter(chapter_num - 1);
    let (content, _) = doc.get_current_str()?;
    Some(crate::epub::html_to_text(&content))
}

/// Check if a character is in the CJK Unicode ranges
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Unified Ideographs Extension A
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
        | '\u{FF00}'..='\u{FFEF}' // Halfwidth and Fullwidth Forms
    )
}

/// Parse a page range string into a sorted, deduplicated Vec of page numbers (1-indexed).
/// Supports: "3-10" (range), "67,68,69" (list), "1,3-5,10" (mixed).
fn parse_page_range(s: &str) -> Result<Vec<usize>, String> {
    let mut pages = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((start_str, end_str)) = part.split_once('-') {
            let start: usize = start_str.trim().parse().map_err(|_| format!("invalid page: '{}'", start_str))?;
            let end: usize = end_str.trim().parse().map_err(|_| format!("invalid page: '{}'", end_str))?;
            if start == 0 || end == 0 || start > end {
                return Err(format!("invalid range: {}-{}", start, end));
            }
            pages.extend(start..=end);
        } else {
            let n: usize = part.parse().map_err(|_| format!("invalid page: '{}'", part))?;
            if n == 0 {
                return Err(format!("page numbers must be >= 1: {}", n));
            }
            pages.push(n);
        }
    }
    pages.sort();
    pages.dedup();
    Ok(pages)
}


fn main() {
    // 核心代码：优先拦截 fzf 交互唤起的 --_open 子进程命令，防止 Clap 拦截报错
    let args_raw: Vec<String> = std::env::args().collect();
    if let Some(pos) = args_raw.iter().position(|x| x == "--_open") {
        let open_args = &args_raw[pos + 1..];
        handle_open_args(open_args);
        std::process::exit(0);
    }

    let args = Cli::parse();

    if args.interactive && args.json {
        eprintln!("Error: -I/--interactive and --json are mutually exclusive");
        std::process::exit(1);
    }

    let directory = Path::new(&args.dir)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(&args.dir));

    // 解析受检索的文件集合
    let mut pdfs = Vec::new();
    let mut txts = Vec::new();
    let mut epubs = Vec::new();

    if let Some(ref explicit_file) = args.file {
        let file_path = Path::new(explicit_file)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(explicit_file));

        if !file_path.is_file() {
            eprintln!("Error: '{}' is not a file or does not exist", explicit_file);
            std::process::exit(1);
        }

        let ext = file_path.extension().map(|s| s.to_ascii_lowercase());
        if ext == Some(std::ffi::OsString::from("txt")) {
            txts.push(file_path);
        } else if ext == Some(std::ffi::OsString::from("epub")) {
            epubs.push(file_path);
        } else {
            pdfs.push(file_path);
        }
    } else {
        if !directory.is_dir() {
            eprintln!("Error: '{}' is not a directory", directory.display());
            std::process::exit(1);
        }
        pdfs = find_pdfs(&directory, args.files.as_deref());
        txts = find_texts(&directory, args.files.as_deref());
        epubs = find_epubs(&directory, args.files.as_deref());
    }

    let exit_no_docs = || {
        let mut msg = format!("No PDFs or EPUBs found in {}", directory.display());
        if let Some(ref filter) = args.files {
            msg += &format!(" matching '{}'", filter);
        }
        println!("{}", msg);
        std::process::exit(0);
    };

    // 1. 构建/重建文本索引
    if args.index || args.reindex {
        let mut all_docs = pdfs.clone();
        all_docs.extend(epubs.clone());
        if all_docs.is_empty() {
            exit_no_docs();
        }
        index::build_index(&all_docs, args.reindex);
        return;
    }

    // 2. 列出文档及基本元数据
    if args.list {
        let mut all_docs = pdfs.clone();
        all_docs.extend(epubs.clone());
        if all_docs.is_empty() {
            exit_no_docs();
        }

        if args.json {
            let mut files_json = Vec::new();
            let mut total_mb = 0.0;
            for p in &all_docs {
                let size_mb = fs::metadata(p).map(|m| m.len()).unwrap_or(0) as f64 / (1024.0 * 1024.0);
                total_mb += size_mb;

                let ext = p.extension().map(|s| s.to_ascii_lowercase());
                let mut pages = 0;
                if ext == Some(std::ffi::OsString::from("pdf")) {
                    if let Ok(doc) = pdf_oxide::PdfDocument::open(p) {
                        pages = doc.page_count().unwrap_or(0);
                    }
                } else if ext == Some(std::ffi::OsString::from("epub")) {
                    if let Ok(doc) = ::epub::doc::EpubDoc::new(p) {
                        pages = doc.spine.len();
                    }
                }

                // Include index quality info if available
                let (indexed_pages, indexed_words) = if let Some(meta) = read_valid_meta(p) {
                    (meta.indexed_pages, meta.indexed_words)
                } else {
                    (0, 0)
                };

                let mut file_entry = serde_json::json!({
                    "filename": p.file_name().unwrap_or_default().to_string_lossy(),
                    "path": p.to_string_lossy(),
                    "pages": pages,
                    "size_mb": (size_mb * 10.0).round() / 10.0,
                });
                if indexed_pages > 0 {
                    file_entry["indexed_pages"] = serde_json::json!(indexed_pages);
                    file_entry["indexed_words"] = serde_json::json!(indexed_words);
                }
                files_json.push(file_entry);
            }

            let output = serde_json::json!({
                "total": all_docs.len(),
                "total_size_mb": (total_mb * 10.0).round() / 10.0,
                "files": files_json,
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        } else {
            println!("\n  Found {} document(s):\n", all_docs.len());
            for p in &all_docs {
                let size_mb = fs::metadata(p).map(|m| m.len()).unwrap_or(0) as f64 / (1024.0 * 1024.0);
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                let ext = p.extension().map(|s| s.to_ascii_lowercase());
                let mut pages = 0;
                if ext == Some(std::ffi::OsString::from("pdf")) {
                    if let Ok(doc) = pdf_oxide::PdfDocument::open(p) {
                        pages = doc.page_count().unwrap_or(0);
                    }
                } else if ext == Some(std::ffi::OsString::from("epub")) {
                    if let Ok(doc) = ::epub::doc::EpubDoc::new(p) {
                        pages = doc.spine.len();
                    }
                }

                let index_info = if let Some(meta) = read_valid_meta(p) {
                    if meta.indexed_pages > 0 {
                        let pct = if pages > 0 { (meta.indexed_pages as f64 / pages as f64 * 100.0) as usize } else { 0 };
                        format!("  Indexed: {}/{} pages ({}%)  {} words", meta.indexed_pages, pages, pct, meta.indexed_words)
                    } else {
                        format!("  Indexed: 0/{} pages ⚠", pages)
                    }
                } else {
                    String::new()
                };

                println!("  \x1b[1m{}\x1b[0m", name);
                println!("    Pages: {}  Size: {:.1} MB", pages, size_mb);
                if !index_info.is_empty() {
                    println!("    {}", index_info);
                }
                println!();
            }
        }
        return;
    }

    // 2.5 提取目录/大纲
    if args.toc {
        let mut all_docs = pdfs.clone();
        all_docs.extend(epubs.clone());
        if all_docs.is_empty() {
            exit_no_docs();
        }
        let target_doc = &all_docs[0];
        let ext = target_doc.extension().map(|s| s.to_ascii_lowercase());
        let is_epub = ext == Some(std::ffi::OsString::from("epub"));
        let total_pages = if is_epub {
            ::epub::doc::EpubDoc::new(target_doc).map(|d| d.spine.len()).unwrap_or(0)
        } else {
            pdf_oxide::PdfDocument::open(target_doc).and_then(|d| d.page_count()).unwrap_or(0)
        };
        eprintln!("Scanning {} pages for TOC patterns...", total_pages);
        let entries = toc::detect_toc(target_doc, is_epub, total_pages);

        if args.json {
            let output = serde_json::json!({
                "file": target_doc.to_string_lossy(),
                "entries": entries,
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        } else {
            if entries.is_empty() {
                println!("No TOC entries detected in {} pages.", total_pages);
            } else {
                println!("\n  Table of Contents ({} entries):\n", entries.len());
                for entry in &entries {
                    let indent = "  ".repeat(entry.level.min(3));
                    println!("{}{}  (p{})", indent, entry.title, entry.page);
                }
            }
        }
        return;
    }

    // 2.6 CJK text quality check
    if args.check {
        let mut all_docs = pdfs.clone();
        all_docs.extend(epubs.clone());
        if all_docs.is_empty() {
            exit_no_docs();
        }
        let target_doc = &all_docs[0];
        let ext = target_doc.extension().map(|s| s.to_ascii_lowercase());
        let is_epub = ext == Some(std::ffi::OsString::from("epub"));
        let total_pages = if is_epub {
            ::epub::doc::EpubDoc::new(target_doc).map(|d| d.spine.len()).unwrap_or(0)
        } else {
            pdf_oxide::PdfDocument::open(target_doc).and_then(|d| d.page_count()).unwrap_or(0)
        };

        if total_pages == 0 {
            println!("No pages found in {}", target_doc.display());
            return;
        }

        // Sample pages at 25%, 50%, 75% of the document
        let sample_pages = [
            (total_pages / 4).max(1),
            (total_pages / 2).max(1),
            (total_pages * 3 / 4).max(1),
        ];

        let mut results = Vec::new();
        for &page_num in &sample_pages {
            let text = if is_epub {
                crate::extract_epub_chapter(target_doc, page_num).unwrap_or_default()
            } else {
                crate::pdf::extract_page(target_doc, page_num).unwrap_or_default()
            };

            let total_chars = text.chars().count();
            let cjk_chars = text.chars().filter(|&c| is_cjk(c)).count();
            let latin_chars = text.chars().filter(|&c| c.is_ascii_alphabetic()).count();
            let digit_chars = text.chars().filter(|&c| c.is_ascii_digit()).count();
            let ratio = if total_chars > 0 { cjk_chars as f64 / total_chars as f64 * 100.0 } else { 0.0 };

            results.push(serde_json::json!({
                "page": page_num,
                "total_chars": total_chars,
                "cjk_chars": cjk_chars,
                "latin_chars": latin_chars,
                "digit_chars": digit_chars,
                "cjk_ratio_pct": (ratio * 10.0).round() / 10.0,
            }));
        }

        let output = serde_json::json!({
            "file": target_doc.to_string_lossy(),
            "total_pages": total_pages,
            "samples": results,
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());

        let any_cjk = results.iter().any(|r| r["cjk_chars"].as_u64().unwrap_or(0) > 0);
        let high_cjk = results.iter().any(|r| r["cjk_ratio_pct"].as_f64().unwrap_or(0.0) > 30.0);
        if any_cjk && high_cjk {
            println!("\n  ✅ CJK characters detected with healthy coverage — text extraction looks good.");
        } else if any_cjk {
            println!("\n  ⚠ CJK characters found but at low density. The document may be mostly non-CJK or have extraction issues.");
        } else {
            println!("\n  ℹ No CJK characters detected. This appears to be a non-CJK document (or extraction is failing).");
        }

        return;
    }

    // 3. 提取页面文本（支持单页、范围、head 三种模式）
    let has_extract_range = args.extract_range.is_some();
    let has_extract_page = args.extract_page.is_some();
    let has_head = args.head.is_some();

    if has_extract_page || has_extract_range || has_head {
        let mut all_docs = pdfs.clone();
        all_docs.extend(epubs.clone());
        if all_docs.is_empty() {
            exit_no_docs();
        }
        let target_doc = &all_docs[0];
        let ext = target_doc.extension().map(|s| s.to_ascii_lowercase());
        let is_epub = ext == Some(std::ffi::OsString::from("epub"));

        // --head: extract first N lines of every page
        if let Some(head_opt) = args.head {
            let n_lines = head_opt.unwrap_or(1);
            let total = if is_epub {
                ::epub::doc::EpubDoc::new(target_doc).map(|d| d.spine.len()).unwrap_or(0)
            } else {
                pdf_oxide::PdfDocument::open(target_doc).and_then(|d| d.page_count()).unwrap_or(0)
            };
            for page_num in 1..=total {
                let text = if is_epub {
                    extract_epub_chapter(target_doc, page_num).unwrap_or_default()
                } else {
                    extract_page(target_doc, page_num).unwrap_or_default()
                };
                let head: String = text.lines().take(n_lines).collect::<Vec<_>>().join("\n");
                if !head.is_empty() {
                    let suffix = if n_lines > 1 || text.lines().count() > n_lines { "…" } else { "" };
                    println!("--- page {} ---\n{}{}", page_num, head, suffix);
                }
            }
            return;
        }

        // Determine pages to extract
        let pages: Vec<usize> = if let Some(ref range_str) = args.extract_range {
            match parse_page_range(range_str) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Error: invalid --extract-range '{}': {}", range_str, e);
                    std::process::exit(1);
                }
            }
        } else if let Some(p) = args.extract_page {
            vec![p]
        } else {
            Vec::new()
        };

        if pages.is_empty() {
            return;
        }

        for &page_num in &pages {
            let text_opt = if is_epub {
                extract_epub_chapter(target_doc, page_num)
            } else {
                extract_page(target_doc, page_num)
            };
            match text_opt {
                Some(text) => {
                    if pages.len() > 1 {
                        println!("--- page {} ---\n{}", page_num, text);
                    } else {
                        println!("{}", text);
                    }
                }
                None => {
                    eprintln!("Page/Chapter {} not found in {}", page_num, target_doc.file_name().unwrap_or_default().to_string_lossy());
                }
            }
        }
        return;
    }

    let query = match args.query {
        Some(q) => q,
        None => {
            // 没有输入查询且未执行其它命令时，打印 Clap 帮助信息
            let mut cmd = Cli::command();
            let _ = cmd.print_help();
            std::process::exit(1);
        }
    };

    let case_sensitive = args.case_sensitive;
    let start_time = Instant::now();

    // 4. 全文检索逻辑
    let mut all_results = Vec::new();
    let mut all_docs = pdfs.clone();
    all_docs.extend(epubs.clone());

    let index_map = if args.no_index {
        std::collections::HashMap::new()
    } else {
        collect_index_map(&all_docs)
    };

    let used_index = !index_map.is_empty();

    // 核心代码：优先检索 ripgrep 本地索引，对新文件（未索引文件）采用直搜补充
    let unindexed_docs = if used_index {
        all_results.extend(search::search_via_ripgrep(&query, &index_map, args.regex, case_sensitive, args.context));
        let indexed_paths: HashSet<PathBuf> = index_map.values()
            .map(|m| PathBuf::from(&m.path).canonicalize().unwrap_or_else(|_| PathBuf::from(&m.path)))
            .collect();
        all_docs.into_iter()
            .filter(|p| {
                let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
                !indexed_paths.contains(&canon)
            })
            .collect::<Vec<PathBuf>>()
    } else {
        all_docs
    };

    // --no-index time estimate
    if !used_index && !unindexed_docs.is_empty() {
        let total_pages: usize = unindexed_docs.iter()
            .map(|p| {
                let ext = p.extension().map(|s| s.to_ascii_lowercase());
                if ext == Some(std::ffi::OsString::from("epub")) {
                    ::epub::doc::EpubDoc::new(p).map(|d| d.spine.len()).unwrap_or(0)
                } else {
                    pdf_oxide::PdfDocument::open(p).and_then(|d| d.page_count()).unwrap_or(0)
                }
            })
            .sum();
        let est_lo = (total_pages as f64 * 0.02) as u32;
        let est_hi = (total_pages as f64 * 0.05) as u32;
        eprintln!("Searching {} file(s) directly ({} pages). Estimated: {}–{} seconds.",
            unindexed_docs.len(), total_pages, est_lo.max(1), est_hi.max(2));
    }

    // 对未索引的 PDF 和 EPUB 文档依次采用实时直搜提取与匹配
    for (i, doc) in unindexed_docs.iter().enumerate() {
        let label = format!("{}  [{}/{}]", doc.file_name().unwrap_or_default().to_string_lossy(), i + 1, unindexed_docs.len());
        let ext = doc.extension().map(|s| s.to_ascii_lowercase());
        if ext == Some(std::ffi::OsString::from("epub")) {
            all_results.extend(search_epub(doc, &query, args.regex, args.context, case_sensitive, &label));
        } else {
            all_results.extend(search_pdf(doc, &query, args.regex, args.context, case_sensitive, &label));
        }
    }

    // 检索所有纯文本 txt 文件
    for txt in &txts {
        all_results.extend(search_txt(txt, &query, args.regex, case_sensitive, args.context));
    }

    let elapsed = start_time.elapsed();
    let has_matches = all_results.iter().any(|r| r.error.is_none());

    // 5. 输出样式路由
    if args.interactive {
        run_fzf_interactive(&all_results);
        if !check_page_viewer() && has_matches {
            eprintln!("  Install Skim for page-accurate PDF opening: brew install --cask skim");
        }
        return;
    }

    // Compute index stats for diagnostics
    let index_stats = if used_index {
        let total_pages: usize = index_map.values().map(|m| m.pages).sum();
        let indexed_pages: usize = index_map.values().map(|m| m.indexed_pages).sum();
        let total_words: usize = index_map.values().map(|m| m.indexed_words).sum();
        Some(serde_json::json!({
            "documents": index_map.len(),
            "total_pages": total_pages,
            "indexed_pages": indexed_pages,
            "total_words": total_words,
        }))
    } else {
        None
    };

    if args.json {
        let match_count = all_results.iter().filter(|r| r.error.is_none()).count();
        let mut output = serde_json::json!({
            "query": query,
            "elapsed_ms": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
            "indexed": used_index,
            "total_matches": match_count,
            "matches": all_results,
        });
        if let Some(ref stats) = index_stats {
            output["index_stats"] = stats.clone();
        }
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    } else {
        let show_context = args.context > 0;
        for r in &all_results {
            print!("{}", format_result(r, show_context));
        }

        let match_count = all_results.iter().filter(|r| r.error.is_none()).count();
        let error_count = all_results.iter().filter(|r| r.error.is_some()).count();
        let files_with_matches: HashSet<&String> = all_results.iter()
            .filter(|r| r.error.is_none())
            .map(|r| &r.file)
            .collect();

        println!("\n  ──────────────────────────────");
        println!("  {} match(es) across {} file(s)", match_count, files_with_matches.len());
        if error_count > 0 {
            println!("  {} file(s) with errors", error_count);
        }

        // Zero-result diagnostics: show index stats to distinguish "index failed" from "content doesn't have it"
        if match_count == 0 {
            if let Some(ref stats) = index_stats {
                let total = stats["total_pages"].as_u64().unwrap_or(0);
                let indexed = stats["indexed_pages"].as_u64().unwrap_or(0);
                let words = stats["total_words"].as_u64().unwrap_or(0);
                println!("  Index: {} of {} pages indexed, {} words", indexed, total, words);
                if indexed < total {
                    println!("  ⚠ {} pages could not be indexed (scanned images or embedded fonts?)", total - indexed);
                    println!("  Try --no-index for direct file search, or run OCR first.");
                } else if indexed > 0 {
                    println!("  Index looks healthy — the term may not appear in these documents.");
                }
            }
        }

        if used_index {
            println!("  Searched index ({} PDFs/EPUBs) in {:.2}s", pdfs.len() + epubs.len(), elapsed.as_secs_f64());
        } else {
            let mut parts = Vec::new();
            if !pdfs.is_empty() {
                parts.push(format!("{} PDF(s)", pdfs.len()));
            }
            if !txts.is_empty() {
                parts.push(format!("{} txt file(s)", txts.len()));
            }
            if !epubs.is_empty() {
                parts.push(format!("{} EPUB(s)", epubs.len()));
            }
            println!("  Searched {} in {:.2}s", parts.join(" + "), elapsed.as_secs_f64());
        }

        if !used_index && (!pdfs.is_empty() || !epubs.is_empty()) {
            println!("  Tip: run 'paperreader --index' for near-instant searches");
        }
        if !check_page_viewer() && has_matches {
            println!("  Tip: install Skim for page-accurate opening: brew install --cask skim");
        }
    }
}
