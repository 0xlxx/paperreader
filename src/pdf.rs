use std::path::{Path, PathBuf};
use std::fs;
use pdf_oxide::PdfDocument;
use walkdir::WalkDir;
use tempfile::tempdir;

use crate::search::{search_via_ripgrep, SearchResult};
use crate::index::DocumentMeta;

/// 递归查找指定目录下的所有 PDF 文件，支持按文件名过滤
pub fn find_pdfs(directory: &Path, name_filter: Option<&str>) -> Vec<PathBuf> {
    let mut pdfs = Vec::new();
    for entry in WalkDir::new(directory).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let path = entry.path();
            if path.extension().map(|s| s.to_ascii_lowercase()) == Some(std::ffi::OsString::from("pdf")) {
                if let Some(filter) = name_filter {
                    let filename = path.file_name().unwrap_or_default().to_string_lossy();
                    if filename.to_lowercase().contains(&filter.to_lowercase()) {
                        pdfs.push(path.to_path_buf());
                    }
                } else {
                    pdfs.push(path.to_path_buf());
                }
            }
        }
    }
    pdfs.sort();
    pdfs
}

/// 提取指定 PDF 文件的某一页文本（page_num 为 1-indexed）
pub fn extract_page(path: &Path, page_num: usize) -> Option<String> {
    let doc = PdfDocument::open(path).ok()?;
    let page_count = doc.page_count().ok()?;
    if page_num < 1 || page_num > page_count {
        return None;
    }
    // pdf_oxide 内部的 page index 是从 0 开始的
    doc.extract_text(page_num - 1).ok()
}

/// 直接搜索单个 PDF 文件
/// 核心逻辑：将每一页文字提取写入临时目录，并复用 ripgrep 引擎进行搜索
pub fn search_pdf(
    path: &Path,
    query: &str,
    is_regex: bool,
    context_lines: usize,
    case_sensitive: bool,
    label: &str,
) -> Vec<SearchResult> {
    let doc = match PdfDocument::open(path) {
        Ok(d) => d,
        Err(e) => {
            return vec![SearchResult::new_error(path.to_string_lossy().into_owned(), e.to_string())];
        }
    };

    let page_count = match doc.page_count() {
        Ok(c) => c,
        Err(e) => {
            return vec![SearchResult::new_error(path.to_string_lossy().into_owned(), e.to_string())];
        }
    };

    if page_count == 0 {
        return Vec::new();
    }

    let temp_dir = match tempdir() {
        Ok(d) => d,
        Err(e) => {
            return vec![SearchResult::new_error(path.to_string_lossy().into_owned(), e.to_string())];
        }
    };

    // 核心代码：多页文本提取并保存到临时文件
    for page_idx in 0..page_count {
        let page_num = page_idx + 1;
        if !label.is_empty() {
            eprint!("\r  {}  extracting page {}/{}", label, page_num, page_count);
        }
        let text = doc.extract_text(page_idx).unwrap_or_default();
        let page_file = temp_dir.path().join(format!("page_{:04}.txt", page_num));
        if let Err(e) = fs::write(&page_file, text) {
            eprintln!("\nWarning: failed to write temp file {:?}: {}", page_file, e);
        }
    }

    if !label.is_empty() {
        eprint!("\r\x1b[K"); // 清除终端当前的提取进度行
    }

    let mut index_map = std::collections::HashMap::new();
    let hash_dir_str = temp_dir.path().to_string_lossy().into_owned();

    let meta = DocumentMeta {
        path: path.canonicalize().unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().into_owned(),
        filename: path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        pages: page_count,
        mtime: 0.0,
        size: 0,
        indexed_pages: page_count,
        indexed_words: 0,
    };
    index_map.insert(hash_dir_str, meta);

    search_via_ripgrep(query, &index_map, is_regex, case_sensitive, context_lines)
}
