use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::fs;
use std::time::Instant;
use serde::{Serialize, Deserialize};
use sha2::{Sha256, Digest};
use rayon::prelude::*;
use pdf_oxide::PdfDocument;
use epub::doc::EpubDoc;

use crate::pdf::find_pdfs;
use crate::epub::find_epubs;

/// 缓存文档索引的元数据
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DocumentMeta {
    pub path: String,
    pub filename: String,
    pub mtime: f64,
    pub size: u64,
    pub pages: usize,
    /// Pages that actually yielded text (may be < pages for scanned/image-heavy PDFs)
    #[serde(default)]
    pub indexed_pages: usize,
    /// Total words extracted across all indexed pages
    #[serde(default)]
    pub indexed_words: usize,
}

/// 对绝对路径进行哈希，生成16位标识符作为索引的文件夹名称
pub fn hash_path(path: &Path) -> String {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(resolved.to_string_lossy().as_bytes());
    let result = hasher.finalize();
    hex::encode(result)[..16].to_string()
}

/// 根据文件路径获取对应的索引存储子目录
pub fn get_index_dir(path: &Path) -> PathBuf {
    crate::config::get_index_root().join(hash_path(path))
}

/// 读取指定索引目录下的 meta.json 文件
fn read_meta(hash_dir: &Path) -> Option<DocumentMeta> {
    let meta_path = hash_dir.join("meta.json");
    let content = fs::read_to_string(meta_path).ok()?;
    serde_json::from_str(&content).ok()
}

/// 检查并获取有效的缓存元数据。如果文件的修改时间（mtime）或大小（size）不符，视为过期并返回 None
pub fn read_valid_meta(doc_path: &Path) -> Option<DocumentMeta> {
    let hash_dir = get_index_dir(doc_path);
    let meta = read_meta(&hash_dir)?;
    let stat = fs::metadata(doc_path).ok()?;

    let mtime = stat.modified().ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);

    if (meta.mtime - mtime).abs() > 0.001 || meta.size != stat.len() {
        eprintln!("Debug: path={}, meta_mtime={}, file_mtime={}, diff={}, size_match={}", 
            doc_path.display(), meta.mtime, mtime, (meta.mtime - mtime).abs(), meta.size == stat.len());
        return None;
    }
    Some(meta)
}

/// 收集所有具有最新（Fresh）索引的文档元数据映射表
pub fn collect_index_map(docs: &[PathBuf]) -> HashMap<String, DocumentMeta> {
    let mut index_map = HashMap::new();
    for doc in docs {
        if let Some(meta) = read_valid_meta(doc) {
            let hash_dir_str = get_index_dir(doc).to_string_lossy().into_owned();
            index_map.insert(hash_dir_str, meta);
        }
    }
    index_map
}

/// 提取单个文件的文本并构建索引。支持 PDF 和 EPUB 格式
pub fn index_one_doc(doc_path: &Path) {
    let hash_dir = get_index_dir(doc_path);
    if let Err(e) = fs::create_dir_all(&hash_dir) {
        eprintln!("  Warning: failed to create index directory {:?}: {}", hash_dir, e);
        return;
    }

    let ext = doc_path.extension().map(|s| s.to_ascii_lowercase());
    let mut total_pages = 0;
    let mut indexed_pages = 0usize;
    let mut indexed_words = 0usize;

    if ext == Some(std::ffi::OsString::from("pdf")) {
        // PDF 索引构建逻辑：使用 pdf_oxide 逐页提取并写入临时 page_NNNN.txt
        let doc = match PdfDocument::open(doc_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("  Warning: failed to open PDF {}: {}", doc_path.display(), e);
                return;
            }
        };
        let page_count = match doc.page_count() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("  Warning: failed to get page count of PDF {}: {}", doc_path.display(), e);
                return;
            }
        };
        total_pages = page_count;
        for page_idx in 0..page_count {
            let page_num = page_idx + 1;
            let text = doc.extract_text(page_idx).unwrap_or_default();
            if !text.trim().is_empty() {
                indexed_pages += 1;
                indexed_words += text.split_whitespace().count();
            }
            let page_file = hash_dir.join(format!("page_{:04}.txt", page_num));
            if let Err(e) = fs::write(&page_file, text) {
                eprintln!("  Warning: failed to write page file {:?}: {}", page_file, e);
            }
        }
    } else if ext == Some(std::ffi::OsString::from("epub")) {
        // EPUB 索引构建逻辑：使用 epub-rs 按 spine 章节抽取、清洗 HTML 格式并写入
        let mut doc = match EpubDoc::new(doc_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("  Warning: failed to open EPUB {}: {}", doc_path.display(), e);
                return;
            }
        };
        let spine_len = doc.spine.len();
        let mut valid_chapters = 0;
        for i in 0..spine_len {
            doc.set_current_chapter(i);
            if let Some((content, _)) = doc.get_current_str() {
                let text = crate::epub::html_to_text(&content);
                if !text.is_empty() {
                    valid_chapters += 1;
                    indexed_words += text.split_whitespace().count();
                    let page_file = hash_dir.join(format!("page_{:04}.txt", valid_chapters));
                    if let Err(e) = fs::write(&page_file, text) {
                        eprintln!("  Warning: failed to write chapter file {:?}: {}", page_file, e);
                    }
                }
            }
        }
        total_pages = valid_chapters;
        indexed_pages = valid_chapters;
    }

    if total_pages == 0 {
        // 如果没有提取到任何页面/章节，清理掉创建的空目录并返回
        let _ = fs::remove_dir_all(&hash_dir);
        return;
    }

    // 获取并存储最新的元数据
    let stat = match fs::metadata(doc_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("  Warning: failed to get metadata of {}: {}", doc_path.display(), e);
            return;
        }
    };

    let mtime = stat.modified().ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);

    let meta = DocumentMeta {
        path: doc_path.canonicalize().unwrap_or_else(|_| doc_path.to_path_buf()).to_string_lossy().into_owned(),
        filename: doc_path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        mtime,
        size: stat.len(),
        pages: total_pages,
        indexed_pages,
        indexed_words,
    };

    let meta_file = hash_dir.join("meta.json");
    if let Ok(json_str) = serde_json::to_string(&meta) {
        if let Err(e) = fs::write(&meta_file, json_str) {
            eprintln!("  Warning: failed to write meta.json {:?}: {}", meta_file, e);
        }
    }
}

/// 对指定目录下所有未索引或过期的 PDF 和 EPUB 文档构建文本索引
pub fn build_index(directory: &Path, force: bool) -> usize {
    let mut docs = find_pdfs(directory, None);
    docs.extend(find_epubs(directory, None));

    if docs.is_empty() {
        println!("No PDFs or EPUBs found in {}", directory.display());
        return 0;
    }

    let mut to_index = Vec::new();
    for doc in &docs {
        if !force && read_valid_meta(doc).is_some() {
            continue;
        }
        to_index.push(doc.clone());
    }

    if to_index.is_empty() {
        println!("All {} document(s) already indexed. Use --reindex to force rebuild.", docs.len());
        print_index_stats(&docs);
        return 0;
    }

    // 确定线程池并发数量，如果没有多核心默认至少4核心
    let workers = rayon::current_num_threads();
    println!("Indexing {} document(s) with {} threads...", to_index.len(), workers);
    let start = Instant::now();

    // 核心代码：多线程并行索引文件（Rayon 驱动）
    to_index.par_iter().for_each(|doc| {
        index_one_doc(doc);
    });

    let elapsed = start.elapsed();
    println!("Indexed {} document(s) in {:.2}s", to_index.len(), elapsed.as_secs_f64());
    print_index_stats(&docs);
    to_index.len()
}

/// Print aggregate index statistics for a set of documents
fn print_index_stats(docs: &[PathBuf]) {
    let mut total_pages = 0usize;
    let mut total_indexed = 0usize;
    let mut total_words = 0usize;
    let mut docs_with_index = 0usize;

    for doc in docs {
        if let Some(meta) = read_valid_meta(doc) {
            docs_with_index += 1;
            total_pages += meta.pages;
            total_indexed += meta.indexed_pages;
            total_words += meta.indexed_words;
        } else {
            // Try to get page count without index for unindexed docs
            let ext = doc.extension().map(|s| s.to_ascii_lowercase());
            let pages = if ext == Some(std::ffi::OsString::from("epub")) {
                EpubDoc::new(doc).map(|d| d.spine.len()).unwrap_or(0)
            } else {
                PdfDocument::open(doc).and_then(|d| d.page_count()).unwrap_or(0)
            };
            total_pages += pages;
        }
    }

    if docs_with_index > 0 {
        println!("  {} document(s) total: {} pages, {} indexed ({}%), {} words",
            docs.len(), total_pages, total_indexed,
            if total_pages > 0 { total_indexed * 100 / total_pages } else { 0 },
            total_words);
        if total_indexed < total_pages {
            println!("  ⚠ {} page(s) could not be indexed (possible scanned images or embedded fonts).",
                total_pages - total_indexed);
            println!("    Use --no-index for direct file search, or run OCR first.");
        }
    }
}
