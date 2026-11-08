use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, Wrap};

use crate::app::{App, EditTarget, Mode, Screen, money};
use crate::money::format_percent_bps;
use crate::split::SplitResult;

pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(frame.area());

    render_tabs(frame, chunks[0], app);
    match app.screen {
        Screen::Extracting => render_extracting(frame, chunks[1], app),
        Screen::Receipt => render_receipt(frame, chunks[1], app),
        Screen::Grid => render_grid(frame, chunks[1], app),
        Screen::Summary => render_summary(frame, chunks[1], app),
    }
    render_match_banner(frame, chunks[2], app);
    render_footer(frame, chunks[3], app);
}

/// Green/red full-width bar showing whether the computed total ties to the receipt.
fn render_match_banner(frame: &mut Frame, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let Some(split) = app.split_result() else {
        return;
    };
    let (text, background, foreground) = if split.total_matches {
        (
            format!(
                "✓ MATCHES RECEIPT   computed {} = receipt {}",
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
                "✗ MISMATCH   computed {} vs receipt {}   (Δ {}{})",
                money(split.comparable_cents),
                money(split.receipt_total_cents),
                if delta >= 0 { "+" } else { "-" },
                money(delta.abs())
            ),
            Color::Red,
            Color::White,
        )
    };
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

fn render_tabs(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![Span::styled(
        " split ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )];
    for index in 0..=3 {
        let screen = Screen::from_index(index);
        let active = app.screen == screen;
        let style = if app.window_focus && active {
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else if app.window_focus {
            Style::default().bg(Color::DarkGray).fg(Color::White)
        } else if active {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(format!(" {} ", screen.title()), style));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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

fn render_receipt(frame: &mut Frame, area: Rect, app: &App) {
    let Some(receipt) = &app.receipt else {
        return;
    };
    let chunks = Layout::vertical([Constraint::Min(4), Constraint::Length(10)]).split(area);

    let header = Row::new(vec![
        Cell::from("Item"),
        Cell::from("Qty"),
        Cell::from("Total"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = receipt
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let row_selected = index == app.row && !app.window_focus;
            let cell = |col: usize, text: String| {
                let target = match col {
                    0 => EditTarget::ItemName(index),
                    1 => EditTarget::ItemQty(index),
                    _ => EditTarget::ItemLineTotal(index),
                };
                let editing = app.edit.as_ref() == Some(&target);
                let display = if editing {
                    format!("{}▏", app.input)
                } else {
                    text
                };
                let style = if editing {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else if row_selected && app.col == col {
                    Style::default()
                        .bg(Color::Cyan)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Cell::from(Span::styled(display, style))
            };
            let row_style = if row_selected {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default()
            };
            Row::new(vec![
                cell(0, item.name.clone()),
                cell(1, item.qty.to_string()),
                cell(2, money(item.line_total_cents)),
            ])
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(16),
            Constraint::Length(4),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(Block::bordered().title(" Items "));
    frame.render_widget(table, chunks[0]);

    let items_sum: i64 = receipt.items.iter().map(|item| item.line_total_cents).sum();
    let mut lines = vec![
        Line::from(format!("Merchant: {}", receipt.merchant)),
        Line::from(format!(
            "Date: {}    Tip: {}%    Tax: {}",
            receipt.transaction_date,
            format_percent_bps(app.tip_bps),
            money(receipt.tax_cents),
        )),
        Line::from(format!(
            "Receipt total {}   (tax {}, tip {})",
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
    if let Some(tip) = receipt.tip_cents {
        lines.push(Line::from(format!("Printed tip/gratuity: {}", money(tip))));
    }
    lines.push(Line::from(format!(
        "Items sum: {}    Subtotal: {}",
        money(items_sum),
        receipt
            .subtotal_cents
            .map(money)
            .unwrap_or_else(|| "—".to_string()),
    )));
    if let Some(split) = app.split_result() {
        lines.extend(recon_lines(&split));
    }
    lines.push(Line::from(Span::styled(
        "i name · e price · u qty · L line · a add · d delete · m merchant · D date · t tax · S subtotal · T total · % tip",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(" Details "))
            .wrap(Wrap { trim: true }),
        chunks[1],
    );
}

fn render_grid(frame: &mut Frame, area: Rect, app: &App) {
    let Some(receipt) = &app.receipt else {
        return;
    };
    let people = &app.people;
    let active_cursor = !app.window_focus;
    let on_assignment = active_cursor
        && app.row >= 1
        && app.row <= receipt.items.len()
        && app.col >= 1
        && app.col <= people.len();
    let chunks = Layout::vertical([Constraint::Min(4), Constraint::Length(10)]).split(area);

    let mut widths = vec![Constraint::Min(20)];
    for _ in people {
        widths.push(Constraint::Length(10));
    }
    widths.push(Constraint::Length(12));

    let mut rows: Vec<Row> = Vec::new();

    // Header row: people columns plus an "add person" column.
    let item_header_selected = active_cursor && app.row == 0 && app.col == 0;
    let mut header = vec![grid_cell(
        "Item".to_string(),
        item_header_selected,
        false,
        true,
        false,
    )];
    for (index, person) in people.iter().enumerate() {
        let selected = active_cursor && app.row == 0 && app.col == index + 1;
        let editing = matches!(app.edit, Some(EditTarget::GridPersonName(i)) if i == index);
        let crosshair = on_assignment && app.col == index + 1;
        let text = if editing {
            format!("{}▏", app.input)
        } else {
            person.name.clone()
        };
        header.push(grid_cell(text, selected, editing, true, crosshair));
    }
    let add_selected = active_cursor && app.row == 0 && app.col == people.len() + 1;
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
        let mut cells = Vec::new();
        let name_selected = active_cursor && app.row == row_index + 1 && app.col == 0;
        let name_editing = matches!(app.edit, Some(EditTarget::GridItemName(i)) if i == row_index);
        let crosshair = on_assignment && app.row == row_index + 1;
        let name_text = if name_editing {
            format!("{}▏", app.input)
        } else {
            format!("{}  {}", item.name, money(item.line_total_cents))
        };
        cells.push(grid_cell(name_text, name_selected, name_editing, false, crosshair));

        for (col_index, person) in people.iter().enumerate() {
            let assigned = app.assignments.is_assigned(&item.id, &person.id);
            let selected = active_cursor && app.row == row_index + 1 && app.col == col_index + 1;
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
            active_cursor && app.row == row_index + 1 && app.col == people.len() + 1,
            false,
        ));
        rows.push(Row::new(cells));
    }

    // Add-item row.
    let add_row_selected = active_cursor && app.row == receipt.items.len() + 1 && app.col == 0;
    let add_row_editing = matches!(app.edit, Some(EditTarget::GridNewItem));
    let add_row_text = if add_row_editing {
        format!("{}▏", app.input)
    } else {
        "+ item".to_string()
    };
    let add_row = receipt.items.len() + 1;
    let mut add_cells = vec![grid_cell(
        add_row_text,
        add_row_selected,
        add_row_editing,
        false,
        false,
    )];
    for col_index in 1..=people.len() + 1 {
        add_cells.push(empty_cell(
            active_cursor && app.row == add_row && app.col == col_index,
            false,
        ));
    }
    rows.push(Row::new(add_cells));

    let table = Table::new(rows, widths)
        .block(Block::bordered().title(" Grid (hjkl move · Enter toggle/edit · type to add) "));
    frame.render_widget(table, chunks[0]);

    let mut lines = Vec::new();
    if let Some(split) = &app.split_result() {
        for share in &split.shares {
            lines.push(Line::from(format!(
                "{:<14} items {:>10}  tax {:>8}  tip {:>8}  owes {:>10}",
                share.name,
                money(share.items_cents),
                money(share.tax_cents),
                money(share.tip_cents),
                money(share.total_cents()),
            )));
        }
        lines.push(Line::from(""));
        lines.extend(recon_lines(split));
    } else {
        lines.push(Line::from(
            "Move to the + person column and press Enter to type names.",
        ));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(" Totals "))
            .wrap(Wrap { trim: true }),
        chunks[1],
    );
}

fn render_summary(frame: &mut Frame, area: Rect, app: &App) {
    let Some(split) = app.split_result() else {
        return;
    };
    let chunks = Layout::vertical([Constraint::Min(4), Constraint::Length(10)]).split(area);

    let header = Row::new(vec![
        Cell::from("Person"),
        Cell::from("Items"),
        Cell::from("Tax"),
        Cell::from("Tip"),
        Cell::from("Owes"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = split
        .shares
        .iter()
        .map(|share| {
            Row::new(vec![
                Cell::from(share.name.clone()),
                Cell::from(money(share.items_cents)),
                Cell::from(money(share.tax_cents)),
                Cell::from(money(share.tip_cents)),
                Cell::from(money(share.total_cents()))
                    .style(Style::default().add_modifier(Modifier::BOLD)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(12),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(Block::bordered().title(" Split "));
    frame.render_widget(table, chunks[0]);

    frame.render_widget(
        Paragraph::new(recon_lines(&split))
            .block(Block::bordered().title(" Reconciliation "))
            .wrap(Wrap { trim: true }),
        chunks[1],
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
        let hints = if app.window_focus {
            "h/l switch window · j/Esc back · q quit"
        } else {
            match app.screen {
                Screen::Extracting => "r retry · q quit",
                Screen::Receipt => {
                    "hjkl move · Enter edit · a add · d delete · m merchant · D date · t tax · S subtotal · T total · % tip · s save · k↑: windows · q quit"
                }
                Screen::Grid => {
                    "hjkl move · Enter toggle/edit · Space toggle · d delete · s save · k↑: windows · q quit"
                }
                Screen::Summary => "y copy · s save · k↑: windows · q quit",
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

fn recon_lines(split: &SplitResult) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let basis = match (split.receipt_includes_tax, split.receipt_includes_tip) {
        (true, true) => "incl. tax & tip",
        (true, false) => "incl. tax, excl. tip",
        (false, true) => "excl. tax, incl. tip",
        (false, false) => "excl. tax & tip",
    };
    lines.push(Line::from(format!(
        "receipt total {} ({basis})",
        money(split.receipt_total_cents),
    )));
    let delta = split.comparable_cents - split.receipt_total_cents;
    let (status, status_style) = if split.total_matches {
        ("✓ match".to_string(), Style::default().fg(Color::Green))
    } else {
        (
            format!(
                "✗ mismatch ({}{})",
                if delta >= 0 { "+" } else { "-" },
                money(delta.abs())
            ),
            Style::default().fg(Color::Red),
        )
    };
    lines.push(Line::from(vec![
        Span::raw(format!(
            "receipt comparison: computed {} vs receipt {}  ",
            money(split.comparable_cents),
            money(split.receipt_total_cents),
        )),
        Span::styled(status, status_style),
    ]));
    lines.push(Line::from(format!(
        "pre-tip {} (items {} + tax {})   + tip {}   = {} to split",
        money(split.computed_pretip_cents),
        money(split.items_sum_cents),
        money(split.tax_cents),
        money(split.tip_cents),
        money(split.computed_total_cents),
    )));
    if !split.unassigned.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("{} item(s) not assigned to anyone", split.unassigned.len()),
            Style::default().fg(Color::Yellow),
        )));
    }
    lines
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

fn tail_chars(input: &str, max: usize) -> String {
    let mut chars: Vec<char> = input.chars().rev().take(max).collect();
    chars.reverse();
    let mut out = String::from("…");
    out.extend(chars);
    out
}
