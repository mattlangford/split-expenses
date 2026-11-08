# split

Terminal UI for splitting a restaurant bill from a photo of the receipt. It shells
out to [`pi`](https://github.com/earendil-works/pi) with a vision model to read the
receipt, then lets you add people and tick off who shared each item.

Flow: **Extract → Receipt → People → Assign → Summary**

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
| Any | `s` save · `q` save & quit |
| Extract | `r` retry on error |
| Receipt | `j`/`k` move · `g`/`G` top/bottom · `i` name · `e` price · `u` qty · `L` line · `a` add · `d` delete · `m` merchant · `D` date · `t` tax · `S` subtotal · `T` total · `%` tip |
| People | `j`/`k` move · `a` add · `i` rename · `d` delete |
| Assign | `hjkl` move cell · `Space` toggle · `A` all on row · `o` solo · `c` clear |
| Summary | `y` copy |
| Screens | `h`/`l` or `Tab`/`Shift+Tab` (in Assign use `n`/`p`) |

## Splitting rules

- Each item is split **equally among the people ticked for it** (3 beers + 3 people = ⅓ each).
- **Tip** is a percentage of the subtotal (basis points internally; e.g. `2000` = 20%).
- **Tax and tip** are allocated proportionally to each person's item subtotal.
- Money is integer cents; allocations use the largest-remainder method so
  per-person amounts sum exactly to the item/tax/tip totals.
- Assign/Summary show a reconciliation line vs the receipt total and flag unassigned items.

## State

```
/tmp/split-expense/<merchant-slug>/<YYYY-MM-DD>/state.json
```

Merchant and date come from the receipt (editable). Re-running with the same
merchant/date resumes people, assignments, and tip.

## Extraction

Runs headlessly and parses the single JSON object it returns:

```sh
pi --print --model <model> --no-session --no-tools --no-extensions \
   --no-skills --no-context-files --no-prompt-templates --no-themes \
   @<image> "<receipt-parsing prompt>"
```
