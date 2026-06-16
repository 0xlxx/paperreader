use std::path::PathBuf;

/// 获取索引存储根目录
/// Default to ~/.paperreader/index
pub fn get_index_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".paperreader")
        .join("index")
}
