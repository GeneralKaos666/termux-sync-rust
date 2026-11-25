# Python semantics that are easy to get wrong in the Rust port

Scope: `termux-sync.py` (3,326 lines, byte-identical to `src/termux_sync/__init__.py`),
`tests/*.py` (2,947 lines), `rust/DECISIONS.md`. Reference runtime is Python 3.13
(`.python-version`), minimum 3.10 (`pyproject.toml`). Where 3.10–3.13 differ, the note says so.

How to use this: each rule has an ID (`P-01`, `S-03`, …). The checklist in §12 maps IDs to
the W1–W7 waves in `MAP.md`, so `port.rs` modules can be reviewed one wave at a time.
Citation keys in brackets resolve in §14. "Verified" means the value was produced by the
Python in this repository on this device; those outputs are in §13 and can become Rust tests.

The governing ADRs matter for every rule below: ADR-003 (exact machine output, normalized
human text), ADR-004 (same files, keys, archive layout), ADR-008 (recorded app cases plus
targeted unit tests, not a literal test translation).

---

## 1. The ten traps most likely to break the port

| # | Trap | Python does | Rust will silently do instead | Rule |
|---|---|---|---|---|
| 1 | Resolving a path that does not exist yet (`_safe_extract_member`, `_storage_excludes`, `_self_excludes`, `LocalStorage.upload`) | `Path.resolve()` with `strict=False` resolves what exists and appends the rest [PY-PATHLIB-RESOLVE] | `std::fs::canonicalize` fails with `NotFound` for the missing tail [RS-CANON] | P-01: write `resolve_lenient`; never call `canonicalize` directly |
| 2 | `Path(member_name).parts` on a tar name containing `\` | POSIX `PurePosixPath` does **not** split on `\`: `Path("a\\b").parts == ('a\\b',)` (verified) [PY-PATHLIB-PARTS] | Rust `Path::components` also does not split `\` on Unix — but a naive `split(['/','\\'])` would change behavior | P-06: split on `/` only; treat `\` as an ordinary byte in member names, exactly like Python |
| 3 | Truncating user text with `s[:16]`, `s[:19]`, `s[:300]`, `s[:12]` | Slices by Unicode code point, clamped, never panics [PY-STR] | `&s[..16]` panics on a char boundary and is byte-based | S-01: `s.chars().take(n).collect()` |
| 4 | Serializing config/manifest with `json.dump(..., indent=2)` | Key order = insertion order; `ensure_ascii=True` writes `\u00e9` [PY-JSON-DUMP] | `serde_json::Map` is a `BTreeMap` (alphabetical) unless `preserve_order` is enabled; strings are written raw UTF-8 | J-01/J-02: enable `preserve_order`, insert in the documented order, escape non-ASCII for on-disk JSON |
| 5 | JSON syntax-error text | `JSONDecodeError.lineno/.colno` are 1-based and character-based; EOF is `line 1 column 1` [PY-JSON-ERROR] | `serde_json::Error::column()` may be `0` at EOF (documented) and counts bytes [SERDE-COLUMN] | J-05: report `line`; clamp `column` to ≥ 1; do not promise identical wording |
| 6 | `human_size`, `_pct_bar`, `fmt_elapsed` | `int()` truncates toward zero; `//` and `%` floor; `f"{x:.1f}"` rounds the binary value [PY-INT][PY-FORMAT] | `as i64` truncates (matches `int()`), but `/` and `%` truncate instead of floor (differs for negatives) | N-01/N-02: use `div_euclid`/`rem_euclid` for `//`/`%`; keep `{:.1}` formatting |
| 7 | `difflib.get_close_matches(..., n=3, cutoff=0.6)` | `SequenceMatcher.ratio()` = `2*M/T`; ties break by comparing the tuple `(ratio, command)` and then `heapq.nlargest` [PY-DIFFLIB-SRC] | The `difflib` crate uses `f32`, stable sort by ratio only, so tie order differs [DIFFLIB-CRATE] | D-02: either port the 20-line algorithm with the tuple tie-break or accept crate output and record the divergent cases |
| 8 | `shutil.disk_usage(path).free` | `f_bavail * f_frsize` (space available to this uid) [PY-SHUTIL-SRC] | `fs4`/`sysinfo` variants may return `f_bfree` or total-free | O-01: compute from `statvfs` explicitly, `free = f_bavail * f_frsize` |
| 9 | `save_config` mode | `os.open(..., 0o600)` (umask applies), then unconditional `chmod 0o600`, chmod failure ignored [PY-OS-OPEN]; test asserts `0o600` | `File::create` uses `0o666 & ~umask` | F-01: `OpenOptionsExt::mode(0o600)` + `set_permissions(0o600)`, ignore its error |
| 10 | `SIGINT` said to be "ignored" during extract | `signal.signal(SIGINT, SIG_IGN)`; the signal is *discarded*, not queued; Ctrl+C afterwards is a fresh signal [PY-SIGNAL] | A `signal-hook` handler on another thread that defers work can replay or double-handle | O-05: use `SIG_IGN` for the protected window, then restore; never "replay" a pending SIGINT |

---

## 2. Paths

**P-01 `resolve()` is lenient; `canonicalize` is strict.**
`Path.resolve(strict=False)` (the default since 3.6) makes the path absolute, resolves
symlinks and eliminates `..`, resolving "as far as possible" and appending the remainder
[PY-PATHLIB-RESOLVE]. `std::fs::canonicalize` requires every component to exist [RS-CANON].
Most calls in this app resolve paths that do not exist yet: `target = (dest_path / member_name).resolve()`
in `_safe_extract_member`, `root.relative_to(src.resolve())` in `_storage_excludes`, and
`dest.resolve()` in `LocalStorage.upload` (destination directory is created after the check).
Rule: implement `resolve_lenient(path)`:
1. Walk components from the left, `lstat` each; on a symlink, read it and continue (bounded,
   e.g. 40 hops); on `NotFound` (or any error other than a loop), append the remaining
   components verbatim and return.
2. Collapse `.` and `..` **after** symlink resolution, never before (Python's docs show
   `Path('docs/../setup.py').resolve()` only becoming `.`-free through resolution
   [PY-PATHLIB-RESOLVE]).
3. Return an absolute path even if the input was relative (`current_dir().join`), matching
   `resolve()`.
On 3.13 a symlink loop in non-strict mode no longer raises [PY-PATHLIB-RESOLVE]; replicate
that by returning the best-effort path when the hop limit is hit. Do not insert an extra
`E LOOP` error the Python path did not have.

**P-02 `relative_to` is lexical and prefix-based.**
`Path.relative_to` succeeds only when the left path starts with the right path's parts; it
never touches the filesystem and treats `..` literally [PY-PATHLIB-RELATIVE].
Rules:
- `_storage_excludes` relies on `ValueError` to mean "local_path is not inside the source".
  In Rust use `path.strip_prefix(root)` and map `Err` to "no exclude" — not string `starts_with`,
  which would treat `/home/x2` as inside `/home/x`.
- `rel.parts` is empty when `root == src`; `parts()` on a `Path` is empty for `""` and `"."`
  and has no trailing empty component (`Path("x/").parts == ("x",)`, verified). Preserve that:
  `Path::new("")` in Rust also has zero components, but `Path::new("./").components()` yields
  `CurDir`, so filter `CurDir` the way `_check_member_root` filters `"."`.

**P-03 `parents` drives the "is inside" test.**
`_within(path, root)` is `path == root or root in path.parents`; `parents` is the logical
ancestor chain, computed lexically [PY-PATHLIB-PARENTS]. Implement `is_within` with
`path == root || path.strip_prefix(root).is_ok()` (same semantics for already-resolved,
absolute paths) and never with `to_string_lossy().contains()`.

**P-04 `~` expansion.**
`Path.expanduser()` uses `os.path.expanduser` semantics: `~` replaces with `$HOME` (Unix),
`~user` via the password database, a `~` that is not a leading component stays literal, and
a missing home raises `RuntimeError` [PY-EXPANDUSER][PY-PATHLIB]. Rust has no `~` expansion
in std. Rules:
- Implement `expand_user` in one helper used by all six call sites:
  `LocalStorage.__init__`, `_storage_excludes`, `cmd_setup`, `cmd_check`,
  `cmd_export_config`, `cmd_import_config`.
- `HOME` empty counts as unset to Python (`os.environ["HOME"] == ""` makes `expanduser`
  fall back to `pwd`). Check "present and non-empty".
- Expand only a leading `~` or `~/`; do not expand `a~b` (verified: `Path("a~b").expanduser() == "a~b"`).
- `expanduser()` does not resolve symlinks; `_storage_excludes` composes
  `expanduser().resolve()`. Keep that order.

**P-05 `Path.home()` and `dirs` behaviour differ.**
`Path.home()` is `expanduser("~")` [PY-PATHLIB]; on Unix it uses `HOME` and falls back to
`pwd`. Tests monkeypatch `Path.home` (`tests/test_formatting.py::test_short_path_home`,
`tests/conftest.py`), so the Rust helper must be injectable. Prefer a single `home_dir()`
that reads `HOME`, then `passwd`, and fails with the same "home cannot be resolved" path
rather than silently returning `/`.

**P-06 Windows separators inside archives.**
Python runs POSIX flavour here: `Path("a\\b").parts == ("a\\b",)` and
`os.path.basename("a\\b") == "a\\b"` (verified) [PY-PATHLIB-PARTS][PY-OS-PATH]. So an archive
member named `..\\..\\evil` is **not** caught by `_safe_extract_member`'s `".." in parts`
check; it becomes one literal filename after `resolve()`. That is the behavior to preserve.
Separately, `_clean_backup_name` deliberately normalizes backslashes itself:
`os.path.basename(str(name).replace("\\", "/"))` — so backup names *do* treat `\` as a
separator. Do not unify the two rules: names use `replace('\\','/')`, tar members do not.
Tar entries are still refused by `data_filter` if they escape the destination, because
`tarfile._get_filtered_attrs` normalizes `..` on `/`-separated segments and compares
`realpath` prefixes [PY-TARFILE-SRC]; mirror that, not a `\`-aware normalizer.

**P-07 Directory iteration order is unspecified.**
`Path.iterdir()` yields arbitrary order [PY-PATHLIB]; the app sorts where order is visible
(`LocalStorage.list_backups` by path descending, `delete_old` by `st_mtime` descending,
GitHub by `ModTime`/`created_at` string descending) and does not sort where it is invisible
(archive member order via `os.walk`). Rules:
- Sorting is by `str`/path code-point order, not locale and not case-folded; Rust `String`
  `Ord` is byte order, which agrees for UTF-8 [RS-ORD].
- `sorted(..., reverse=True)` on paths is a *stable* reversal of ascending order; use
  `sort_by(|a,b| b.cmp(a))`, which is equivalent here (distinct keys) and stable.
- `delete_old` must use `metadata.modified()` (mtime) and must not fall back to name order;
  `tests/test_storage.py::test_local_delete_old_keeps_newest` sets mtimes with `os.utime`
  to values older than the file names suggest.

**P-08 `mkdir(parents=True, exist_ok=True)` everywhere.**
`create_dir_all` matches it, except Python creates intermediate directories with default
permissions and ignores `mode` for them [PY-PATHLIB]. No call site passes `mode` for
`mkdir`, so `create_dir_all` is correct. `tmp_dir.mkdir(parents=True, exist_ok=True)` may
reuse a directory from an earlier crashed run; do not add a "must be empty" check.

---

## 3. Strings and text

**S-01 Indexing and slicing.**
Python `str` indexes code points; slices clamp and never fail [PY-STR]. Rust `&str` slices
by byte and panic off a char boundary [RS-SLICE]. Every truncation in the codebase is a
display/log truncation:
`_backup_date` `[:16]`, `_backup_date`/date cells `[:19]`, `_http_error` raw `[:300]`,
`cmd_restore` checksum `[:12]`, `_bytes_from_rclone` none.
Rule: one helper `head(s, n_chars)`. Note Python counts code points, so an astral character
(emoji in a label or log message) counts 1 while Rust counts 4 bytes; `chars().take(n)` is
the correct match. Add a test with `"📦"` and with a combining accent.

**S-02 `strip()` whitespace set differs from Rust `trim()`.**
Python's whitespace predicate includes characters whose bidirectional class is `WS`, `B` or
`S`, which covers `U+001C`–`U+001F` [PY-STR-ISSPACE]; Rust `char::is_whitespace` is the
Unicode `White_Space` property, which excludes those four [RS-ISWHITESPACE].
Verified: `"\x1c x \x1f".strip() == "x"`. `strip()` is used on user input, manifest names,
exclude patterns, and the restore confirmation, where a paste can contain control bytes.
Rule: `py_strip` = `trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))`.
`strip("_")`, `strip("/")`, `strip("\\")` are plain character-set strips (no range semantics)
and map to `trim_matches` with an explicit set — note Python strips *any* combination of the
given characters, not a prefix [PY-STR-STRIP].

**S-03 `startswith` is ordinal, not path-aware.**
`employee_name.startswith("/")`, `tag.startswith("termux_backup_")`,
`text.startswith(prefix + "/")` in `short_path`, `pat.startswith("./")` in
`_exclude_prefix`. Rust `starts_with(&str)` agrees exactly. Do not replace `short_path`'s
`text == prefix || text.starts_with(prefix + "/")` with `strip_prefix(prefix)`, which would
also shorten `/usr2` for prefix `/usr` (verified tests cover `/opt/termux/usr/bin/tar`).

**S-04 `splitlines` is not `lines`.**
`cmd_logs` uses `read_text().splitlines()`; Python splits on `\n`, `\r`, `\r\n`, `\v`,
`\f`, `\x1c`–`\x1e`, `\x85`, `U+2028`, `U+2029` [PY-STR-SPLITLINES]. Verified:
`"a\x0bb\x0cc\x1cd\x85e\u2028f\r\ng".splitlines()` yields 7 pieces. Rust `str::lines`
splits only on `\n` and strips a trailing `\r`. Rule: implement `py_splitlines` once and use
it in `cmd_logs` and in the `dpkg --get-selections` parser (`stdout.splitlines()`, where the
difference is harmless but the helper keeps behavior uniform).

**S-05 `bytes` vs `str` and non-UTF-8 names.**
Python decodes file names with the filesystem encoding and `surrogateescape`, so arbitrary
bytes round-trip through `open()`; `os.fsencode` re-encodes them. Rust `Path`/`OsStr` hold
bytes and `to_string_lossy()` is lossy. Rules:
- Never build a `Path` from a `String` after reading a directory entry; keep `OsString`
  until a human-readable form is needed.
- Log and event fields are strings; use `to_string_lossy()` there, which is where Python
  would have produced lone surrogates and then `json.dumps` would have failed. Since
  `EventWriter.emit` swallows all exceptions, the Python behavior is "that event line is
  silently dropped". Do not make it fatal.
- Tar member names from `tar-rs` are bytes; decode as UTF-8, and if invalid, keep the lossy
  form for safety checks only after the byte-level `..`/absolute checks (P-06). Python would
  have decoded with surrogateescape and still rejected `..` structurally.

**S-06 Regular expressions.**
Four patterns:
- `_clean_label`: `[^\w.-]+` → `_`, then `strip("_")`. Python `\w` is Unicode word
  characters (letters, digits, underscore) by default; Rust `regex` `\w` is also Unicode by
  default [REGEX-CRATE]. `-` is literal at the end of the class. Same match set; verified
  `_clean_label("before reset") == "before_reset"`, `/` never survives.
- `_PKG_NAME_RE`: `[A-Za-z0-9][A-Za-z0-9+._-]*` used with `fullmatch`. Rust `^...$` is
  end-of-text, but use `\A...\z` to be explicit and avoid any `$`-before-final-newline
  assumption. Verified: `pkg install -y -- git` runs, option-like names are recorded as
  failed, never passed to `pkg` (`tests/test_audit_fixes.py::test_install_packages_refuses_option_like_names`).
- `_RCLONE_TRANSFERRED`: `Transferred:\s+([\d.]+)\s*([A-Za-z]+)\s*/\s*([\d.]+)\s*([A-Za-z]+)`
  with `.search` per stderr line. Rust `Regex::captures` per line is equivalent; `\s` is
  Unicode in both. `.` in Python does not match `\n`; irrelevant per line.
- `chunk_re`: `^(.+)\.part(\d{4})$` applied with `.match` to file names. Python `$` also
  matches before a trailing newline, so a name `x.part0001\n` matches. Preserve with
  `(?s)`-free `^(.+)\.part(\d{4})\n?$`, or reject newline-containing names earlier (they are
  impossible from `read_dir` on Unix? they are possible). Keep the Python-visible behavior:
  use the `\n?$` form.
`isalpha` is not used anywhere in this codebase; the only letter checks are the ASCII class
above and `str.isdigit()` in `cmd_setup`.

**S-07 `isdigit()` + `int()` accept Unicode digits.**
`raw.isdigit()` is true for non-ASCII decimal digits and `int("٣") == 3`. Rust
`parse::<u32>()` rejects them. If exactness matters (setup prompt validation), either accept
the divergence in a recorded case or normalize Unicode `Nd` digits first. `int()` also
accepts surrounding whitespace and `_` separators (`int(" 12 ") == 12`, `int("1_0") == 10`,
verified) [PY-INT]; `" 12 ".isdigit()` is false, so the code's `isdigit` gate already blocks
those paths before `int()`.

---

## 4. JSON

**J-01 Key order.**
Python dicts preserve insertion order (language guarantee since 3.7), so `json.dump` writes
`DEFAULT_CONFIG` keys in declaration order and manifest keys in the order the code assigns
them. `serde_json::Map` is a `BTreeMap` by default; the `preserve_order` feature switches it
to `IndexMap` [SERDE-FEATURES]. Rules:
- Enable `serde_json/preserve_order` in the core crate.
- Prefer structs with fields declared in Python's insertion order for `manifest.json`,
  `config.json`, `cron.json`, and event payloads; serde writes fields in declaration order.
- Config may contain unknown user keys (loaded then re-saved); those must be preserved, and
  a struct loses them. Use an order-preserving map for config.
- `EventWriter` builds `{"v":1,"event":...,"ts":...}` then `payload.update(fields)`: order is
  `v, event, ts, <fields in call order>`. Reproduce that order in one builder.

**J-02 `ensure_ascii`.**
`json.dump`/`json.dumps` default `ensure_ascii=True`: non-ASCII becomes `\uXXXX` (lowercase
hex, surrogate pairs above BMP) [PY-JSON-DUMP]. Verified:
`json.dumps({"label": "café"})` → `{"label": "caf\u00e9"}`. The events writer explicitly
passes `ensure_ascii=False`, so event JSON is raw UTF-8. Rules:
- On-disk JSON (`config.json`, `manifest.json`, `cron.json`, export file): ASCII-escaped, to
  keep files byte-comparable with recorded cases and with the Python app's own output.
  Implement a `serde_json::ser::Formatter` override for byte strings, or post-process only
  string values (not valid: escapes can appear inside strings only, but doing it on the
  serialized text is unsafe). The formatter approach is the correct one.
- Event JSON: raw UTF-8, `separators=(",", ":")` → no spaces, one line + `\n`, flushed
  immediately. `serde_json::to_writer` is compact by default.

**J-03 Numbers.**
`json.loads("1.0")` is `float`, `json.loads("<huge int>")` is exact `int` (verified).
`serde_json` maps JSON numbers to `i64`/`u64` when possible and `f64` otherwise; with
`arbitrary_precision` it keeps the literal. Rules:
- Manifest `size` values are written by this app as integers (`stat().st_size`, `sink.total`)
  but read back through `int(...)` in `_verify_local_backup`, `size` sums, and
  `_manifest_shape` does not type-check `size`. Read as `serde_json::Value` and coerce with
  a `py_int` helper (integer → as-is, float → truncate toward zero, string → reject) to
  reproduce `int()` [PY-INT].
- `max_backups` from a hand-edited config can be a float; Python then raises
  `TypeError: slice indices must be integers` inside `delete_old`, exit code 1. Do not
  silently round. Decide explicitly: reject at load with a clear error (recommended) and
  record the case, because "match the crash" is not worth reproducing.
- `serde_json` serializes `NaN`/`Infinity` as `null`; Python writes bare `NaN`/`Infinity`,
  which is invalid JSON (verified: `json.dumps({"x": nan})` → `{"x": NaN}`). No code path
  produces floats in persisted JSON, so assert that in the writer (fail the build/test if a
  float appears) rather than special-casing.

**J-04 Pretty-print shape.**
Verified Python output for `json.dumps({"b":1,"a":2}, indent=2)`:
```
{
  "b": 1,
  "a": 2
}
```
Python's default separators change to `(',', ': ')` when `indent` is not `None`, lists get one
item per line, and empty containers stay `{}`/`[]` [PY-JSON-DUMP]. `serde_json::to_string_pretty`
matches this shape (two spaces, `": "`, `",\n"`, inline empties). Differences to test:
trailing newline (Python `json.dump` writes none; `write_manifest` adds none; the log line
does add `\n`), and file ending. Golden test both directions: read a Python-written file,
write it back, diff.

**J-05 Parse errors.**
[PY-JSON-ERROR] documents `msg`, `lineno`, `colno`, `pos`. Verified `json.loads("{not json")` →
`msg='Expecting property name enclosed in double quotes'`, `lineno=1`, `colno=2`, `pos=1`.
`serde_json::Error` exposes `line()` and `column()`; its own docs warn that column can be
`0` for an I/O failure at a line start [SERDE-COLUMN]. Rules:
- `load_config` prints `line {L}, column {C}`; keep the exact phrase and substitute
  serde's numbers, clamping `column` to at least 1.
- `cmd_import_config` prints `Line {L}, column {C}: {msg}`. Python's `msg` strings are not
  reproducible; substitute a stable serde-derived message and update the recorded case.
  Tests only assert the title and `"line 1"` (`tests/test_config.py`), so keep the line
  number contract and treat the wording as ADR-003 "human text".
- `json.load` accepts UTF-8/16/32 with BOM via `detect_encoding` [PY-JSON]. `serde_json::from_slice`
  rejects a UTF-8 BOM. Strip `EF BB BF` before parsing to keep a BOM-prefixed config working.

**J-06 Python `str()` coercion of JSON values.**
`_clean_backup_name`/`_valid_backup_name`/`_clean_label` call `str(name)` first. Consequences
on attacker-controlled or hand-edited manifests: a numeric `name` becomes `"123"` and is
considered valid; `True` becomes `"True"`, `None` becomes `"None"`, a list becomes
`"[1, 2]"`. Rust `Value::to_string()` gives `true`, `null`, `[1,2]`. Rule: a `py_str(&Value)`
function with Python's spellings, used by the name/label sanitizers and by the `list` event
fields (`str(backup.get("name",""))` etc.). Otherwise a crafted manifest changes validity.

**J-07 Encoding of files read as text.**
`open()`/`read_text()` use `locale.getpreferredencoding(False)`; on Termux this is UTF-8.
`cmd_logs` and `read_manifest` therefore fail on invalid UTF-8 in Python only if the locale
is not UTF-8. Rule: read as UTF-8 with a BOM strip for JSON; for the log, read lossily
(`String::from_utf8_lossy`) and document the difference, because a corrupt log should still
be viewable.

---

## 5. Time and datetime

**T-01 `now()` is naive local time, not UTC.**
`datetime.datetime.now()` returns local wall-clock without tzinfo. Used for backup names,
manifest `date`, log timestamps, and the daemon's next-run math. Rule: `chrono::Local::now().naive_local()`
[CHRONO-LOCAL]; never `Utc::now()`. Golden test: compare the first 10 characters with
`date +%Y-%m-%d` under `TZ=Pacific/Auckland` and `TZ=UTC`.

**T-02 `%Y%m%d_%H%M%S` naming.**
`datetime.now().strftime("%Y%m%d_%H%M%S")` → `20261124_130501`, no separators, zero-padded,
24-hour clock [PY-STRFTIME]. `chrono` uses the same `%Y %m %d %H %M %S` specifiers, but
`%Y` padding differs for years < 1000 (not reachable). Rule: format the name with the
literal template `%Y%m%d_%H%M%S`; then `termux_backup_<stamp>` plus `_<clean_label>` when a
label survives cleaning. Do not add uniqueness, millisecond suffix, or a retry loop:
`tests/test_commands.py` asserts `name.endswith("_before_reset")` and storage tests rely on
`termux_backup_` prefix sorting.

**T-03 `isoformat()` precision.**
`datetime.now().isoformat()` omits the fractional part when microsecond is 0 and prints
exactly six digits otherwise: `2026-01-02T03:04:05`, `2026-01-02T03:04:05.123000`
(verified) [PY-DATETIME-ISO]. Rule:
```
if micros == 0 { format("%Y-%m-%dT%H:%M:%S") } else { format("%Y-%m-%dT%H:%M:%S%.6f") }
```
Do not use `to_rfc3339()` (adds an offset) or `%.f` (variable length). Display code then
takes `head(date, 16)` (cards) or `head(date, 19)` (panels) and replaces `T` with a space;
`tests/test_formatting.py::test_backup_date_trims_iso_timestamp` fixes the 16-character rule.

**T-04 Log timestamps.**
`log()` writes `[{ts}] [{LEVEL}] {message}\n` with `ts = now().strftime("%Y-%m-%d %H:%M:%S")`
and `LEVEL.upper()`. `tests/test_config.py` asserts lines *end with* `[INFO] first`, so the
prefix exactness is not tested but the suffix is. Keep one space inside both brackets, one
space between them, no timezone, no milliseconds. Appends only; never rewrite the file
except `clear-logs`, which truncates to empty.

**T-05 Elapsed time.**
`fmt_elapsed(seconds)` = `int(seconds)`, then
`f"{s // 3600}:{s % 3600 // 60:02d}:{s % 60:02d}"` [PY-INT]. Verified: `0 → "0:00:00"`,
`3725.8 → "1:02:05"`. Hours are not zero-padded. Rules:
- `int()` truncates toward zero; `as i64` matches for positives.
- `//` and `%` **floor** in Python; Rust `/` and `%` truncate. For negative input (clock
  skew is possible with `time.monotonic()`? no, but the function is public and tested) the
  outputs differ: Python `-1 → "-1:59:59"`, Rust truncating math gives `"0:00:-1"`.
  Use `div_euclid`/`rem_euclid` to match.
- Durations use `time.monotonic()` (`duration_ms = int((monotonic - t0) * 1000)`), never wall
  clock. In Rust: `Instant::elapsed().as_millis() as i64`, which truncates the same way.

**T-06 Daemon scheduling.**
`target = now.replace(hour=h, minute=m, second=0, microsecond=0)`; if `target <= now`,
`target += timedelta(days=1)`; `sleep_sec = (target - now).total_seconds()`; then
`time.sleep(sleep_sec)`. `timedelta(days=1)` is exactly 24 h of naive arithmetic (DST changes
are ignored), and the pattern is recomputed after every wake. Rule: `NaiveDateTime` +
`Duration::days(1)` + `signed_duration_since`, then `thread::sleep(Duration::from_secs_f64(s))`;
do not use a cron crate, an absolute UTC instant, or an alarm. Reload the schedule after the
sleep and skip the backup if `enabled` became false
(`tests/test_audit_fixes.py::test_daemon_skips_backup_when_disabled_during_wait`).

**T-07 Event timestamps.**
`int(time.time() * 1000)` = Unix epoch milliseconds, truncating; `EventWriter` puts it in
`ts`. Wall clock, so it can go backwards. Rule: `SystemTime::now().duration_since(UNIX_EPOCH).as_millis() as i64`.
No monotonic substitution: the TUI may sort by `ts`.

---

## 6. Numbers and formatting

**N-01 `human_size`.**
```python
size_bytes = int(size_bytes)
for unit in ("B","KB","MB","GB"):
    if size_bytes < 1024: return f"{size_bytes:.1f} {unit}"
    size_bytes /= 1024
return f"{size_bytes:.1f} TB"
```
Rules:
- `int()` truncates (`human_size(2048.9) == "2.0 KB"`, verified by test). If the Rust
  signature is `u64`, that test cannot be ported; keep a `f64` entry point for internal
  parity or record the dropped case. All callers pass integers.
- Comparison `size_bytes < 1024` happens on the *float* after the first division. Floating
  point can produce surprises at exact boundaries; verified boundaries are `1023 → "1023.0 B"`,
  `1024 → "1.0 KB"`, `1024^4 → "1.0 TB"`, `2*1024^4 → "2.0 TB"`.
- `{:.1}` rounds the binary value, ties to even in CPython's dtoa; Rust's `{:.1}` is also
  correctly rounded with ties to even [PY-FORMAT][RS-FMT]. Same for all test values; add a
  property test over the corpus `0..=2*1024^4` comparing against the Python helper.
- Size-1 values: `1.0 B`, never `1 B`; the space is a normal space.

**N-02 Percentage bar.**
`pct = part / total if total else 0`; `filled = int(width * pct)`; color `green < 0.5`,
`yellow < 0.8`, else `red`; label `f"{pct*100:.1f}%"`; 24-column default, `int(width*pct)`
truncates. Verified: `(50,100,width=10)` → 5 filled, 5 empty; `(5,0)` → all empty, `0.0%`.
Rules: `f64` division, `as usize` truncation (same for non-negative), thresholds strict `<`,
and the exact `█`/`░` characters. `tests/test_formatting.py` asserts the thresholds and
counts, not the escape codes.

**N-03 Integer division.**
`//` appears in `fmt_elapsed`, `_emit_package_diff` (`(len+chunk-1)//chunk`, chunk 500), and
`x % 3600 // 60`. All operands are non-negative, so `usize` division is identical. Keep
`div_euclid` only where negatives are reachable (elapsed formatting). `max(1, ...)` in
`_emit_package_diff` guarantees one part for an empty list, but the empty case returns early
with `parts=1` anyway; preserve both paths (`tests/test_events.py` asserts 1201 → 3 parts of
500/500/201).

**N-04 `_bytes_from_rclone`.**
`int(float(value) * factor)` with factors `{B:1, Bytes:1, KiB:1024, MiB:1024^2, GiB:1024^3,
TiB:1024^4, kB:1000, MB:1000^2, GB:1000^3, TB:1000^4}`; unknown unit raises
`ValueError("unknown rclone unit: ...")`. Rules:
- The regex guarantees `[\d.]+`, so `float()` can still fail on `"1.2.3"` or `"."`; Python
  raises `ValueError`, which `_explain` renders as "Operation failed", exit code 1. Map parse
  failure to the same error category.
- `float()` accepts `nan`/`inf`/scientific notation and underscores; the regex excludes all
  but underscores? no — regex excludes letters and `_`, so only digits and dots reach
  `float`. Still use `f64::from_str`, then truncate with `as i64` for positives; guard
  non-finite and negative to avoid a saturating cast (Python would raise `OverflowError` for
  `int(inf)`).
- Unit lookup is case-sensitive: `kB` is 1000, `KiB` is 1024, `KB` is *unknown* (not in the
  map) → error. Reproduce the exact set, including the `Bytes` spelling.
- The regex is not anchored, so rclone's `Transferred:   1.234 MiB / 5.678 MiB, 21%, ...`
  matches anywhere in the line and the second unit may differ (`MiB` current, `GiB` total).

---

## 7. `difflib.get_close_matches`

**D-01 Call site.**
Only used in `FriendlyParser.error` for an unknown command, `n=3, cutoff=0.6`, skipped when
the bad word is in `COMMAND_HINTS`. Output is joined with three spaces inside a "Did you
mean:" hint line. Verified results on the real command list:
`backpu → [backup]`, `restor → [restore]`, `lst → [list]`, `export_config → [export-config,
import-config]`, `clear-cach → [clear-cache, clear-logs]`, `hlep → [help]`,
`config → [import-config, export-config]`, `sync → []`, `upgrade → []`.

**D-02 Algorithm, exactly.**
`get_close_matches` sets sequence 2 to the word, sequence 1 to each candidate, filters with
`real_quick_ratio() >= cutoff and quick_ratio() >= cutoff and ratio() >= cutoff`, appends
`(ratio, candidate)` and returns `_nlargest(n, result)` stripped of scores
[PY-DIFFLIB-SRC]. Consequences:
- `ratio = 2*M/T` where `T = len(a)+len(b)` and `M` is the sum of `get_matching_blocks`
  sizes; for empty sequences `T == 0` → ratio 1.0.
- `M` is *not* the LCS length; matching blocks come from the recursive
  `find_longest_match` (Ratcliff/Obershelp) with a stable tie-break "earliest in `a`, then
  earliest in `b`".
- `autojunk` only activates for sequences of length ≥ 200; command names are shorter, so it
  is irrelevant here. Do not port it if you only need this call site.
- Ties: `heapq.nlargest` on `(ratio, candidate)` tuples compares the candidate strings when
  ratios are equal, so equal scores come out in *descending* code-point order. Verified
  `config` ties between `import-config` and `export-config` and returns `import-config`
  first even though `export-config` comes earlier in `COMMANDS`.
- Python compares code points; Rust `&str` `Ord` compares bytes. Equal for UTF-8 ordering of
  code points (UTF-8 preserves code-point order) [RS-ORD].

**D-03 Port options.**
- (a) Port `SequenceMatcher.find_longest_match/get_matching_blocks/ratio` (roughly 60 lines,
  no junk support needed) plus the tuple tie-break and `sort` + `truncate`. Exact, testable
  against the §13 table, and independent of a third-party algorithm.
- (b) Use the `difflib` crate. It has `get_close_matches(word, possibilities, n, cutoff)` and
  `SequenceMatcher` [DIFFLIB-CRATE], but it uses `f32` ratios and a stable ratio-only sort
  [crate source], so tie order and 0.6-boundary cases can differ (`f32` rounding can move a
  value across `0.6`). If used, pin the crate, add the §13 goldens, and record any divergence.
- (c) Reject the feature and hardcode hints. Not acceptable: ADR-003 keeps behavior-bearing
  wording.
Recommendation: (a). The call site is small, the algorithm is fully specified by the source,
and the tests in ADR-008 want exact internal rules.

---

## 8. OS services

**O-01 `disk_usage`.**
`shutil.disk_usage(path)` = `statvfs`: `total = f_blocks*f_frsize`, `used =
(f_blocks-f_bfree)*f_frsize`, `free = f_bavail*f_frsize` [PY-SHUTIL-SRC]. `begin_stream`
requires `free >= int(CHUNK_SIZE * 1.2)` = `int(200 MiB * 1.2)` = `251658240` bytes
(200 MiB = 209715200; ×1.2 = 251658240 exactly, so `int` is exact). Rules:
- Use `nix::sys::statvfs::statvfs` and the same fields [NIX-STATVFS]; do not compute
  `available = total - used`, which overstates space reserved for root.
- `statvfs` follows symlinks; `work_dir` exists by then (`create_dir_all` first).
- Error text uses `human_size(free)` and `human_size(need)`;
  `tests/test_storage.py::test_github_begin_stream_requires_free_space` injects `free=1` and
  asserts the title `"Not enough free space"`, not the numbers. Add a recorded case with
  both numbers.

**O-02 `which`.**
`shutil.which(name)` POSIX rules [PY-SHUTIL-SRC]: a name containing `/` is checked directly;
otherwise split `PATH` on `:`; **an empty `PATH` returns `None`** (explicit code path, no
fallback to `os.defpath`); `PATH` unset falls back to `confstr("CS_PATH")` then `os.defpath`;
each candidate is checked with `access(F_OK|X_OK)`; duplicate directories are skipped by
`normcase` (a no-op on POSIX, so duplicates are *not* skipped on POSIX:
`os.path.normcase` is identity). Rules:
- Implement the search yourself (20 lines) or use `which` and verify the empty-`PATH` and
  unset-`PATH` cases.
- Do not use shell lookup, `command -v`, or `PATH` entries split on whitespace.
- `cmd_status` calls `which` for `python3`, `dpkg`, `tar`, `rclone`, `git`, `curl` and shows
  the resolved path, so a wrong lookup changes visible output. Keep checking `python3` during
  the port even though Rust does not need it (recorded output); revisit at lift.

**O-03 `tempfile.gettempdir()`.**
`_tmp_root` order: `TMPDIR` if non-empty → `$PREFIX/tmp` if it is a directory → else
`tempfile.gettempdir()`. `gettempdir()` itself tries `TMPDIR`, `TEMP`, `TMP` then
`/tmp`, `/var/tmp`, `/usr/tmp`, `cwd`, requires the directory to exist and be writable, and
**caches the first result** [PY-TEMPFILE]. Rust `std::env::temp_dir()` reads `TMPDIR` then
`/tmp` with no existence check, no fallback chain, no cache [RS-TEMPDIR]. Rule: keep the
explicit three-step `_tmp_root` and implement the Python fallback chain for the last step,
including the writability probe; do not let `std::env::temp_dir()` decide. Tests monkeypatch
`TMPDIR` (`tests/conftest.py`) and assert all three branches
(`tests/test_audit_fixes.py::test_tmp_root_*`).

**O-04 `os.execv` (bootstrap only).**
`os.execv(sys.executable, [sys.executable] + sys.argv)` replaces the process image to rerun
after installing `rich`, keeping pid, fds, and signal dispositions [PY-EXECV]. Rust has no
equivalent need (no runtime dependency bootstrap). If the front door ever re-execs the Python
fallback, use `std::os::unix::process::CommandExt::exec` [RS-EXEC] (not `spawn`) so the exit
code and signal identity are preserved; pass argv unchanged (ADR-006), including `argv[0]`
choices.

**O-05 Signals on Android.**
- Python turns `SIGINT` into `KeyboardInterrupt`; `main` catches it, prints the "Operation
  cancelled" panel, emits `{"event":"cancelled","reason":"user"}` and exits **130**
  [PY-SIGNAL]; tests assert 130 and the text.
- Restore wraps extraction in `signal.signal(SIGINT, SIG_IGN)` and restores the previous
  handler afterwards, because a partial restore is worse than an interrupted one. Ignored
  signals are discarded, not queued; do not deliver a stale one on restore.
- `SIGTERM`/`SIGHUP` are not handled anywhere. On Android, Termux sends `SIGINT` for Ctrl+C
  and `SIGKILL` when the process is reaped; do not add a `SIGTERM` handler that changes exit
  codes or cleanup, or a recorded case can flip.
- Rust rules: install handlers with `signal-hook`/`nix` [NIX-SIGNAL]; handler sets an atomic
  flag; the main loop observes it and performs cleanup and `process::exit(130)`; block all
  `SIGINT` delivery to worker threads or route it through the same flag. Never do I/O,
  allocation, or `rmtree` inside the handler. `SIG_IGN` during extract must be the real
  disposition, not "handler that ignores", because a concurrent clear of the flag could let
  a Ctrl+C through between members.
- `handle_crash` calls `sys.exit(1)`; `_abort` calls `sys.exit(130)`. Keep those numbers
  literal.

**O-06 `os.access` (real uid) and unlocked archives.**
`_unlock` tests `os.access(d, R_OK|X_OK)` then chmods `m | 0o500` (dirs) or `m | 0o400`
(files), remembering the original mode, and `_relock` restores in reverse order. `os.access`
uses the *real* uid/gid (not effective) and follows symlinks by default [PY-OS-ACCESS].
Rules: `nix::unistd::access(path, AccessFlags::R_OK|X_OK)` [NIX-ACCESS]; chmod with
`std::fs::set_permissions` from the original `Permissions` (which preserves setuid/setgid/sticky
bits on Unix); restore even when archiving fails (`finally`), in reverse order, swallowing
errors exactly like `_relock`. Do not use `metadata.permissions().readonly()`, which is not
the same predicate.

**O-07 `shutil.rmtree(ignore_errors=True)`.**
Ignores every error, including permission and missing; used for temp cleanup and old-backup
pruning. Rule: recursive remove that ignores errors, including `NotFound`, and does not
follow symlinks (remove the link, not its target). `std::fs::remove_dir_all` follows
symlinks? It removes the link for entries but errors if the top is a symlink; write the
walker explicitly if exactness matters. For `delete_backup` (no `ignore_errors`), errors must
propagate: `LocalStorage.delete_backup` lets `OSError` reach `_explain`; the CLI catches and
reports "Failed to delete backup".

---

## 9. Files, modes, atomicity

**F-01 Config write.**
`save_config`:
```python
fd = os.open(CONFIG_FILE, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with os.fdopen(fd, "w") as f: json.dump(cfg, f, indent=2)
try: os.chmod(CONFIG_FILE, 0o600)
except OSError: pass
```
Rules: `OpenOptions::new().write(true).create(true).truncate(true).mode(0o600)`
[RS-OPENOPT] (the mode is masked by umask, which is why the later `chmod` exists), then
`set_permissions(0o600)` and **ignore** its error — `tests/test_audit_fixes.py` monkeypatches
`os.chmod` to raise and expects the write to succeed. The file is truncated before writing,
so a crash can leave invalid JSON; keep that failure mode unless an ADR changes it (an
atomic temp+rename is a behavior change: a crashed save would leave the old config instead of
a corrupt one). If adopted, it needs its own ADR and recorded case.

**F-02 Other write modes.**
- `write_manifest`, `save_schedule`, `cmd_export_config`: plain `open(..., "w")` → `0o666 &
~umask`. Keep default permissions; do not "harden" these silently, except note that
`cron.json` may be world-readable today (contains only schedule+label) while
`config.json` is 0600 (contains the token). A hardening change is allowed at lift with a
changelog entry.
- `packages.txt`: `write_text("\n".join(pkgs))`, **no trailing newline**, UTF-8. Reproduce
  exactly; it is inside the archive only, but a diff changes archive bytes and therefore
  checksums across implementations.
- `_write_daemon_script`: literal shebang `#!/data/data/com.termux/files/usr/bin/sh`, body
  `python3 "<abs script>" daemon &`, then `chmod 0o755`. At lift the body must call the Rust
  binary, or the boot hook breaks; that is an explicit lift decision, not a silent change.
- `cmd_clear_logs`: `write_text("")` truncates, keeping mode.

**F-03 `Path.unlink(missing_ok=True)`.**
Ignores `FileNotFoundError` only; a directory still raises `IsADirectoryError` [PY-PATHLIB-UNLINK].
Rust `remove_file` returns `NotFound`; map that to `Ok(())` and propagate everything else
[RS-REMOVE]. Used in `_PartWriter._flush` (after upload, in `finally`) and `abort`, and in
`clear-cache` for non-directory children.

**F-04 `copy2` and `copystat`.**
`shutil.copy2` = copy data + `copystat` (mode bits, atime/mtime, flags, and Linux xattrs)
[PY-SHUTIL-SRC]. `_copy_with_sha` writes data itself then calls `copystat(follow_symlinks=False)`.
Rules: copy the mode and nanosecond atime/mtime; xattrs are best-effort and may be skipped
on Android (note it). The `follow_symlinks=False` argument only matters when both source and
destination are symlinks (docs wording: "symlinks aren't followed if and only if both src and
dst are symlinks"); archives are regular files, so copying permissions and times is enough.

**F-05 Same-file detection.**
`src.resolve() == dst.resolve() or (dst.exists() and os.path.samefile(src, dst))` guards
`_copy_with_sha`; `LocalStorage.upload` compares `dest.resolve() == backup_dir.resolve()`.
Rule: compare `(dev, ino)` from `metadata()` for existence-independent equality, and the
lenient resolved paths for the not-yet-existing destination. Do not compare only strings: a
bind mount or hard link must also be refused (that is what `samefile` adds).

**F-06 Atomic renames and part naming.**
`_PartWriter._flush(final=True)` renames the only part from `<base>.part0000` to `<base>`
using `Path.rename` → `os.rename`, which replaces an existing target on Unix [PY-PATHLIB].
Rule: `fs::rename` (same semantics). Part names are `{base}.part{idx:04d}` with four digits,
zero-padded, starting at 0; the GitHub download reassembles names matching
`^(.+)\.part(\d{4})$` and sorts them by name. Any change here breaks old backups (ADR-004).

**F-07 Archive byte equality is not required.**
Python writes gzip with `compresslevel=9` [PY-TARFILE-SRC] and PAX format
(`DEFAULT_FORMAT = PAX_FORMAT`) [PY-TARFILE-SRC]. `flate2` defaults to level 6 and tar-rs uses
`HeaderMode::Complete` with GNU long-name extensions by default [FLATE2][TAR-BUILDER].
Manifest checksums are computed over the bytes this app produced (`sha256(actual_out)`), so
cross-implementation archives differ and that is fine, as long as:
- Rust reads Python archives (`r:*` equivalent: sniff gzip/bzip2/xz), and Python reads Rust
  archives (GNU long names and PAX both parse).
- Set gzip level 9 to keep sizes and the free-space checks comparable.
- Do not add a global PAX header that Python does not write (Python writes one only when
  `pax_headers` is non-empty; this code never sets it).
- Deterministic gzip mtime is an improvement, but it changes archive bytes; it is not
  required and not tested. If added, it needs a recorded-case refresh.

**F-08 `data_filter` on extract.**
`extract_archive` uses `filter="data"` whenever `tarfile.data_filter` exists (3.12+; the
pinned runtime is 3.13). That filter [PY-TARFILE-SRC]:
- strips leading `/` and `os.sep`, then rejects absolute names;
- normalizes names containing `..` and rejects results outside the destination;
- rejects a link member named `.` (would replace the destination);
- masks mode with `0o755`, forces `0o600` for regular files/hardlinks, strips all exec bits
  when the user bit is absent, ignores mode for dirs/symlinks;
- drops uid/gid/uname/gname;
- rejects absolute link targets, normalizes link targets, resolves them and rejects escapes;
- rejects special files.
Then this app's own `_safe_extract_member`/`_check_member_root` add the stricter
`expected_top` jail. Rust must replicate the *filter* as well, otherwise restored files get
owner/exec bits that the Python restore did not set (and a crafted archive can set setuid).
On extraction failure the code retries with `member.uid = os.getuid()`, `member.gid =
os.getgid()`, `set_attrs=False`; mirror that fallback (write data, skip metadata).

---

## 10. Locale, environment, platform

**E-01 Environment variable truthiness.**
- `DEBUG = bool(os.environ.get("TERMUX_SYNC_DEBUG"))`: any non-empty value enables debug,
  including `"0"` and `"false"`. Do not parse as bool.
- `_TERMUX_SYNC_BOOTSTRAP != "1"` is an exact string compare.
- `TMPDIR` uses truthiness (`if env:`), so `TMPDIR=""` is skipped.
- `PREFIX` uses `os.environ.get("PREFIX", default)`, so `PREFIX=""` yields `Path("") == "."`,
  *not* the default. Mirror the difference; do not unify "empty means unset" across all
  variables.
- `HOME` is read by `Path.home()`, `short_path`, `handle_crash` (`str(Path.home())`), and
  `TMPDIR` fallbacks. One accessor, injectable for tests.

**E-02 Locale-independent output.**
Formatting is ASCII (`%.1f`, `%02d`, `{}` of ints), sorting is code-point order, and no
message goes through `gettext`. Rust beats Python here: but do not introduce locale-aware
number formatting (`,` thousands, `,` decimal), case-insensitive sorting, or `NL_LANGINFO`
collation — all three would break recorded output under any `LANG`.

**E-03 Raw bytes from subprocesses.**
`subprocess.run(..., capture_output=True)` without `text=True` (the `pkg install` and `rclone
copy/purge` calls) returns bytes and the code ignores the content; `_rclone_progress` uses
`text=True` and iterates `stderr` lines with the locale encoding, errors strict by default
(3.13: `encoding=None` → locale, `errors=None` → strict). Rust: read bytes for the ignored
cases; for rclone decode lossily, since a decode error in Python would raise
`UnicodeDecodeError` and abort a backup mid-stream. Decide and record; lossy decoding is the
recommended divergence because the progress line is only used for the regex.

**E-04 Android path assumptions.**
`TERMUX_ROOT = /data/data/com.termux/files`; `PREFIX` defaults to
`/data/data/com.termux/files/usr`; `dir_size` swallows `PermissionError`/`OSError`, so a
partially unreadable tree returns a partial sum (no error, no warning). Reproduce the
partial sum; do not surface an error. `dir_size` skips symlinks entirely
(`entry.is_symlink()` first) and uses `lstat` sizes for non-directories.

**E-05 `os.getuid()`/`os.getgid()`.**
Used only in the tar extraction retry. Rust `nix::unistd::getuid()`. On Termux this is the app
uid; setting ownership of extracted files is not permitted anyway, which is why
`set_attrs=False` is the fallback.

---

## 11. CLI surface that shapes the port

**C-01 argparse behavior is observable.**
`FriendlyParser` (add_help=False) defines `command` (`nargs="?"`, `choices=COMMANDS`,
default `"help"`), `target` (positional), and flags `--label/-l`, `--name/-n`, `--lines`
(int, default 40), `--json-events`, `--yes`, `--debug`, `--version/-v`, `--help/-h`
[PY-ARGPARSE]. Behaviors a clap port must consciously match or change:
- Unknown command → exit 2, custom panel, and the regex `invalid choice: '([^']*)'` is parsed
  from argparse's message. In Rust, detect it before clap so the `COMMAND_HINTS` /
  `difflib` branch still runs.
- Unknown option / bad int → exit 2 with "Invalid usage"; argparse's wording is normalized
  (`"argument --lines/"` → `"--lines"`, `"argument "` removed). Rust wording differs;
  ADR-003 covers it, but exit code 2 and the `COMMAND_HINTS` text must survive.
- `--lines=many` and `--lines many` both parse; `--lab` abbreviates `--label` (argparse
  allows unambiguous prefixes by default). clap needs `infer_long_args(true)` to match;
  otherwise a script using `--lab` breaks.
- `-h`/`--help` are explicit store-true flags, so `backup --help` prints the global help and
  exits 0 rather than a per-command help. Do not let clap's built-in `--help` win.
- A second positional (`termux-sync check ~ extra`) is "unrecognized arguments" → exit 2.
- `--version` wins over `--help` and over an unknown command? No: `parse_args` validates
  choices first, so `termux-sync --version backpu` exits 2 before version printing. Order in
  `main`: version, then help, then dispatch. Preserve that validation order.

**C-02 Event protocol is fixed.**
One JSON object per line on stdout, `v:1`, `event`, `ts` (ms), compact separators, raw UTF-8,
secret redaction applied to every string in the payload (recursively for lists/dicts),
secrets = `github_token` and `encrypt_password`, empty secrets skipped, replacement `***`,
write errors swallowed. Human output and Rich progress move to stderr in event mode.
Stage ids and indices are part of the protocol (`preflight 1`, `packages 2`, `archive:home 3`,
`archive:usr_etc 4`, `manifest 5`, `upload 6`, `verify 7`; restore `metadata 1`, `download 2`,
`verify 3`, `extract:<key>` 4/5, `packages 6`, `cleanup 7`), and tests assert specific ids
(`tests/test_events.py`). Keep a single event builder with a fixed field order.

**C-03 stdin confirmation.**
`event_confirmation` reads exactly one line, parses JSON, requires a dict and
`str(value).strip() == "RESTORE"`. A malformed line, an empty line, or EOF are all "declined"
→ `cancelled` event, no error. Rules: read one line with a bounded wait? No — Python blocks
until newline or EOF; keep blocking. Empty stdin (EOF) must decline, not hang or error.

**C-04 Interactive prompts.**
`Prompts` use Rich semantics: `Prompt.ask(..., default=...)` returns the default on empty
input; `Confirm.ask(default=False)` accepts y/n; `choices=[...]` re-prompts until valid;
`password=True, show_default=False` never echoes the token. `tests/test_interactive.py` stubs
`Prompt.ask`/`Confirm.ask`, so a Rust port that reads stdin directly cannot reuse those tests
(the ADR-008 recorded cases cover prompt I/O instead). Keep the validation loops:
storage 1/2/3 (default 3), non-empty local path, non-empty token, `owner/repo` with exactly
one slash and no leading/trailing slash, `max_backups` whole number ≥ 1, exclude patterns
split on `,` then stripped and empties dropped, hour 0–23, minute 0–59.

---

## 12. Port checklist by wave

Every row is a review gate. "Must" means a recorded case or a unit test should fail if the
rule is broken.

### W1 — config, paths, logging, events, errors, formatting helpers

- [ ] `resolve_lenient` (P-01) with tests: missing tail, existing symlink, `..` after
      symlink, loop on 3.13 returns best effort.
- [ ] `strip_prefix`-based `relative_to` and `is_within` (P-02, P-03); test `/a/b2` not
      inside `/a/b`.
- [ ] `expand_user` + injectable `home_dir` (P-04, P-05); tests for `~`, `~/x`, `a~b`, empty
      `HOME`.
- [ ] `py_strip` includes `U+001C–U+001F` (S-02); test the verified string.
- [ ] `head_chars` for `[:16]`, `[:19]`, `[:300]`, `[:12]` (S-01); test emoji + combining
      accent.
- [ ] `py_splitlines` for `cmd_logs` (S-04); test the verified 7-piece string.
- [ ] `human_size` f64/u64 entry, `fmt_elapsed` with `div_euclid`, `_pct_bar` (N-01..N-03);
      port the whole `tests/test_formatting.py` table as Rust tests.
- [ ] JSON writer: `preserve_order`, insertion/field order, `ensure_ascii` escaping (J-01,
      J-02, J-04); golden files compare against Python output for a non-ASCII label.
- [ ] JSON reader: BOM strip, `py_str(&Value)`, `py_int(&Value)` (J-03, J-06).
- [ ] Error mapping: config corruption `line/column`, import-config `Line/column`, EOF
      column clamp (J-05).
- [ ] `log()` exact line format and append semantics (T-04).
- [ ] Event builder field order, redaction recursion, `ts` from epoch ms, compact separators,
      stdout/stderr split, swallow write errors (T-07, C-02).
- [ ] `fmt_elapsed`/date display use local time and the 16/19-character truncations (T-02,
      T-03).

### W2 — archive create/extract, checksums, manifests

- [ ] `create_archive`: compression whitelist fallback to `gz`, extension rewrite rules,
      `arcname = source.name`, exclude wrapping `/{pattern}/`, `on_member` called before the
      exclude decision, chmod unlock/relock in `finally` (F-06, O-06, F-07).
- [ ] gzip level 9, bz2/xz via system libs, stream mode writer (`w|gz` equivalent) (F-07).
- [ ] `sha256` 1 MiB chunks, lowercase hex, empty-file digest (tests exist).
- [ ] `_safe_extract_member`: absolute `/` or `\` prefix rejection, `..` in POSIX parts,
      lenient resolve containment, symlink and hardlink base (`dest` for hardlinks,
      `target.parent` for symlinks), device/fifo rejection (P-01, P-06).
- [ ] `_check_member_root` `expected_top` jail with resolved prefix check and link check.
- [ ] `data_filter` replication: mode mask, forced `0o600`, exec stripping, uid/gid strip,
      link normalization and escape rejection, special-file rejection (F-08).
- [ ] extraction retry with `set_attrs=False` after uid/gid fallback; per-member failures are
      logged and skipped, not fatal (F-08, tests assert skip-and-log).
- [ ] `manifest.json` read/write exact shape and key order; `_manifest_shape` type checks;
      missing checksum → restore refuses (J-01, J-04).

### W3 — local storage and backup/restore orchestration

- [ ] `LocalStorage.upload` same-file guard, per-file checksums, event progress per file,
      `iterdir` filtered to files (F-05).
- [ ] `list_backups`: sort by path descending, skip non-backup dirs and unreadable manifests,
      `PermissionError` tolerated (P-07).
- [ ] `delete_old`: mtime sort, only dirs with a manifest carrying `name`, `rmtree` ignoring
      errors (P-07, O-07).
- [ ] `delete_backup`: `_clean_backup_name` round-trip check, errors propagate.
- [ ] `_storage_excludes` and `_self_excludes`: `expanduser().resolve()`, `relative_to`,
      `as_posix`, `src.name/rel` composition (P-02, P-04); verified expected values are
      `["home/termux-backups"]` and `[".config/termux-sync"]`.
- [ ] Backup name: `termux_backup_%Y%m%d_%H%M%S[_label]`, `_clean_label` Unicode `\w`
      (T-02, S-06).
- [ ] Restore: 7 stages, plan fields, 500-package diff chunks, confirmation, checksum gate
      before extraction, SIGINT ignore window, per-archive failure aggregation, result event
      fields (C-02, O-05).
- [ ] `packages.txt` no trailing newline; `dpkg` parse (`parts[1] == "install"`, arbitrary
      order); `pkg install -y -- <name>` with regex gate (S-06).

### W4 — rclone, GitHub Releases, streaming parts

- [ ] rclone command lines and flags verbatim; stderr progress regex; unit table and
      truncation (N-04).
- [ ] `which("rclone")` check with UserError title `"rclone not found"` (O-02).
- [ ] `CHUNK_SIZE = 200*1024*1024`, `int(CHUNK_SIZE*1.2)` free-space guard from
      `f_bavail*f_frsize` (O-01).
- [ ] Part writer: `write` returns full length, part naming `%04d`, single-part rename to
      plain name, delete after upload, `StreamAbort` wrapping, `total`/`sha` accumulate the
      whole stream (F-06).
- [ ] Split-on-upload: `<= CHUNK_SIZE` returns the original file (no copy).
- [ ] Download join: `^(.+)\.part(\d{4})$`, group by prefix, sort by name, unlink parts.
- [ ] Retries: 8 attempts, `15*(n+1)` s sleeps, 422 `already_exists` treated as success,
      500/429 retryable, ENOSPC re-raised (it must reach `_explain`'s "Device is out of
      space" branch).
- [ ] GitHub headers (`token <tok>`, `application/vnd.github.v3+json`,
      `application/octet-stream`), API paths, release prefix filter `termux_backup_`.
- [ ] Secret redaction covers tokens in events and never prints a saved token in prompts.

### W5 — maintenance, setup, schedule, daemon

- [ ] `status` table rows and `which` probe list, including `python3` during the port (O-02).
- [ ] `logs`: default 40, `--lines`, TRACE hidden unless `--debug`, coloring by substring,
      `splitlines` semantics (S-04).
- [ ] `clear-logs`: size display, confirmation, truncate, then log the new entry.
- [ ] `check`: path aliases (`~`, `$HOME`, `home`, `$PREFIX`, `prefix`, `usr`, `termux`,
      `root`), `expanduser().resolve()` for other inputs, `dir_size` skipping symlinks and
      swallowing errors, percentage bar (N-02, E-04).
- [ ] `clear-cache`: target list, size table, comma selection parsing incl. invalid → abort,
      double confirmation, per-child removal with `missing_ok`, freed-total message.
- [ ] `export-config` redaction, `import-config` REDACTED skip, dict-only check, compression
      validation, 0600 save (F-01, J-02).
- [ ] `schedule`: defaults, validation ranges, `cron.json` pretty JSON, daemon script
      regeneration (F-02); lift decision on the script body.
- [ ] `daemon`: naive local next-run math, 24 h step, reload after sleep, disabled stop
      (T-06).

### W6 — CLI and dispatch

- [ ] Exit codes 0/1/2/130, help for no args, version string `termux-sync v1.3.9` (ADR-007).
- [ ] `FriendlyParser` equivalent: unknown command panel with `COMMAND_HINTS` then
      `get_close_matches` (D-01..D-03), bad int → "Invalid usage".
- [ ] Option parsing equivalence: `--label/-l`, `--name/-n`, `--lines`, `--json-events`,
      `--yes`, `--debug`, `--version/-v`, `--help/-h`, unambiguous long-option prefixes,
      `--flag=value`, second positional rejected (C-01).
- [ ] Event mode enables before dispatch and moves human output to stderr (C-02).
- [ ] KeyboardInterrupt → 130 with panel; `handle_crash` → log ERROR+TRACE then exit 1
      (O-05).

### W7 — TUI

- [ ] Consume the event stream as data only: parse JSON, tolerate unknown fields, never
      depend on key order or whitespace (C-02).
- [ ] Confirmation writer sends a single line `{"confirm":"RESTORE"}`.

---

## 13. Verified goldens (produced on this device, Python 3.13)

Formatting:
```
human_size: 0→"0.0 B" 1→"1.0 B" 1023→"1023.0 B" 1024→"1.0 KB" 1536→"1.5 KB"
            1024^2→"1.0 MB" 5*1024^3→"5.0 GB" 2*1024^4→"2.0 TB" 2048.9→"2.0 KB" (int() truncates)
fmt_elapsed: 0→"0:00:00" 59→"0:00:59" 61→"0:01:01" 3600→"1:00:00" 3725.8→"1:02:05"
_pct_bar(50,100,width=10): 5 "█" + 5 "░", label "50.0%"; (5,0): 10 "░", "0.0%"
Backup date: "2026-10-30T10:41:55.123"[:16] → "2026-10-30 10:41"
```
`_clean_label`: `"before reset" → "before_reset"`; no `/` survives from `"../a/b"`.
`short_path`: prefix `/opt/termux/usr` → `/opt/termux/usr/bin/tar` → `$PREFIX/bin/tar`;
`/opt/termux/usr` → `$PREFIX`; home `X/.config/app` → `~/.config/app`; `/etc/hosts`
unchanged.

Names:
```
Path("./a").parts        == ("a",)
Path("a//b").parts       == ("a","b")
Path("a\\b").parts       == ("a\\b",)
Path("a/../b").parts     == ("a","..","b")
Path("//x/y").parts      == ("//","x","y")
Path("").parts == ()     Path(".").parts == ()     Path("x/").parts == ("x",)
os.path.basename("../etc") == "etc"    os.path.basename("a\\b") == "a\\b"
```

Strings/JSON:
```
"\x1c x \x1f".strip() == "x"; "\x1c".isspace() is True; "\x0b".isspace() is True
"a\x0bb\x0cc\x1cd\x85e\u2028f\r\ng".splitlines() == ["a","b","c","d","e","f","g"]
json.loads("{not json") → msg "Expecting property name enclosed in double quotes", line 1, col 2
json.dumps({"label":"café"}) → {"label": "caf\u00e9"}
json.dumps({"v":1,"event":"x","ts":2}, separators=(",",":")) → {"v":1,"event":"x","ts":2}
datetime(2026,1,2,3,4,5).isoformat() → "2026-01-02T03:04:05"
datetime(...,123000).isoformat() → "2026-01-02T03:04:05.123000"
```

`get_close_matches(word, COMMANDS, n=3, cutoff=0.6)`:
```
backpu → [backup]            restor → [restore]          lst → [list]
delet  → [delete]            hlep   → [help]              sync → []
export_config → [export-config, import-config]
clear-cach    → [clear-cache, clear-logs]
config        → [import-config, export-config]
```
Tie evidence: `config` vs `import-config` and vs `export-config` both score
`0.631578947368421`; descending candidate-string order puts `import-config` first.

Helpers worth keeping as Rust literals: `_clean_backup_name("../../etc") == "etc"`,
`_clean_backup_name("..") == ""`, `_storage_excludes({"storage":"local","local_path":"home/termux-backups"}, home) == ["home/termux-backups"]`,
`_self_excludes(home) == [".config/termux-sync"]`, `_backup_size({"files":None}) == 0`.

---

## 14. Decisions this research asks for

These are not settled by the code; they need an ADR or a recorded-case decision before the
relevant wave closes.

1. **difflib strategy** (D-03): port the algorithm (recommended) vs the `difflib` crate with
   golden coverage.
2. **Config write atomicity** (F-01): keep truncate-in-place (current) vs temp+rename. The
   latter changes crash behavior and needs an ADR.
3. **`max_backups` wrong type** (J-03): reproduce exit 1 / "Unexpected error", or reject at
   load. Rejecting is safer; record the case either way.
4. **`status` python3 row** (O-02): keep for parity during the port; at lift, decide whether
   the row stays (checks an unused tool) or moves to a "compat" note.
5. **Daemon boot script body** (F-02): keep `python3 ...` for byte compatibility, or switch
   to `termux-sync` at lift. The file path must not change.
6. **Gzip determinism** (F-07): level 9 is required for comparable sizes; mtime/filename in
   the gzip header can stay Python-like (nondeterministic) or be zeroed (new bytes).
7. **Non-UTF-8 file names and rclone stderr** (S-05, E-03): lossy decode is recommended;
   record the divergence from Python's strict decode.

---

## 15. Sources

Python documentation and source (3.12 branch used for source reading; runtime is 3.13):

- [PY-PATHLIB] `pathlib` — https://docs.python.org/3/library/pathlib.html
- [PY-PATHLIB-RESOLVE] `Path.resolve` — https://docs.python.org/3/library/pathlib.html#pathlib.Path.resolve
- [PY-PATHLIB-PARTS] `PurePath.parts` — https://docs.python.org/3/library/pathlib.html#pathlib.PurePath.parts
- [PY-PATHLIB-PARENTS] `PurePath.parents` — https://docs.python.org/3/library/pathlib.html#pathlib.PurePath.parents
- [PY-PATHLIB-RELATIVE] `PurePath.relative_to` — https://docs.python.org/3/library/pathlib.html#pathlib.PurePath.relative_to
- [PY-PATHLIB-UNLINK] `Path.unlink` — https://docs.python.org/3/library/pathlib.html#pathlib.Path.unlink
- [PY-EXPANDUSER] `os.path.expanduser` — https://docs.python.org/3/library/os.path.html#os.path.expanduser
- [PY-OS-PATH] `os.path` — https://docs.python.org/3/library/os.path.html
- [PY-STR] `str` (sequence semantics) — https://docs.python.org/3/library/stdtypes.html#text-sequence-type-str
- [PY-STR-ISSPACE] `str.isspace` — https://docs.python.org/3/library/stdtypes.html#str.isspace
- [PY-STR-STRIP] `str.strip` — https://docs.python.org/3/library/stdtypes.html#str.strip
- [PY-STR-SPLITLINES] `str.splitlines` — https://docs.python.org/3/library/stdtypes.html#str.splitlines
- [PY-INT] numeric types (`int`, `//`, `%`) — https://docs.python.org/3/library/stdtypes.html#numeric-types-int-float-complex
- [PY-FORMAT] format specification mini-language — https://docs.python.org/3/library/string.html#format-specification-mini-language
- [PY-JSON] `json` — https://docs.python.org/3/library/json.html
- [PY-JSON-DUMP] `json.dump` (`indent`, `ensure_ascii`, separators) — https://docs.python.org/3/library/json.html#json.dump
- [PY-JSON-ERROR] `json.JSONDecodeError` — https://docs.python.org/3/library/json.html#json.JSONDecodeError
- [PY-DATETIME-ISO] `datetime.isoformat` — https://docs.python.org/3/library/datetime.html#datetime.datetime.isoformat
- [PY-STRFTIME] `strftime` format codes — https://docs.python.org/3/library/datetime.html#strftime-and-strptime-format-codes
- [PY-DIFFLIB-SRC] CPython `Lib/difflib.py` — https://github.com/python/cpython/blob/3.12/Lib/difflib.py
- [PY-SHUTIL-SRC] CPython `Lib/shutil.py` (`disk_usage`, `which`, `copystat`) — https://github.com/python/cpython/blob/3.12/Lib/shutil.py
- [PY-TARFILE-SRC] CPython `Lib/tarfile.py` (`DEFAULT_FORMAT`, `gzopen`, `_get_filtered_attrs`) — https://github.com/python/cpython/blob/3.12/Lib/tarfile.py
- [PY-TEMPFILE] `tempfile.gettempdir` — https://docs.python.org/3/library/tempfile.html#tempfile.gettempdir
- [PY-OS-ACCESS] `os.access` — https://docs.python.org/3/library/os.html#os.access
- [PY-OS-OPEN] `os.open` — https://docs.python.org/3/library/os.html#os.open
- [PY-SIGNAL] `signal` — https://docs.python.org/3/library/signal.html
- [PY-EXECV] `os.execv` — https://docs.python.org/3/library/os.html#os.execv
- [PY-ARGPARSE] `argparse` — https://docs.python.org/3/library/argparse.html

Rust standard library and crates:

- [RS-CANON] `std::fs::canonicalize` — https://doc.rust-lang.org/std/fs/fn.canonicalize.html
- [RS-TEMPDIR] `std::env::temp_dir` — https://doc.rust-lang.org/std/env/fn.temp_dir.html
- [RS-REMOVE] `std::fs::remove_file` — https://doc.rust-lang.org/std/fs/fn.remove_file.html
- [RS-OPENOPT] `OpenOptionsExt::mode` (umask) — https://doc.rust-lang.org/std/os/unix/fs/trait.OpenOptionsExt.html
- [RS-SLICE] `str` slice indexing (panics on non-boundary) — https://doc.rust-lang.org/std/primitive.str.html#slice-index
- [RS-ISWHITESPACE] `char::is_whitespace` — https://doc.rust-lang.org/std/primitive.char.html#method.is_whitespace
- [RS-TRIM] `str::trim` — https://doc.rust-lang.org/std/primitive.str.html#method.trim
- [RS-ORD] `Ord for str` (byte order) — https://doc.rust-lang.org/std/cmp/trait.Ord.html
- [RS-EXEC] `CommandExt::exec` — https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html#tymethod.exec
- [RS-FMT] `std::fmt` precision (rounded) — https://doc.rust-lang.org/std/fmt/index.html
- [SERDE-FEATURES] `serde_json` feature flags (`preserve_order`) — https://docs.rs/serde_json/latest/serde_json/#feature-flags
- [SERDE-COLUMN] `serde_json::Error::column` — https://docs.rs/serde_json/latest/serde_json/struct.Error.html#method.column
- [TAR-BUILDER] `tar::Builder` (`HeaderMode`) — https://docs.rs/tar/latest/tar/struct.Builder.html
- [FLATE2] `flate2::Compression` — https://docs.rs/flate2/latest/flate2/struct.Compression.html
- [DIFFLIB-CRATE] `difflib::get_close_matches` — https://docs.rs/difflib/latest/difflib/fn.get_close_matches.html
- [REGEX-CRATE] `regex` Unicode classes — https://docs.rs/regex/latest/regex/#unicode
- [NIX-STATVFS] `nix::sys::statvfs::statvfs` — https://docs.rs/nix/latest/nix/sys/statvfs/fn.statvfs.html
- [NIX-ACCESS] `nix::unistd::access` — https://docs.rs/nix/latest/nix/unistd/fn.access.html
- [NIX-SIGNAL] `nix::sys::signal` — https://docs.rs/nix/latest/nix/sys/signal/index.html
- [CHRONO-LOCAL] `chrono::Local` — https://docs.rs/chrono/latest/chrono/offset/struct.Local.html

---

## 16. Confidence and limits

- Every behavior above was read from the repository and cross-checked against the cited
  source or documentation. §13 values were executed, not recalled.
- The Python version pinned by the repository is 3.13; source citations point at the 3.12
  branch because the relevant functions are unchanged for 3.10+ except
  `Path.resolve`'s symlink-loop change (noted in P-01) and `tarfile`'s filter support
  (noted in F-08).
- Not verified by execution: `statvfs` field values on this specific Android filesystem,
  `os.access` behavior under Termux's per-app uid model, exact `chrono` formatting for
  microsecond `%f`, and `serde_json` column counting for multibyte input. Each has a
  dedicated test in the checklist; treat them as "verify on device" rather than assumed.
- `rust/app-cases/` does not exist yet (ADR-008), so no recorded case was available to
  compare; §13 is the substitute corpus for the rules it covers.
