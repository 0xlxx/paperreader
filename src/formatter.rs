use crate::search::SearchResult;

/// 格式化单条搜索匹配记录并添加终端 ANSI 高亮色彩
/// 核心逻辑：使用 Vec<char> 做 Unicode 安全的字串切片，完全避免直接用字节索引截取汉字时的崩溃
pub fn format_result(result: &SearchResult, show_context: bool) -> String {
    if let Some(ref err) = result.error {
        return format!("  ⚠ {}\n", err);
    }

    let file_label = format!(
        "{}  p{}/{}  L{}",
        result.filename, result.page, result.total_pages, result.line_num
    );
    let line = &result.line;

    let line_chars: Vec<char> = line.chars().collect();
    let m_start = std::cmp::min(result.match_start, line_chars.len());
    let m_end = std::cmp::min(result.match_end, line_chars.len());

    let before_match: String = line_chars[..m_start].iter().collect();
    let matched: String = line_chars[m_start..m_end].iter().collect();
    let after_match: String = line_chars[m_end..].iter().collect();

    // 黄色粗体高亮匹配文字，青色粗体高亮文件和位置标签
    let highlighted = format!("{}{}{}{}{}", before_match, "\x1b[1;33m", matched, "\x1b[0m", after_match);
    let mut out = format!("\n  \x1b[1;36m{}\x1b[0m\n", file_label);

    if show_context && (!result.context_before.is_empty() || !result.context_after.is_empty()) {
        for ctx_line in &result.context_before {
            out += &format!("    {}\n", ctx_line);
        }
        out += &format!("  ▶ {}\n", highlighted);
        for ctx_line in &result.context_after {
            out += &format!("    {}\n", ctx_line);
        }
    } else {
        out += &format!("  ▶ {}\n", highlighted);
    }

    out
}
