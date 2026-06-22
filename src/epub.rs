use std::path::{Path, PathBuf};
use std::fs;
use epub::doc::EpubDoc;
use walkdir::WalkDir;
use tempfile::tempdir;
use regex::{Regex, RegexBuilder};
use html_escape::decode_html_entities;

use crate::search::{search_via_ripgrep, SearchResult};
use crate::index::DocumentMeta;

/// 递归查找指定目录下的所有 EPUB 文件，支持按文件名过滤
pub fn find_epubs(directory: &Path, name_filter: Option<&str>) -> Vec<PathBuf> {
    let mut epubs = Vec::new();
    for entry in WalkDir::new(directory).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let path = entry.path();
            if path.extension().map(|s| s.to_ascii_lowercase()) == Some(std::ffi::OsString::from("epub")) {
                if let Some(filter) = name_filter {
                    let filename = path.file_name().unwrap_or_default().to_string_lossy();
                    if filename.to_lowercase().contains(&filter.to_lowercase()) {
                        epubs.push(path.to_path_buf());
                    }
                } else {
                    epubs.push(path.to_path_buf());
                }
            }
        }
    }
    epubs.sort();
    epubs
}

/// 将 XHTML/HTML 格式的章节内容转换为纯文本
/// 核心逻辑：移除标签，换行符折叠，HTML实体字符还原
pub fn html_to_text(html_text: &str) -> String {
    // 1. 将 <br> 标签替换为换行符
    let re_br = RegexBuilder::new(r"<br\s*/?>")
        .case_insensitive(true)
        .build()
        .unwrap();
    let text = re_br.replace_all(html_text, "\n");

    // 2. 将块级元素（p, div, li, sections 等）的闭合/开启替换为换行符
    let re_blocks = RegexBuilder::new(r"</?(?:p|div|h[1-6]|li|tr|section|article|header|footer|table)[^>]*>")
        .case_insensitive(true)
        .build()
        .unwrap();
    let text = re_blocks.replace_all(&text, "\n");

    // 3. 去除所有其他的 HTML 标签
    let re_tags = Regex::new(r"<[^>]+>").unwrap();
    let text = re_tags.replace_all(&text, "");

    // 4. 解码 HTML 字符转义（例如 &amp; -> &, &quot; -> "）
    let decoded = decode_html_entities(&text);

    // 5. 压缩多余的空白行和连续空格
    let re_newlines = Regex::new(r"\n{3,}").unwrap();
    let text = re_newlines.replace_all(&decoded, "\n\n");

    let re_spaces = Regex::new(r"[ \t]+").unwrap();
    let text = re_spaces.replace_all(&text, " ");

    text.trim().to_string()
}

/// 搜索单个 EPUB 文件中的关键字
pub fn search_epub(
    path: &Path,
    query: &str,
    is_regex: bool,
    context_lines: usize,
    case_sensitive: bool,
    label: &str,
) -> Vec<SearchResult> {
    let mut doc = match EpubDoc::new(path) {
        Ok(d) => d,
        Err(e) => {
            return vec![SearchResult::new_error(path.to_string_lossy().into_owned(), e.to_string())];
        }
    };

    let temp_dir = match tempdir() {
        Ok(d) => d,
        Err(e) => {
            return vec![SearchResult::new_error(path.to_string_lossy().into_owned(), e.to_string())];
        }
    };

    let mut valid_chapters = 0;
    let spine_len = doc.spine.len();

    if spine_len == 0 {
        return vec![SearchResult::new_error(path.to_string_lossy().into_owned(), "No content found in EPUB".into())];
    }

    // 核心代码：遍历 EPUB 结构中的 spine 列表，依次解压提取并解析出纯文本
    for i in 0..spine_len {
        if !label.is_empty() {
            eprint!("\r  {}  extracting chapter {}/{}", label, i + 1, spine_len);
        }
        
        doc.set_current_chapter(i);
        if let Some((content, _)) = doc.get_current_str() {
            let text = html_to_text(&content);
            if !text.is_empty() {
                valid_chapters += 1;
                let page_file = temp_dir.path().join(format!("page_{:04}.txt", valid_chapters));
                if let Err(e) = fs::write(&page_file, text) {
                    eprintln!("\nWarning: failed to write EPUB temp file {:?}: {}", page_file, e);
                }
            }
        }
    }

    if !label.is_empty() {
        eprint!("\r\x1b[K"); // 清楚行
    }

    if valid_chapters == 0 {
        return Vec::new();
    }

    let mut index_map = std::collections::HashMap::new();
    let hash_dir_str = temp_dir.path().to_string_lossy().into_owned();

    let meta = DocumentMeta {
        path: path.canonicalize().unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().into_owned(),
        filename: path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        pages: valid_chapters,
        mtime: 0.0,
        size: 0,
        indexed_pages: valid_chapters,
        indexed_words: 0,
    };
    index_map.insert(hash_dir_str, meta);

    search_via_ripgrep(query, &index_map, is_regex, case_sensitive, context_lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_to_text() {
        let raw = "<p>Hello &amp; welcome!<br/>This is <b>Rust</b>.</p>";
        let text = html_to_text(raw);
        assert_eq!(text, "Hello & welcome!\nThis is Rust.");
    }
}

