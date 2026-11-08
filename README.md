# split

A terminal UI for splitting a restaurant bill from a photo of the receipt.
It shells out to [`pi`](https://github.com/earendil-works/pi) with a vision model
to read the receipt, then lets you add people and tick off who shared each item.

```
launch ─▶ Extract ─▶ Receipt ─▶ People ─▶ Assign ─▶ Summary
             │          │          │         │          │
          pi vision   edit $     add/        [x] per   per-person
          OCR         tax/tip    remove      item/     totals +
                      reconcile              person    reconciliation
```

## Requirements

- Rust toolchain (`cargo`).
- `pi` on your `PATH`, configured with a model that accepts images
  (`pi --list-models` shows an `images` column).

If `cargo` is not on `PATH` in your environment, `scripts/dev-cargo.sh`
falls back to a nix-provided toolchain.

## Build & run

```sh
cargo build --release
./target/release/split path/to/receipt.jpg
```

The default model is `google/gemini-3.1-flash-lite-image`. Override it with `--model`:

```sh
./target/release/split --model openrouter/~anthropic/claude-haiku-latest receipt.jpg
```

Or during development:

```sh
./scripts/dev-cargo.sh run -- receipt.jpg
```

### Flags

| Flag | Description |
|------|-------------|
| `--model <MODEL>` | pi model used for extraction; must support images (default `google/gemini-3.1-flash-lite-image`) |
| `--state-dir <DIR>` | Root directory for saved state (default `/tmp/split-expense`) |
| `--pi-bin <PATH>` | pi executable to shell out to (default `pi`) |

## Vim keybindings

The UI has a **Normal** and an **Insert** mode, like vim.

- Normal mode: navigate with `h` `j` `k` `l` (arrow keys work too).
- Insert mode: triggered by `i`/`a`/`e` or `Enter`; `Enter` commits, `Esc` cancels.

| Screen | Keys |
|--------|------|
| Any (Normal) | `s` save state · `q` save and quit |
| Extract | `r` retry on error |
| Receipt | `j`/`k` move · `g`/`G` top/bottom · `i` name · `e` price · `u` qty · `L` line total · `a` add item · `d` delete · `m` merchant · `D` date · `t` tax · `S` subtotal · `T` total · `%` tip percent |
| People | `j`/`k` move · `a` add · `i` rename · `d` delete |
| Assign | `h`/`j`/`k`/`l` move cell · `Space` toggle · `A` all people on row · `o` solo · `c` clear row |
| Summary | `y` copy summary to clipboard |
| Navigation | `h`/`l` (or `Tab`/`Shift+Tab`) previous/next screen · in Assign use `n`/`p` |

## Splitting rules

- Each item's line total is split **equally among the people ticked for it**.
  Three beers with three people ticked means each pays one third.
- **Tip** is a percentage (stored as basis points, e.g. `2000` = 20.00%) and is
  calculated on the subtotal. If the receipt prints a tip you can set the field
  to the implied percentage.
- **Tax and tip** are allocated in proportion to each person's item subtotal.
- All money is integer cents. Amounts are allocated with the
  **largest-remainder method**, so the per-person amounts always add up to the
  exact tax/tip/item totals (no lost or invented cents).
- The **Summary** and **Assign** screens show a reconciliation line comparing the
  computed total against the receipt total, and warn about unassigned items.

## State

State is saved to:

```
/tmp/split-expense/<merchant-slug>/<YYYY-MM-DD>/state.json
```

The merchant and transaction date come from the receipt (editable on the
Receipt screen). Re-running `split` with a receipt that maps to the same
merchant and date resumes the saved people, assignments, and tip percentage.

## How extraction works

`split` runs, headlessly:

```sh
pi --print --model <model> --no-session --no-tools --no-extensions \
   --no-skills --no-context-files --no-prompt-templates --no-themes \
   @<image> "<receipt-parsing prompt>"
```

and parses the single JSON object pi prints back into the receipt model.
