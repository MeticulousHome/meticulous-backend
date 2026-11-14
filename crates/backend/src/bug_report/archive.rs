//! The report archive (`<localID>.zstd`): the draft directory as a tar,
//! compressed with zstd.
//!
//! The Python writes the tar with `tarfile` (PAX format) and compresses it
//! with `zstd -10`. The tar here is written the way CPython 3.11's `tarfile`
//! writes it, block for block: the files in `sorted(rglob("*"))` order, each
//! ustar header preceded by a pax header carrying the float `mtime`, two zero
//! blocks at the end and the archive padded to a 20-block record. It is
//! compressed in-process at level 10 with a checksum and the content size,
//! like the `zstd` command does for a file.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::py;

const BLOCK: u64 = 512;
const RECORD: u64 = 20 * BLOCK;
const COMPRESSION_LEVEL: i32 = 10;
const POSIX_MAGIC: &[u8; 8] = b"ustar\x0000";

/// Why an archive could not be written, as the Python exception would say.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct ArchiveError {
    /// `type(exc).__name__`.
    pub type_name: &'static str,
    /// `str(exc)`.
    pub message: String,
}

impl ArchiveError {
    fn io(error: &io::Error, path: &Path) -> ArchiveError {
        ArchiveError {
            type_name: py::os_error_name(error),
            message: py::os_error(error, path),
        }
    }

    fn compression(error: &io::Error) -> ArchiveError {
        ArchiveError {
            type_name: "RuntimeError",
            message: error.to_string(),
        }
    }
}

/// One `tarfile.TarInfo` as `gettarinfo` fills it for a file in the draft.
#[derive(Debug, Clone)]
pub struct MemberInfo {
    pub name: Vec<u8>,
    pub mode: u32,
    pub uid: u64,
    pub gid: u64,
    pub size: u64,
    pub mtime: f64,
    pub typeflag: u8,
    pub linkname: Vec<u8>,
    pub uname: String,
    pub gname: String,
}

struct Member {
    path: PathBuf,
    headers: Vec<u8>,
    size: u64,
}

fn padding(len: u64) -> u64 {
    (BLOCK - len % BLOCK) % BLOCK
}

/// `stn(s, length, "ascii", "replace")`: non-ASCII characters (and bytes
/// that are not UTF-8) become `?`, then the field is cut or NUL padded.
fn ascii_replace(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            out.push(if c.is_ascii() { c as u8 } else { b'?' });
        }
        out.extend(std::iter::repeat_n(b'?', chunk.invalid().len()));
    }
    out
}

fn put_str(block: &mut [u8; 512], offset: usize, length: usize, value: &[u8]) {
    let value = ascii_replace(value);
    let n = value.len().min(length);
    block[offset..offset + n].copy_from_slice(&value[..n]);
}

/// `itn(n, digits)` for a value that fits: zero-padded octal and a NUL.
fn put_octal(block: &mut [u8; 512], offset: usize, digits: usize, value: u64) {
    let text = format!("{value:0width$o}", width = digits - 1);
    block[offset..offset + digits - 1].copy_from_slice(text.as_bytes());
    block[offset + digits - 1] = 0;
}

/// `TarInfo._create_header(info, USTAR_FORMAT, "ascii", "replace")`.
#[allow(clippy::too_many_arguments)]
fn ustar_block(
    name: &[u8],
    mode: u32,
    uid: u64,
    gid: u64,
    size: u64,
    mtime: u64,
    typeflag: u8,
    linkname: &[u8],
    uname: &[u8],
    gname: &[u8],
) -> [u8; 512] {
    let mut block = [0u8; 512];
    put_str(&mut block, 0, 100, name);
    put_octal(&mut block, 100, 8, u64::from(mode & 0o7777));
    put_octal(&mut block, 108, 8, uid);
    put_octal(&mut block, 116, 8, gid);
    put_octal(&mut block, 124, 12, size);
    put_octal(&mut block, 136, 12, mtime);
    block[148..156].copy_from_slice(b"        ");
    block[156] = typeflag;
    put_str(&mut block, 157, 100, linkname);
    block[257..265].copy_from_slice(POSIX_MAGIC);
    put_str(&mut block, 265, 32, uname);
    put_str(&mut block, 297, 32, gname);
    // devmajor and devminor stay NUL for anything but device files.
    let checksum: u32 = block.iter().map(|b| u32::from(*b)).sum();
    let text = format!("{checksum:06o}\0");
    block[148..155].copy_from_slice(text.as_bytes());
    block
}

/// `_create_pax_generic_header(pax_headers, XHDTYPE, encoding)`.
fn pax_header(records: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let binary = records
        .iter()
        .any(|(_, value)| std::str::from_utf8(value).is_err());
    let mut payload = Vec::new();
    if binary {
        payload.extend_from_slice(b"21 hdrcharset=BINARY\n");
    }
    for (keyword, value) in records {
        let l = keyword.len() + value.len() + 3;
        let mut p = 0usize;
        loop {
            let n = l + p.to_string().len();
            if n == p {
                break;
            }
            p = n;
        }
        payload.extend_from_slice(p.to_string().as_bytes());
        payload.push(b' ');
        payload.extend_from_slice(keyword.as_bytes());
        payload.push(b'=');
        payload.extend_from_slice(value);
        payload.push(b'\n');
    }
    let mut out = ustar_block(
        b"././@PaxHeader",
        0,
        0,
        0,
        payload.len() as u64,
        0,
        b'x',
        b"",
        b"",
        b"",
    )
    .to_vec();
    let pad = padding(payload.len() as u64) as usize;
    out.extend_from_slice(&payload);
    out.extend(std::iter::repeat_n(0u8, pad));
    out
}

/// `TarInfo.create_pax_header(info, encoding)`: the ustar header, preceded by
/// a pax header for whatever does not fit in it (always the float mtime).
pub fn member_headers(info: &MemberInfo) -> Vec<u8> {
    let mut records: Vec<(&str, Vec<u8>)> = Vec::new();
    for (value, keyword, length) in [
        (info.name.as_slice(), "path", 100usize),
        (info.linkname.as_slice(), "linkpath", 100),
        (info.uname.as_bytes(), "uname", 32),
        (info.gname.as_bytes(), "gname", 32),
    ] {
        if !value.is_ascii() || value.len() > length {
            records.push((keyword, value.to_vec()));
        }
    }
    let mut number = |name: &'static str, value: u64, digits: u32| -> u64 {
        if value < 8u64.pow(digits - 1) {
            value
        } else {
            records.push((name, value.to_string().into_bytes()));
            0
        }
    };
    let uid = number("uid", info.uid, 8);
    let gid = number("gid", info.gid, 8);
    let size = number("size", info.size, 12);
    let rounded = info.mtime.round_ties_even();
    let mtime = if (0.0..8f64.powi(11)).contains(&rounded) {
        rounded as u64
    } else {
        0
    };
    records.push(("mtime", pyfmt::repr_f64(info.mtime).into_bytes()));
    let mut out = pax_header(&records);
    out.extend_from_slice(&ustar_block(
        &info.name,
        info.mode,
        uid,
        gid,
        size,
        mtime,
        info.typeflag,
        &info.linkname,
        info.uname.as_bytes(),
        info.gname.as_bytes(),
    ));
    out
}

/// Every path under `dir` as `Path.rglob("*")` yields it (symlinked
/// directories are not entered; unreadable directories are skipped).
fn walk(dir: &Path, relative: &mut Vec<Vec<u8>>, out: &mut Vec<Vec<Vec<u8>>>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        relative.push(entry.file_name().as_bytes().to_vec());
        out.push(relative.clone());
        let is_real_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_real_dir {
            walk(&entry.path(), relative, out);
        }
        relative.pop();
    }
}

/// The files of a draft in `sorted(draft_dir.rglob("*"))` order, as
/// `(archive name, path)`.
pub fn draft_files_sorted(draft_dir: &Path) -> Vec<(Vec<u8>, PathBuf)> {
    let mut all = Vec::new();
    walk(draft_dir, &mut Vec::new(), &mut all);
    // PurePath ordering compares the parts, not the joined string.
    all.sort();
    all.into_iter()
        .filter_map(|parts| {
            let name = parts.join(&b'/');
            let path = draft_dir.join(std::ffi::OsStr::from_bytes(&name));
            path.is_file().then_some((name, path))
        })
        .collect()
}

#[derive(Default)]
struct Names {
    users: HashMap<u32, String>,
    groups: HashMap<u32, String>,
}

impl Names {
    fn user(&mut self, uid: u32) -> String {
        self.users
            .entry(uid)
            .or_insert_with(|| {
                nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
                    .ok()
                    .flatten()
                    .map(|u| u.name)
                    .unwrap_or_default()
            })
            .clone()
    }

    fn group(&mut self, gid: u32) -> String {
        self.groups
            .entry(gid)
            .or_insert_with(|| {
                nix::unistd::Group::from_gid(nix::unistd::Gid::from_raw(gid))
                    .ok()
                    .flatten()
                    .map(|g| g.name)
                    .unwrap_or_default()
            })
            .clone()
    }
}

/// `TarFile.gettarinfo` for each file, with hard links to a file already in
/// the archive stored as links like `tarfile` does.
fn members(draft_dir: &Path) -> Result<Vec<Member>, ArchiveError> {
    let mut names = Names::default();
    let mut inodes: HashMap<(u64, u64), Vec<u8>> = HashMap::new();
    let mut out = Vec::new();
    for (name, path) in draft_files_sorted(draft_dir) {
        let meta = fs::symlink_metadata(&path).map_err(|e| ArchiveError::io(&e, &path))?;
        let (typeflag, linkname, size) = if meta.file_type().is_symlink() {
            let target = fs::read_link(&path).map_err(|e| ArchiveError::io(&e, &path))?;
            (b'2', target.as_os_str().as_bytes().to_vec(), 0)
        } else {
            let inode = (meta.ino(), meta.dev());
            match inodes.get(&inode) {
                Some(first) if meta.nlink() > 1 && *first != name => (b'1', first.clone(), 0),
                _ => {
                    if inode.0 != 0 {
                        inodes.insert(inode, name.clone());
                    }
                    (b'0', Vec::new(), meta.len())
                }
            }
        };
        let info = MemberInfo {
            name,
            mode: meta.mode(),
            uid: u64::from(meta.uid()),
            gid: u64::from(meta.gid()),
            size,
            mtime: py::mtime(&meta),
            typeflag,
            linkname,
            uname: names.user(meta.uid()),
            gname: names.group(meta.gid()),
        };
        out.push(Member {
            path,
            headers: member_headers(&info),
            size,
        });
    }
    Ok(out)
}

/// `TarFile.add` of each member, then `TarFile.close()`: two zero blocks
/// and zeros up to a whole record.
fn write_tar<W: Write>(members: &[Member], sink: &mut W) -> Result<(), ArchiveError> {
    let zeros = [0u8; RECORD as usize];
    let mut offset: u64 = 0;
    let write = |sink: &mut W, bytes: &[u8]| {
        sink.write_all(bytes)
            .map_err(|e| ArchiveError::compression(&e))
    };
    for member in members {
        write(sink, &member.headers)?;
        offset += member.headers.len() as u64;
        if member.size > 0 {
            let file = File::open(&member.path).map_err(|e| ArchiveError::io(&e, &member.path))?;
            let mut limited = file.take(member.size);
            let copied =
                io::copy(&mut limited, sink).map_err(|e| ArchiveError::io(&e, &member.path))?;
            if copied < member.size {
                return Err(ArchiveError {
                    type_name: "OSError",
                    message: "unexpected end of data".into(),
                });
            }
            let pad = padding(member.size) as usize;
            write(sink, &zeros[..pad])?;
            offset += member.size + pad as u64;
        }
    }
    write(sink, &zeros[..2 * BLOCK as usize])?;
    offset += 2 * BLOCK;
    let remainder = offset % RECORD;
    if remainder > 0 {
        write(sink, &zeros[..(RECORD - remainder) as usize])?;
    }
    Ok(())
}

fn tar_size(members: &[Member]) -> u64 {
    let body: u64 = members
        .iter()
        .map(|m| m.headers.len() as u64 + m.size + padding(m.size))
        .sum::<u64>()
        + 2 * BLOCK;
    body.div_ceil(RECORD) * RECORD
}

/// The archive of `draft_dir` compressed into `sink`, like
/// `zstd -10 -f -q -o <sink> draft.tar`.
pub fn write_archive<W: Write>(draft_dir: &Path, sink: W) -> Result<W, ArchiveError> {
    let members = members(draft_dir)?;
    let mut encoder = zstd::stream::write::Encoder::new(sink, COMPRESSION_LEVEL)
        .map_err(|e| ArchiveError::compression(&e))?;
    encoder
        .include_checksum(true)
        .and_then(|()| encoder.include_contentsize(true))
        .and_then(|()| encoder.set_pledged_src_size(Some(tar_size(&members))))
        .map_err(|e| ArchiveError::compression(&e))?;
    write_tar(&members, &mut encoder)?;
    encoder.finish().map_err(|e| ArchiveError::compression(&e))
}

fn fsync_path(path: &Path) -> Result<(), ArchiveError> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| ArchiveError::io(&e, path))
}

/// `_write_tar_zstd_from_draft(output_path, draft_dir)`: written to
/// `<output>.tmp`, synced, renamed over `output_path`, the directory synced;
/// the temporary file never outlives a failure.
pub fn write_tar_zstd_from_draft(output_path: &Path, draft_dir: &Path) -> Result<(), ArchiveError> {
    let parent = output_path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| ArchiveError::io(&e, parent))?;
    let mut tmp_name = output_path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp_path = output_path.with_file_name(tmp_name);
    let result = (|| {
        let file = File::create(&tmp_path).map_err(|e| ArchiveError::io(&e, &tmp_path))?;
        let file = write_archive(draft_dir, io::BufWriter::new(file))?;
        file.into_inner()
            .map_err(|e| ArchiveError::compression(e.error()))?
            .sync_all()
            .map_err(|e| ArchiveError::io(&e, &tmp_path))?;
        fsync_path(&tmp_path)?;
        fs::rename(&tmp_path, output_path).map_err(|e| ArchiveError::io(&e, output_path))?;
        fsync_path(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

/// The archive of a draft, in memory (GET of a draft still being edited).
pub fn archive_bytes(draft_dir: &Path) -> Result<Vec<u8>, ArchiveError> {
    write_archive(draft_dir, Vec::new())
}

/// `_read_tar_zstd`: the report info, the other files by archive name, and
/// the directory they were extracted to.
#[cfg(test)]
pub fn read_tar_zstd(
    archive_path: &Path,
) -> (
    serde_json::Value,
    std::collections::BTreeMap<String, PathBuf>,
    tempfile::TempDir,
) {
    let compressed = fs::read(archive_path).expect("archive");
    let tar = zstd::stream::decode_all(compressed.as_slice()).expect("zstd stream");
    let dir = tempfile::tempdir().expect("temporary directory");
    let mut archive = tar::Archive::new(tar.as_slice());
    for entry in archive.entries().expect("tar entries") {
        let mut entry = entry.expect("tar entry");
        let name = entry.path().expect("member name").into_owned();
        assert!(
            name.components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "Unsafe archive member: {}",
            name.display()
        );
        entry.unpack_in(dir.path()).expect("unpack");
    }
    let info_path = dir.path().join(super::REPORT_INFO_NAME);
    let report_info = serde_json::from_slice(&fs::read(info_path).expect("report info"))
        .expect("report info JSON");
    let files = draft_files_sorted(dir.path())
        .into_iter()
        .filter(|(_, path)| path.file_name() != Some(super::REPORT_INFO_NAME.as_ref()))
        .map(|(name, path)| (String::from_utf8_lossy(&name).into_owned(), path))
        .collect();
    (report_info, files, dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bytes CPython 3.11's `tarfile` writes for one file
    /// (`TarInfo.tobuf(PAX_FORMAT)` of name `report_info.json`, mode 0o644,
    /// uid/gid 0, uname/gname `root`, size 2, mtime 1716025200.25).
    #[test]
    fn headers_match_cpython_tarfile() {
        let info = MemberInfo {
            name: b"report_info.json".to_vec(),
            mode: 0o100644,
            uid: 0,
            gid: 0,
            size: 2,
            mtime: 1716025200.25,
            typeflag: b'0',
            linkname: Vec::new(),
            uname: "root".into(),
            gname: "root".into(),
        };
        let bytes = member_headers(&info);
        assert_eq!(bytes.len(), 3 * 512);
        assert_eq!(&bytes[0..14], b"././@PaxHeader");
        assert_eq!(&bytes[100..108], b"0000000\0");
        assert_eq!(&bytes[124..136], b"00000000027\0");
        assert_eq!(&bytes[148..156], b"010214\0 ");
        assert_eq!(bytes[156], b'x');
        assert_eq!(&bytes[257..265], b"ustar\x0000");
        assert_eq!(&bytes[512..536], b"23 mtime=1716025200.25\n\0");
        let ustar = &bytes[1024..1536];
        assert_eq!(&ustar[0..17], b"report_info.json\0");
        assert_eq!(&ustar[100..108], b"0000644\0");
        assert_eq!(&ustar[124..136], b"00000000002\0");
        assert_eq!(&ustar[136..148], b"14622073560\0");
        assert_eq!(&ustar[148..156], b"013032\0 ");
        assert_eq!(&ustar[265..270], b"root\0");
        assert_eq!(&ustar[297..302], b"root\0");
        assert_eq!(&ustar[329..345], &[0u8; 16]);
    }

    /// Long names go to a pax `path` record, the ustar field keeps the first
    /// 100 characters.
    #[test]
    fn long_names_use_pax_path() {
        let name = "d/".repeat(60).into_bytes();
        let info = MemberInfo {
            name: name.clone(),
            mode: 0o644,
            uid: 1000,
            gid: 1000,
            size: 0,
            mtime: 1.0,
            typeflag: b'0',
            linkname: Vec::new(),
            uname: String::new(),
            gname: String::new(),
        };
        let bytes = member_headers(&info);
        let records = String::from_utf8_lossy(&bytes[512..1024]).into_owned();
        assert!(records.starts_with("130 path=d/d/"), "{records}");
        assert!(records.contains("\n13 mtime=1.0\n"), "{records}");
        assert_eq!(&bytes[1024..1124], &name[..100]);
    }

    #[test]
    fn archives_read_back_with_a_standard_tar_reader() {
        let dir = tempfile::tempdir().unwrap();
        let draft = dir.path().join("draft");
        fs::create_dir_all(draft.join("debug/2026-05-18")).unwrap();
        fs::write(
            draft.join(super::super::REPORT_INFO_NAME),
            "{\"localID\": \"x\"}",
        )
        .unwrap();
        fs::write(
            draft.join("debug/2026-05-18/10:00:00.shot.json.zst"),
            vec![7u8; 1000],
        )
        .unwrap();
        fs::write(draft.join("debug.txt"), "x").unwrap();
        let names: Vec<String> = draft_files_sorted(&draft)
            .into_iter()
            .map(|(n, _)| String::from_utf8(n).unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "debug/2026-05-18/10:00:00.shot.json.zst",
                "debug.txt",
                "report_info.json"
            ]
        );
        let out = dir.path().join("out.zstd");
        write_tar_zstd_from_draft(&out, &draft).unwrap();
        let tar = zstd::stream::decode_all(fs::read(&out).unwrap().as_slice()).unwrap();
        assert_eq!(tar.len() % RECORD as usize, 0);
        let (info, files, _extracted) = read_tar_zstd(&out);
        assert_eq!(info["localID"], "x");
        assert_eq!(
            files.keys().cloned().collect::<Vec<_>>(),
            ["debug.txt", "debug/2026-05-18/10:00:00.shot.json.zst"]
        );
        // The members are in sorted(rglob) order inside the tar.
        let mut tar_archive = tar::Archive::new(tar.as_slice());
        let members: Vec<String> = tar_archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().display().to_string())
            .collect();
        assert_eq!(members, names);
        assert!(!dir.path().join("out.zstd.tmp").exists());
    }
}
