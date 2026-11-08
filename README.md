# split

Terminal UI for splitting a restaurant bill from a photo of the receipt. It shells
out to [`pi`](https://github.com/earendil-works/pi) with a vision model to read the
receipt, then lets you add people and tick off who shared each item.

Flow: **Extract → Receipt → Grid → Summary**

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

Insert mode is entered with `i`/`a`/`e` or `Enter`; `Enter` commits, `Esc` cancels.
Normal mode navigates with `h` `j` `k` `l` (arrows also work).

| Screen | Keys |
|--------|------|
| Any | `s` save · `q` save & quit · `Ctrl+C` save & quit |
| Extract | `r` retry on error |
| Receipt | `hjkl` move cell · `Enter` edit the focused field (name/qty/unit/line) · `g`/`G` top/bottom · `a` add · `d` delete · `m` merchant · `D` date · `t` tax · `S` subtotal · `T` total · `%` tip |
| Grid | `hjkl` move between cells · `Enter` toggles the assignment or edits the focused name · typing on `+ person` / `+ item` enters names inline (Enter for next, Esc done) · `d` deletes the row/column |
| Summary | `y` copy |
| Windows | `k`/`↑` at the top row enters the window bar; only there do `h`/`l` switch screens (tabs highlight) and `j`/`Esc` returns |

Screens switch **only** from the window bar, and the tabs only highlight while it is
focused. At the top of the Grid or Receipt screen, pressing `k` again moves the cursor
into the window bar (on Summary, `k` enters it directly).

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
- Grid/Summary show a reconciliation line (computed pre-tip vs the amount the
  receipt total implies) and flag unassigned items.

## State

```
/tmp/split-expense/<merchant-slug>/<YYYY-MM-DD>/state.json
/tmp/split-expense/by-image/<image-content-hash>   -> points at the file above
```

Passing the **same image** again reloads its saved receipt, people, assignments,
and tip automatically (no re-OCR).

Errors hit during the session (extraction failure, save failure, clipboard
failure) are printed to the terminal after the UI exits, so they stay in your
scrollback once you quit with `q` or `Ctrl+C`.

## Extraction

Runs headlessly in `--mode json`, streaming events so the UI can show the stage,
elapsed time, and partial output while the model works:

```sh
pi --mode json --model <model> --no-session --no-tools --no-extensions \
   --no-skills --no-context-files --no-prompt-templates --no-themes \
   @<image> "<receipt-parsing prompt>"
```

The single JSON object is parsed from the final assistant message.
