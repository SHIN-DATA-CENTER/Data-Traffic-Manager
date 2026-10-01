//! Small helpers for persisting JSON files safely.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Name of the folder that holds all application data.
const APP_DIR_NAME: &str = "Data-Traffic-Manager";

/// Directory where settings and usage history are stored.
///
/// `%LOCALAPPDATA%\Data-Traffic-Manager` on Windows. It can be overridden
/// with the `DTM_DATA_DIR` environment variable (portable use, testing).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("DTM_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::data_local_dir().or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir).join(APP_DIR_NAME)
}

/// Reads a JSON file. A missing file yields `Ok(None)`.
///
/// A file that cannot be parsed is renamed to `<name>.broken` so the data
/// is not silently overwritten, and `Ok(None)` is returned.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    match serde_json::from_str(&text) {
        Ok(value) => Ok(Some(value)),
        Err(err) => {
            let backup = with_suffix(path, ".broken");
            let _ = fs::rename(path, &backup);
            eprintln!("{} を読み込めませんでした ({err})。{} に退避しました。", path.display(), backup.display());
            Ok(None)
        }
    }
}

/// Writes `value` as JSON by writing a temporary file first and renaming it
/// over the destination, so a crash never leaves a truncated file behind.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    let tmp = with_suffix(path, ".tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A unique, empty temporary directory removed on drop.
    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("dtm-test-{tag}-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn roundtrip_and_broken_file() {
        let dir = TempDir::new("storage");
        let path = dir.0.join("sub").join("value.json");
        assert_eq!(read_json::<Vec<u32>>(&path).unwrap(), None);

        write_json_atomic(&path, &vec![1u32, 2, 3]).unwrap();
        assert_eq!(read_json::<Vec<u32>>(&path).unwrap(), Some(vec![1, 2, 3]));
        write_json_atomic(&path, &vec![4u32]).unwrap();
        assert_eq!(read_json::<Vec<u32>>(&path).unwrap(), Some(vec![4]));

        fs::write(&path, "{ not json").unwrap();
        assert_eq!(read_json::<Vec<u32>>(&path).unwrap(), None);
        assert!(!path.exists());
        assert!(dir.0.join("sub").join("value.json.broken").exists());
    }
}
