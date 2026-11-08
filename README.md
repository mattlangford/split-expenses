# split

Terminal UI for splitting a restaurant bill from a photo of the receipt. It shells
out to [`pi`](https://github.com/earendil-works/pi) with a vision model to read the
receipt, then lets you add people and tick off who shared each item.

Flow: **Extract → Grid** (a single view)

## Build & run

```sh
cargo build --release
./target/release/split path/to/receipt.jpg
```

Requires `pi` on your `PATH` configured with an image-capable model
(`pi --list-models` shows an `images` column).

## Flags

| Flag | Description |
|------|-------------|
| `--model <MODEL>` | pi model used for extraction; must support images (default `google/gemini-3.1-flash-lite-image`) |
| `--state-dir <DIR>` | Root directory for saved state (default `/tmp/split-expense`) |
| `--pi-bin <PATH>` | pi executable to shell out to (default `pi`) |

## Keys (vim-style: Normal + Insert modes)

Insert mode is entered with `Enter` (or `a` for a person); `Enter` commits, `Esc` cancels.
Normal mode navigates with `h` `j` `k` `l` (arrows also work). There is a single view.

| Keys | Action |
|------|--------|
| `hjkl` / arrows | move across cells: item name, qty, total, each person, `+ person` |
| `Enter` | on an item×person cell, toggle that person's share; otherwise edit the focused cell inline |
| `Space` | toggle the focused assignment |
| typing on `+ person` / `+ item` | add several people/items in a row (Enter for next, Esc done) |
| `a` | add a person |
| `d` | delete the focused item (its name cell) or person (its header) |
| `m` `D` `t` `S` `T` `%` | edit merchant, date, tax, subtotal, receipt total, tip % |
| `s` / `q` / `Ctrl+C` | save / save & quit / quit |
| `r` | retry extraction after an error |

## Splitting rules

- The grid is items (rows) × people (columns); each item is split **equally among
  the people ticked for it** (3 beers + 3 people = ⅓ each).
- **Tip** is a percentage of the pre-tax subtotal (basis points internally; e.g.
  `2000` = 20%). If the receipt prints a tip/gratuity, that amount is used instead.
- **Tax and tip** are allocated proportionally to each person's item subtotal.
- The model reports whether the printed total already includes tax and/or tip. The
  pre-tip reconciliation and the "total to split" respect those flags. Item prices
  are assumed to exclude tax.
- Money is integer cents; allocations use the largest-remainder method so
  per-person amounts sum exactly to the item/tax/tip totals.
- Two checks run at the bottom of the grid: a **receipt check** (green when parsed
  items+tax tie to the parsed total, red with the delta when they don't) and an
  **allocation check** (green when everyone's shares sum to items+tax+tip, yellow
  showing unassigned items and the shortfall when they don't).

## State

```
/tmp/split-expense/<merchant-slug>/<YYYY-MM-DD>/state.json
/tmp/split-expense/by-image/<image-content-hash>   -> points at the file above
```

Passing the **same image** again reloads its saved receipt, people, assignments,
and tip automatically (no re-OCR).

Errors hit during the session (extraction failure, save failure) are printed to the
terminal after the UI exits, so they stay in your scrollback once you quit with `q`
or `Ctrl+C`.

## Extraction

Runs headlessly in `--mode json`, streaming events so the UI can show the stage,
elapsed time, and partial output while the model works:

```sh
pi --mode json --model <model> --no-session --no-tools --no-extensions \
   --no-skills --no-context-files --no-prompt-templates --no-themes \
   @<image> "<receipt-parsing prompt>"
```

The single JSON object is parsed from the final assistant message.
