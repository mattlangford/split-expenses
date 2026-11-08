use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, List, ListItem, Paragraph, Row, Table, Wrap};

use crate::app::{App, Mode, Screen, money};
use crate::money::format_percent_bps;
use crate::split::SplitResult;

pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(frame.area());

    render_tabs(frame, chunks[0], app);
    match app.screen {
        Screen::Extracting => render_extracting(frame, chunks[1], app),
        Screen::Receipt => render_receipt(frame, chunks[1], app),
        Screen::People => render_people(frame, chunks[1], app),
        Screen::Assign => render_assign(frame, chunks[1], app),
        Screen::Summary => render_summary(frame, chunks[1], app),
    }
    render_footer(frame, chunks[2], app);
}

fn render_tabs(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![Span::styled(
        " split ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )];
    for index in 0..=4 {
        let screen = Screen::from_index(index);
        let active = app.screen == screen;
        let style = if active {
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
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
    let mut lines = vec![
        Line::from(vec![
            Span::styled(spinner, Style::default().fg(Color::Cyan)),
            Span::raw("  Parsing receipt with "),
            Span::styled(app.args.model.clone(), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(format!("image: {}", app.args.image.display())),
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
    let chunks = Layout::vertical([Constraint::Min(4), Constraint::Length(8)]).split(area);

    let header = Row::new(vec![
        Cell::from("Item"),
        Cell::from("Qty"),
        Cell::from("Unit"),
        Cell::from("Total"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = receipt
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let style = if index == app.row {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(item.name.clone()),
                Cell::from(item.qty.to_string()),
                Cell::from(money(item.unit_price_cents)),
                Cell::from(money(item.line_total_cents)),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(16),
            Constraint::Length(4),
            Constraint::Length(10),
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
            "Items sum: {}    Subtotal: {}    Printed total: {}",
            money(items_sum),
            receipt
                .subtotal_cents
                .map(money)
                .unwrap_or_else(|| "—".to_string()),
            money(receipt.total_cents),
        )),
    ];
    if let Some(split) = app.split_result() {
        lines.push(Line::from(format!(
            "Assigned: {}    Tip amount: {}    Computed total: {}",
            money(split.assigned_sum_cents),
            money(split.tip_cents),
            money(split.computed_total_cents),
        )));
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

fn render_people(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .people
        .iter()
        .enumerate()
        .map(|(index, person)| {
            let style = if index == app.row {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(person.name.clone())).style(style)
        })
        .collect();
    let title = if app.people.is_empty() {
        " People — press a to add "
    } else {
        " People "
    };
    frame.render_widget(
        List::new(items).block(Block::bordered().title(title)),
        area,
    );
}

fn render_assign(frame: &mut Frame, area: Rect, app: &App) {
    let Some(receipt) = &app.receipt else {
        return;
    };
    let chunks = Layout::vertical([Constraint::Min(4), Constraint::Length(8)]).split(area);

    let mut widths = vec![Constraint::Min(16)];
    for _ in &app.people {
        widths.push(Constraint::Length(9));
    }
    widths.push(Constraint::Length(10));

    let mut header_cells = vec![Cell::from("Item")];
    for person in &app.people {
        header_cells.push(Cell::from(person.name.clone()));
    }
    header_cells.push(Cell::from("Line"));
    let header = Row::new(header_cells).style(Style::default().add_modifier(Modifier::BOLD));

    let mut rows = Vec::new();
    for (row_index, item) in receipt.items.iter().enumerate() {
        let mut cells = vec![Cell::from(item.name.clone())];
        for (col_index, person) in app.people.iter().enumerate() {
            let assigned = app.assignments.is_assigned(&item.id, &person.id);
            let mark = if assigned { "[x]" } else { "[ ]" };
            let selected = row_index == app.row && col_index == app.col;
            let style = if selected {
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            cells.push(Cell::from(Span::styled(mark, style)));
        }
        cells.push(Cell::from(money(item.line_total_cents)));
        rows.push(Row::new(cells));
    }

    let title = if app.people.is_empty() {
        " Assign — add people first (people screen) "
    } else {
        " Assign (space toggle · A all · o solo · c clear) "
    };
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().title(title));
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
    let chunks = Layout::vertical([Constraint::Min(4), Constraint::Length(9)]).split(area);

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
                " EDIT ",
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
            Screen::Receipt => {
                "j/k move · i name · e price · u qty · L line · a add · d delete · m/D/t/S/T/% fields · s save · h/l screens · q quit"
            }
            Screen::People => "j/k move · a add · i rename · d delete · h/l screens · q quit",
            Screen::Assign => {
                "hjkl move · space toggle · A all · o solo · c clear · s save · n/p screens · q quit"
            }
            Screen::Summary => "y copy · s save · h/l screens · q quit",
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
    let total_matches = split.computed_total_cents == split.receipt_total_cents;
    lines.push(Line::from(vec![
        Span::raw("computed total "),
        Span::raw(money(split.computed_total_cents)),
        Span::raw("  vs receipt "),
        Span::raw(money(split.receipt_total_cents)),
        Span::raw("  "),
        Span::styled(
            if total_matches { "✓ match" } else { "✗ mismatch" },
            if total_matches {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Red)
            },
        ),
    ]));
    if !split.unassigned.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("{} item(s) not assigned to anyone", split.unassigned.len()),
            Style::default().fg(Color::Yellow),
        )));
    }
    lines.push(Line::from(format!(
        "tax {}   tip {}   items {}",
        money(split.tax_cents),
        money(split.tip_cents),
        money(split.items_sum_cents),
    )));
    lines
}
