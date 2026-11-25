# CLI parse and output parity: Rust, no Rich, on Termux

Research for the Python → Rust port. Read: `termux-sync.py` (argparse block at 3140-3326,
`print_help`, `FriendlyParser`, all `console.*` / `Prompt.*` / `Confirm.*` sites),
`tui/src/{main,app,data,ops}.rs`, `rust/DECISIONS.md`.

Every claim carries a reference number; the list is at the bottom. Findings marked
**Observed** were produced by importing the real module and calling `build_parser()`
with a captured stdout, not by reading code alone.

---

## 0. Verdict

| Question | Answer |
| --- | --- |
| Positional `command` or clap subcommands? | **Positional command** with `value_parser(COMMANDS)` [30]. One global flag set, flags accepted before and after the command, `target` as a second optional positional. Subcommands would need every flag marked `.global(true)` and still change the error wording [5]. |
| Who prints help/version? | Us. `disable_help_flag(true)`, `disable_version_flag(true)`, `disable_help_subcommand(true)`, then two global `SetTrue` args handled after a successful parse [1][4]. |
| Exit codes | 0 success/help/version, 1 handled failure, 2 parse error, 130 SIGINT. clap already produces 2 for parse errors and 0 for help/version [2]. |
| Suggestions | Use the `difflib` crate (exact Python port, **zero dependencies**) [11][12], not `strsim`. Python's `get_close_matches` is Ratcliff/Obershelp, `n=3`, `cutoff=0.6` [10]. |
| Rich replacement | Own renderer (`render.rs`) + `owo-colors` for codes + `unicode-width` for cells. `comfy-table` optional (see §4). `dialoguer` **not** for scripted prompts: it hard-errors on a non-TTY [25]. |
| Color policy | Explicit: `NO_COLOR` > `CLICOLOR_FORCE`/`FORCE_COLOR` > `CLICOLOR` > `TERM=dumb` > TTY. Do not rely on clap's color [6] or owo's detection [19]. |
| Width | `COLUMNS` → ioctl on the console's stream → 80, matching Rich [14]; `is_narrow()` = `width < 72` [30]. |
| Events | Protocol v1 unchanged: `{"v":1,"event":...,"ts":...}` compact JSON, one line per event, stdout only [30][31]. |

---

## 1. What the original does

### 1.1 Parser shape

`FriendlyParser(prog="termux-sync", add_help=False)` with exactly ten arguments [30]:

```
command        nargs="?" default="help" choices=COMMANDS metavar="command"
target         nargs="?" default=""
--label  -l    default=""
--name   -n    default=""
--lines        default=40 type=int
--json-events  store_true
--yes          store_true
--debug        store_true
--version -v   store_true
--help    -h   store_true
```

Consequences that must survive the port:

- The command list is the 15 entries of `COMMAND_GROUPS` in help order (`backup`, `restore`,
  `list`, `delete`, `setup`, `schedule`, `daemon`, `export-config`, `import-config`,
  `status`, `logs`, `clear-logs`, `check`, `clear-cache`, `help`) [30]. ADR-002 says "18
  commands"; that count includes `--version` and `--help` as paths, not positional choices.
- `target` is *accepted and ignored* by commands that do not use it. **Observed**:
  `backup extra` parses fine; only a third positional is rejected.
- Argparse long-option abbreviations are on by default [8], so `--lab x list` and
  `--json` are accepted. **Observed**.
- `-h`, `--help` and `-v`, `--version` are plain flags, not argparse actions [30]. Parsing
  therefore completes (and validates everything else) before they are honoured. **Observed**:
  `termux-sync bogus --version` exits 2, `-h bogus` exits 2, `--l x list` exits 2.
- `--version` wins over `--help` because `main` checks it first (3286-3292) [30].
- `help` as a command and no command both print the same hand-written help and return 0 [30].
  `print_help` never uses argparse formatting.

### 1.2 Precedence and exit codes

Order in `main` [30]:

1. parse → on error, `FriendlyParser.error` prints an error panel and `sys.exit(2)`.
2. `args.version` → print `termux-sync v1.3.9`, return **0**.
3. `args.help` or command in `(None, "help")` → `print_help()`, return **0**.
4. load config, enable event mode if `--json-events`, dispatch.
5. `KeyboardInterrupt` → `_abort()`: emit `cancelled`, print panel, `sys.exit(130)`.
6. any other exception → `handle_crash()` (log traceback, panel, `error` event) then
   `sys.exit(1)`.

argparse itself documents "exit with a status code of 2" for invalid argument lists, and
`ArgumentParser.error` does `exit(2, ...)` [8][9]. `--version`/`--help` as real argparse
actions exit 0; here they are store_true, so the 0 comes from `main` returning.

### 1.3 Exact error strings

`FriendlyParser.error` rewrites argparse's message before printing [30]:

- `argument --lines/` is collapsed to `--lines`, and the literal `argument ` prefix is
  removed (3243). So `argument --lines: invalid int value: 'x'` becomes
  `--lines: invalid int value: 'x'`.
- If the message matches `invalid choice: '([^']*)'`, it prints title **Unknown command**,
  body `'X' is not a termux-sync command.`, then hints.
- Otherwise title **Invalid usage** and the rewritten message.

**Observed** stdout of `termux-sync bogus` (width 80, no ANSI because stdout was a pipe):

```
╭──────────────────  ERROR  ──────────────────╮
│                                             │
│  Unknown command                            │
│                                             │
│  'bogus' is not a termux-sync command.      │
│                                             │
│  How to fix                                 │
│  Did you mean:  logs                        │
│                                             │
│  Run termux-sync help to see all commands.  │
│                                             │
╰─────────────────────────────────────────────╯
```

Other **observed** messages, all exit 2, all on **stdout**:

| argv | message line |
| --- | --- |
| `--lines x` | `--lines: invalid int value: 'x'` |
| `--name` | `--name/-n: expected one argument` |
| `--l x list` | `ambiguous option: --l could match --label, --lines` |
| `check -x` | `unrecognized arguments: -x` |
| `import-config -- --json-events --json-events` | `unrecognized arguments: --json-events` |

Panel geometry: `Panel(..., expand=False)`, `padding=(1, 2)`, red border, title
`[bold red] ERROR [/bold red]`, one blank line before and after [30].

### 1.4 Suggestions

`COMMAND_HINTS` short-circuits six words (`update`, `upgrade`, `install`, `remove`,
`config`, `sync`) with a hand-written sentence and suppresses fuzzy matches for them;
everything else goes through `difflib.get_close_matches(bad, COMMANDS, n=3, cutoff=0.6)`
[30]. `get_close_matches` scores with `SequenceMatcher.ratio()` = `2.0*M/T`, returns at
most `n` scores ≥ cutoff, best first [10]. `bogus` → `logs`, `lst` → `list` **Observed**.

### 1.5 Human output still on stdout during parse failures

`console` is constructed as `Console()` at import, i.e. **stdout** [30]; it is only moved to
stderr by `enable_event_mode` after a successful parse (376). Therefore parse errors,
`--help`, the banner and crash panels all go to **stdout**; stderr is effectively unused by
the CLI. **Observed**: parse-error panels appear on stdout with empty stderr.

This is a latent bug. ADR-003 requires matching stdout/stderr after normalization [32], so
preserve it during the port and record it; move it to stderr only in a deliberate 2.0.0
behaviour change with re-recorded cases.

---

## 2. clap 4 mapping

### 2.1 Positional command, not subcommands

The original has one flat flag namespace and accepts flags anywhere (`list --json-events`,
`--json-events list`, `--lab x list`) [30]. clap parses parent args before a subcommand, and
only `Arg::global(true)` args are matched by child subcommands [5]. Reproducing the flat
namespace with subcommands needs `.global(true)` on all eight flags, `subcommand_required(false)`,
`disable_help_subcommand(true)`, plus per-command positionals — and clap's "unexpected
argument 'extra' found" wording is further from argparse than the flat model.

Flat model (recommended), mirroring §1.1 exactly:

```rust
use clap::{Arg, ArgAction, ColorChoice, Command};

const COMMANDS: &[&str] = &[
    "backup","restore","list","delete","setup","schedule","daemon","export-config",
    "import-config","status","logs","clear-logs","check","clear-cache","help",
];

fn parser() -> Command {
    Command::new("termux-sync")
        .disable_help_flag(true)          // we own -h/--help          [1]
        .disable_version_flag(true)       // we own -v/--version       [1]
        .disable_help_subcommand(true)
        .arg_required_else_help(false)    // bare argv must print *our* help, exit 0 [3]
        .infer_long_args(true)            // argparse allow_abbrev     [1][8]
        .args_override_self(true)         // repeated flags/options must not conflict [1][4]
        .color(ColorChoice::Never)        // clap never prints; keep its fragments plain [6]
        .arg(Arg::new("command").value_name("command")
             .value_parser(COMMANDS).default_value("help").num_args(0..=1))
        .arg(Arg::new("target").value_name("target").num_args(0..=1).default_value(""))
        .arg(Arg::new("label").short('l').long("label").num_args(1).default_value(""))
        .arg(Arg::new("name").short('n').long("name").num_args(1).default_value(""))
        .arg(Arg::new("lines").long("lines").num_args(1).default_value("40"))
        .arg(Arg::new("json-events").long("json-events").action(ArgAction::SetTrue))
        .arg(Arg::new("yes").long("yes").action(ArgAction::SetTrue))
        .arg(Arg::new("debug").long("debug").action(ArgAction::SetTrue))
        .arg(Arg::new("version").short('v').long("version").action(ArgAction::SetTrue))
        .arg(Arg::new("help").short('h').long("help").action(ArgAction::SetTrue))
}
```

Notes:

- `num_args(0..=1)` on `command` plus `default_value("help")` reproduces `nargs="?"`
  [30]; `target` likewise.
- Parse `lines` yourself. Python `int()` accepts `1_0` (**Observed**: `--lines 1_0` → 10),
  surrounding whitespace, and arbitrary precision. Use a helper that trims, strips `_`,
  parses `i64`, and falls back to a saturating value; record the divergence for values
  above `i64::MAX`.
- Keep `lines` signed: `cmd_logs` does `all_lines[-lines:]` (2813) [30], so `0` means *all*
  lines and `-5` means *skip the first five*. Rust must implement that slicing rule, not
  `usize` truncation.
- `args_override_self(true)` is required because clap treats a repeated `Set`/`SetTrue`
  argument as a conflict by default [4]; argparse lets the last occurrence win.
- Use `try_get_matches_from` — it never exits, and returns `DisplayHelp`/`DisplayVersion`
  errors instead of exiting [1]. We disable those flags, so they cannot occur; still map
  them to exit 0 defensively [3].

### 2.2 Help, version, and no-argument behaviour

`arg_required_else_help(true)` would make clap print its own help to **stderr** and exit
non-zero (`DisplayHelpOnMissingArgumentOrSubcommand`) [3] — wrong here. Keep it `false`
and do:

```rust
let m = parser().try_get_matches_from(argv)?;
if m.get_flag("version") { println!("termux-sync v{VERSION}"); return Ok(0); }
if m.get_flag("help") || m.get_one::<String>("command").map(String::as_str) == Some("help") {
    print_help(); return Ok(0);
}
```

Order matters (version before help) and both must run only after a successful parse, which
is what §1.1 requires.

### 2.3 Error mapping and exit codes

clap's `Error::exit_code()` returns `2` when the error would print to stderr, otherwise
`0`; `Error::exit()` prints to stderr and exits 2, or stdout and exits 0 [2]. We never call
`exit()`: render our own panel, then `exit(2)`.

Map the two shapes we can actually produce:

- Unknown command: positional with `value_parser` yields `ErrorKind::InvalidValue` [3].
  Read the bad token with `err.get(ContextKind::InvalidValue)` (needs the default
  `error-context` feature) [2][7], then apply `COMMAND_HINTS` / `difflib` [11].
- Everything else: `ErrorKind::UnknownArgument` (unknown flag, extra positional),
  `ValueValidation` (bad number), or `InvalidValue` without an invalid-value context
  (missing option value) [3]. Pull `ContextKind::InvalidArg` / `InvalidValue` and format
  the §1.3 wording.

If the context is absent (clap changed its shape), fall back to a generic invalid-usage
panel rather than leaking clap's sentence. Keep a unit test per message in §1.3.

`Error::to_string()` is not a stable user-facing contract; treat it as a last resort.

### 2.4 Abbreviations

`Command::infer_long_args(true)` allows unambiguous prefixes of long options and their
aliases, and propagates to children [1]. That matches argparse's default `allow_abbrev=True`
[8], including the ambiguity failure (`--l` matches both `--label` and `--lines`). Do **not**
enable `infer_subcommands`; the command is a positional, not a subcommand, and argparse does
not do prefix matching on positional choices [8].

### 2.5 What clap must not print

We replace all of clap's generated text: help is hand-written (`print_help`), errors are our
panels, and `--version` is a plain line [30]. Setting `ColorChoice::Never` keeps any
accidental clap output free of ANSI [6]; clap's `Auto` only checks whether the stream is a
TTY and does not know about `NO_COLOR` [6], so it must not be trusted for our color policy.

---

## 3. Replacing Rich

### 3.1 Feature inventory and replacement

Rich API used by the CLI (grep of `termux-sync.py`) and its Rust target:

| Rich | Used for | Replacement |
| --- | --- | --- |
| `Console()`, `Console(stderr=True)` | all output, width, color decision | `render::Console { out, color, width }`, one per stream [12] |
| `print(markup)` + `escape()` | styled text, literal user data | our `Line`/`Style` builder; never parse markup from user data |
| `Panel(expand=False, padding=(1,2))` | errors, cancel, setup/schedule summary | `panel()` with the rounded box |
| `Table(box=ROUNDED, no_wrap, justify, width)` | backup list (wide) | `render::table()`; `comfy-table` optional (§4) |
| `Table(box=None, pad_edge=False, padding=(0,2))` | help groups/options | fixed two-column layout, no box |
| `Rule(title)` | section headers | `─` fill with a centred title [12] |
| `Align.center` | banner/version block | centre by display width |
| `Padding((0,1,0,2))` | `note()` body | 2-space left / 1-space right margin, wrap at `width - 3` |
| `Progress` + `SpinnerColumn`, `BarColumn`, `TextColumn`, `TimeElapsedColumn`, `TaskProgressColumn` | install/upload/download steps | `indicatif` or a minimal own bar (§3.5) |
| `Prompt.ask`, `Confirm.ask` | setup/schedule/delete/restore | own line reader (§3.6) |
| `console.width`, `is_narrow()` | layout switches | §3.3 |

There is no `Live`, `Screen`, `Syntax`, `JSON`, `Markdown` or pager use, so no full-screen
or syntax-highlight replacement is needed.

### 3.2 Streams and color

Two rules, applied once at startup:

1. Human output goes to stdout normally, or to **stderr** when `--json-events` is active
   (`enable_event_mode` sets `Console(stderr=True)`), because stdout then belongs to events
   [30]. Progress and prompts follow the same stream.
2. Color is decided per console, not globally. Precedence:

| Condition | Color | Source |
| --- | --- | --- |
| `NO_COLOR` non-empty | off (color only; bold/dim stay) | Rich honours `NO_COLOR` and notes it removes *color*, not styles [12]; spec says non-empty [15] |
| `FORCE_COLOR` set (non-empty) / `CLICOLOR_FORCE` non-empty | on even without a TTY | Rich treats `FORCE_COLOR` as a TTY override [14]; CLICOLOR spec aliases `FORCE_COLOR` to `CLICOLOR_FORCE`, `NO_COLOR` wins [16] |
| `CLICOLOR=0` | off | CLICOLOR spec: `NO_COLOR` > `CLICOLOR_FORCE` > `CLICOLOR` > unset [16] |
| `CLICOLOR` set, nothing above | on only when the stream is a TTY | same [16] |
| `TERM=dumb`/`unknown` | off, and no animations | Rich disables color/styles and cursor features [12][14] |
| otherwise | on only when the stream is a TTY | `std::io::IsTerminal` [13] |

Rich's actual `is_terminal` order is: explicit `force_terminal`, `TTY_COMPATIBLE` (`0`/`1`),
`FORCE_COLOR`, then `file.isatty()` [14]. Mirror `TTY_COMPATIBLE` too if recorded cases use
it; otherwise document that only the six rows above are supported.

Do not use `console::style` for this: its enable flags are process-global
(`set_colors_enabled*`) [20], which is wrong when stdout is events and stderr is human.
`anstream` exists to adapt ANSI to the target stream automatically [17] and `anstyle-query`
exposes `no_color`, `clicolor`, `clicolor_force`, `term_supports_color` [18]; use them as
inputs to *our* decision, not as a second decision-maker.

ANSI codes: `owo-colors` gives the colour/effect wrappers with no runtime detection
[19]; write through a thin `paint()` that emits nothing when `color == false`. Bright
colours (`bold cyan`, `bold red`, `bold green`, `bold yellow`, `dim`, `blue`) map to the
SGR codes Rich uses; verify by recording with `FORCE_COLOR=1`.

### 3.3 Width, narrow mode, Unicode

Rich: if `width`/`height` are not forced, it probes the standard streams with
`os.get_terminal_size`, then overrides with numeric `COLUMNS`/`LINES`, then falls back to
`80 x 25` [14]. Replicate exactly:

```
width = COLUMNS (if all digits)
      ?? ioctl(TIOCGWINSZ) on the console's stream
      ?? 80
```

`COLUMNS` matters: recorded cases can pin width with `COLUMNS=80` / `COLUMNS=45` without a
PTY. `console::Term::size()` is usable, but returns its own defaults when unknown; use
`size_checked()` and keep Rich's fallback [21].

Layout switches that depend on width:

- banner art when `width >= 55 + 2 = 57`, otherwise the `T E R M U X - S Y N C` wordmark
  [30]; the art is 55 columns wide (**Observed**).
- `is_narrow()` when `width < 72` [30]: backup list switches from a 6-column table to
  2-line cards (576-591); progress bar shrinks from 24 to 10 cells (550); restore's name
  shortening applies (1487).

Measure cells with `unicode-width`, not `chars().count()`: box alignment, ellipsis
truncation and prompt wrapping all break on CJK/emoji otherwise [26]. `unicode-width` is
`no_std`, uses UAX #11, and is what `comfy-table` and `console` already use [22][20].

### 3.4 Panels, rules, tables, banner

- Error panel: rounded box, `expand=False`, padding 1 row / 2 columns, red border, title
  ` ERROR ` [30]. Body lines are `title`, blank, message, optional `at <origin>`, optional
  `How to fix` + hint. Rich wraps body text to the available width (client width minus
  padding and border); reproduce with `unicode-width` wrapping at `width - 6` and let the
  box grow only when the terminal is narrower.
- Cancel panel: yellow border, same padding [30].
- Section header: `Rule(f"[bold yellow]{title}[/bold yellow]", style="yellow dim")`, then an
  optional dim subtitle via `note()` [30].
- Wide backup table: columns `#` (right, width 3), `Name` (no wrap), `Date` (no wrap),
  `Pkgs` (right), `Size` (right), `Label`; rounded box; `padding=(0,1)` [30]. `comfy-table`'s
  `UTF8_FULL.with_rounded_corners()` produces the same glyph family [22].
- Help layout: group heading, then a borderless two-column table with 2-space padding and
  `pad_edge=False`; options table uses green flags, descriptions plain; examples are printed
  with `soft_wrap=True` so they are not cropped [30]. On a 45-column Termux window this
  overflows exactly as Rich does; do not "fix" it in the port unless a recorded case says so.

### 3.5 Progress

Rich uses one transient line per task for package installs (spinner, description with a
`ratio=1` ellipsis column, 10/24-cell bar, percent, elapsed) and non-transient lines for
storage steps [30]. Only one task is live at a time in practice.

`indicatif` matches the important semantics: bars draw to **stderr** by default, are
completely hidden when the target is not a terminal, and support `{spinner}`, `{wide_msg}`
(truncating), `{bar:10}`/`{bar:24}`, `{percent}`, `{elapsed_precise}`, plus
`finish_and_clear()` for transient [23]. That gives the required behaviour for free:
`--json-events` piped into the TUI must not draw bars, and recorded cases must contain no
bar text.

If a dependency-free renderer is preferred, the same rules are ~80 lines: tick at 10 Hz,
`\r` + clear-to-EOL, hide when `!stderr.is_terminal()`, and emit nothing on finish when
transient.

### 3.6 Prompts (setup, schedule, delete, restore)

Do **not** use `dialoguer` for these. Its `Confirm::_interact_on` returns
`io::ErrorKind::NotConnected`, `"not a terminal"`, as soon as `term.is_term()` is false
[25]; `console::Term::read_line` returns an empty string and `read_key` an unknown key when
not user-attended [21]. The Python CLI is explicitly expected to work with piped input: it
catches `EOFError` and explains "Input was closed" (428-435), and restore reads a JSON line
from stdin when events are on (358-367) [30]. `dialoguer` cannot reproduce that.

Implement a small `prompt` module with these semantics (all call sites: 2240, 2311, 2328,
2466, 2483, 2508, 2514, 2526, 2546, 2558, 2572, 2578, 2583, 2595, 2600, 2639, 2645, 2653,
2661, 2841, 3008, 3023, 3034, 3084, 3102):

- `ask(prompt, default, choices, show_default, password) -> Result<String>`:
  write `prompt` + ` (default)` + ` [choices]` to the human console, flush, read one line,
  strip the newline, return default on empty. With `choices`, loop until the answer is one
  of them (`Prompt.ask` re-asks). With `password`, do not echo and never render a saved
  default (`show_default=False` on the token prompt, 2550).
- `confirm(prompt, default) -> Result<bool>`: `[y/n]` hint, empty means default, invalid
  input re-asks until valid, `n`/`y` case-insensitive. `delete` defaults to false (2483),
  restore to false (2328), checksum-continue to false (2311), schedule-enable to the saved
  value (2639).
- Empty input on a prompt with a default yields the default. EOF (`read_line` → 0 bytes)
  becomes the `Input was closed` error (428-435), not a panic.
- In event mode, stdin may already be consumed by `event_confirmation` for restore
  (358-367): the prompt module must operate on the same locked `Stdin`, never reopen it.
- Validation loops are behavioural, not cosmetic: setup's local path rejects empty (2513),
  GitHub token rejects empty (2545), repo requires exactly one internal `/` and no leading
  or trailing slash (2566), `max_backups` requires a whole number ≥ 1 (2582), schedule
  hours 0-23 and minutes 0-59 (2645/2653). Keep the same red feedback lines.
- Setup defaults: storage `3` (GitHub, marked recommended), compression `gz`, branch
  `main`, max backups `5`; storage map `1→local`, `2→gdrive`, `3→github` (2508-2510)
  [30]. Schedule defaults come from `cron.json` (`enabled:false`, `hour:2`, `minute:0`,
  `label:"auto"`) (2616-2620) [30].

Prompt rendering must wrap and pad by display width (`unicode-width`) and use the same two
leading spaces as the rest of the human output [30].

### 3.7 Non-Python residue to record

`_write_daemon_script` writes `~/.termux/boot/termux-sync-daemon.sh`, mode 0755, containing
`python3 "<abs path>" daemon &` (2682-2692) [30]. After the port this must invoke the Rust
binary (`std::env::current_exe()`), while keeping the path and mode. Existing boot scripts
created by Python still point at Python, so this is a real migration item for ADR-004 and
needs a recorded case plus a note in the lift checklist.

---

## 4. Crates: pure Rust and Termux

All candidates are pure Rust (no `build.rs` compiling C). `libc`/`windows-sys` are Rust FFI
bindings, not C sources. `Termux` is `aarch64-linux-android`; every crate below takes a
generic `unix` path, with `#[cfg(unix)]` rather than an Android exclusion [24].

| Crate | Version seen | Deps | Verdict for this port |
| --- | --- | --- | --- |
| `clap` | 4.6.x | `clap_builder`, optional `clap_derive` | Yes. `default` features (`color`,`error-context`,`help`,`std`,`suggestions`,`usage`) [7]. Abandon clap's printing, keep its parser. |
| `difflib` | 0.4.0 | **none** | Yes. Exposes `get_close_matches` plus `SequenceMatcher`, explicitly "Port of Python's difflib" [11]. |
| `unicode-width` | 0.2.3 | optional `rustc-std-workspace-*` only | Yes. UAX #11 widths, `no_std` [26]. |
| `owo-colors` | 4.4.0 | optional `supports-color` | Yes. Use without `supports-colors` so detection stays ours [19]. |
| `anstream` | 1.0.0 | `anstyle`, `anstyle-parse`, optional `anstyle-query`, `colorchoice`, `is_terminal_polyfill`, `utf8parse` | Optional. Only worth it if we want automatic ANSI stripping; our renderer can skip writing codes instead [17]. |
| `anstyle-query` | 1.1.5 | `windows-sys` | Optional. Correct source of `NO_COLOR`/`CLICOLOR`/`CLICOLOR_FORCE`/`TERM` checks [18]. |
| `console` | 0.16.x | `libc` (opt), `unicode-width` (opt), `encode_unicode`, `windows-sys` | Optional. `is_term`, `size_checked`, `measure_text_width`; beware process-global color flags [20][21]. |
| `comfy-table` | 8.0.1 | `unicode-segmentation`, `unicode-width`, optional `ansi-str`, `console`, `crossterm` | Optional. Default `tty` pulls crossterm and does one `ioctl` in a dependency (documented as the crate's only unsafe call) [22]. If used, prefer `default-features = false` + `set_width(our_width)` so Termux only relies on `unicode-width`. |
| `indicatif` | 0.18.x | `console`, `portable-atomic`, `unit-prefix` | Recommended for progress. Hidden on non-TTY, draws to stderr, `finish_and_clear` [23]. |
| `dialoguer` | 0.12.0 | `console`, `shell-words` | **No** for the CLI prompts: hard error on non-TTY [25]. |
| `trycmd` | 1.2.1 | `glob`, `humantime`, `rayon`, `serde`, `shlex`, `snapbox`, `toml_edit` | Yes for app cases (§6). |
| `insta` | 1.49.0 | — | Yes for renderer snapshots (§6.3). |
| `signal-hook` | — | `libc` | Needed if the `cancelled` event + panel + 130 must be emitted on SIGINT (§7). |

Two Termux-specific cautions:

- Do not enable compression crates here; ADR-005 fixes bz2/xz to system libraries. None of
  the crates above touch those.
- Anything that opens `/dev/tty` directly will fail under `adb shell`/Deno-style
  non-interactive runners; the `console`/`crossterm` paths use the passed stream's fd, not
  `/dev/tty` [24][22]. Still run one on-device check before committing to `comfy-table`'s
  `tty` feature.

---

## 5. JSON event protocol v1

The TUI's contract, from `tui/src/ops.rs` and `tui/src/data.rs`:

- One JSON object per line on stdout, compact separators, `ensure_ascii=False` [30].
- Every event has `v`, `event`, `ts` (ms) [30]; the TUI rejects `v > 1` and lines without
  `event` [31].
- Recognised events: `stage`, `progress`, `log`, `plan`, `package_diff`, `backup`,
  `result`, `error`, `cancelled` [31]. `list --json-events` must end with `result`, else
  `collect_backups` fails [31].
- Field names and types are read by key; `error` carries `message`, `hint`, `exit_code`
  [31]; `progress` carries `stage`, `current`, `total?`, `unit`, `detail?` [30].
- Secrets (`github_token`, `encrypt_password`) are redacted as `***` before serialisation,
  recursively (203-215) [30]. Keep that.
- `enable_event_mode` moves human output to stderr **after** config load [30], so a config
  failure produces a human panel and no JSON event. Preserve that ordering.
- `supports_flag` probes `termux-sync <flag> --version` and requires exit 0 [31]; therefore
  `--version` must succeed with any other valid flag present and no command. The flat clap
  model satisfies this automatically.

Do not add fields or change `v`. New events, if ever needed, are additive and still `v:1`.

---

## 6. Testing

### 6.1 trycmd for app cases

`trycmd::TestCases::new().case("rust/app-cases/*.toml")` [27]. The cases already in this
repo use the recorded-Python convention:

```toml
bin.name = "termux-sync"
args = ["list"]
env.add.HOME = "home"          # relative to the sandbox
env.add.PREFIX = "prefix"
env.add.TMPDIR = "tmp"
env.add.PATH = "bin:/data/data/com.termux/files/usr/bin"
status.code = 0
fs.sandbox = true
```

Sibling `.stdin`, `.stdout`, `.stderr` files hold expected bytes [27]. Keep
`bin.name = "termux-sync"` while porting and point the harness at the Rust binary with
`default_bin_path`/`default_bin_name` (ADR-006: the front door is `termux-sync-rs`) [28];
at lift the name is correct without changes.

Defaults that matter: newlines and path separators are normalized when `binary = false`
(the default); `...` alone on a line elides a block; `[..]` elides within a line;
`[ROOT]`, `[CWD]`, `[EXE]` are built-in variables [27]. Project paths that vary can be
registered with `insert_var` [28].

Add `env.add.COLUMNS = "80"` (and `NO_COLOR = "1"` while diagnosing) to every case so the
layout is pinned even if the test host exports `COLUMNS`. Note that the existing recorded
`help.stdout` is padded with trailing spaces to width 80; verify whether snapbox trims
trailing whitespace before depending on exact padding, otherwise the renderer must pad
cells to the same width.

Record with `TRYCMD=overwrite cargo test --test cli_cases`, review the diff, commit. `dump`
is for first capture [27].

### 6.2 Making recordings stable without normalization

The point of §3.3-§3.5 is that the human output is deterministic when the stream is not a
terminal: Rich strips ANSI and removes animations when not a terminal [12], and `indicatif`
hides bars on a non-TTY [23]; trycmd pipes both streams, so both CLIs see a non-TTY. Add
`COLUMNS=80` (or `45`) to pin width, and `NO_COLOR=1` to be explicit. Then the only
nondeterminism left is paths and timestamps, handled by `insert_var`/`[..]`.

Normalization steps for the port's test helper, applied when a case must be shared with the
Python recording (ADR-003) [32]:

1. strip ANSI (`console::strip_ansi_codes` or `strip-ansi-escapes`),
2. replace the home, `PREFIX`, `TMPDIR` and config paths with `[HOME]`, `[PREFIX]`,
   `[TMPDIR]`, `[CONFIG]`,
3. collapse trailing spaces and drop the box border if the two renderers differ
   (this is the one place ADR-003 permits a difference),
4. replace timestamps with `[..]`.

Prefer raw comparison; only box geometry should need step 3.

### 6.3 Snapshots for the renderer

Renderer units (panel, table at 45/80, help layout, banner threshold, prompt echo) are best
tested in-process with `insta::assert_snapshot!`: snapshots live in `snapshots/*.snap`,
`INSTA_UPDATE=always` updates, `cargo insta review` reviews [29]. Drive them with an
explicit `Console { width: 45|80, color: false, out: Vec<u8> }` so no terminal is involved.

### 6.4 Case list to record first

`rust/app-cases/` already covers `help`, `version`, `list`, several `delete`, `logs`,
`check`, `restore` and config cases. The list below is the gap that this research implies —
parser and error paths that the existing set does not exercise.

Exit codes and messages: no args; `help`; `-h`; `--version`; `-h -v`; `help --version`;
`bogus`; `lst`; `update` (hint path); `--l x list`; `--lines x`; `--name` (missing value);
`check -x`; `check -- -x`; `backup extra`; `import-config -- --json-events`;
`--lab x list`; `--lines 1_0 logs`; `--lines 0 logs`; `--lines -2 logs`.

Machine output: `list --json-events` (empty and non-empty), `--json-events --version`,
`--json-events help` (no events), `status --json-events`, `logs --lines 5 --json-events`,
`delete --name missing --json-events`, a forced failure (unreadable `HOME`) for `error`.

Environment matrix: `NO_COLOR=1`, `FORCE_COLOR=1`, `CLICOLOR=0`, `CLICOLOR=1`,
`TERM=dumb`, `COLUMNS=45`, `COLUMNS=80`, `TTY_COMPATIBLE=0/1`.

---

## 7. Risks and decisions to settle

1. **Parse errors on stdout.** Preserve (parity) or fix (correctness). Recommendation:
   preserve now, change at 2.0.0 with re-recorded cases; document in `DECISIONS.md`.
2. **SIGINT path.** Default disposition gives exit status 130 but no panel and no
   `cancelled` event, which the TUI surfaces as a failure [31]. Use `signal-hook` to run
   the abort path once (emit `cancelled`, print the yellow panel, exit 130). Verify with
   `kill -INT` on device and with the TUI's process-group kill.
3. **`--debug` traceback.** There is no Python traceback to reproduce; keep the panel and
   log lines, then print a Rust backtrace when `RUST_BACKTRACE` is set. Recorded cases must
   never include traceback text.
4. **`comfy-table` tty feature.** Decide after one Termux build; the fallback is our own
   table renderer (already needed for the narrow card layout and help).
5. **`int()` permissiveness.** `1_0`, leading `+`, unicode digits, and >`i64` values. Support
   underscore and `+`, reject the rest with the exact §1.3 message; document the divergence.
6. **Daemon script** still calls Python (§3.7). Needs an explicit migration task.
7. **`--json-events` + human prompts.** In event mode stdin is a JSON confirmation channel
   for restore but a text channel for other prompts. No command currently mixes both, but
   the prompt module must take an explicit reader so this cannot regress.

---

## References

1. clap 4 `Command` — `try_get_matches`, `disable_help_flag`, `disable_version_flag`,
   `infer_long_args`, `args_override_self`, `help_template`, `override_usage`:
   <https://docs.rs/clap/latest/clap/struct.Command.html>
2. clap `Error` — `exit_code`, `exit`, `kind`, `get`, `print`, `use_stderr`:
   <https://docs.rs/clap/latest/clap/error/struct.Error.html>
3. clap `ErrorKind` — `InvalidValue`, `UnknownArgument`, `DisplayHelp`, `DisplayVersion`,
   `DisplayHelpOnMissingArgumentOrSubcommand`:
   <https://docs.rs/clap/latest/clap/error/enum.ErrorKind.html>
4. clap `ArgAction` — `Set`, `SetTrue`, self-conflict note:
   <https://docs.rs/clap/latest/clap/builder/enum.ArgAction.html>
5. clap `Arg` — `global`, `num_args`, `default_value`, `value_parser`:
   <https://docs.rs/clap/latest/clap/struct.Arg.html>
6. clap `ColorChoice` — `Auto` means "only when a terminal", no `NO_COLOR` awareness:
   <https://docs.rs/clap/latest/clap/enum.ColorChoice.html>
7. clap 4.6.9 feature flags (defaults: `color`, `error-context`, `help`, `std`,
   `suggestions`, `usage`): <https://docs.rs/crate/clap/4.6.9/features>
8. Python `argparse` — `add_help`, `allow_abbrev`, `choices`, `nargs="?"`, exit status 2,
   `error()`/`exit()`:
   <https://docs.python.org/3/library/argparse.html>
9. CPython 3.13 `argparse.py` — `error()` → `exit(2, ...)`, `_check_value` message,
   `_get_value` message, `ArgumentError.__str__`:
   <https://github.com/python/cpython/blob/3.13/Lib/argparse.py>
10. CPython 3.13 `difflib.py` — `get_close_matches(n=3, cutoff=0.6)`, `SequenceMatcher.ratio`:
    <https://github.com/python/cpython/blob/3.13/Lib/difflib.py>
11. `difflib` crate 0.4.0 — `get_close_matches`, `SequenceMatcher`:
    <https://docs.rs/difflib/0.4.0/difflib/>
12. Rich Console API — terminal detection, `force_terminal`, `force_interactive`,
    `NO_COLOR`, `FORCE_COLOR`, `TERM=dumb`, `TTY_COMPATIBLE`, `COLUMNS`/`LINES`,
    overflow, rule:
    <https://rich.readthedocs.io/en/stable/console.html>
13. Rust `std::io::IsTerminal`:
    <https://doc.rust-lang.org/std/io/trait.IsTerminal.html>
14. Rich `rich/console.py` — `is_terminal` order, `is_dumb_terminal`, `size` fallback and
    `COLUMNS` override, `NO_COLOR` handling:
    <https://github.com/Textualize/rich/blob/master/rich/console.py>
15. `NO_COLOR` standard:
    <https://no-color.org/>
16. `CLICOLOR` / `CLICOLOR_FORCE` standard, precedence and `FORCE_COLOR` alias:
    <https://bixense.com/clicolors/>
17. `anstream` — auto-adapting stdout/stderr:
    <https://docs.rs/anstream/latest/anstream/>
18. `anstyle-query` — `no_color`, `clicolor`, `clicolor_force`, `term_supports_color`:
    <https://docs.rs/anstyle-query/latest/anstyle_query/>
19. `owo-colors` — `OwoColorize`, `Style`, optional `supports-colors` detection:
    <https://docs.rs/owo-colors/latest/owo_colors/>
20. `console` — `Term`, `style`, `colors_enabled`, `measure_text_width`,
    `strip_ansi_codes`:
    <https://docs.rs/console/latest/console/>
21. `console::Term` — `size`/`size_checked`, `is_term`, `read_line`/`read_key` on
    non-attended terminals:
    <https://docs.rs/console/latest/console/struct.Term.html>
22. `comfy-table` — presets, `ContentArrangement::Dynamic`, `set_width`, `tty` feature and
    its single `ioctl` dependency, `force_no_tty`:
    <https://docs.rs/comfy-table/latest/comfy_table/>
23. `indicatif` — hidden on non-TTY, stderr draw target, templates, `finish_and_clear`:
    <https://docs.rs/indicatif/latest/indicatif/>
24. `console` unix backend (`#[cfg(unix)]`, `ioctl(TIOCGWINSZ)`), no Android exclusion:
    <https://github.com/console-rs/console/blob/main/src/unix_term.rs>
25. `dialoguer` `Confirm` — returns `NotConnected`, `"not a terminal"`:
    <https://github.com/console-rs/dialoguer/blob/main/src/prompts/confirm.rs>
26. `unicode-width` — UAX #11 rules, C0/CJK handling, `no_std`:
    <https://docs.rs/unicode-width/latest/unicode_width/>
27. `trycmd` — case files, `.stdout`/`.stderr`, `TRYCMD=dump|overwrite`, `...`/`[..]`/
    `[ROOT]`/`[CWD]` elisions:
    <https://docs.rs/trycmd/latest/trycmd/>
28. `trycmd::TestCases` — `env`, `insert_var`, `default_bin_name`:
    <https://docs.rs/trycmd/latest/trycmd/struct.TestCases.html>
29. `insta` — snapshot files, `INSTA_UPDATE`, `cargo insta review`:
    <https://docs.rs/insta/latest/insta/>
30. `termux-sync.py` — parser, help, errors, prompts, events (line numbers in text):
    `/data/data/com.termux/files/home/repos/termux-sync-rust-tui/termux-sync.py`
31. `tui/src/ops.rs` and `tui/src/data.rs` — event keys, protocol version check,
    `--json-events` probing, process-group SIGINT:
    `/data/data/com.termux/files/home/repos/termux-sync-rust-tui/tui/src/`
32. `rust/DECISIONS.md` — ADR-003 (normalized human text, exact machine output), ADR-004
    (backward compatibility), ADR-005 (Termux aarch64, no OpenSSL):
    `/data/data/com.termux/files/home/repos/termux-sync-rust-tui/rust/DECISIONS.md`
