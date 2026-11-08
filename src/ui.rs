use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, Wrap};

use crate::app::{App, EditTarget, ITEM_COLS, Mode, Screen, money};
use crate::money::format_percent_bps;

pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Min(5),
        Constraint::Length(5),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(frame.area());

    match app.screen {
        Screen::Extracting => render_extracting(frame, chunks[0], app),
        Screen::Grid => {
            render_grid(frame, chunks[0], app);
            render_details(frame, chunks[1], app);
            render_receipt_banner(frame, chunks[2], app);
            render_owed_banner(frame, chunks[3], app);
        }
    }
    render_footer(frame, chunks[4], app);
}

fn render_extracting(frame: &mut Frame, area: Rect, app: &App) {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let spinner = FRAMES[app.spinner % FRAMES.len()];
    let elapsed = app.extract_started.elapsed().as_secs();
    let mut lines = vec![
        Line::from(vec![
            Span::styled(spinner, Style::default().fg(Color::Cyan)),
            Span::raw("  Reading receipt with "),
            Span::styled(app.args.model.clone(), Style::default().fg(Color::Yellow)),
            Span::styled(format!("   {elapsed}s"), Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(Span::styled(
            format!("image: {}", app.args.image.display()),
            Style::default().fg(Color::DarkGray),
        )),
    ];

    if let Some(error) = &app.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("error: {error}"),
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from(Span::styled(
            "press r to retry, q to quit",
            Style::default().fg(Color::Yellow),
        )));
    } else {
        if !app.extract_status.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                app.extract_status.clone(),
                Style::default().fg(Color::Cyan),
            )));
        }
        if app.extract_partial.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "waiting for pi… (large images and reasoning models can take 10–30s)",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "model output so far:",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::styled(
                tail_chars(&app.extract_partial, 600),
                Style::default().fg(Color::Gray),
            )));
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(" Extract "))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_grid(frame: &mut Frame, area: Rect, app: &App) {
    let Some(receipt) = &app.receipt else {
        return;
    };
    let people = &app.people;

    let person_index = if app.col >= ITEM_COLS && app.col < ITEM_COLS + people.len() {
        Some(app.col - ITEM_COLS)
    } else {
        None
    };
    let on_assignment =
        app.row >= 1 && app.row <= receipt.items.len() && person_index.is_some();

    let mut widths = vec![
        Constraint::Min(16),
        Constraint::Length(5),
        Constraint::Length(10),
    ];
    for _ in people {
        widths.push(Constraint::Length(10));
    }
    widths.push(Constraint::Length(12));

    let mut rows: Vec<Row> = Vec::new();

    // Header row: item fields, people columns, and an "add person" column.
    let mut header = vec![
        grid_cell(
            "Item".to_string(),
            app.row == 0 && app.col == 0,
            false,
            true,
            false,
        ),
        grid_cell(
            "Qty".to_string(),
            app.row == 0 && app.col == 1,
            false,
            true,
            false,
        ),
        grid_cell(
            "Total".to_string(),
            app.row == 0 && app.col == 2,
            false,
            true,
            false,
        ),
    ];
    for (index, person) in people.iter().enumerate() {
        let selected = app.row == 0 && app.col == ITEM_COLS + index;
        let editing = matches!(app.edit, Some(EditTarget::GridPersonName(i)) if i == index);
        let crosshair = on_assignment && person_index == Some(index);
        let text = if editing {
            format!("{}▏", app.input)
        } else {
            person.name.clone()
        };
        header.push(grid_cell(text, selected, editing, true, crosshair));
    }
    let add_selected = app.row == 0 && app.col == ITEM_COLS + people.len();
    let add_editing = matches!(app.edit, Some(EditTarget::GridNewPerson));
    let add_text = if add_editing {
        format!("{}▏", app.input)
    } else {
        "+ person".to_string()
    };
    header.push(grid_cell(add_text, add_selected, add_editing, true, false));
    rows.push(Row::new(header));

    // One row per item.
    for (row_index, item) in receipt.items.iter().enumerate() {
        let row_selected = app.row == row_index + 1;
        let name_editing = matches!(app.edit, Some(EditTarget::GridItemName(i)) if i == row_index);
        let qty_editing = matches!(app.edit, Some(EditTarget::GridItemQty(i)) if i == row_index);
        let total_editing =
            matches!(app.edit, Some(EditTarget::GridItemTotal(i)) if i == row_index);
        let crosshair = on_assignment && app.row == row_index + 1;

        let name_text = if name_editing {
            format!("{}▏", app.input)
        } else {
            item.name.clone()
        };
        let qty_text = if qty_editing {
            format!("{}▏", app.input)
        } else {
            item.qty.to_string()
        };
        let total_text = if total_editing {
            format!("{}▏", app.input)
        } else {
            money(item.line_total_cents)
        };

        let mut cells = vec![
            grid_cell(
                name_text,
                row_selected && app.col == 0,
                name_editing,
                false,
                crosshair,
            ),
            grid_cell(
                qty_text,
                row_selected && app.col == 1,
                qty_editing,
                false,
                false,
            ),
            grid_cell(
                total_text,
                row_selected && app.col == 2,
                total_editing,
                false,
                false,
            ),
        ];

        for (col_index, person) in people.iter().enumerate() {
            let assigned = app.assignments.is_assigned(&item.id, &person.id);
            let selected = row_selected && app.col == ITEM_COLS + col_index;
            let mark = if assigned { " [x] " } else { " [ ] " };
            let style = if selected {
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else if assigned {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            cells.push(Cell::from(Span::styled(mark.to_string(), style)));
        }
        cells.push(empty_cell(
            row_selected && app.col == ITEM_COLS + people.len(),
            false,
        ));
        rows.push(Row::new(cells));
    }

    // Add-item row.
    let add_row = receipt.items.len() + 1;
    let add_row_selected = app.row == add_row;
    let add_row_editing = matches!(app.edit, Some(EditTarget::GridNewItem));
    let add_row_text = if add_row_editing {
        format!("{}▏", app.input)
    } else {
        "+ item".to_string()
    };
    let mut add_cells = vec![
        grid_cell(
            add_row_text,
            add_row_selected && app.col == 0,
            add_row_editing,
            false,
            false,
        ),
        empty_cell(add_row_selected && app.col == 1, false),
        empty_cell(add_row_selected && app.col == 2, false),
    ];
    for offset in 0..people.len() + 1 {
        add_cells.push(empty_cell(
            add_row_selected && app.col == ITEM_COLS + offset,
            false,
        ));
    }
    rows.push(Row::new(add_cells));

    let table = Table::new(rows, widths).block(
        Block::bordered().title(" Grid (hjkl move · Enter edit/toggle · type on + cells to add) "),
    );
    frame.render_widget(table, area);
}

fn render_details(frame: &mut Frame, area: Rect, app: &App) {
    let Some(receipt) = &app.receipt else {
        return;
    };
    let mut lines = vec![
        Line::from(format!(
            "Merchant: {}    Date: {}    Tip: {}%",
            receipt.merchant,
            receipt.transaction_date,
            format_percent_bps(app.tip_bps),
        )),
        Line::from(format!(
            "Tax: {}    Subtotal: {}    Receipt total: {}   (tax {}, tip {})",
            money(receipt.tax_cents),
            receipt
                .subtotal_cents
                .map(money)
                .unwrap_or_else(|| "—".to_string()),
            money(receipt.total_cents),
            if receipt.total_includes_tax {
                "included"
            } else {
                "excluded"
            },
            if receipt.total_includes_tip {
                "included"
            } else {
                "excluded"
            },
        )),
    ];
    if let Some(split) = app.split_result() {
        let owed: Vec<String> = split
            .shares
            .iter()
            .map(|share| format!("{} {}", share.name, money(share.total_cents())))
            .collect();
        if !owed.is_empty() {
            lines.push(Line::from(format!("Owed: {}", owed.join("   "))));
        }
    }
    lines.push(Line::from(Span::styled(
        "keys: hjkl move · Enter edit/toggle · a add person · d delete · m merchant · D date · t tax · S subtotal · T total · % tip · s save",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(" Details "))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_receipt_banner(frame: &mut Frame, area: Rect, app: &App) {
    let Some(split) = app.split_result() else {
        return;
    };
    let (text, background, foreground) = if split.total_matches {
        (
            format!(
                "✓ receipt check: parsed items+tax {} matches receipt total {}",
                money(split.comparable_cents),
                money(split.receipt_total_cents)
            ),
            Color::Green,
            Color::Black,
        )
    } else {
        let delta = split.comparable_cents - split.receipt_total_cents;
        (
            format!(
                "✗ receipt check: parsed items+tax {} vs receipt total {}   (Δ {}{})",
                money(split.comparable_cents),
                money(split.receipt_total_cents),
                if delta >= 0 { "+" } else { "-" },
                money(delta.abs())
            ),
            Color::Red,
            Color::White,
        )
    };
    render_bar(frame, area, &text, background, foreground);
}

fn render_owed_banner(frame: &mut Frame, area: Rect, app: &App) {
    let Some(split) = app.split_result() else {
        return;
    };
    let owed: i64 = split.shares.iter().map(|share| share.total_cents()).sum();
    let target = split.computed_total_cents;

    if split.shares.is_empty() {
        render_bar(
            frame,
            area,
            "⚠ no people yet — put the cursor on + person and press Enter",
            Color::Yellow,
            Color::Black,
        );
        return;
    }

    let complete = split.unassigned.is_empty() && owed == target;
    if complete {
        render_bar(
            frame,
            area,
            &format!("✓ everyone's shares sum to {}", money(target)),
            Color::Green,
            Color::Black,
        );
    } else {
        let delta = target - owed;
        render_bar(
            frame,
            area,
            &format!(
                "⚠ owed {} of {}   ·   {} item(s) unassigned   (Δ {})",
                money(owed),
                money(target),
                split.unassigned.len(),
                money(delta)
            ),
            Color::Yellow,
            Color::Black,
        );
    }
}

fn render_bar(frame: &mut Frame, area: Rect, text: &str, background: Color, foreground: Color) {
    if area.height == 0 {
        return;
    }
    let style = Style::default()
        .bg(background)
        .fg(foreground)
        .add_modifier(Modifier::BOLD);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(format!(" {text}"), style)))
            .block(Block::default().style(style)),
        area,
    );
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let line = if app.mode == Mode::Insert {
        Line::from(vec![
            Span::styled(
                format!(" {} ", app.edit_prompt()),
                Style::default()
                    .bg(Color::Yellow)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::raw(app.input.clone()),
            Span::styled(
                "   (Enter save · Esc cancel)",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        let hints = match app.screen {
            Screen::Extracting => "r retry · q quit",
            Screen::Grid => {
                "hjkl move · Enter edit/toggle · Space toggle · a add person · d delete · s save · q quit"
            }
        };
        let status = if app.status.is_empty() {
            String::new()
        } else {
            format!("   · {}", app.status)
        };
        Line::from(vec![
            Span::styled(hints, Style::default().fg(Color::DarkGray)),
            Span::styled(status, Style::default().fg(Color::Cyan)),
        ])
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn grid_cell(
    text: String,
    selected: bool,
    editing: bool,
    bold: bool,
    crosshair: bool,
) -> Cell<'static> {
    let style = if editing {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else if selected {
        Style::default()
            .bg(Color::Cyan)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else if crosshair {
        Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else if bold {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Cell::from(Span::styled(text, style))
}

fn empty_cell(selected: bool, crosshair: bool) -> Cell<'static> {
    let style = if selected {
        Style::default()
            .bg(Color::Cyan)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else if crosshair {
        Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Cell::from(Span::styled(" ", style))
}

fn tail_chars(input: &str, max: usize) -> String {
    let mut chars: Vec<char> = input.chars().rev().take(max).collect();
    chars.reverse();
    let mut out = String::from("…");
    out.extend(chars);
    out
}
