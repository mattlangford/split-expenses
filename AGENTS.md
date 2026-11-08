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
src/cli.rs    clap Args (--model, --state-dir, --pi-bin, --clean)
src/types.rs  Receipt (incl. tax/tip inclusion flags), ReceiptItem, Person, Assignments
src/money.rs  cents parse/format, parse/format tip basis points, allocate()
src/split.rs  compute_split(): per-person allocation + reconciliation
src/pi.rs     extract_receipt(): shell out to pi (--mode json), stream progress, normalize JSON -> Receipt
src/state.rs  save/load /tmp/.../state.json keyed by image hash, slug, clipboard
src/app.rs    App state machine, single Grid view, vim Normal/Insert key handling
src/ui.rs     ratatui rendering: grid, details, match/allocation banners
```

Dependency direction: `app`/`ui`/`pi`/`state` depend on `types`/`money`/`split`.
Keep the math modules free of I/O and TUI types.

## Conventions

- **Money is always integer cents** (`i64`), never floats. Parse/format via
  `money.rs`.
- **Allocations go through `money::allocate`** (largest-remainder) so split sums
  are exact. Do not hand-roll proportional division.
- Tax and tip are stored as **basis points** (`Receipt::tax_bps`, `App::tip_bps`),
  applied to the receipt **subtotal** (`subtotal_cents`, falling back to the items
  sum) via `split::percent_of`. The parser fills `tax_bps` from the printed tax (or
  `total − subtotal`) and `finish_extraction` derives `tip_bps` from a printed
  gratuity; both are editable `Tax`/`Tip` rows in the grid.
- Item split rule: `line_total` divided equally among the people assigned to that
  item (`Assignments`).
- Tax and tip are allocated in proportion to each person's item subtotal, and only
  among people with a non-zero item subtotal. `money::allocate` returns all zeros for
  an all-zero weight vector, so an entirely unassigned bill owes nothing.
- The TUI has a Normal/Insert mode model: `app.rs` handles keys, `ui.rs` only
  renders. Navigation is **arrow keys only**; typing on the highlighted cell/field
  enters Insert mode with the typed character replacing the value.
- State autosaves after every change (`commit_edit`, toggles, deletes, undo,
  extraction); quit is `Ctrl+C` (saves state first).
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
  `--clean` bypasses the load and forces a fresh extraction.
- There is a single Grid view (screens are only `Extracting` and `Grid`), titled
  "Receipt · merchant · date". It is a spreadsheet: item name/qty/total columns, one
  column per person, then a `+ person` column, plus `+ item`, `Tax`, `Tip`, and
  `Owed` rows. Below the grid are four receipt detail rows (merchant, date, subtotal,
  receipt total) the cursor can move into. Typing starts editing the
  highlighted cell/field (`App::focused_target`): names/date accept any sensible
  character, numeric fields only when a digit/`.`/`-` is typed (`App::type_into_cell`).
  `Enter` commits, `Esc` cancels, and an arrow key commits then moves
  (`App::commit_and_move`). `Space`/`Enter` toggles an item×person assignment;
  `d`/`Delete` requests deletion behind a confirmation (`App::confirm_delete` /
  `DeleteTarget`). `+ person`/`+ item` add continuously across Enters. `?` opens the
  contextual info popup. Unit price is stored but not shown or edited.
- Undo (`u` or `Ctrl+Z`) uses an in-memory stack of `Snapshot`s (`receipt`, `people`,
  `assignments`, `tip_bps`, cursor). `App::checkpoint` is called before every mutation
  (`commit_edit`, `grid_toggle`, `confirm_delete_now`); `App::restore` clamps the
  cursor. Capped at 100 snapshots. There is no redo yet.
- The bottom of the grid shows two full-width coloured bars: a receipt check
  (`SplitResult::total_matches`, parsed items+tax vs parsed total) and an allocation
  check (`SplitResult::owed_matches`, sum of each person's owed vs
  `expected_charge_cents` = receipt total plus any tax/tip it excludes). Keep those
  two error conditions distinct.
- The model only **transcribes values printed on the receipt** (merchant, date,
  currency, item lines, subtotal, tax, tip, total). Composition is inferred in
  Rust: `pi::infer_inclusions` decides whether the printed total already includes
  tax/tip by testing which printed components reconcile with it. Item prices are
  assumed to exclude tax. `pi.rs` derives `tax_cents` from `total − subtotal`
  (minus any included tip) when the receipt does not itemise tax, and sets
  `tax_bps` via `money::rate_bps`. The receipt check compares the **parsed** items
  + parsed tax/tip to the parsed total (`SplitResult::comparable_cents`), while the
  allocation uses the chosen rates. Keep that logic in `split.rs`.

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
