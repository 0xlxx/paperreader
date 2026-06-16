use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::fs;
use std::process::Command;
use serde::{Serialize, Deserialize};

use crate::index::DocumentMeta;

/// 统一的搜索匹配结果结构体
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SearchResult {
    pub file: String,
    pub filename: String,
    pub page: usize,
    pub total_pages: usize,
    pub line_num: usize,
    pub line: String,
    pub r#match: String,
    pub match_start: usize,
    pub match_end: usize,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SearchResult {
    /// 构造一个读取/搜索异常结果
    pub fn new_error(file: String, error: String) -> Self {
        Self {
            file,
            filename: "".into(),
            page: 0,
            total_pages: 0,
            line_num: 0,
            line: "".into(),
            r#match: "".into(),
            match_start: 0,
            match_end: 0,
            context_before: Vec::new(),
            context_after: Vec::new(),
            error: Some(error),
        }
    }
}

/// 递归查找指定目录下的所有 TXT 文件，支持文件名过滤
pub fn find_texts(directory: &Path, name_filter: Option<&str>) -> Vec<PathBuf> {
    let mut texts = Vec::new();
    for entry in walkdir::WalkDir::new(directory).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let path = entry.path();
            if path.extension().map(|s| s.to_ascii_lowercase()) == Some(std::ffi::OsString::from("txt")) {
                if let Some(filter) = name_filter {
                    let filename = path.file_name().unwrap_or_default().to_string_lossy();
                    if filename.to_lowercase().contains(&filter.to_lowercase()) {
                        texts.push(path.to_path_buf());
                    }
                } else {
                    texts.push(path.to_path_buf());
                }
            }
        }
    }
    texts.sort();
    texts
}

/// 获取匹配行前后的上下文行
pub fn compute_context(lines: &[String], line_idx: usize, context_lines: usize) -> (Vec<String>, Vec<String>) {
    if context_lines == 0 {
        return (Vec::new(), Vec::new());
    }
    let start = if line_idx >= context_lines { line_idx - context_lines } else { 0 };
    let end = std::cmp::min(lines.len(), line_idx + context_lines + 1);

    let before: Vec<String> = lines[start..line_idx].iter().map(|s| s.trim().to_string()).collect();
    let after: Vec<String> = lines[line_idx + 1..end].iter().map(|s| s.trim().to_string()).collect();
    (before, after)
}

/// 通过 ripgrep 在已索引文件的文本片段中执行高效率全文检索
pub fn search_via_ripgrep(
    query: &str,
    index_map: &HashMap<String, DocumentMeta>,
    is_regex: bool,
    case_sensitive: bool,
    context_lines: usize,
) -> Vec<SearchResult> {
    if index_map.is_empty() {
        return Vec::new();
    }

    // 构建 ripgrep 参数
    let mut rg_args = vec![
        "--json".to_string(),
        "--with-filename".to_string(),
        "--line-number".to_string(),
        "--no-heading".to_string(),
        "-g".to_string(),
        "page_*.txt".to_string(),
    ];
    if !is_regex {
        rg_args.push("--fixed-strings".to_string());
    }
    if !case_sensitive {
        rg_args.push("--ignore-case".to_string());
    }
    rg_args.push("--".to_string());
    rg_args.push(query.to_string());

    for hash_dir_str in index_map.keys() {
        rg_args.push(hash_dir_str.clone());
    }

    let output = match Command::new("rg").args(&rg_args).output() {
        Ok(out) => out,
        Err(_) => {
            eprintln!("ripgrep (rg) not found. Please install it: brew install ripgrep");
            return Vec::new();
        }
    };

    let stdout_str = String::from_utf8_lossy(&output.stdout);

    // ripgrep 的退出码：0=发现匹配，1=没有匹配，其他=出错。若没有匹配直接返回
    if output.status.code() != Some(0) && output.status.code() != Some(1) {
        return Vec::new();
    }

    let mut results = Vec::new();

    for line_text in stdout_str.lines() {
        let entry: serde_json::Value = match serde_json::from_str(line_text) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if entry["type"] != "match" {
            continue;
        }

        let data = &entry["data"];
        let matched_page_path_str = data["path"]["text"].as_str().unwrap_or_default();
        let matched_page_path = Path::new(matched_page_path_str);
        let hash_dir = matched_page_path.parent().unwrap_or(Path::new("")).to_string_lossy().into_owned();

        let meta = match index_map.get(&hash_dir) {
            Some(m) => m,
            None => continue,
        };

        // 提取文件名中的页码（格式如 page_0042.txt）
        let stem = matched_page_path.file_stem().unwrap_or_default().to_string_lossy();
        let page_num = stem.split('_').nth(1).and_then(|num| num.parse::<usize>().ok()).unwrap_or(1);

        let line_num = data["line_number"].as_u64().unwrap_or(1) as usize;
        let line_idx = line_num - 1;

        // 读取页面文本文件做精确偏移转换与上下文提取
        let page_lines = match fs::read_to_string(matched_page_path) {
            Ok(content) => content.lines().map(|s| s.to_string()).collect::<Vec<String>>(),
            Err(_) => continue,
        };

        if line_idx >= page_lines.len() {
            continue;
        }

        let matched_line = &page_lines[line_idx];

        let submatches = data["submatches"].as_array();
        let mut char_start = 0;
        let mut char_end = 0;
        let mut match_text = String::new();

        if let Some(sub_list) = submatches {
            if !sub_list.is_empty() {
                match_text = sub_list[0]["match"]["text"].as_str().unwrap_or_default().to_string();
                let m_start_byte = sub_list[0]["start"].as_u64().unwrap_or(0) as usize;
                let m_end_byte = sub_list[0]["end"].as_u64().unwrap_or(0) as usize;

                // 核心代码：将 ripgrep 报告的字节偏移转换成 UTF-8 字符（Char）偏移，保证中文高亮渲染无位移
                let line_bytes = matched_line.as_bytes();
                char_start = if m_start_byte <= line_bytes.len() {
                    std::str::from_utf8(&line_bytes[..m_start_byte]).map(|s| s.chars().count()).unwrap_or(0)
                } else { 0 };
                char_end = if m_end_byte <= line_bytes.len() {
                    std::str::from_utf8(&line_bytes[..m_end_byte]).map(|s| s.chars().count()).unwrap_or(0)
                } else { 0 };
            }
        }

        let (context_before, context_after) = if context_lines > 0 {
            compute_context(&page_lines, line_idx, context_lines)
        } else {
            (Vec::new(), Vec::new())
        };

        results.push(SearchResult {
            file: meta.path.clone(),
            filename: meta.filename.clone(),
            page: page_num,
            total_pages: meta.pages,
            line_num: line_idx + 1,
            line: matched_line.trim().to_string(),
            r#match: match_text,
            match_start: char_start,
            match_end: char_end,
            context_before,
            context_after,
            error: None,
        });
    }

    results
}

/// 直接检索一个纯文本 TXT 文件
pub fn search_txt(
    path: &Path,
    query: &str,
    is_regex: bool,
    case_sensitive: bool,
    context_lines: usize,
) -> Vec<SearchResult> {
    let mut rg_args = vec![
        "--json".to_string(),
        "--with-filename".to_string(),
        "--line-number".to_string(),
        "--no-heading".to_string(),
    ];
    if !is_regex {
        rg_args.push("--fixed-strings".to_string());
    }
    if !case_sensitive {
        rg_args.push("--ignore-case".to_string());
    }
    rg_args.push("--".to_string());
    rg_args.push(query.to_string());
    rg_args.push(path.to_string_lossy().into_owned());

    let output = match Command::new("rg").args(&rg_args).output() {
        Ok(out) => out,
        Err(_) => {
            eprintln!("ripgrep (rg) not found. Please install it: brew install ripgrep");
            return Vec::new();
        }
    };

    if output.status.code() != Some(0) && output.status.code() != Some(1) {
        return Vec::new();
    }

    let file_lines: Vec<String> = match fs::read_to_string(path) {
        Ok(c) => c.lines().map(|s| s.to_string()).collect(),
        Err(e) => {
            return vec![SearchResult::new_error(path.to_string_lossy().into_owned(), e.to_string())];
        }
    };

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line_text in stdout_str.lines() {
        let entry: serde_json::Value = match serde_json::from_str(line_text) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if entry["type"] != "match" {
            continue;
        }

        let data = &entry["data"];
        let line_num = data["line_number"].as_u64().unwrap_or(1) as usize;
        let line_idx = line_num - 1;

        if line_idx >= file_lines.len() {
            continue;
        }

        let matched_line = &file_lines[line_idx];
        let submatches = data["submatches"].as_array();
        let mut char_start = 0;
        let mut char_end = 0;
        let mut match_text = String::new();

        if let Some(sub_list) = submatches {
            if !sub_list.is_empty() {
                match_text = sub_list[0]["match"]["text"].as_str().unwrap_or_default().to_string();
                let m_start_byte = sub_list[0]["start"].as_u64().unwrap_or(0) as usize;
                let m_end_byte = sub_list[0]["end"].as_u64().unwrap_or(0) as usize;

                let line_bytes = matched_line.as_bytes();
                char_start = if m_start_byte <= line_bytes.len() {
                    std::str::from_utf8(&line_bytes[..m_start_byte]).map(|s| s.chars().count()).unwrap_or(0)
                } else { 0 };
                char_end = if m_end_byte <= line_bytes.len() {
                    std::str::from_utf8(&line_bytes[..m_end_byte]).map(|s| s.chars().count()).unwrap_or(0)
                } else { 0 };
            }
        }

        let (context_before, context_after) = if context_lines > 0 {
            compute_context(&file_lines, line_idx, context_lines)
        } else {
            (Vec::new(), Vec::new())
        };

        results.push(SearchResult {
            file: path.to_string_lossy().into_owned(),
            filename: path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            page: 1,
            total_pages: 1,
            line_num: line_idx + 1,
            line: matched_line.trim().to_string(),
            r#match: match_text,
            match_start: char_start,
            match_end: char_end,
            context_before,
            context_after,
            error: None,
        });
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_context() {
        let lines = vec![
            "line1".to_string(),
            "line2".to_string(),
            "line3".to_string(),
            "line4".to_string(),
            "line5".to_string(),
        ];
        let (before, after) = compute_context(&lines, 2, 2);
        assert_eq!(before, vec!["line1", "line2"]);
        assert_eq!(after, vec!["line4", "line5"]);

        let (before2, after2) = compute_context(&lines, 0, 2);
        assert_eq!(before2, Vec::<String>::new());
        assert_eq!(after2, vec!["line2", "line3"]);
    }
}

