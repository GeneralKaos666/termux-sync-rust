# Rust migration plan: termux-sync-rust-tui (Python)

1 package, 2 source files (5732 lines), 12 test files, 0 UI files, grouped into 2 units.

| Ready | Caution | Blocked | Keep in source language |
| ---: | ---: | ---: | ---: |
| 0 | 2 | 0 | 0 |

## Start here

Best first ports: safe to port (readiness) and worth porting (loops and string work, tests, size).

| # | Unit | Lines | Priority | Readiness | Tests | Bridge | Main risks |
| ---: | --- | ---: | ---: | ---: | ---: | --- | --- |
| 1 | `src/termux_sync/` | 2866 | 30 | 0 | 11 | PyO3 | String offsets and slicing (18); Hash iteration order (4); Dynamic typing (57) |
| 2 | `termux-sync.py` | 2866 | 22 | 0 | 0 | PyO3 | String offsets and slicing (18); Hash iteration order (4); Dynamic typing (57) |

### `src/termux_sync/`

2866 lines in 1 file, wave 0, priority 30, readiness 0, bridge **PyO3**.

- Leaf: imports no other unit.
- 11 test files import it directly.
- String offsets and slicing x18: `src/termux_sync/__init__.py:564`, `src/termux_sync/__init__.py:722`, `src/termux_sync/__init__.py:751`
- Hash iteration order x4: `src/termux_sync/__init__.py:739`, `src/termux_sync/__init__.py:1398`, `src/termux_sync/__init__.py:1967`
- Dynamic typing x57: `src/termux_sync/__init__.py:96`, `src/termux_sync/__init__.py:103`, `src/termux_sync/__init__.py:108`
- Integer width and overflow x12: `src/termux_sync/__init__.py:272`, `src/termux_sync/__init__.py:273`, `src/termux_sync/__init__.py:274`
- Division and modulo x5: `src/termux_sync/__init__.py:560`, `src/termux_sync/__init__.py:1989`
- Class inheritance x3: `src/termux_sync/__init__.py:381`, `src/termux_sync/__init__.py:1573`, `src/termux_sync/__init__.py:3227`
- Exceptions x115: `src/termux_sync/__init__.py:30`, `src/termux_sync/__init__.py:163`, `src/termux_sync/__init__.py:167`
- Number semantics x9: `src/termux_sync/__init__.py:181`, `src/termux_sync/__init__.py:649`, `src/termux_sync/__init__.py:776`
- Clock and random numbers x5: `src/termux_sync/__init__.py:133`, `src/termux_sync/__init__.py:218`, `src/termux_sync/__init__.py:1711`

### `termux-sync.py`

2866 lines in 1 file, wave 0, priority 22, readiness 0, bridge **PyO3**.

- Leaf: imports no other unit.
- String offsets and slicing x18: `termux-sync.py:564`, `termux-sync.py:722`, `termux-sync.py:751`
- Hash iteration order x4: `termux-sync.py:739`, `termux-sync.py:1398`, `termux-sync.py:1967`
- Dynamic typing x57: `termux-sync.py:96`, `termux-sync.py:103`, `termux-sync.py:108`
- Integer width and overflow x12: `termux-sync.py:272`, `termux-sync.py:273`, `termux-sync.py:274`
- Division and modulo x5: `termux-sync.py:560`, `termux-sync.py:1989`
- Class inheritance x3: `termux-sync.py:381`, `termux-sync.py:1573`, `termux-sync.py:3227`
- Exceptions x115: `termux-sync.py:30`, `termux-sync.py:163`, `termux-sync.py:167`
- Number semantics x9: `termux-sync.py:181`, `termux-sync.py:649`, `termux-sync.py:776`
- Clock and random numbers x5: `termux-sync.py:133`, `termux-sync.py:218`, `termux-sync.py:1711`

## Order by package

Packages come after the workspace packages they use. Inside a package, port by wave: a unit's imports are in earlier waves.

### 1. termux-sync (`.`, portable, 12 test files)

| Wave | Unit | Status | Lines | Readiness | Bridge | Main risks |
| ---: | --- | --- | ---: | ---: | --- | --- |
| 0 | `src/termux_sync/` | caution | 2866 | 0 | PyO3 | String offsets and slicing (18); Hash iteration order (4) |
| 0 | `termux-sync.py` | caution | 2866 | 0 | PyO3 | String offsets and slicing (18); Hash iteration order (4) |

## PyPI dependencies

| Package | Units | Rust | Fit | Note |
| --- | ---: | --- | --- | --- |
| `rich` | 2 | owo-colors, anstream | good |  |

## Risk guide

- **String offsets and slicing** (High): JS indexes strings in UTF-16 code units, Rust in UTF-8 bytes. Offsets that cross the boundary must be converted, or non-ASCII text gets wrong ranges silently.
- **Dynamic typing** (Medium): Values whose shape is checked at runtime need an explicit Rust enum or `serde_json::Value`.
- **Exceptions** (Low): Thrown errors become `Result`. Errors crossing the bridge must map back to the same JS error shape.
- **Class inheritance** (Medium): Class hierarchies become traits plus composition; `instanceof` checks become enums.
- **Number semantics** (Low): JS numbers are f64; bitwise ops truncate to i32; parseInt is lenient. Pick Rust integer types deliberately. Printing differs: `${n}` gives `1e+21`, `1e-7` and `0` for -0 where Rust's `{}` does not, so use `js::number_to_string`. `Math.round` rounds halves up (-2.5 gives -2), Rust's `round` away from zero: use `js::math_round`. `toFixed` rounds exact halves up ((2.5).toFixed(0) is "3"), Rust's `{:.0}` to even.
- **Integer width and overflow** (Medium): Python ints never overflow; C, Go and Java ints wrap or are undefined on overflow. Pick i64, i128 or a bignum crate deliberately, and use wrapping_* where the source wraps.
- **Division and modulo** (Medium): Python `//` and `%` floor toward negative infinity; Rust `/` and `%` truncate: use `div_euclid`/`rem_euclid` or explicit floor math. C division by zero is undefined.
- **Hash iteration order** (High): Set and map iteration order comes from the source language's hash function. A port cannot reproduce it: sort before output, or make the order part of the contract.
- **Clock and random numbers** (Low): Results that depend on the time or on random numbers differ on every run, so a plain recording cannot pin them down. Record with `record --pin-clock` (TS/JS with the napi bridge, Python): `Date.now()`, `new Date()` and `Math.random` (`time.time`, `datetime.now` and `random`) then return fixed values during record, diff and gate, and the port reads the same ones from `src/port/pinned.rs` (`now_ms()`, `random()`, a documented mulberry32). Or pass the time and the seed in as parameters.

## Whole app

Shape: **cli**.

| Entry point | Kind | Found from |
| --- | --- | --- |
| termux-sync: `termux_sync:main` | cli | pyproject.toml scripts |
| `src/termux_sync/__init__.py` | cli | `if __name__ == "__main__"` at src/termux_sync/__init__.py:3325 |
| `termux-sync.py` | cli | `if __name__ == "__main__"` at termux-sync.py:3325 |

### Every dependency

| Dependency | Declared | Used by | Rust | Fit | Note |
| --- | --- | --- | --- | --- | --- |
| `rich` | yes | 2 | owo-colors, anstream | good |  |

### Pieces, in port order

- wave 0: `src/termux_sync/` (2866 lines)
- wave 0: `termux-sync.py` (2866 lines)

### Decisions for the user

**D1** (shape): The app is a cli, entered through termux-sync (termux_sync:main), src/termux_sync/__init__.py, termux-sync.py. Port every entry point, in that shape?

> Yes: a clap front door (`app front-door`) takes every command over first

