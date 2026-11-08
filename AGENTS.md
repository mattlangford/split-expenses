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
src/types.rs  Receipt (incl. tax/tip inclusion flags), ReceiptItem, Person, Assignments
src/money.rs  cents parse/format, parse/format tip basis points, allocate()
src/split.rs  compute_split(): per-person allocation + reconciliation
src/pi.rs     extract_receipt(): shell out to pi (--mode json), stream progress, normalize JSON -> Receipt
src/state.rs  save/load /tmp/.../state.json keyed by image hash, slug, clipboard
src/app.rs    App state machine, spreadsheet grid, vim Normal/Insert key handling
src/ui.rs     ratatui rendering for all screens
```

Dependency direction: `app`/`ui`/`pi`/`state` depend on `types`/`money`/`split`.
Keep the math modules free of I/O and TUI types.

## Conventions

- **Money is always integer cents** (`i64`), never floats. Parse/format via
  `money.rs`.
- **Allocations go through `money::allocate`** (largest-remainder) so split sums
  are exact. Do not hand-roll proportional division.
- Tip is stored as **basis points** (`u32`), default `2000` (= 20%), computed on
  the pre-tax subtotal; a tip/gratuity printed on the receipt takes precedence.
- Item split rule: `line_total` divided equally among the people assigned to that
  item (`Assignments`).
- Tax and tip are allocated in proportion to each person's item subtotal.
- The TUI has a Normal/Insert mode model: `app.rs` handles keys, `ui.rs` only
  renders. Navigation is vim-style (`hjkl`), arrows also accepted.
- Quit keys are `q` (Normal mode) and `Ctrl+C` (any mode); both save state first.
- Errors are shown in the TUI **and** replayed to stderr after the terminal is
  restored, so they survive exit. Route failure paths through `App::record_error`
  (or `App::error` for extraction) rather than only setting `status`; `main`
  calls `App::print_terminal_messages` after `ratatui::restore()`.
- Extraction output is model-generated and untrusted: `pi.rs` tolerates JSON in
  prose, numbers or strings for money, missing totals (falls back to
  subtotal+tax), and missing dates (falls back to today).
- State is keyed by the receipt image's content hash (`state::image_hash`). Passing
  the same image reloads the saved split at startup and skips OCR
  (`state::load_by_image`); the canonical file still lives under
  `/tmp/split-expense/<merchant>/<date>/` with a pointer in `by-image/<hash>`.
- Screens are Extract / Receipt / Grid / Summary. The Grid is a spreadsheet
  (items × people): `hjkl` moves between cells, `Enter` toggles the focused
  assignment or edits the focused name, and the trailing `+ person` column / `+ item`
  row open inline text entry. `App::adding_people` and `adding_items` keep that
  entry going across Enters; `begin_edit` clears them and `App::edit_prompt` labels
  the footer.
- The Receipt item table is cell-editable too: `hjkl` moves across name/qty/unit/line
  and `Enter` edits the focused cell inline (receipt-level fields use the footer keys
  `m`/`D`/`t`/`S`/`T`/`%`).
- Pressing up (`k`) at the top row of Grid/Receipt moves the cursor into the window
  bar (`App::window_focus`): the tabs get the blue highlight and `h`/`l` switch
  screens via `switch_screen`, while `j`/`Esc` returns. Outside window focus the tabs
  are not highlighted and `h`/`l` move within the current screen. Screen switching
  happens only through `switch_screen` from window focus; do not add other next/prev
  screen shortcuts.
- Receipt composition comes from the model: `total_includes_tax`,
  `total_includes_tip`, and an optional printed `tip_cents`. Item prices are assumed
  to exclude tax. `split::compute_split` uses these so reconciliation compares the
  implied pre-tip amount. Keep that logic in `split.rs`.

## Testing

Unit tests are inline `#[cfg(test)]` modules in `money.rs`, `split.rs`, and
`pi.rs`. Run them with the cargo command above. When changing split/rounding
behaviour, add or update tests there first.

## Gotchas

- `Receipt.items` and `Person` IDs are UUIDs generated in Rust, never by the model.
- The merchant/date directory is only for human organisation; loading is by image
  hash, so two different photos from the same merchant/day do not share assignments.
- Non-interactive `pi` runs use `--no-*` flags to avoid loading project
  extensions/context; keep those when calling pi for extraction.
