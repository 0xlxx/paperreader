use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::io::Write;

use crate::search::SearchResult;

/// 辅助检查特定的系统 CLI 命令（例如 fzf, sioyek）是否在 PATH 中可用
fn is_binary_available(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 检查 macOS 系统中是否安装了 Skim 电子书/PDF 阅读器
fn has_skim() -> bool {
    Path::new("/Applications/Skim.app").exists()
        || dirs::home_dir()
            .map(|h| h.join("Applications/Skim.app").exists())
            .unwrap_or(false)
}

/// 返回当前环境是否具备精确跳转页码的 PDF 阅读器（Skim 或 Sioyek）
pub fn check_page_viewer() -> bool {
    has_skim() || is_binary_available("sioyek")
}

/// 在 Skim 中使用 AppleScript 精确跳转并展示特定的 PDF 页码
fn open_pdf_in_skim(path: &Path, page: usize) -> bool {
    let script = format!(
        "tell application \"Skim\"\n\
         activate\n\
         open POSIX file \"{}\"\n\
         delay 0.3\n\
         tell document 1\n\
             set current page to page {}\n\
         end tell\n\
         end tell",
        path.display(),
        page
    );

    Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 核心逻辑：分派不同类型的文件至最合适的系统/第三方阅读器，并执行页面/行号精确定位
pub fn open_result(filepath: &str, page: usize, line_num: usize) -> bool {
    let path = Path::new(filepath);
    if !path.exists() {
        eprintln!("Error: file not found: {}", filepath);
        return false;
    }

    let ext = path.extension().map(|s| s.to_ascii_lowercase());

    if ext == Some(std::ffi::OsString::from("pdf")) {
        // PDF 精确页码跳转：首选 Skim，次选 Sioyek，最后使用系统默认预览
        if has_skim() && open_pdf_in_skim(path, page) {
            return true;
        }
        if is_binary_available("sioyek") {
            let status = Command::new("sioyek")
                .arg(path)
                .arg("--page")
                .arg(page.to_string())
                .status();
            if status.map(|s| s.success()).unwrap_or(false) {
                return true;
            }
        }
        let _ = Command::new("open").arg(path).status();
        false
    } else if ext == Some(std::ffi::OsString::from("epub")) {
        // EPUB：直接调用系统默认应用打开（例如 Apple Books）
        let _ = Command::new("open").arg(path).status();
        false
    } else if ext == Some(std::ffi::OsString::from("txt")) {
        // TXT 精确行号跳转：首选 VS Code (code -g)，次选 Sublime Text (subl)，最后用系统 open
        if line_num > 0 && is_binary_available("code") {
            let _ = Command::new("code")
                .arg("-g")
                .arg(format!("{}:{}", path.display(), line_num))
                .status();
            return true;
        }
        if line_num > 0 && is_binary_available("subl") {
            let _ = Command::new("subl")
                .arg(format!("{}:{}", path.display(), line_num))
                .status();
            return true;
        }
        let _ = Command::new("open").arg(path).status();
        false
    } else {
        let _ = Command::new("open").arg(path).status();
        false
    }
}

/// 处理隐藏的 --_open 操作调用接口，供 fzf 等外部子进程定位唤起
pub fn handle_open_args(args: &[String]) {
    if args.len() >= 2 {
        let filepath = &args[0];
        let page = args[1].parse::<usize>().unwrap_or(1);
        let line_num = args.get(2).and_then(|l| l.parse::<usize>().ok()).unwrap_or(0);
        open_result(filepath, page, line_num);
    }
}

/// 唤起 fzf 交互终端展示搜索匹配结果，支持键盘快捷键交互和二次定位
pub fn run_fzf_interactive(results: &[SearchResult]) {
    if !is_binary_available("fzf") {
        eprintln!("fzf not found. Please install it: brew install fzf");
        return;
    }

    let valid: Vec<&SearchResult> = results.iter().filter(|r| r.error.is_none()).collect();
    if valid.is_empty() {
        eprintln!("No matches to open.");
        return;
    }

    let mut fzf_input = String::new();
    for r in &valid {
        let name = &r.filename;
        let loc = if name.to_lowercase().ends_with(".txt") {
            format!("L{}", r.line_num)
        } else if name.to_lowercase().ends_with(".epub") {
            format!("ch{}/{}", r.page, r.total_pages)
        } else {
            format!("p{}/{}", r.page, r.total_pages)
        };

        let display_line: String = r.line.chars().take(120).collect();
        let display = format!("{}  {}  ▶  {}", name, loc, display_line);
        // 字段以 \t 分隔：display \t filepath \t page \t line_num
        fzf_input.push_str(&format!("{}\t{}\t{}\t{}\n", display, r.file, r.page, r.line_num));
    }

    let self_path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("paperreader"));
    let open_cmd = format!(
        "\"{}\" --_open '{{2}}' '{{3}}' '{{4}}'",
        self_path.to_string_lossy()
    );

    let fzf_args = [
        "--delimiter=\t",
        "--with-nth=1",
        "--bind", "f1:accept,f2:accept,f3:accept,f4:accept,f5:accept,f6:accept,f7:accept,f8:accept,f9:accept",
        "--bind", &format!("ctrl-o:execute-silent({})", open_cmd),
        "--bind", &format!("double-click:execute-silent({})+accept", open_cmd),
        "--multi",
        "--header", "Enter=open+quit  Ctrl-O=open+stay  F1-F9=jump  Tab=multi  DblClick=open  Esc=quit",
        "--header-first",
    ];

    let mut child = match Command::new("fzf")
        .args(&fzf_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to spawn fzf: {}", e);
                return;
            }
        };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(fzf_input.as_bytes());
    }

    let output = match child.wait_with_output() {
        Ok(out) => out,
        Err(e) => {
            eprintln!("Failed to read fzf output: {}", e);
            return;
        }
    };

    if output.status.code() != Some(0) {
        return;
    }

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    for line in stdout_str.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            let filepath = parts[1];
            let page = parts[2].parse::<usize>().unwrap_or(1);
            let line_num = parts.get(3).and_then(|l| l.parse::<usize>().ok()).unwrap_or(0);

            let name = Path::new(filepath).file_name().unwrap_or_default().to_string_lossy();
            if name.to_lowercase().ends_with(".txt") {
                println!("\n  → Opening {} at line {}...", name, line_num);
            } else if name.to_lowercase().ends_with(".epub") {
                println!("\n  → Opening {} at chapter {}...", name, page);
            } else {
                println!("\n  → Opening {} at page {}...", name, page);
            }
            open_result(filepath, page, line_num);
        }
    }
}
