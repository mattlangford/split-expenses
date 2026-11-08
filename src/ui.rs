use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Table, Wrap};

use crate::app::{App, DeleteTarget, EditTarget, ITEM_COLS, Mode, Screen, money};
use crate::money::format_percent_bps;
use crate::split::percent_of;

pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Min(5),
        Constraint::Length(8),
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
    render_help(frame, app);
    render_confirm(frame, app);
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
            Span::styled(
                format!("   {elapsed}s"),
                Style::default().fg(Color::DarkGray),
            ),
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
            "press r to retry, Ctrl+C to quit",
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
    let grid_rows = receipt.items.len() + 5;
    let grid_focus = app.row < grid_rows;

    let on_item_row = grid_focus && app.row >= 1 && app.row <= receipt.items.len();

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
            grid_focus && app.row == 0 && app.col == 0,
            false,
            true,
            on_item_row && app.col == 0,
        ),
        grid_cell(
            "Qty".to_string(),
            grid_focus && app.row == 0 && app.col == 1,
            false,
            true,
            on_item_row && app.col == 1,
        ),
        grid_cell(
            "Total".to_string(),
            grid_focus && app.row == 0 && app.col == 2,
            false,
            true,
            on_item_row && app.col == 2,
        ),
    ];
    for (index, person) in people.iter().enumerate() {
        let selected = grid_focus && app.row == 0 && app.col == ITEM_COLS + index;
        let editing = matches!(app.edit, Some(EditTarget::GridPersonName(i)) if i == index);
        let crosshair = on_item_row && app.col == ITEM_COLS + index;
        let text = edit_text(editing, &app.input, person.name.clone());
        header.push(grid_cell(text, selected, editing, true, crosshair));
    }
    let add_selected = grid_focus && app.row == 0 && app.col == ITEM_COLS + people.len();
    let add_editing = matches!(app.edit, Some(EditTarget::GridNewPerson));
    let add_text = edit_text(add_editing, &app.input, "+ person");
    header.push(grid_cell(
        add_text,
        add_selected,
        add_editing,
        true,
        on_item_row && app.col == ITEM_COLS + people.len(),
    ));
    rows.push(Row::new(header));

    // One row per item.
    for (row_index, item) in receipt.items.iter().enumerate() {
        let row_selected = grid_focus && app.row == row_index + 1;
        let name_editing = matches!(app.edit, Some(EditTarget::GridItemName(i)) if i == row_index);
        let qty_editing = matches!(app.edit, Some(EditTarget::GridItemQty(i)) if i == row_index);
        let total_editing =
            matches!(app.edit, Some(EditTarget::GridItemTotal(i)) if i == row_index);
        let crosshair = on_item_row && app.row == row_index + 1;

        let name_text = edit_text(name_editing, &app.input, item.name.clone());
        let qty_text = edit_text(qty_editing, &app.input, item.qty.to_string());
        let total_text = edit_text(total_editing, &app.input, money(item.line_total_cents));

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
                focus_style(true, false)
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
    let add_row_selected = grid_focus && app.row == add_row;
    let add_row_editing = matches!(app.edit, Some(EditTarget::GridNewItem));
    let add_row_text = edit_text(add_row_editing, &app.input, "+ item");
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

    // Tax and Tip percentage rows.
    let tax_row = receipt.items.len() + 2;
    let tip_row = receipt.items.len() + 3;
    let owed_row = receipt.items.len() + 4;

    let tax_selected = grid_focus && app.row == tax_row;
    let tax_editing = matches!(app.edit, Some(EditTarget::TaxPercent));
    let tax_text = edit_text(
        tax_editing,
        &app.input,
        format!("{}%", format_percent_bps(receipt.tax_bps)),
    );
    let mut tax_cells = vec![
        grid_cell("Tax".to_string(), tax_selected, false, true, false),
        empty_cell(false, false),
        grid_cell(tax_text, tax_selected, tax_editing, true, false),
    ];
    for _ in 0..people.len() + 1 {
        tax_cells.push(empty_cell(false, false));
    }
    rows.push(Row::new(tax_cells));

    let tip_selected = grid_focus && app.row == tip_row;
    let tip_editing = matches!(app.edit, Some(EditTarget::TipPercent));
    let tip_text = edit_text(
        tip_editing,
        &app.input,
        format!("{}%", format_percent_bps(app.tip_bps)),
    );
    let mut tip_cells = vec![
        grid_cell("Tip".to_string(), tip_selected, false, true, false),
        empty_cell(false, false),
        grid_cell(tip_text, tip_selected, tip_editing, true, false),
    ];
    for _ in 0..people.len() + 1 {
        tip_cells.push(empty_cell(false, false));
    }
    rows.push(Row::new(tip_cells));

    // "Owed" summary row: each person's total under their column.
    let owed_selected = grid_focus && app.row == owed_row;
    let split = app.split_result();
    let mut owed_cells = vec![
        grid_cell(
            "Owed".to_string(),
            owed_selected && app.col == 0,
            false,
            true,
            false,
        ),
        empty_cell(owed_selected && app.col == 1, false),
        empty_cell(owed_selected && app.col == 2, false),
    ];
    for (index, _) in people.iter().enumerate() {
        let amount = split
            .as_ref()
            .and_then(|s| s.shares.get(index))
            .map(|share| money(share.total_cents()))
            .unwrap_or_default();
        owed_cells.push(grid_cell(
            amount,
            owed_selected && app.col == ITEM_COLS + index,
            false,
            true,
            false,
        ));
    }
    owed_cells.push(empty_cell(
        owed_selected && app.col == ITEM_COLS + people.len(),
        false,
    ));
    rows.push(Row::new(owed_cells));

    let title = format!(
        " Receipt · {} · {} ",
        receipt.merchant, receipt.transaction_date
    );
    let table = Table::new(rows, widths).block(Block::bordered().title(title));
    frame.render_widget(table, area);
}

fn render_details(frame: &mut Frame, area: Rect, app: &App) {
    let Some(receipt) = &app.receipt else {
        return;
    };
    let focus = app.detail_focus();

    let field = |index: usize, label: &str, target: EditTarget, value: String| -> Line<'static> {
        let editing = app.edit.as_ref() == Some(&target);
        let focused = focus == Some(index);
        let text = if editing {
            format!("{label:<14}{}▏", app.input)
        } else {
            format!("{label:<14}{value}")
        };
        let style = if editing {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if focused {
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Line::from(Span::styled(text, style))
    };

    let subtotal = receipt.subtotal_or_items_cents();
    let lines = vec![
        field(
            0,
            "Merchant",
            EditTarget::Merchant,
            receipt.merchant.clone(),
        ),
        field(
            1,
            "Date",
            EditTarget::Date,
            receipt.transaction_date.to_string(),
        ),
        field(
            2,
            "Subtotal",
            EditTarget::Subtotal,
            receipt
                .subtotal_cents
                .map(money)
                .unwrap_or_else(|| money(subtotal)),
        ),
        field(
            3,
            "Tax",
            EditTarget::TaxPercent,
            format!(
                "{} ({}%)",
                money(percent_of(subtotal, receipt.tax_bps)),
                format_percent_bps(receipt.tax_bps)
            ),
        ),
        field(4, "Total", EditTarget::Total, money(receipt.total_cents)),
        field(
            5,
            "Tip",
            EditTarget::TipPercent,
            format!(
                "{} ({}%)",
                money(percent_of(subtotal, app.tip_bps)),
                format_percent_bps(app.tip_bps)
            ),
        ),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(" Receipt "))
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
    if split.shares.is_empty() {
        render_bar(
            frame,
            area,
            "⚠ no people yet — move to + person and type a name",
            Color::Yellow,
            Color::Black,
        );
        return;
    }

    if split.owed_matches {
        render_bar(
            frame,
            area,
            &format!(
                "✓ everyone's shares sum to the total {}",
                money(split.expected_charge_cents)
            ),
            Color::Green,
            Color::Black,
        );
    } else {
        let delta = split.expected_charge_cents - split.owed_cents;
        let unassigned = if split.unassigned.is_empty() {
            String::new()
        } else {
            format!("   ·   {} item(s) unassigned", split.unassigned.len())
        };
        render_bar(
            frame,
            area,
            &format!(
                "⚠ owed {} of {}{}   (Δ {})",
                money(split.owed_cents),
                money(split.expected_charge_cents),
                unassigned,
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
            Screen::Extracting => "r retry · Ctrl+C quit",
            Screen::Grid => {
                "? help · arrows move · type or Enter edit · Space toggle · d delete · u undo · Ctrl+C quit"
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

fn render_help(frame: &mut Frame, app: &App) {
    let Some(text) = &app.help else {
        return;
    };
    let screen = frame.area();
    let width = screen.width.saturating_sub(4).clamp(24, 72);
    let lines = text.lines().count() as u16;
    let height = (lines + 2).clamp(3, screen.height.saturating_sub(2).max(3));
    let x = screen.x + screen.width.saturating_sub(width) / 2;
    let y = screen.y + screen.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(text.clone())
            .block(Block::bordered().title(" Info · any key to close "))
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn render_confirm(frame: &mut Frame, app: &App) {
    let Some(target) = &app.confirm_delete else {
        return;
    };
    let label = match target {
        DeleteTarget::Item(index) => app
            .receipt
            .as_ref()
            .and_then(|receipt| receipt.items.get(*index))
            .map(|item| format!("item \"{}\"", item.name))
            .unwrap_or_else(|| "this item".to_string()),
        DeleteTarget::Person(index) => app
            .people
            .get(*index)
            .map(|person| format!("person \"{}\"", person.name))
            .unwrap_or_else(|| "this person".to_string()),
    };
    let text =
        format!("Delete {label}?\n\nThis removes it from the split.\n\ny = yes    n / Esc = no");

    let screen = frame.area();
    let width = screen.width.saturating_sub(4).clamp(28, 60);
    let height = 6u16.clamp(3, screen.height.saturating_sub(2).max(3));
    let x = screen.x + screen.width.saturating_sub(width) / 2;
    let y = screen.y + screen.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::bordered()
                    .title(" Confirm ")
                    .style(Style::default().fg(Color::Yellow)),
            )
            .wrap(Wrap { trim: true }),
        popup,
    );
}

/// Style for the cell under the cursor and the row it crosses.
fn focus_style(selected: bool, crosshair: bool) -> Style {
    if selected {
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
    }
}

/// Show the live input with a caret while editing, otherwise the committed value.
fn edit_text(editing: bool, input: &str, value: impl Into<String>) -> String {
    if editing {
        format!("{input}▏")
    } else {
        value.into()
    }
}

fn grid_cell(
    text: String,
    selected: bool,
    editing: bool,
    bold: bool,
    crosshair: bool,
) -> Cell<'static> {
    let style = if editing {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else if selected || crosshair {
        focus_style(selected, crosshair)
    } else if bold {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Cell::from(Span::styled(text, style))
}

fn empty_cell(selected: bool, crosshair: bool) -> Cell<'static> {
    Cell::from(Span::styled(" ", focus_style(selected, crosshair)))
}

fn tail_chars(input: &str, max: usize) -> String {
    let mut chars: Vec<char> = input.chars().rev().take(max).collect();
    chars.reverse();
    let mut out = String::from("…");
    out.extend(chars);
    out
}
