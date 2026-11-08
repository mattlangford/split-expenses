# AGENTS.md

Guidance for coding agents working in this repo.

## What this is

`split` — a Rust TUI that splits a restaurant bill. It shells out to `pi` (with an
image model) to OCR a receipt photo, then lets the user add people and tick off who
shared each item, producing per-person totals.

## Toolchain / build environment

Cargo is **not on PATH** in this sandbox (only `rustc` is, and `~` is read-only).
Run cargo through nix, keeping the caches on `/tmp`:

```sh
XDG_CACHE_HOME=/tmp/nix-cache CARGO_HOME=/tmp/cargo-home \
  nix shell nixpkgs#cargo nixpkgs#rustc -c cargo test
```

Substitute `build`, `run -- <args>`, etc. for `test`. `target/` lives in the
project dir (writable). On a machine with a normal Rust install, plain `cargo`
works. There is no `scripts/` directory.

To exercise the CLI without opening a TUI: `cargo build` then
`./target/debug/split --help`.

## Module map

```
src/main.rs   entry point, terminal init, event loop, mod declarations
src/cli.rs    clap Args (--model default google/gemini-3.1-flash-lite-image, --state-dir, --pi-bin)
src/types.rs  Receipt, ReceiptItem, Person, Assignments
src/money.rs  cents parse/format, parse/format tip basis points, allocate()
src/split.rs  compute_split(): per-person allocation + reconciliation
src/pi.rs     extract_receipt(): shell out to pi, normalize JSON -> Receipt
src/state.rs  save/load /tmp/.../state.json, slug, clipboard
src/app.rs    App state machine, vim Normal/Insert key handling
src/ui.rs     ratatui rendering for all screens
```

Dependency direction: `app`/`ui`/`pi`/`state` depend on `types`/`money`/`split`.
Keep the math modules free of I/O and TUI types.

## Conventions

- **Money is always integer cents** (`i64`), never floats. Parse/format via
  `money.rs`.
- **Allocations go through `money::allocate`** (largest-remainder) so split sums
  are exact. Do not hand-roll proportional division.
- Tip is stored as **basis points** (`u32`), default `2000` (= 20%). Tip is
  computed on the subtotal.
- Item split rule: `line_total` divided equally among the people assigned to that
  item (`Assignments`).
- Tax and tip are allocated in proportion to each person's item subtotal.
- The TUI has a Normal/Insert mode model: `app.rs` handles keys, `ui.rs` only
  renders. Navigation is vim-style (`hjkl`), arrows also accepted.
- Extraction output is model-generated and untrusted: `pi.rs` tolerates JSON in
  prose, numbers or strings for money, missing totals (falls back to
  subtotal+tax), and missing dates (falls back to today).

## Testing

Unit tests are inline `#[cfg(test)]` modules in `money.rs`, `split.rs`, and
`pi.rs`. Run them with the cargo command above. When changing split/rounding
behaviour, add or update tests there first.

## Gotchas

- `Receipt.items` and `Person` IDs are UUIDs generated in Rust, never by the model.
- State is keyed by slugified merchant + transaction date; two receipts from the
  same merchant on the same day share a state file by design.
- Non-interactive `pi` runs use `--no-*` flags to avoid loading project
  extensions/context; keep those when calling pi for extraction.
