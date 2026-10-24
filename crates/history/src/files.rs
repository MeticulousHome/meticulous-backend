//! The zstd-compressed JSON files the history points at.
//!
//! Shots are written by `shot_manager.py` as
//! `HISTORY_PATH/shots/<YYYY-MM-DD>/<HH:MM:SS>.shot.json.zst`
//! (`json.dumps(shot, ensure_ascii=False)` piped to `zstd -10`), and served by
//! `api/history.py`'s `ZstdHistoryHandler` either decompressed (the default)
//! or as the raw `.zst` (`?compressed`). Files written before the ESP32
//! stopped sending non-finite setpoints contain bare `Infinity`/`NaN` tokens;
//! readers patch `: Infinity` and `: NaN` to `: 0.0` before parsing or serving.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::localtime::LocalZone;

/// The level `shot_manager.py` passes to the zstd CLI (`zstd -10`).
pub const SHOT_COMPRESSION_LEVEL: i32 = 10;

fn zstd_error(path: &Path, error: impl std::fmt::Display) -> Error {
    Error::Zstd {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

/// `ZstdDecompressor().stream_reader(file).read(limit)`: the first frame's
/// content, at most `limit` bytes (all of it with `None`).
pub fn decompress_file(path: &Path, limit: Option<u64>) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut decoder = zstd::stream::read::Decoder::new(file)
        .map_err(|e| zstd_error(path, e))?
        .single_frame();
    let mut raw = Vec::new();
    let result = match limit {
        Some(limit) => (&mut decoder).take(limit).read_to_end(&mut raw),
        None => decoder.read_to_end(&mut raw),
    };
    result.map_err(|e| zstd_error(path, e))?;
    Ok(raw)
}

/// Replace the bare non-finite tokens the way `api/history.py` and
/// `search_history()` do: only `: Infinity` and `: NaN`, byte for byte.
/// Returns whether anything was patched.
pub fn patch_non_finite(raw: &mut Vec<u8>) -> bool {
    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }
    fn replace(haystack: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(haystack.len());
        let mut i = 0;
        while i < haystack.len() {
            if haystack[i..].starts_with(needle) {
                out.extend_from_slice(with);
                i += needle.len();
            } else {
                out.push(haystack[i]);
                i += 1;
            }
        }
        out
    }
    if !contains(raw, b": Infinity") && !contains(raw, b": NaN") {
        return false;
    }
    let patched = replace(raw, b": Infinity", b": 0.0");
    *raw = replace(&patched, b": NaN", b": 0.0");
    true
}

/// The body `ZstdHistoryHandler.serve_decompressed_file` writes: the
/// decompressed bytes with the non-finite tokens patched.
pub fn read_patched(path: &Path) -> Result<Vec<u8>> {
    let mut raw = decompress_file(path, None)?;
    if patch_non_finite(&mut raw) {
        tracing::warn!(
            "Patching non-finite JSON token in shot file: {}",
            path.display()
        );
    }
    Ok(raw)
}

/// A shot file parsed the way `search_history()` reads it.
///
/// PARITY: tokens the patch does not reach (`: -Infinity`, `[NaN`) parse as
/// non-finite floats in Python and are written back as bare tokens by
/// json.dumps; JSON values here hold `null` for them.
pub fn read_shot_json(path: &Path) -> Result<Value> {
    let raw = read_patched(path)?;
    let text = String::from_utf8(raw)
        .map_err(|e| Error::InvalidValue(format!("shot file is not UTF-8: {e}")))?;
    Ok(pyfmt::loads_lenient(&text)?)
}

/// Compress like the zstd CLI at `level` (one frame, with a checksum).
pub fn compress(raw: &[u8], level: i32) -> Result<Vec<u8>> {
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), level)?;
    encoder.include_checksum(true)?;
    encoder.write_all(raw)?;
    Ok(encoder.finish()?)
}

/// What `ShotManager.stop()` writes: `json.dumps(shot, ensure_ascii=False)`
/// compressed at level 10, creating the day directory. The file is written
/// in place, as `zstd -f -o` does.
pub fn write_shot_file(path: &Path, shot: &Value) -> Result<()> {
    let json = pyfmt::dumps(shot, &pyfmt::DumpOpts::unicode());
    write_compressed(path, json.as_bytes())
}

/// `mkdir -p` then `zstd -10 -f -q -o path`: how shots and debug shots are
/// written.
pub fn write_compressed(path: &Path, raw: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let compressed = compress(raw, SHOT_COMPRESSION_LEVEL)?;
    fs::write(path, compressed)?;
    Ok(())
}

/// `ShotManager._timestampToFilePaths(timestamp)`: the day folder and the
/// shot file (relative to `SHOT_PATH`) of a shot that started at `timestamp`,
/// in local time.
pub fn shot_file_paths(zone: &LocalZone, timestamp: f64) -> Result<(String, String)> {
    let start = zone
        .from_timestamp(timestamp)?
        .to_naive()
        .ok_or_else(|| Error::InvalidValue("year is out of range".into()))?;
    let folder = start.format("%Y-%m-%d").to_string();
    let file = format!("{folder}/{}.shot.json.zst", start.format("%H:%M:%S"));
    Ok((folder, file))
}

/// Python's `str.rstrip(chars)`: strips any trailing character in `chars`.
pub fn rstrip_chars<'a>(text: &'a str, chars: &str) -> &'a str {
    text.trim_end_matches(|c| chars.contains(c))
}

/// One entry of a history directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub url: String,
}

impl DirectoryEntry {
    pub fn to_json(&self) -> Value {
        pyfmt::object([
            ("name", Value::String(self.name.clone())),
            ("url", Value::String(self.url.clone())),
        ])
    }
}

fn mtime(path: &Path) -> f64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs_f64())
}

/// `ZstdHistoryHandler.list_directory`: the entries newest first by mtime.
///
/// PARITY: the name is `f.rstrip(".zst")`, which strips any trailing `.`,
/// `z`, `s` and `t` characters, not the suffix (`test.zst` lists as `te`).
pub fn list_directory(dir: &Path) -> Result<Vec<DirectoryEntry>> {
    let mut entries: Vec<(f64, String)> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        entries.push((mtime(&dir.join(&name)), name));
    }
    entries.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(entries
        .into_iter()
        .map(|(_, name)| DirectoryEntry {
            name: rstrip_chars(&name, ".zst").to_string(),
            url: name,
        })
        .collect())
}

/// `LastDebugFileHandler._find_latest_debug_file`: the most recently modified
/// `*.zst` anywhere under `debug_dir` (the first one found on a tie).
pub fn latest_debug_file(debug_dir: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, found);
            } else if path.to_string_lossy().ends_with(".zst") {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(debug_dir, &mut found);
    let mut latest: Option<(f64, PathBuf)> = None;
    for path in found {
        let modified = mtime(&path);
        if latest.as_ref().is_none_or(|(best, _)| modified > *best) {
            latest = Some((modified, path));
        }
    }
    latest.map(|(_, path)| path)
}

/// What `ZstdHistoryHandler.get` does with a request path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryFile {
    /// A directory: answer with [`list_directory`].
    Directory(PathBuf),
    /// Serve [`read_patched`] of this `.zst` file, with
    /// [`decompressed_content_type`].
    Decompressed(PathBuf),
    /// Serve the file as is (tornado's static file handling); a `.zst` is
    /// served as a download named [`compressed_download_name`].
    Raw(PathBuf),
    /// 404 `{"status": "error", "error": "history entry not found", "path": ...}`.
    NotFound,
    /// The path leaves the root.
    ///
    /// PARITY: Python resolves `os.path.abspath(join(root, path))` and only
    /// the static-file branch checks the root (403); directory listings and
    /// decompression follow `..` out of it. Rust reports every escape here.
    Forbidden,
}

/// `os.path.abspath(os.path.join(root, path))`, lexically.
fn lexical_join(root: &Path, path: &str) -> PathBuf {
    let mut result = PathBuf::new();
    for component in root.join(path).components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// Resolve a `/history/files/<path>` (or `/history/debug/<path>`) request
/// against its root the way `ZstdHistoryHandler.get` does.
pub fn resolve_history_file(root: &Path, path: &str, serve_compressed: bool) -> HistoryFile {
    let full = lexical_join(root, path);
    if !full.starts_with(lexical_join(root, "")) {
        return HistoryFile::Forbidden;
    }
    let mut compressed = full.clone().into_os_string();
    compressed.push(".zst");
    let compressed = PathBuf::from(compressed);
    if full.is_dir() {
        HistoryFile::Directory(full)
    } else if !full.is_file() {
        if compressed.is_file() {
            HistoryFile::Decompressed(compressed)
        } else {
            HistoryFile::NotFound
        }
    } else if full.to_string_lossy().ends_with(".zst") && !serve_compressed {
        HistoryFile::Decompressed(full)
    } else {
        HistoryFile::Raw(full)
    }
}

/// The `Content-Type` of a decompressed history file.
pub fn decompressed_content_type(path: &Path) -> &'static str {
    if path.to_string_lossy().ends_with(".csv.zst") {
        "text/csv"
    } else {
        "application/json"
    }
}

/// The `Content-Disposition` file name of a raw `.zst` download:
/// `<device>_<YYYY_MM_DD>_<file>` from the request path.
pub fn compressed_download_name(device_identifier: &str, path: &str) -> String {
    let date = path.split('/').next().unwrap_or_default().replace('-', "_");
    let base = path.rsplit('/').next().unwrap_or_default();
    format!("{device_identifier}_{date}_{base}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_touches_only_colon_space_tokens() {
        let mut raw =
            br#"{"a": Infinity, "b": NaN, "c": -Infinity, "d": [NaN], "e": "NaN"}"#.to_vec();
        assert!(patch_non_finite(&mut raw));
        assert_eq!(
            raw,
            br#"{"a": 0.0, "b": 0.0, "c": -Infinity, "d": [NaN], "e": "NaN"}"#.to_vec()
        );
        let mut clean = b"{\"a\": 1}".to_vec();
        assert!(!patch_non_finite(&mut clean));
    }

    #[test]
    fn rstrip_is_a_character_set() {
        assert_eq!(
            rstrip_chars("08:30:00.shot.json.zst", ".zst"),
            "08:30:00.shot.json"
        );
        assert_eq!(rstrip_chars("test.zst", ".zst"), "te");
        assert_eq!(rstrip_chars("2026-10-09", ".zst"), "2026-10-09");
    }

    #[test]
    fn download_name_uses_the_day_folder() {
        assert_eq!(
            compressed_download_name("Meticulous42", "2026-10-09/08:30:00.shot.json.zst"),
            "Meticulous42_2026_10_09_08:30:00.shot.json.zst"
        );
    }
}
