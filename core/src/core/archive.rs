//! Archive creation and safe extraction.
//!
//! Python's `tarfile.add` recursion, filters and `filter="data"` checks are all
//! reproduced here; the `tar` crate does not provide them.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};

use rustix::fs::{Mode, OFlags, open};

use bzip2::Compression as BzCompression;
use bzip2::read::MultiBzDecoder;
use bzip2::write::BzEncoder;
use flate2::Compression as GzCompression;
use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;
use tar::{Builder, EntryType, Header, HeaderMode};
use xz2::read::XzDecoder;
use xz2::write::XzEncoder;

use crate::core::paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Gz,
    Bz2,
    Xz,
}

impl Codec {
    pub fn parse(compression: &str) -> Self {
        match compression {
            "bz2" => Self::Bz2,
            "xz" => Self::Xz,
            _ => Self::Gz,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Gz => "gz",
            Self::Bz2 => "bz2",
            Self::Xz => "xz",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArchiveOptions {
    pub compression: String,
    pub excludes: Vec<String>,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct ArchiveResult {
    pub path: PathBuf,
    pub size: u64,
}

#[derive(Debug, Default)]
pub struct ExtractResult {
    pub extracted: usize,
    pub warnings: Vec<String>,
}

/// Hard error once an archive's file data would exceed the configured budget.
const SIZE_LIMIT_ERROR: &str = "Archive exceeds size limit";

/// True when an extraction warning is benign enough to keep restoring. Device and
/// FIFO entries are deliberately archived but skipped on extraction; every other
/// warning (escape, traversal, unreadable member, failed write) is fatal.
pub fn is_ignorable_warning(warning: &str) -> bool {
    warning.starts_with("Refusing device/fifo entry")
}

fn exclude_prefix(pattern: &str) -> String {
    let mut pat = pattern.trim();
    while let Some(rest) = pat.strip_prefix("./") {
        pat = rest;
    }
    let pat = pat.trim_matches('/');
    if pat.is_empty() {
        String::new()
    } else {
        format!("/{pat}/")
    }
}

fn wrapped_excludes(excludes: &[String]) -> Vec<String> {
    excludes
        .iter()
        .map(|pattern| exclude_prefix(pattern))
        .filter(|pattern| !pattern.is_empty())
        .collect()
}

fn excluded(arc: &Path, excludes: &[String]) -> bool {
    let wrapped = format!("/{}/", arc.to_string_lossy().trim_matches('/'));
    excludes.iter().any(|pattern| wrapped.contains(pattern))
}

/// Add read/execute bits to unreadable entries so one file cannot abort a backup.
/// The returned guard restores the original modes on drop.
struct UnlockGuard {
    changed: Vec<(PathBuf, u32)>,
}

impl Drop for UnlockGuard {
    fn drop(&mut self) {
        relock(std::mem::take(&mut self.changed));
    }
}

fn unlock_tree(root: &Path, excludes: &[String]) -> UnlockGuard {
    let wrapped = wrapped_excludes(excludes);
    let mut changed = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if !wrapped.is_empty() {
            let rel = path.strip_prefix(root).unwrap_or(path.as_path());
            if excluded(rel, &wrapped) {
                continue;
            }
        }
        let mode = meta.permissions().mode();
        if meta.is_dir() {
            if fs::read_dir(&path).is_err() {
                let wanted = mode | 0o500;
                if wanted != mode
                    && fs::set_permissions(&path, fs::Permissions::from_mode(wanted)).is_ok()
                {
                    changed.push((path.clone(), mode));
                }
            }
            if let Ok(entries) = fs::read_dir(&path) {
                for entry in entries.flatten() {
                    stack.push(entry.path());
                }
            }
        } else if meta.is_file() && File::open(&path).is_err() {
            let wanted = mode | 0o400;
            if wanted != mode
                && fs::set_permissions(&path, fs::Permissions::from_mode(wanted)).is_ok()
            {
                changed.push((path.clone(), mode));
            }
        }
    }
    UnlockGuard { changed }
}

fn relock(changed: Vec<(PathBuf, u32)>) {
    for (path, mode) in changed.into_iter().rev() {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
}

pub fn archive_path_for(archive_path: &Path, codec: Codec) -> PathBuf {
    let ext = format!(".tar.{}", codec.extension());
    if archive_path.to_string_lossy().ends_with(&ext) {
        return archive_path.to_path_buf();
    }
    let mut stem = archive_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    for old in [".tar.gz", ".tar.bz2", ".tar.xz", ".tar"] {
        if let Some(rest) = stem.strip_suffix(old) {
            stem = rest.to_string();
            break;
        }
    }
    archive_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}{ext}"))
}

pub fn create_archive_to_writer<W: Write>(
    source: &Path,
    sink: W,
    options: &ArchiveOptions,
    on_member: &mut dyn FnMut(&Path, u64),
) -> Result<W, String> {
    let _guard = unlock_tree(source, &options.excludes);
    write_archive(source, sink, options, on_member)
}

fn write_archive<W: Write>(
    source: &Path,
    sink: W,
    options: &ArchiveOptions,
    on_member: &mut dyn FnMut(&Path, u64),
) -> Result<W, String> {
    let codec = Codec::parse(&options.compression);
    match codec {
        Codec::Gz => {
            let encoder = GzEncoder::new(sink, GzCompression::new(9));
            let mut builder = Builder::new(encoder);
            fill_archive(&mut builder, source, options, on_member)?;
            let encoder = builder
                .into_inner()
                .map_err(|error| format!("OSError: {error}"))?;
            encoder
                .finish()
                .map_err(|error| format!("OSError: {error}"))
        }
        Codec::Bz2 => {
            let encoder = BzEncoder::new(sink, BzCompression::new(9));
            let mut builder = Builder::new(encoder);
            fill_archive(&mut builder, source, options, on_member)?;
            let encoder = builder
                .into_inner()
                .map_err(|error| format!("OSError: {error}"))?;
            encoder
                .finish()
                .map_err(|error| format!("OSError: {error}"))
        }
        Codec::Xz => {
            let encoder = XzEncoder::new(sink, 6);
            let mut builder = Builder::new(encoder);
            fill_archive(&mut builder, source, options, on_member)?;
            let encoder = builder
                .into_inner()
                .map_err(|error| format!("OSError: {error}"))?;
            encoder
                .finish()
                .map_err(|error| format!("OSError: {error}"))
        }
    }
}

pub fn create_archive(
    source: &Path,
    archive_path: &Path,
    options: &ArchiveOptions,
    on_member: &mut dyn FnMut(&Path, u64),
) -> Result<ArchiveResult, String> {
    let codec = Codec::parse(&options.compression);
    let out_path = archive_path_for(archive_path, codec);
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("OSError: {error}"))?;
    }
    let file = File::create(&out_path).map_err(|error| format!("OSError: {error}"))?;
    create_archive_to_writer(source, file, options, on_member)?;
    let size = fs::metadata(&out_path)
        .map_err(|error| format!("OSError: {error}"))?
        .len();
    Ok(ArchiveResult {
        path: out_path,
        size,
    })
}

fn fill_archive<W: Write>(
    builder: &mut Builder<W>,
    source: &Path,
    options: &ArchiveOptions,
    on_member: &mut dyn FnMut(&Path, u64),
) -> Result<(), String> {
    builder.follow_symlinks(false);
    builder.sparse(false);
    let excludes = wrapped_excludes(&options.excludes);
    let root_name = source
        .file_name()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("root"));
    let mut seen: HashMap<(u64, u64), PathBuf> = HashMap::new();
    let mut stack = vec![(source.to_path_buf(), root_name)];
    while let Some((path, arc)) = stack.pop() {
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !excludes.is_empty() && excluded(&arc, &excludes) {
            continue;
        }
        let size = if meta.is_file() { meta.len() } else { 0 };
        on_member(&arc, size);
        let file_type = meta.file_type();
        if file_type.is_dir() {
            builder
                .append_dir(&arc, &path)
                .map_err(|error| format!("OSError: {error}"))?;
            let mut children: Vec<(PathBuf, PathBuf)> = Vec::new();
            if let Ok(entries) = fs::read_dir(&path) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    children.push((entry.path(), arc.join(&name)));
                }
            }
            children.sort_by(|left, right| left.1.cmp(&right.1));
            for child in children.into_iter().rev() {
                stack.push(child);
            }
        } else if file_type.is_symlink()
            || file_type.is_fifo()
            || file_type.is_char_device()
            || file_type.is_block_device()
        {
            builder
                .append_path_with_name(&path, &arc)
                .map_err(|error| format!("OSError: {error}"))?;
        } else if file_type.is_socket() {
            continue;
        } else if file_type.is_file() {
            let key = (meta.dev(), meta.ino());
            if meta.nlink() > 1
                && let Some(first) = seen.get(&key)
            {
                let mut header = Header::new_gnu();
                header.set_metadata_in_mode(&meta, HeaderMode::Complete);
                header.set_entry_type(EntryType::Link);
                header.set_size(0);
                builder
                    .append_link(&mut header, &arc, first)
                    .map_err(|error| format!("OSError: {error}"))?;
                continue;
            }
            if meta.nlink() > 1 {
                seen.insert(key, arc.clone());
            }
            let mut file = File::open(&path).map_err(|error| format!("OSError: {error}"))?;
            builder
                .append_file(&arc, &mut file)
                .map_err(|error| format!("OSError: {error}"))?;
        }
    }
    Ok(())
}

fn source_reader(path: &Path) -> Result<Box<dyn Read>, String> {
    let mut file = File::open(path).map_err(|error| format!("OSError: {error}"))?;
    let mut magic = [0u8; 6];
    let read = file
        .read(&mut magic)
        .map_err(|error| format!("OSError: {error}"))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("OSError: {error}"))?;
    let _ = read;
    if magic.starts_with(&[0x1f, 0x8b]) {
        Ok(Box::new(MultiGzDecoder::new(file)))
    } else if magic.starts_with(b"BZh") {
        Ok(Box::new(MultiBzDecoder::new(file)))
    } else if magic.starts_with(&[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00]) {
        Ok(Box::new(XzDecoder::new_multi_decoder(file)))
    } else {
        Ok(Box::new(file))
    }
}

fn link_name(entry: &tar::Entry<'_, Box<dyn Read>>) -> Option<PathBuf> {
    entry
        .link_name_bytes()
        .map(|bytes| PathBuf::from(OsStr::from_bytes(&bytes).to_os_string()))
}

fn reject_name(name: &Path) -> Option<String> {
    let bytes = name.as_os_str().as_bytes();
    if bytes
        .first()
        .is_some_and(|byte| *byte == b'/' || *byte == b'\\')
    {
        return Some(format!("Unsafe absolute path in archive: {name:?}"));
    }
    for component in name.components() {
        if matches!(component, Component::ParentDir) {
            return Some(format!("Path traversal attempt in archive: {name:?}"));
        }
    }
    None
}

fn normalized_relative(name: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in name.components() {
        if let Component::Normal(part) = component {
            out.push(part);
        }
    }
    out
}

fn first_component(path: &Path) -> Option<&OsStr> {
    path.components().find_map(|component| match component {
        Component::Normal(part) => Some(part),
        _ => None,
    })
}

/// `root` must already be resolved (see `extract_archive`); only `path` is
/// resolved here to avoid re-statting every component of `root` per member.
fn inside(path: &Path, root: &Path) -> bool {
    paths::is_within(&paths::resolve_lenient(path), root)
}

fn check_member_root(
    rel: &Path,
    target: &Path,
    dest: &Path,
    expected_top: Option<&str>,
    entry_type: EntryType,
) -> Result<(), String> {
    let Some(top) = expected_top else {
        return Ok(());
    };
    let Some(first) = first_component(rel) else {
        return Err(format!(
            "Archive member outside expected root {top:?}: {rel:?}"
        ));
    };
    if first != OsStr::new(top) {
        return Err(format!(
            "Archive member outside expected root {top:?}: {rel:?}"
        ));
    }
    if !entry_type.is_dir() && normalized_relative(rel) == Path::new(top) {
        return Err(format!(
            "Archive member replaces expected root {top:?}: {rel:?}"
        ));
    }
    let jail = dest.join(top);
    if !inside(target, &jail) {
        return Err(format!("Archive member escapes {top:?}: {rel:?}"));
    }
    Ok(())
}

fn filtered_mode(entry_type: EntryType, mode: u32) -> u32 {
    if entry_type.is_dir() {
        return mode & 0o777;
    }
    let mut mode = mode & 0o755;
    if entry_type.is_file() || entry_type.is_hard_link() {
        if mode & 0o100 == 0 {
            mode &= !0o111;
        }
        mode |= 0o600;
    }
    mode
}

fn remove_existing(path: &Path) {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.is_dir() {
            let _ = fs::remove_dir_all(path);
        } else {
            let _ = fs::remove_file(path);
        }
    }
}

fn set_mtime(path: &Path, seconds: i64) {
    if seconds < 0 {
        return;
    }
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds as u64);
    let times = std::fs::FileTimes::new()
        .set_modified(time)
        .set_accessed(time);
    let _ = std::fs::set_times(path, times);
}

fn create_regular(
    entry: &mut tar::Entry<'_, Box<dyn Read>>,
    target: &Path,
    mode: u32,
    mtime: i64,
    budget: &mut u64,
) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("OSError: {error}"))?;
    }
    if fs::symlink_metadata(target).is_ok_and(|meta| meta.is_dir()) {
        return Err(format!(
            "Refusing to replace directory with file: {}",
            target.display()
        ));
    }
    remove_existing(target);
    let fd = open(
        target,
        OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(mode),
    )
    .map_err(|error| format!("OSError: {error}"))?;
    let mut file = File::from(fd);
    let copied = std::io::copy(&mut entry.take(budget.saturating_add(1)), &mut file)
        .map_err(|error| format!("OSError: {error}"))?;
    if copied > *budget {
        return Err(SIZE_LIMIT_ERROR.to_string());
    }
    *budget -= copied;
    let _ = fs::set_permissions(target, fs::Permissions::from_mode(mode));
    set_mtime(target, mtime);
    Ok(())
}

fn create_dir(target: &Path, mode: u32, mtime: i64) -> Result<(), String> {
    if let Ok(meta) = fs::symlink_metadata(target)
        && meta.file_type().is_symlink()
    {
        let _ = fs::remove_file(target);
    }
    fs::create_dir_all(target).map_err(|error| format!("OSError: {error}"))?;
    let _ = fs::set_permissions(target, fs::Permissions::from_mode(mode));
    set_mtime(target, mtime);
    Ok(())
}

fn link_target(
    entry: &tar::Entry<'_, Box<dyn Read>>,
    dest: &Path,
    target: &Path,
    expected_top: Option<&str>,
) -> Result<PathBuf, String> {
    let name = entry.path_bytes().to_vec();
    let name = PathBuf::from(OsStr::from_bytes(&name).to_os_string());
    let Some(link) = link_name(entry) else {
        return Err(format!("Archive link has no target: {name:?}"));
    };
    if reject_name(&link).is_some() {
        return Err(format!(
            "Unsafe absolute link target in archive: {name:?} -> {link:?}"
        ));
    }
    let base = if entry.header().entry_type().is_symlink() {
        target.parent().unwrap_or(dest)
    } else {
        dest
    };
    let resolved = paths::resolve_lenient(&base.join(&link));
    if !inside(&resolved, dest) {
        return Err(format!(
            "Archive link escapes destination: {name:?} -> {link:?}"
        ));
    }
    if let Some(top) = expected_top {
        let jail = dest.join(top);
        if !inside(&resolved, &jail) {
            return Err(format!(
                "Archive link escapes {top:?}: {name:?} -> {link:?}"
            ));
        }
    }
    Ok(resolved)
}

pub fn extract_archive(
    archive_path: &Path,
    dest: &Path,
    expected_top: Option<&str>,
    max_bytes: Option<u64>,
    on_member: &mut dyn FnMut(&Path),
) -> Result<ExtractResult, String> {
    let dest = paths::resolve_lenient(dest);
    fs::create_dir_all(&dest).map_err(|error| format!("OSError: {error}"))?;
    let reader = source_reader(archive_path)?;
    let mut archive = tar::Archive::new(reader);
    let mut result = ExtractResult::default();
    let mut budget = max_bytes.unwrap_or(u64::MAX);
    let mut entries_seen = 0usize;
    let mut pending_links: Vec<(PathBuf, PathBuf, i64)> = Vec::new();
    let entries = archive
        .entries()
        .map_err(|error| format!("ReadError: {error}"))?;
    for entry in entries {
        entries_seen += 1;
        if entries_seen > 1_000_000 {
            return Err("Archive has more than 1000000 members".to_string());
        }
        let mut entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                result
                    .warnings
                    .push(format!("Could not read archive member: {error}"));
                continue;
            }
        };
        let raw_name = entry.path_bytes().to_vec();
        let rel = PathBuf::from(OsStr::from_bytes(&raw_name).to_os_string());
        on_member(&rel);
        if let Some(reason) = reject_name(&rel) {
            result.warnings.push(reason);
            continue;
        }
        let rel = normalized_relative(&rel);
        if rel.as_os_str().is_empty() {
            result
                .warnings
                .push("Archive member has an empty path".to_string());
            continue;
        }
        let target = dest.join(&rel);
        if !inside(&target, &dest) {
            result
                .warnings
                .push(format!("Archive member escapes destination: {rel:?}"));
            continue;
        }
        let entry_type = entry.header().entry_type();
        if let Err(reason) = check_member_root(&rel, &target, &dest, expected_top, entry_type) {
            result.warnings.push(reason);
            continue;
        }

        let mode = filtered_mode(entry_type, entry.header().mode().unwrap_or(0o644));
        let mtime = entry.header().mtime().unwrap_or(0) as i64;
        if entry_type.is_dir() {
            if let Err(error) = create_dir(&target, mode, mtime) {
                result
                    .warnings
                    .push(format!("Could not extract {rel:?}: {error}"));
            } else {
                result.extracted += 1;
            }
            continue;
        }
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            let resolved = match link_target(&entry, &dest, &target, expected_top) {
                Ok(path) => path,
                Err(reason) => {
                    result.warnings.push(reason);
                    continue;
                }
            };
            if entry_type.is_symlink() {
                let Some(link) = link_name(&entry) else {
                    result
                        .warnings
                        .push(format!("Archive link has no target: {rel:?}"));
                    continue;
                };
                if let Some(parent) = target.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                remove_existing(&target);
                if let Err(error) = symlink(&link, &target) {
                    result
                        .warnings
                        .push(format!("Could not extract {rel:?}: {error}"));
                } else {
                    result.extracted += 1;
                }
            } else {
                pending_links.push((target, resolved, mtime));
            }
            continue;
        }
        if entry_type.is_file() {
            if let Err(error) = create_regular(&mut entry, &target, mode, mtime, &mut budget) {
                if error == SIZE_LIMIT_ERROR {
                    return Err(error);
                }
                result
                    .warnings
                    .push(format!("Could not extract {rel:?}: {error}"));
            } else {
                result.extracted += 1;
            }
            continue;
        }
        result
            .warnings
            .push(format!("Refusing device/fifo entry in archive: {rel:?}"));
    }
    for (target, resolved, _mtime) in pending_links {
        if let Some(parent) = target.parent() {
            let _ = fs::create_dir_all(parent);
        }
        remove_existing(&target);
        let rel = target.strip_prefix(&dest).unwrap_or(target.as_path());
        if let Err(error) = fs::hard_link(&resolved, &target) {
            result
                .warnings
                .push(format!("Could not extract {rel:?}: {error}"));
        } else {
            result.extracted += 1;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn raw_header(name: &str, size: u64, kind: EntryType, link: &[u8]) -> Header {
        let mut header = Header::new_gnu();
        header.set_size(size);
        header.set_mode(if kind.is_symlink() { 0o777 } else { 0o644 });
        header.set_entry_type(kind);
        {
            let bytes = header.as_mut_bytes();
            bytes[..name.len()].copy_from_slice(name.as_bytes());
            if !link.is_empty() {
                bytes[157..157 + link.len()].copy_from_slice(link);
            }
        }
        header.set_cksum();
        header
    }

    fn make_tree(root: &Path) {
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), b"alpha").unwrap();
        fs::write(root.join("sub/b.txt"), b"beta").unwrap();
        symlink("a.txt", root.join("link")).unwrap();
    }

    #[test]
    fn roundtrip_gz_excludes_and_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("home");
        make_tree(&source);
        fs::create_dir_all(source.join("node_modules")).unwrap();
        fs::write(source.join("node_modules/drop.txt"), b"drop").unwrap();
        let options = ArchiveOptions {
            compression: "gz".to_string(),
            excludes: vec!["node_modules".to_string()],
            label: String::new(),
        };
        let out = temp.path().join("out.tar.gz");
        let mut seen = Vec::new();
        let result = create_archive(&source, &out, &options, &mut |path, size| {
            seen.push((path.to_path_buf(), size));
        })
        .unwrap();
        assert_eq!(result.path, out);
        let dest = temp.path().join("dest");
        let extracted = extract_archive(&out, &dest, Some("home"), None, &mut |_| {}).unwrap();
        assert!(extracted.warnings.is_empty(), "{:?}", extracted.warnings);
        assert_eq!(fs::read(dest.join("home/a.txt")).unwrap(), b"alpha");
        assert_eq!(fs::read(dest.join("home/sub/b.txt")).unwrap(), b"beta");
        assert!(!dest.join("home/node_modules").exists());
        let link = fs::read_link(dest.join("home/link")).unwrap();
        assert_eq!(link, PathBuf::from("a.txt"));
        assert!(
            !seen
                .iter()
                .any(|(path, _)| path == Path::new("home/node_modules"))
        );
    }

    #[test]
    fn rejects_traversal_absolute_and_escaping_links() {
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("traversal.tar");
        {
            let file = File::create(&archive_path).unwrap();
            let mut builder = Builder::new(file);
            builder
                .append(
                    &raw_header("../evil.txt", 1, EntryType::Regular, b""),
                    Cursor::new(b"x"),
                )
                .unwrap();
            builder
                .append(
                    &raw_header("/abs.txt", 1, EntryType::Regular, b""),
                    Cursor::new(b"x"),
                )
                .unwrap();
            builder
                .append(
                    &raw_header("home/link", 0, EntryType::Symlink, b"../../outside"),
                    Cursor::new(b""),
                )
                .unwrap();
            builder
                .append(
                    &raw_header("home/fifo", 0, EntryType::Fifo, b""),
                    Cursor::new(b""),
                )
                .unwrap();
            builder.finish().unwrap();
        }
        let dest = temp.path().join("dest");
        let result =
            extract_archive(&archive_path, &dest, Some("home"), None, &mut |_| {}).unwrap();
        assert!(!temp.path().join("evil.txt").exists());
        assert!(!dest.join("abs.txt").exists());
        assert!(!dest.join("home/link").exists());
        assert!(!dest.join("home/fifo").exists());
        assert!(result.warnings.len() >= 4, "{:?}", result.warnings);
    }

    #[test]
    fn ignorable_warning_matches_only_device_fifo_refusals() {
        assert!(is_ignorable_warning(
            "Refusing device/fifo entry in archive: \"home/fifo\""
        ));
        assert!(!is_ignorable_warning(
            "Path traversal attempt in archive: \"../evil.txt\""
        ));
        assert!(!is_ignorable_warning(
            "Could not read archive member: failed to read entire block"
        ));
    }

    #[test]
    fn expected_top_rejects_other_roots() {
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("wrong.tar");
        {
            let file = File::create(&archive_path).unwrap();
            let mut builder = Builder::new(file);
            let mut header = Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "other/file.txt", Cursor::new(b"x"))
                .unwrap();
            builder.finish().unwrap();
        }
        let dest = temp.path().join("dest");
        let result =
            extract_archive(&archive_path, &dest, Some("home"), None, &mut |_| {}).unwrap();
        assert!(!dest.join("other/file.txt").exists());
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn roundtrip_all_compressions() {
        for compression in ["gz", "bz2", "xz"] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("home");
            make_tree(&source);
            let options = ArchiveOptions {
                compression: compression.to_string(),
                excludes: Vec::new(),
                label: String::new(),
            };
            let out = temp.path().join("out.tar");
            let result = create_archive(&source, &out, &options, &mut |_, _| {}).unwrap();
            assert!(
                result
                    .path
                    .to_string_lossy()
                    .ends_with(&format!(".tar.{compression}"))
            );
            let dest = temp.path().join("dest");
            let extracted =
                extract_archive(&result.path, &dest, Some("home"), None, &mut |_| {}).unwrap();
            assert!(extracted.warnings.is_empty(), "{:?}", extracted.warnings);
            assert_eq!(fs::read(dest.join("home/sub/b.txt")).unwrap(), b"beta");
        }
    }

    #[test]
    fn python_reads_rust_archives_and_rust_reads_python_archives() {
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("home");
        make_tree(&source);
        for compression in ["gz", "bz2", "xz"] {
            let options = ArchiveOptions {
                compression: compression.to_string(),
                excludes: Vec::new(),
                label: String::new(),
            };
            let out = temp.path().join("out.tar");
            let result = create_archive(&source, &out, &options, &mut |_, _| {}).unwrap();
            let script = "import sys,tarfile; tf=tarfile.open(sys.argv[1],'r:*'); names=tf.getnames(); assert 'home/a.txt' in names, names";
            let status = std::process::Command::new("python3")
                .args(["-c", script, result.path.to_str().unwrap()])
                .status()
                .unwrap();
            assert!(status.success());
        }

        let py_out = temp.path().join("python.tar.gz");
        let script = "import sys,tarfile,io; p=sys.argv[1]; tf=tarfile.open(p,'w:gz'); data=b'x'; i=tarfile.TarInfo('home/caf\\u00e9.txt'); i.size=1; tf.addfile(i, io.BytesIO(data)); tf.close()";
        let status = std::process::Command::new("python3")
            .args(["-c", script, py_out.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());
        let dest = temp.path().join("python-dest");
        let extracted = extract_archive(&py_out, &dest, Some("home"), None, &mut |_| {}).unwrap();
        assert!(extracted.warnings.is_empty(), "{:?}", extracted.warnings);
        assert_eq!(fs::read(dest.join("home/café.txt")).unwrap(), b"x");
    }

    #[test]
    fn rejects_regular_entry_over_expected_root() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("dest");
        fs::create_dir_all(dest.join("home")).unwrap();
        fs::write(dest.join("home/keep.txt"), b"keep").unwrap();
        let archive = temp.path().join("root.tar");
        let file = File::create(&archive).unwrap();
        let mut builder = Builder::new(file);
        builder
            .append(
                &raw_header("home", 1, EntryType::Regular, b""),
                Cursor::new(b"x"),
            )
            .unwrap();
        builder.finish().unwrap();
        let result = extract_archive(&archive, &dest, Some("home"), None, &mut |_| {}).unwrap();
        assert!(dest.join("home/keep.txt").exists());
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn restores_directory_mode() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("dest");
        let archive = temp.path().join("mode.tar");
        let file = File::create(&archive).unwrap();
        let mut builder = Builder::new(file);
        let mut header = raw_header("home/private", 0, EntryType::dir(), b"");
        header.set_mode(0o700);
        header.set_cksum();
        builder.append(&header, Cursor::new(b"")).unwrap();
        builder.finish().unwrap();
        extract_archive(&archive, &dest, Some("home"), None, &mut |_| {}).unwrap();
        let mode = fs::metadata(dest.join("home/private"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn resolves_forward_hard_link() {
        let temp = tempfile::tempdir().unwrap();
        // Android's SELinux policy denies `link(2)` for app domains, so hard links
        // are unavailable here; skip rather than fail on a capable system.
        let probe = temp.path().join("probe");
        fs::write(&probe, b"x").unwrap();
        if fs::hard_link(&probe, temp.path().join("probe-link")).is_err() {
            return;
        }
        let dest = temp.path().join("dest");
        let archive = temp.path().join("links.tar");
        let file = File::create(&archive).unwrap();
        let mut builder = Builder::new(file);
        builder
            .append(
                &raw_header("home/link", 0, EntryType::Link, b"home/target"),
                Cursor::new(b""),
            )
            .unwrap();
        builder
            .append(
                &raw_header("home/target", 4, EntryType::Regular, b""),
                Cursor::new(b"data"),
            )
            .unwrap();
        builder.finish().unwrap();
        extract_archive(&archive, &dest, Some("home"), None, &mut |_| {}).unwrap();
        assert_eq!(fs::read(dest.join("home/link")).unwrap(), b"data");
    }

    #[test]
    fn refuses_archive_over_byte_budget() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("dest");
        let archive = temp.path().join("big.tar");
        let file = File::create(&archive).unwrap();
        let mut builder = Builder::new(file);
        builder
            .append(
                &raw_header("home/big", 100, EntryType::Regular, b""),
                Cursor::new(vec![b'x'; 100]),
            )
            .unwrap();
        builder.finish().unwrap();
        let error =
            extract_archive(&archive, &dest, Some("home"), Some(10), &mut |_| {}).unwrap_err();
        assert!(error.contains("size limit"));
    }

    #[test]
    fn rejects_link_entries_over_expected_root() {
        for kind in [EntryType::Symlink, EntryType::Link] {
            let temp = tempfile::tempdir().unwrap();
            let dest = temp.path().join("dest");
            fs::create_dir_all(dest.join("home")).unwrap();
            fs::write(dest.join("home/keep.txt"), b"keep").unwrap();
            let archive = temp.path().join("link-root.tar");
            let file = File::create(&archive).unwrap();
            let mut builder = Builder::new(file);
            builder
                .append(
                    &raw_header("home", 0, kind, b"home/keep.txt"),
                    Cursor::new(b""),
                )
                .unwrap();
            builder.finish().unwrap();
            let result = extract_archive(&archive, &dest, Some("home"), None, &mut |_| {}).unwrap();
            assert!(dest.join("home/keep.txt").exists(), "{kind:?}");
            assert!(!result.warnings.is_empty(), "{kind:?}");
        }
    }
}
