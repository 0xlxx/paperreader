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

use cli::Cli;
use search::{search_txt, find_texts};
use index::collect_index_map;
use pdf::{find_pdfs, extract_page, search_pdf};
use epub::{find_epubs, search_epub};
use interactive::{run_fzf_interactive, handle_open_args, check_page_viewer};
use formatter::format_result;

/// 导出 EPUB 的指定章节（chapter_num 为 1-indexed）并清洗 HTML 格式
fn extract_epub_chapter(path: &Path, chapter_num: usize) -> Option<String> {
    let mut doc = ::epub::doc::EpubDoc::new(path).ok()?;
    if chapter_num < 1 || chapter_num > doc.spine.len() {
        return None;
    }
    doc.set_current_chapter(chapter_num - 1);
    let (content, _) = doc.get_current_str()?;
    Some(crate::epub::html_to_text(&content))
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
        if pdfs.is_empty() && epubs.is_empty() {
            exit_no_docs();
        }
        index::build_index(&directory, args.reindex);
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

                files_json.push(serde_json::json!({
                    "filename": p.file_name().unwrap_or_default().to_string_lossy(),
                    "path": p.to_string_lossy(),
                    "pages": pages,
                    "size_mb": (size_mb * 10.0).round() / 10.0,
                }));
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
                println!("  \x1b[1m{}\x1b[0m", name);
                println!("    Pages: {}  Size: {:.1} MB\n", pages, size_mb);
            }
        }
        return;
    }

    // 3. 提取指定页面的全文（Agent 核心功能，用于阅读具体章节）
    if let Some(page_num) = args.extract_page {
        let mut all_docs = pdfs.clone();
        all_docs.extend(epubs.clone());
        if all_docs.is_empty() {
            exit_no_docs();
        }

        let target_doc = &all_docs[0];
        let ext = target_doc.extension().map(|s| s.to_ascii_lowercase());
        let text_opt = if ext == Some(std::ffi::OsString::from("epub")) {
            extract_epub_chapter(target_doc, page_num)
        } else {
            extract_page(target_doc, page_num)
        };

        match text_opt {
            Some(text) => {
                println!("{}", text);
            }
            None => {
                eprintln!("Page/Chapter {} not found in {}", page_num, target_doc.file_name().unwrap_or_default().to_string_lossy());
                std::process::exit(1);
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

    if args.json {
        let match_count = all_results.iter().filter(|r| r.error.is_none()).count();
        let output = serde_json::json!({
            "query": query,
            "elapsed_ms": (elapsed.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
            "indexed": used_index,
            "total_matches": match_count,
            "matches": all_results,
        });
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
