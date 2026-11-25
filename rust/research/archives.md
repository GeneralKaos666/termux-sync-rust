# Archive creation and safe extraction in Rust

Scope: replace `create_archive`, `_safe_extract_member`, `_check_member_root`,
`extract_archive` and `_PartWriter` in `termux-sync.py` on Termux aarch64.
Target crates: `tar`, `flate2`, `bzip2`, `xz2` (or `liblzma` / `lzma-rust2`).
Python reference is 3.13 (`.python-version`); the device also has 3.14.10.

Verdict: the Rust ecosystem can reproduce the *behavior*, but not the exact
archive bytes. The `tar` crate does enough of the format work, and almost all
security checks in `_safe_extract_member` / `_check_member_root` must be
hand-written because `Entry::unpack_in` silently strips absolute paths, skips
`..` components, and only additionally blocks parent-directory escapes.
Python's `filter="data"` mode/ownership rewriting is not implemented by the
crate.

Byte-for-byte equality is not required: the manifest checksum is computed over
whatever archive the current run wrote, and restore compares against that same
manifest entry (`termux-sync.py:1828-1831`, `termux-sync.py:2123`).

---

## 1. What the Python code actually does

### 1.1 Creation (`termux-sync.py:727-815`)

- Compression is one of `gz`, `bz2`, `xz`; anything else silently falls back to
  `gz`. Output name is forced to `<stem>.tar.<compression>` by stripping
  `.tar.gz`, `.tar.bz2`, `.tar.xz`, `.tar` (`728-753`).
- With a `sink`, it opens `tarfile.open(fileobj=sink, mode="w|gz"|"w|bz2"|"w|xz")`
  (streaming, no seek). Without a sink it opens `mode="w:gz"` etc. on the path
  (`802-805`). The two paths produce different gzip headers: the file path gets
  an `FNAME` header, the stream path does not.
- Directory sources are passed to `tf.add(source_path, arcname=source_path.name,
  filter=_filter)`; a single-file source is added **without** the filter
  (`806-810`). So excludes apply only to directory trees.
- `_filter` first calls `on_member(tarinfo)` for every member, then builds
  `"/" + name.strip("/") + "/"` and drops the member if any wrapped exclude is a
  substring (`761-768`). Returning `None` for a directory prevents recursion
  into it, so the whole subtree is dropped.
- Excludes are wrapped as `/<pattern>/` after stripping `./` and `/`
  (`_exclude_prefix`, `718-724`). Matching is a substring test, not a path
  component match.
- Before archiving a directory, `_unlock` walks it and adds `0o500` to
  unreadable directories and `0o400` to unreadable files; `_relock` restores the
  original modes in reverse order (`770-812`). This exists so one unreadable
  file cannot abort the whole backup.
- `tarfile` defaults: `PAX_FORMAT` since 3.8, so Python writes POSIX pax headers
  when needed (non-ASCII name, name/link over the ustar limit, float mtime,
  numeric overflow) [10][11][19]. `TarInfo` comes from `os.lstat`, records
  `st_uid`, `st_gid`, `st_mtime` (float), `st_mode`, and `uname`/`gname` from
  `pwd`/`grp`; hardlinks are deduplicated by `(st_dev, st_ino)` when
  `st_nlink > 1` [12].
- Directory recursion is sorted (`for f in sorted(os.listdir(name))`, since
  Python 3.7), parent directory entry first [11][13].

### 1.2 Extraction (`termux-sync.py:818-912`)

`_safe_extract_member` rejects, before any write:

1. names starting with `/` or `\` (`822-823`);
2. any path component equal to `..` (`824-825`);
3. a target that resolves outside `dest_path` (`827-831`);
4. for symlink/hardlink: an absolute link target (`833-836`), or a link whose
   resolution escapes the destination (`837-842`); hardlink targets resolve
   relative to `dest_path`, symlink targets relative to the symlink's parent;
5. character devices, block devices and FIFOs (`844-845`).

`_check_member_root` additionally, when `expected_top` is set:

- requires the first non-`.` component of the member name to equal
  `expected_top` (`855-859`);
- requires the resolved target to be inside `dest/expected_top` (`860-863`);
- applies the same jail to link targets (`864-870`).

`extract_archive` iterates `tf.getmembers()` (all `TarInfo` loaded first),
logs a warning and skips members that fail either check, then calls
`tf.extract(member, dest_path, set_attrs=True, filter="data")` when available
(`886-901`). On **any** exception it retries once with `member.uid =
os.getuid()`, `member.gid = os.getgid()` and `set_attrs=False` (`902-911`).

Important 3.13 semantics of that retry:

- `TarFile.chown` is a no-op unless `os.geteuid() == 0`, so on Termux the uid/gid
  fallback is effectively dead code for ownership [15].
- `chmod`/`utime` failures raise `ExtractError`, which is *non-fatal* at the
  default `errorlevel=1`; they do **not** trigger the retry [15].
- The retry therefore only matters for file/directory creation errors.
- Symlinks skip `chmod` and `utime` entirely [15].

`filter="data"` (3.13) performs these additional transformations [9][14]:

- strip leading `/` (already rejected by the local check);
- reject absolute names, or names that leave the destination after resolution;
- reject a link named `.` (the destination itself);
- `mode &= 0o755`; for regular files and hardlinks, clear all execute bits when
  the owner execute bit is absent, then force `mode |= 0o600`; for directories
  and symlinks set `mode = None`; any other special file is rejected;
- set `uid`, `gid`, `uname`, `gname` to `None`;
- reject absolute link targets, normalize link targets with `normpath`, and
  reject links resolving outside the destination.

### 1.3 Parts (`termux-sync.py:1580-1667`)

`_PartWriter` is a `Write` sink around a temporary part file:

- part limit is `storage.CHUNK_SIZE`, 200 MiB for GitHub (`1307`);
- current part path is `<base>.part{idx:04d}` (`1621`);
- when full, it closes the part, uploads it, deletes it, increments `idx`
  (`1634-1651`);
- on `finish`, if only one part was ever opened (`idx == 0`), the part is
  renamed to the plain `<base>` and uploaded as-is (`1638-1641`);
- `total` counts all compressed bytes and `sha` is a streaming SHA-256 over the
  same bytes (`1626-1627`); the caller records both in the manifest
  (`1822-1831`);
- upload failure is wrapped in `StreamAbort`; `abort()` closes and unlinks the
  current part (`1643-1667`).

Restore reassembles by regex `^(.+)\.part(\d{4})$`, sorting names, concatenating
in order and deleting the parts (`1527-1545`). The `.partNNNN` naming is a
compatibility surface (ADR-004) and must stay.

---

## 2. Crate set

Recommended, all pure Rust or bound to the system libraries already present:

```toml
tar    = { version = "0.4.46", default-features = false } # xattr not needed
flate2 = { version = "1", default-features = false, features = ["miniz_oxide"] }
bzip2  = { version = "0.6", default-features = false, features = ["bzip2-sys"] }
xz2    = { version = "0.1" }
sha2   = { version = "0.10" }
```

- `tar` default features enable `xattr`; Python's `tarfile` does not preserve
  xattrs, and `unpack_xattrs` defaults to false, so disable the feature
  [25][38].
- `flate2`'s `miniz_oxide` feature selects the pure-Rust backend explicitly;
  the `rust_backend` alias may switch backends later [24].
- `bzip2` 0.6's default is the pure-Rust `libbz2-rs-sys`; the `bzip2-sys`
  feature selects the C library [26].
- `xz2` has no default features and probes `liblzma` through `pkg-config`
  [28][39].
- Alternative for xz: `liblzma` 0.4 (maintained fork of `xz2`); use
  `default-features = false` to avoid its `bindgen` build, or keep defaults
  because libclang is installed on this device [31][37].
- Alternative that removes the C dependency entirely: `lzma-rust2`
  (`XzWriter`/`XzReader`) [30]. This would contradict ADR-005.

### Termux measurements (this device)

- `rustc 1.100.0`, `cargo 1.100.0`.
- `pkg-config --modversion liblzma` → `5.8.4`; `liblzma.pc` exists;
  `$PREFIX/include/lzma.h` exists. `xz2`/`lzma-sys` will link the system
  library.
- `pkg-config --modversion bzip2` → **not found**; there is no `bzip2.pc`,
  although `libbz2` and `$PREFIX/include/bzlib.h` are installed.
  `bzip2-sys`'s build script probes `bzip2` with `pkg-config` and, on failure,
  compiles its bundled bzip2 1.0.8 C sources statically [27]. So
  `features = ["bzip2-sys"]` still works, but it does **not** link the system
  `libbz2` on this device. Options: accept the bundled C build, use `bzip2`'s
  pure-Rust default, or install a `bzip2.pc`. This is an ADR-005 deviation to
  record.

---

## 3. Streaming writer stack

`tar::Builder<W>` needs only `Write`; `append`, `append_path*`, `append_file`
and `append_dir_all` all write sequentially, no seek [16][23]. Therefore the
whole stack is:

```text
tar::Builder<GzEncoder<PartSink>>       // w:gz
tar::Builder<BzEncoder<PartSink>>       // w:bz2
tar::Builder<XzEncoder<PartSink>>       // w:xz
```

```rust
let sink = PartSink::new(storage, release_id, base_name, work_dir, progress)?;
let enc  = GzEncoder::new(sink, flate2::Compression::new(9));
let mut ar = tar::Builder::new(enc);
ar.follow_symlinks(false); // default is true in this crate
ar.sparse(false);          // default is true in this crate
// ... append entries ...
let enc  = ar.into_inner()?; // writes the 1024-byte terminator
let sink = enc.finish()?;    // writes the gzip/bzip2/xz trailer
sink.finish()?;              // uploads the last part
```

Finish order is mandatory. `Builder::finish` writes exactly
`[0u8; 1024]` [23]; `into_inner` calls it if it was not called [16]. The
compressor's `finish` writes the frame trailer [24][26][28][30]. Dropping a
`Builder` also tries to finish, but errors are discarded, so do it explicitly.

`PartSink` requirements (equivalent of `_PartWriter`):

```rust
impl Write for PartSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> { /* split at limit */ }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
```

- never write more than the space left in the current part; the caller may pass
  a multi-megabyte buffer;
- hash and count every byte accepted (`sha2::Sha256`, `total += n`);
- on a full part: close, upload, unlink, `idx += 1`, open next
  `<base>.part{idx:04d}`;
- `finish()` uploads the remainder; if `idx == 0`, rename to `<base>` first;
- `abort()` closes and unlinks the current part;
- an upload error must surface as `io::Error` (e.g. `io::Error::other`) so the
  tar/compressor stack unwinds; the caller must not try to resume;
- expose `total`, `sha256`, `idx`, and a progress callback every 8 MiB to match
  `_PartWriter`'s note interval (`1629-1631`).

No temp file for the whole archive is needed; only the current 200 MiB part is
on disk, exactly like Python. The `w|` streaming mode in Python is the direct
equivalent; there is no seek requirement anywhere in the Rust stack.

---

## 4. Creation algorithm

Do **not** call `append_dir_all` for the real backup. It has no exclude hook,
no `on_member` hook, no hardlink deduplication, and its traversal order is a
LIFO stack over `fs::read_dir`, so it is filesystem-order dependent [23].

Use a hand-written pre-order walk:

```text
root_name = src.file_name()                 // "home", "etc"
stack = [(src, root_name)]
while let Some((path, arc)) = stack.pop():
    meta = symlink_metadata(path)           // never follow symlinks
    on_member(arc, meta)                    // before excludes, like Python
    if excluded("/" + arc + "/"): continue  // skips subtree
    match meta.file_type():
      dir      => append_dir(arc, path);
                  children = read_dir(path).sorted();
                  push children in reverse sorted order
      symlink  => append_path_with_name(path, arc)   // stores link, no follow
      file     => if meta.nlink() > 1 && seen[(dev,ino)] => append hardlink
                  else { seen[(dev,ino)] = arc; append_file(arc, &mut file) }
      fifo/char/block => append_path_with_name(path, arc) // crate handles these
      socket   => skip                     // Python gettarinfo returns None
```

Key details:

- `Builder::follow_symlinks(false)` is required; the crate defaults to `true`
  and dereferences links, including links escaping the source tree [17][23].
- `Builder::sparse(false)` is required; the crate defaults to `true` and writes
  GNU sparse type `'S'` entries on Android/Linux, while Python always writes
  the file data verbatim [18][23][12].
- `append_file`/`append_path_with_name` fill a GNU header with `st_mode`,
  `st_uid`, `st_gid` and `st_mtime` in seconds (`HeaderMode::Complete`), set
  size 0 for directories and symlinks, and never set `uname`/`gname`
  [25][40].
- Paths longer than the 100-byte header field are written as a GNU `L`
  long-name entry automatically; link targets use `K` [23]. Python reads both
  [11][19]. The crate never writes PAX.
- Hardlinks: build `Header::new_gnu()`, `set_metadata_in_mode(&meta,
  HeaderMode::Complete)`, `set_entry_type(EntryType::Link)`, `set_size(0)`,
  then `append_link(&mut header, arc, first_arc)` [16][20]. Target must already
  have been written, which the walk order guarantees.
- Permissions: `_unlock`/`_relock` must be ported. Without it, one EACCES
  aborts the whole item archive, which the caller treats as a skipped item
  (only ENOSPC is re-raised, `termux-sync.py:1846-1856`).
- Exclude matching must use the same wrapped-substring rule, not globs and not
  component equality. Build `/` + arc + `/`, then `wrapped_excludes.iter()
  .any(|p| wrapped_name.contains(p))`.
- `on_member` is called before the exclude test, so excluded bytes still count
  toward the progress event in Python (`761-768`, `1814-1817`). Preserve that
  quirk.

Python writes PAX; Rust writes GNU. Both are readable by Python (`GNU_FORMAT`
is "fully supported" for long names; sparse is read-only) [19]. Consequences:
sub-second mtimes and `uname`/`gname` are lost in Rust-written archives, and
non-ASCII names are stored as raw GNU long names instead of pax `path` keys.
Restoring a *Python* archive with Rust also loses sub-second mtime: the crate
applies PAX `size`/`uid`/`gid`, but mtime comes from the integer header field
only [25].

---

## 5. Extraction: what the crate does and what to hand-write

`Archive::new<R: Read>` works on a non-seekable decoder, and `entries()`
streams members [21][22]. `entries()` (non-raw) consumes GNU `L`/`K` and pax
extension records and yields the logical member with `path_bytes()` and
`link_name_bytes()` resolved [21][22].

`Entry::unpack_in(dst)` behavior in 0.4.46 [21]:

- skips `Prefix`, `RootDir` and `.` components; **leading `/` is stripped, not
  rejected**;
- returns `Ok(false)` for any `..` component;
- creates missing parent directories and canonicalizes the parent, rejecting a
  parent outside `dst`;
- creates regular files, directories, symlinks (without validating the target)
  and hardlinks (validating the hardlink source inside `dst`);
- falls through to creating a regular file for unrecognized type flags; it does
  **not** call `mknod`, so char/block/FIFO entries become empty regular files;
- default flags: `preserve_permissions = false`, `preserve_ownerships =
  false`, `preserve_mtime = true`, `overwrite = true` [25].

That is materially weaker than Python's checks. Hand-write all of these:

| Check | Python | `tar` crate | Action |
|---|---|---|---|
| Absolute member name | raises, member skipped | strips `/`, extracts | reject before unpack |
| `..` in name | raises, skipped | `Ok(false)`, silent | reject before unpack, log |
| Path escapes dest via symlink | `.resolve()` check | parent canonicalization | keep, plus own check |
| Symlink target absolute | rejects | **not checked** | hand-check |
| Symlink target escapes dest/expected_top | rejects | **not checked** | hand-check |
| Hardlink target escapes expected_top | rejects | only checks inside dest | hand-check |
| Link to destination itself | data filter rejects | not checked | hand-check |
| Char/block/FIFO | rejects | creates regular file | reject by `EntryType` |
| Mode rewrite (data filter) | clears high bits, `go-w`, `u+rw`, exec rule | not implemented | hand-apply |
| uid/gid clearing | data filter sets `None` | already not preserved | keep `preserve_ownerships=false` |
| Expected top component | enforces | not implemented | hand-check every member |
| Non-fatal chmod/utime | logged, continues | `unpack_in` returns Err | apply attrs best-effort |

Suggested order per member:

1. `let name = entry.path_bytes()` (bytes, not `path()`), reject a leading
   `/` or `\`.
2. Walk `Path::new(OsStr::from_bytes(name)).components()`; reject `ParentDir`,
   ignore `CurDir`. Reject empty results only when `expected_top` is set.
3. `target = dest.join(rel)`; `resolve_allow_missing(target)` must start with
   `resolve_allow_missing(dest)` (component-wise, on canonicalized existing
   ancestors).
4. If `expected_top`: first component must equal it and the resolved target
   must be inside `resolve_allow_missing(dest/expected_top)`.
5. Read `entry.header().entry_type()`:
   - `Char | Block | Fifo` → reject;
   - `Symlink | Link` → get `link_name_bytes()`; reject absolute; normalize
     lexically (`normpath`); base is `target.parent()` for symlink and `dest`
     for hardlink; resolve and require inside dest (and inside the expected
     jail). Reject a link named `.`.
   - unknown type flag → reject or, to match Python's `makeunknown`, treat as
     regular only if you deliberately choose that; the local Python code never
     reaches it because `_safe_extract_member` rejects only devices/FIFOs.
6. Extract with attributes disabled, then apply filtered attributes manually:
   - `mode &= 0o755`; regular/hardlink: if `mode & 0o100 == 0` then
     `mode &= !0o111`; `mode |= 0o600`; directories/symlinks: no chmod.
   - `chmod` regular files and directories; never symlinks.
   - `utime` regular files and directories (`FileTime::from_unix_time(secs,
     0)`); never symlinks. Ignore/report errors instead of failing the member.
   - `chown` only when `geteuid() == 0`; otherwise skip. If implementing the
     Python fallback, set uid/gid to the current process ids on retry.

### `resolve_allow_missing`

`Path::canonicalize` fails on missing paths, while Python's
`Path.resolve()` resolves the existing prefix and normalizes the rest
(`termux-sync.py:827-842`). Implement:

```text
fn resolve_allow_missing(p):
    tail = []
    cur  = p
    loop:
        if let Ok(c) = fs::canonicalize(cur):
            for part in tail.iter().rev(): c.push(part)
            return c
        tail.push(cur.file_name())
        cur = cur.parent() or return Err
```

Assumes the tail contains no `..`; normalize link targets with a lexical
`normpath` first, and reject `..` in member names outright. This also follows
pre-existing symlinks in `dest`, matching Python's escape detection.

### Links

Create them yourself after validation, instead of relying on `unpack_in`:

- symlink: remove an existing entry, then `std::os::unix::fs::symlink(target,
  path)`; do not chmod/utime it.
- hardlink: Python 3.13 resolves the link target with `realpath` before
  `os.link` (CVE fix) [15]; do `fs::canonicalize(dest.join(linkname))` when it
  exists, then `fs::hard_link`. If the target does not exist, Python (with a
  seekable archive) searches for the member and extracts it as a fallback;
  Rust streaming extraction cannot do that. Log a warning and skip, or accept
  a later error. This only affects foreign archives: Python's own writer only
  emits hardlinks to already-written members [12].

### Retry semantics

Mirror `extract_archive`:

1. run the checks; on failure log
   `Skipped unsafe archive member during restore: ...` and continue;
2. extract with attributes off;
3. on creation error, retry once with uid/gid set to `getuid()/getgid()` (only
   meaningful as root) and still attributes off;
4. if that fails, log `Could not extract {name!r}: {e}` and continue.

Attribute failures must not take the retry path; log them and continue, matching
Python's non-fatal `ExtractError` handling [15].

---

## 6. Compression compatibility

Python mode mapping and defaults:

| Python mode | Encoder | Level | Rust equivalent |
|---|---|---|---|
| `w:gz` / `w\|gz` | `gzip.GzipFile` | 9 | `flate2::write::GzEncoder::new(w, Compression::new(9))` |
| `w:bz2` / `w\|bz2` | `bz2.BZ2File` | 9 | `bzip2::write::BzEncoder::new(w, bzip2::Compression::new(9))` |
| `w:xz` / `w\|xz` | `lzma.LZMAFile` | preset 6 | `xz2::write::XzEncoder::new(w, 6)` |
| `r:*` | sniff | — | sniff magic, then decoder |

Python's levels are in `tarfile.py` `gzopen(compresslevel=9)`,
`bz2open(compresslevel=9)`, `xzopen(preset=None)`; `lzma` documents the default
preset as 6 [16][32].

`r:*` means transparent compression [7]. Implement it by peeking six bytes and
chaining them back:

- `1f 8b` → gzip;
- `42 5a 68` (`BZh`) → bzip2;
- `fd 37 7a 58 5a 00` → xz;
- otherwise → plain tar.

Use `MultiGzDecoder` and `MultiBzDecoder`; Python's `gzip`/`bz2` readers accept
concatenated members [24][26]. For xz use `XzDecoder::new_multi_decoder` [29].
Single-stream archives (the normal case) work with the simple decoder too.

gzip header note: `flate2` writes `mtime = 0` and OS byte 255 by default; at
level 9 it writes `XFL = 2` [24]. Python writes the current time and OS 255, and
adds `FNAME` when compressing to a named file [16][18]. The deflate payload also
differs because `miniz_oxide` is not zlib. None of this affects readability.

Memory: xz preset 6 allocates the largest working set of the three; bzip2 level
9 buffers a 900 KiB block; gzip/miniz is small. The encoders themselves make
the sink's 200 MiB part the dominant disk cost, not RAM. Keep xz at 6 (Python
parity) rather than 9 on low-memory devices.

---

## 7. Determinism

What Python writes by default:

- entry order: sorted directory listing, parent before children
  [11][13];
- metadata: uid, gid, uname, gname, mode and float mtime from `lstat`
  [12];
- pax extended headers only when a field needs them; `mtime` is always emitted
  as a pax float because `st_mtime` is a float, while the ustar field gets the
  rounded value [14];
- gzip header mtime is `time.time()`, so two identical gz backups differ
  [18];
- bz2 and xz streams carry no timestamp.

What Rust writes with `HeaderMode::Complete`:

- whatever order the walk emits; sort children byte-wise to be deterministic
  (UTF-8 byte order equals code-point order for valid names);
- uid/gid/mode/mtime from `symlink_metadata`, mtime truncated to seconds;
- no `uname`/`gname`;
- GNU headers and GNU long names; no pax;
- gzip mtime 0, so gzip output is reproducible across runs.

Do **not** use `HeaderMode::Deterministic` for backups: it forces uid/gid 0,
mtime to the crate constant, and mode to `0o755` for directories or files with
owner-execute (otherwise `0o644`), losing group/other execute bits, setuid/
setgid/sticky and every other real permission bit [25]. Keep `Complete` and
accept second-granularity mtimes.

Determinism to aim for: same input tree + same crate versions → identical
archive bytes for all three formats. Cross-language byte equality is neither
achievable (gzip) nor required (manifest checksums are per-run).

---

## 8. Traps

- **Sparse files.** `Builder` defaults to `sparse(true)` and emits GNU sparse
  entries on Android; Python never writes sparse but can read all sparse
  variants [18][19]. Call `sparse(false)` for output parity. Reading a sparse
  archive created elsewhere works; tar handles the hole records.
- **Hardlinks.** Python deduplicates by inode; `append_dir_all`/`append_path`
  do not, so a naive port inflates the archive and loses link identity. Do the
  `(dev, ino)` map. On extraction, hardlink targets must already exist;
  foreign archives with forward references need Python's seek-based fallback,
  which the streaming extractor cannot provide.
- **Long paths.** GNU `L`/`K` records are written automatically above 100
  bytes; Python splits ustar names at 100/155 prefix or uses pax. Both sides
  read each other. Do not truncate paths manually.
- **Permission errors.** `_unlock`/`_relock` is behavior, not an optimization:
  without it an unreadable file silently drops the whole item archive. On
  extraction, chmod/utime failures are warnings, not member failures.
- **Unicode / invalid UTF-8.** Use `path_bytes()`/`link_name_bytes()` and
  `OsStr::from_bytes`; never `to_string_lossy()` for decisions or paths.
  Python uses UTF-8 pax `path` keys for non-ASCII names and `surrogateescape`
  for invalid bytes; GNU long names carry raw bytes and round-trip on Linux.
- **Tar terminator and padding.** Each entry's data is padded to a 512-byte
  boundary; `Builder::finish` appends 1024 zero bytes [23]. Python does the
  same on close [20]. `ignore_zeros` is false by default on both sides, so
  trailing junk after the terminator is ignored; concatenated archives are not
  read unless `ignore_zeros` is enabled [7][21].
- **Truncated archives.** `entries()` returns an error on a short header/data
  block; treat that like Python's `ReadError` and abort the restore with the
  same user-visible failure, not a partially successful exit.
- **Existing symlink at a file's final path.** `unpack_in` removes and recreates
  it; Python opens through it (after realpath checks). If you hand-handle
  regular files, unlink first to avoid writing through an attacker-controlled
  link.
- **TOCTOU.** Both implementations are path-based; the tar crate explicitly
  excludes concurrent destination mutation from its threat model [22]. Do not
  claim stronger guarantees.
- **`getmembers` vs streaming.** Python loads all headers before extracting, so
  it can resolve later link targets; Rust streams, which is better for memory
  but loses that fallback.

---

## 9. Test plan for the module

Create fixtures with Python 3.13 for all three compressions, then:

1. **Round trip Rust → Python.** Build tar.gz/bz2/xz from a tree containing
   regular files, empty dirs, symlinks (relative, absolute, `..`), hardlinks,
   FIFO, mode 0o4755/0o600/0o000, names >100 bytes, non-ASCII and invalid
   UTF-8. Verify with `python3 -m tarfile -l` and `tar -tvf`.
2. **Round trip Python → Rust.** Extract into an empty dir; compare file bytes,
   symlink targets, hardlink inode identity, mode (after data-filter masking)
   and mtime seconds.
3. **Rejection fixtures.** Absolute name, `../` name, symlink to `/etc/passwd`,
   symlink to `../../outside`, hardlink to outside, link named `.`, FIFO, char
   device, member outside `expected_top`, member whose parent is a pre-existing
   symlink pointing outside. Assert warning text and that nothing was written
   outside.
4. **`r:*` sniffing.** Plain tar, gz, bz2, xz, gz with two members, and a
   truncated stream.
5. **Determinism.** Same tree twice → identical bytes; sorted order regardless
   of `readdir` order.
6. **Part sink.** Fake uploader that fails on part 2: assert the error
   propagates, the current part is unlinked, no further writes happen, and
   `sha256(concat(parts)) == sink.sha`.
7. **Memory.** 2–3 GiB tree with 200 MiB parts: peak RSS must stay near the
   encoder working set, not the archive size.

---

## 10. Open decisions

1. **bzip2 backend.** ADR-005 says system `libbz2`; this device has no
   `bzip2.pc`, so `bzip2-sys` builds the bundled C source. Accept, switch to
   the pure-Rust default, or ship a `.pc`. Owner: ADR update.
2. **xz crate.** `xz2` is unmaintained but small and pre-generated;
   `liblzma` is maintained and works with libclang installed; `lzma-rust2`
   removes C entirely. ADR-005 currently points at the system C library, so
   `xz2` or `liblzma` fit.
3. **PAX writing.** If exact Python metadata (sub-second mtime, uname/gname,
   pax path keys) is required, the `tar` crate cannot do it; a custom header
   writer or another crate is needed. Not required by the current manifest or
   restore path.
4. **Hardlink fallback on extraction.** Accept the streaming limitation or
   reopen the file for a second pass when a hardlink target is missing.

---

## References

1. `termux-sync.py:718-724` `_exclude_prefix`.
2. `termux-sync.py:727-815` `create_archive`.
3. `termux-sync.py:818-845` `_safe_extract_member`.
4. `termux-sync.py:848-870` `_within`, `_check_member_root`.
5. `termux-sync.py:873-912` `extract_archive`.
6. `termux-sync.py:1580-1667` `_PartWriter`; `1527-1545` part reassembly;
   `1307` `CHUNK_SIZE`.
7. Python docs, `tarfile` opening modes and `r:*` transparent compression:
   <https://docs.python.org/3/library/tarfile.html#tarfile.TarFile>
8. Python docs, extraction filters:
   <https://docs.python.org/3/library/tarfile.html#extraction-filters>
9. Python docs, `data_filter`:
   <https://docs.python.org/3/library/tarfile.html#tarfile.data_filter>
10. Python docs, `DEFAULT_FORMAT` and supported formats:
    <https://docs.python.org/3/library/tarfile.html#tarfile.DEFAULT_FORMAT>,
    <https://docs.python.org/3/library/tarfile.html#supported-formats>
11. Python docs, `TarFile.add` sorted recursion:
    <https://docs.python.org/3/library/tarfile.html#tarfile.TarFile.add>
12. CPython 3.13 `Lib/tarfile.py`, `gettarinfo` (lstat, hardlink map, uid/gid,
    mtime, uname/gname): <https://github.com/python/cpython/blob/3.13/Lib/tarfile.py#L2152-L2245>
13. CPython 3.13 `Lib/tarfile.py`, `add` (sorted children, filter before
    recursion): <https://github.com/python/cpython/blob/3.13/Lib/tarfile.py#L2295-L2345>
14. CPython 3.13 `Lib/tarfile.py`, `_get_filtered_attrs` / `data_filter`:
    <https://github.com/python/cpython/blob/3.13/Lib/tarfile.py#L798-L900>
15. CPython 3.13 `Lib/tarfile.py`, `_extract_member`, `makelink_with_filter`,
    `chown`, `utime`, error handling:
    <https://github.com/python/cpython/blob/3.13/Lib/tarfile.py#L2473-L2561>,
    <https://github.com/python/cpython/blob/3.13/Lib/tarfile.py#L2595-L2840>
16. CPython 3.13 `Lib/tarfile.py`, compression levels and pax header creation:
    <https://github.com/python/cpython/blob/3.13/Lib/tarfile.py#L1980-L2098>,
    <https://github.com/python/cpython/blob/3.13/Lib/tarfile.py#L1113-L1165>
17. `tar` crate `Builder::follow_symlinks` (default true):
    <https://docs.rs/tar/0.4.46/tar/struct.Builder.html#method.follow_symlinks>
18. `tar` crate `Builder::sparse` (default true):
    <https://docs.rs/tar/0.4.46/tar/struct.Builder.html#method.sparse>
19. Python docs, GNU/PAX support and sparse read-only:
    <https://docs.python.org/3/library/tarfile.html#supported-formats>
20. Python docs, `TarFile.close` appends two zero blocks:
    <https://docs.python.org/3/library/tarfile.html#tarfile.TarFile.close>
21. `tar` crate `Entry::unpack_in` and `Archive`:
    <https://docs.rs/tar/0.4.46/tar/struct.Entry.html#method.unpack_in>,
    <https://docs.rs/tar/0.4.46/tar/struct.Archive.html>
22. `tar` crate security model:
    <https://docs.rs/tar/0.4.46/tar/#security>
23. `tar` 0.4.46 `src/builder.rs` (defaults, stack traversal, GNU long name,
    1024-byte terminator, 512 padding, no exclude API):
    <https://github.com/alexcrichton/tar-rs/blob/0.4.46/src/builder.rs>
24. `flate2` backend features and `GzEncoder`; gzip header defaults:
    <https://docs.rs/flate2/latest/flate2/write/struct.GzEncoder.html>,
    <https://github.com/rust-lang/flate2-rs/blob/main/src/lib.rs>,
    <https://github.com/rust-lang/flate2-rs/blob/main/src/gz/mod.rs#L325-L465>
25. `tar` 0.4.46 `src/archive.rs` and `src/entry.rs` (unpack defaults, path
    stripping, `..` skip, symlink creation, PAX uid/gid/size only, integer
    mtime):
    <https://github.com/alexcrichton/tar-rs/blob/0.4.46/src/archive.rs>,
    <https://github.com/alexcrichton/tar-rs/blob/0.4.46/src/entry.rs>
26. `bzip2` 0.6.1 features (`libbz2-rs-sys` default, `bzip2-sys` C backend) and
    `BzEncoder`/`MultiBzDecoder`:
    <https://docs.rs/crate/bzip2/latest/features>,
    <https://docs.rs/bzip2/latest/bzip2/write/struct.BzEncoder.html>,
    <https://docs.rs/bzip2/latest/bzip2/read/struct.MultiBzDecoder.html>
27. `bzip2-sys` build script (`pkg-config bzip2`, bundled C fallback):
    <https://github.com/trifectatechfoundation/bzip2-rs/blob/master/bzip2-sys/build.rs#L10-L45>
28. `xz2` features (none default) and `XzEncoder`:
    <https://docs.rs/crate/xz2/latest/features>,
    <https://docs.rs/xz2/latest/xz2/write/struct.XzEncoder.html>
29. `xz2` `XzDecoder::new_multi_decoder`:
    <https://docs.rs/xz2/latest/xz2/read/struct.XzDecoder.html>
30. `lzma-rust2` `XzWriter`/`XzOptions`/`XzReader` (pure Rust):
    <https://docs.rs/lzma-rust2/latest/lzma_rust2/struct.XzWriter.html>,
    <https://docs.rs/lzma-rust2/latest/lzma_rust2/struct.XzOptions.html>,
    <https://docs.rs/lzma-rust2/latest/lzma_rust2/struct.XzReader.html>
31. `liblzma` maintained fork of `xz2`:
    <https://docs.rs/liblzma/latest/liblzma/>
32. Python docs, `lzma` default preset 6:
    <https://docs.python.org/3/library/lzma.html#lzma.LZMACompressor>
33. CPython 3.13 `Lib/gzip.py`, header mtime (`time.time()` when unset), OS
    byte, `FNAME`, level: <https://github.com/python/cpython/blob/3.13/Lib/gzip.py#L277-L308>
34. `sha2` crate (streaming SHA-256): <https://docs.rs/sha2/latest/sha2/>
35. `tar` `Header::set_metadata_in_mode`, `HeaderMode`, `EntryType`:
    <https://docs.rs/tar/0.4.46/tar/struct.Header.html#method.set_metadata_in_mode>,
    <https://docs.rs/tar/0.4.46/tar/enum.HeaderMode.html>,
    <https://docs.rs/tar/0.4.46/tar/enum.EntryType.html>
36. `Builder::append_link`:
    <https://docs.rs/tar/0.4.46/tar/struct.Builder.html#method.append_link>
37. Local measurements on the target device (not a URL): `pkg-config
    --modversion liblzma` = 5.8.4, `bzip2` = not found, `rustc 1.100.0`,
    `$PREFIX/include/bzlib.h` and `lzma.h` present.
38. `tar` 0.4.46 `Cargo.toml` (`default = ["xattr"]`):
    <https://github.com/alexcrichton/tar-rs/blob/0.4.46/Cargo.toml>
39. `lzma-sys` build script (probes `liblzma` unless `static`):
    <https://github.com/alexcrichton/xz2-rs/blob/master/lzma-sys/build.rs>
40. `tar` 0.4.46 `src/header.rs` (`fill_platform_from`, `HeaderMode`,
    `uname`/`gname` untouched):
    <https://github.com/alexcrichton/tar-rs/blob/0.4.46/src/header.rs#L760-L860>
