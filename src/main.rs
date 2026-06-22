use std::path::{Path, PathBuf};
use std::fs;
use std::time::Instant;
use std::collections::HashSet;
use clap::{Parser, CommandFactory};

mod cli;
mod config;
mod pdf;
mod index;
mod search;
mod formatter;
mod interactive;
mod toc;

use cli::Cli;
use search::{search_txt, find_texts};
use index::{collect_index_map, read_valid_meta};
use pdf::{find_pdfs, extract_page, search_pdf};
use interactive::{run_fzf_interactive, handle_open_args, check_page_viewer};
use formatter::format_result;

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{3040}'..='\u{309F}'
        | '\u{30A0}'..='\u{30FF}'
        | '\u{AC00}'..='\u{D7AF}'
        | '\u{3000}'..='\u{303F}'
        | '\u{FF00}'..='\u{FFEF}'
    )
}

fn parse_page_range(s: &str) -> Result<Vec<usize>, String> {
    let mut pages = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() { continue; }
        if let Some((start_str, end_str)) = part.split_once('-') {
            let start: usize = start_str.trim().parse().map_err(|_| format!("invalid page: '{}'", start_str))?;
            let end: usize = end_str.trim().parse().map_err(|_| format!("invalid page: '{}'", end_str))?;
            if start == 0 || end == 0 || start > end {
                return Err(format!("invalid range: {}-{}", start, end));
            }
            pages.extend(start..=end);
        } else {
            let n: usize = part.parse().map_err(|_| format!("invalid page: '{}'", part))?;
            if n == 0 { return Err(format!("page numbers must be >= 1: {}", n)); }
            pages.push(n);
        }
    }
    pages.sort();
    pages.dedup();
    Ok(pages)
}

fn page_count(path: &Path) -> usize {
    pdf_oxide::PdfDocument::open(path).and_then(|d| d.page_count()).unwrap_or(0)
}

fn main() {
    let args_raw: Vec<String> = std::env::args().collect();
    if let Some(pos) = args_raw.iter().position(|x| x == "--_open") {
        handle_open_args(&args_raw[pos + 1..]);
        std::process::exit(0);
    }

    let args = Cli::parse();

    if args.interactive && args.json {
        eprintln!("Error: -I/--interactive and --json are mutually exclusive");
        std::process::exit(1);
    }

    let directory = Path::new(&args.dir).canonicalize().unwrap_or_else(|_| PathBuf::from(&args.dir));

    let mut pdfs = Vec::new();
    let mut txts = Vec::new();

    if let Some(ref explicit_file) = args.file {
        let file_path = Path::new(explicit_file).canonicalize().unwrap_or_else(|_| PathBuf::from(explicit_file));
        if !file_path.is_file() {
            eprintln!("Error: '{}' is not a file or does not exist", explicit_file);
            std::process::exit(1);
        }
        if file_path.extension().map(|s| s.to_ascii_lowercase()) == Some(std::ffi::OsString::from("txt")) {
            txts.push(file_path);
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
    }

    let exit_no_docs = || {
        let mut msg = format!("No PDFs found in {}", directory.display());
        if let Some(ref filter) = args.files { msg += &format!(" matching '{}'", filter); }
        println!("{}", msg);
        std::process::exit(0);
    };

    // 1. Index
    if args.index || args.reindex {
        if pdfs.is_empty() { exit_no_docs(); }
        index::build_index(&pdfs, args.reindex);
        return;
    }

    // 2. List
    if args.list {
        if pdfs.is_empty() { exit_no_docs(); }
        if args.json {
            let mut files_json = Vec::new();
            let mut total_mb = 0.0;
            for p in &pdfs {
                let size_mb = fs::metadata(p).map(|m| m.len()).unwrap_or(0) as f64 / (1024.0 * 1024.0);
                total_mb += size_mb;
                let pages = page_count(p);
                let (indexed_pages, indexed_words) = if let Some(meta) = read_valid_meta(p) {
                    (meta.indexed_pages, meta.indexed_words)
                } else { (0, 0) };
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
            let output = serde_json::json!({"total": pdfs.len(), "total_size_mb": (total_mb * 10.0).round() / 10.0, "files": files_json});
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        } else {
            println!("\n  Found {} document(s):\n", pdfs.len());
            for p in &pdfs {
                let size_mb = fs::metadata(p).map(|m| m.len()).unwrap_or(0) as f64 / (1024.0 * 1024.0);
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                let pages = page_count(p);
                let index_info = if let Some(meta) = read_valid_meta(p) {
                    if meta.indexed_pages > 0 {
                        let pct = if pages > 0 { (meta.indexed_pages as f64 / pages as f64 * 100.0) as usize } else { 0 };
                        format!("  Indexed: {}/{} pages ({}%)  {} words", meta.indexed_pages, pages, pct, meta.indexed_words)
                    } else { format!("  Indexed: 0/{} pages ⚠", pages) }
                } else { String::new() };
                println!("  \x1b[1m{}\x1b[0m", name);
                println!("    Pages: {}  Size: {:.1} MB", pages, size_mb);
                if !index_info.is_empty() { println!("    {}", index_info); }
                println!();
            }
        }
        return;
    }

    // 2.5 TOC
    if args.toc {
        if pdfs.is_empty() { exit_no_docs(); }
        let target = &pdfs[0];
        let total_pages = page_count(target);
        let _toc_start = Instant::now();
        eprintln!("Scanning up to {} pages (~{} sampled) for TOC patterns...", total_pages, (total_pages / 25).max(5) + 10);
        let entries = toc::detect_toc(target, total_pages);
        eprintln!("TOC scan completed in {:.1}s", _toc_start.elapsed().as_secs_f64());
        if args.json {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({"file": target.to_string_lossy(), "entries": entries})).unwrap());
        } else if entries.is_empty() {
            println!("No TOC entries detected in {} pages.", total_pages);
        } else {
            println!("\n  Table of Contents ({} entries):\n", entries.len());
            for entry in &entries {
                let indent = "  ".repeat(entry.level.min(3));
                println!("{}{}  (p{})", indent, entry.title, entry.page);
            }
        }
        return;
    }

    // 2.6 CJK check
    if args.check {
        if pdfs.is_empty() { exit_no_docs(); }
        let target = &pdfs[0];
        let total_pages = page_count(target);
        if total_pages == 0 { println!("No pages found in {}", target.display()); return; }
        let sample_pages = [(total_pages / 4).max(1), (total_pages / 2).max(1), (total_pages * 3 / 4).max(1)];
        let mut results = Vec::new();
        for &page_num in &sample_pages {
            let text = extract_page(target, page_num).unwrap_or_default();
            let total_chars = text.chars().count();
            let cjk_chars = text.chars().filter(|&c| is_cjk(c)).count();
            let ratio = if total_chars > 0 { cjk_chars as f64 / total_chars as f64 * 100.0 } else { 0.0 };
            results.push(serde_json::json!({"page": page_num, "total_chars": total_chars, "cjk_chars": cjk_chars, "cjk_ratio_pct": (ratio * 10.0).round() / 10.0}));
        }
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({"file": target.to_string_lossy(), "total_pages": total_pages, "samples": results})).unwrap());
        let any_cjk = results.iter().any(|r| r["cjk_chars"].as_u64().unwrap_or(0) > 0);
        if any_cjk { println!("\n  ✅ CJK characters detected."); }
        else { println!("\n  ℹ No CJK characters detected."); }
        return;
    }

    // 3. Extract pages
    let has_extract_range = args.extract_range.is_some();
    let has_extract_page = args.extract_page.is_some();
    let has_head = args.head.is_some();

    if has_extract_page || has_extract_range || has_head {
        if pdfs.is_empty() { exit_no_docs(); }
        let target = &pdfs[0];

        if let Some(head_opt) = args.head {
            let n_lines = head_opt.unwrap_or(1);
            let total = page_count(target);
            for page_num in 1..=total {
                let text = extract_page(target, page_num).unwrap_or_default();
                let head: String = text.lines().take(n_lines).collect::<Vec<_>>().join("\n");
                if !head.is_empty() {
                    let suffix = if n_lines > 1 || text.lines().count() > n_lines { "…" } else { "" };
                    println!("--- page {} ---\n{}{}", page_num, head, suffix);
                }
            }
            return;
        }

        let pages: Vec<usize> = if let Some(ref range_str) = args.extract_range {
            match parse_page_range(range_str) {
                Ok(p) => p,
                Err(e) => { eprintln!("Error: invalid --extract-range '{}': {}", range_str, e); std::process::exit(1); }
            }
        } else if let Some(p) = args.extract_page { vec![p] }
        else { Vec::new() };

        for &page_num in &pages {
            match extract_page(target, page_num) {
                Some(text) => {
                    if pages.len() > 1 { println!("--- page {} ---\n{}", page_num, text); }
                    else { println!("{}", text); }
                }
                None => eprintln!("Page {} not found in {}", page_num, target.file_name().unwrap_or_default().to_string_lossy()),
            }
        }
        return;
    }

    let query = match args.query {
        Some(q) => q,
        None => { let _ = Cli::command().print_help(); std::process::exit(1); }
    };

    let case_sensitive = args.case_sensitive;
    let start_time = Instant::now();

    // 4. Search
    let mut all_results = Vec::new();
    let all_docs = pdfs.clone();
    let index_map = if args.no_index { std::collections::HashMap::new() } else { collect_index_map(&all_docs) };
    let used_index = !index_map.is_empty();

    let unindexed_docs = if used_index {
        all_results.extend(search::search_via_ripgrep(&query, &index_map, args.regex, case_sensitive, args.context));
        let indexed_paths: HashSet<PathBuf> = index_map.values()
            .map(|m| PathBuf::from(&m.path).canonicalize().unwrap_or_else(|_| PathBuf::from(&m.path)))
            .collect();
        all_docs.into_iter().filter(|p| !indexed_paths.contains(&p.canonicalize().unwrap_or_else(|_| p.clone()))).collect()
    } else { all_docs };

    if !used_index && !unindexed_docs.is_empty() {
        let total: usize = unindexed_docs.iter().map(|p| page_count(p)).sum();
        eprintln!("Searching {} file(s) directly ({} pages). Estimated: {}–{} seconds.",
            unindexed_docs.len(), total, ((total as f64 * 0.02) as u32).max(1), ((total as f64 * 0.05) as u32).max(2));
    }

    // Parallelize across PDFs — each PDF gets its own PdfDocument, no I/O contention
    use rayon::prelude::*;
    let pdf_results: Vec<Vec<search::SearchResult>> = unindexed_docs
        .par_iter()
        .enumerate()
        .map(|(i, doc)| {
            let label = format!("{}  [{}/{}]", doc.file_name().unwrap_or_default().to_string_lossy(), i + 1, unindexed_docs.len());
            search_pdf(doc, &query, args.regex, args.context, case_sensitive, &label)
        })
        .collect();
    for r in pdf_results {
        all_results.extend(r);
    }

    for txt in &txts {
        all_results.extend(search_txt(txt, &query, args.regex, case_sensitive, args.context));
    }

    let elapsed = start_time.elapsed();
    let has_matches = all_results.iter().any(|r| r.error.is_none());

    // 5. Output
    if args.interactive {
        run_fzf_interactive(&all_results);
        if !check_page_viewer() && has_matches { eprintln!("  Install Skim for page-accurate PDF opening: brew install --cask skim"); }
        return;
    }

    let index_stats = if used_index {
        Some(serde_json::json!({
            "documents": index_map.len(),
            "total_pages": index_map.values().map(|m| m.pages).sum::<usize>(),
            "indexed_pages": index_map.values().map(|m| m.indexed_pages).sum::<usize>(),
            "total_words": index_map.values().map(|m| m.indexed_words).sum::<usize>(),
        }))
    } else { None };

    if args.json {
        let match_count = all_results.iter().filter(|r| r.error.is_none()).count();
        let mut output = serde_json::json!({"query": query, "elapsed_ms": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0, "indexed": used_index, "total_matches": match_count, "matches": all_results});
        if let Some(ref stats) = index_stats { output["index_stats"] = stats.clone(); }
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    } else {
        let show_context = args.context > 0;
        for r in &all_results { print!("{}", format_result(r, show_context)); }
        let match_count = all_results.iter().filter(|r| r.error.is_none()).count();
        let error_count = all_results.iter().filter(|r| r.error.is_some()).count();
        let files_with_matches: HashSet<&String> = all_results.iter().filter(|r| r.error.is_none()).map(|r| &r.file).collect();
        println!("\n  ──────────────────────────────");
        println!("  {} match(es) across {} file(s)", match_count, files_with_matches.len());
        if error_count > 0 { println!("  {} file(s) with errors", error_count); }
        if match_count == 0 {
            if let Some(ref stats) = index_stats {
                let total = stats["total_pages"].as_u64().unwrap_or(0);
                let indexed = stats["indexed_pages"].as_u64().unwrap_or(0);
                println!("  Index: {} of {} pages indexed, {} words", indexed, total, stats["total_words"].as_u64().unwrap_or(0));
                if indexed < total { println!("  ⚠ {} pages could not be indexed. Try --no-index or OCR.", total - indexed); }
                else if indexed > 0 { println!("  Index looks healthy — the term may not appear in these documents."); }
            }
        }
        println!("  Searched {} PDF(s){} in {:.2}s", pdfs.len(), if !txts.is_empty() { format!(" + {} txt file(s)", txts.len()) } else { String::new() }, elapsed.as_secs_f64());
        if !used_index && !pdfs.is_empty() { println!("  Tip: run 'paperreader --index' for near-instant searches"); }
        if !check_page_viewer() && has_matches { println!("  Tip: install Skim for page-accurate opening: brew install --cask skim"); }
    }
}
