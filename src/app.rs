use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Instant;

use anyhow::Result;
use chrono::NaiveDate;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::cli::Args;
use crate::money::{format_cents, format_percent_bps, parse_cents, parse_percent_bps};
use crate::split::{SplitResult, compute_split};
use crate::types::{Assignments, Person, Receipt, ReceiptItem};
use crate::{pi, state};

/// Columns before the people columns: item name, qty, line total.
pub const ITEM_COLS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Extracting,
    Grid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditTarget {
    GridItemName(usize),
    GridItemQty(usize),
    GridItemTotal(usize),
    GridPersonName(usize),
    GridNewPerson,
    GridNewItem,
    Merchant,
    Date,
    Tax,
    Subtotal,
    Total,
    TipPercent,
}

enum ExtractMsg {
    Progress(pi::ExtractProgress),
    Done(Result<Receipt, String>),
}

pub struct App {
    pub args: Args,
    pub screen: Screen,
    pub mode: Mode,
    pub quit: bool,
    pub status: String,
    pub error: Option<String>,
    pub receipt: Option<Receipt>,
    pub people: Vec<Person>,
    pub assignments: Assignments,
    pub tip_bps: u32,
    pub row: usize,
    pub col: usize,
    pub edit: Option<EditTarget>,
    pub input: String,
    pub adding_people: bool,
    pub adding_items: bool,
    pub spinner: usize,
    pub extract_started: Instant,
    pub extract_status: String,
    pub extract_partial: String,
    pub saved_path: Option<PathBuf>,
    /// Errors to replay to the terminal after exit.
    pub errors: Vec<String>,
    rx: Receiver<ExtractMsg>,
}

impl App {
    pub fn new(args: Args) -> Self {
        let (_tx, rx) = channel();
        let mut app = Self {
            args,
            screen: Screen::Extracting,
            mode: Mode::Normal,
            quit: false,
            status: "Extracting receipt…".to_string(),
            error: None,
            receipt: None,
            people: Vec::new(),
            assignments: Assignments::default(),
            tip_bps: 2000,
            row: 0,
            col: 0,
            edit: None,
            input: String::new(),
            adding_people: false,
            adding_items: false,
            spinner: 0,
            extract_started: Instant::now(),
            extract_status: String::new(),
            extract_partial: String::new(),
            saved_path: None,
            errors: Vec::new(),
            rx,
        };
        match state::load_by_image(&app.args.state_dir, &app.args.image) {
            Ok(Some(saved)) => {
                let merchant = saved.receipt.merchant.clone();
                app.receipt = Some(saved.receipt);
                app.people = saved.people;
                app.assignments = saved.assignments;
                app.tip_bps = saved.tip_bps;
                app.screen = Screen::Grid;
                app.status = format!("Reloaded saved split for {merchant}");
            }
            _ => app.start_extraction(),
        }
        app
    }

    fn start_extraction(&mut self) {
        self.error = None;
        self.screen = Screen::Extracting;
        self.status = "Extracting receipt…".to_string();
        self.extract_started = Instant::now();
        self.extract_status = "starting".to_string();
        self.extract_partial.clear();
        let (tx, rx) = channel();
        self.rx = rx;
        let args = self.args.clone();
        std::thread::spawn(move || {
            let progress = tx.clone();
            let result = pi::extract_receipt(&args.pi_bin, &args.model, &args.image, |event| {
                let _ = progress.send(ExtractMsg::Progress(event));
            })
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(ExtractMsg::Done(result));
        });
    }

    pub fn tick(&mut self) {
        if self.screen != Screen::Extracting || self.error.is_some() {
            return;
        }
        loop {
            match self.rx.try_recv() {
                Ok(ExtractMsg::Progress(pi::ExtractProgress::Status(text))) => {
                    self.extract_status = text;
                }
                Ok(ExtractMsg::Progress(pi::ExtractProgress::Partial(text))) => {
                    self.extract_partial = text;
                }
                Ok(ExtractMsg::Done(Ok(receipt))) => {
                    self.finish_extraction(receipt);
                    return;
                }
                Ok(ExtractMsg::Done(Err(message))) => {
                    self.error = Some(message);
                    return;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.error = Some("extraction ended unexpectedly".to_string());
                    return;
                }
            }
        }
        self.spinner = self.spinner.wrapping_add(1);
    }

    fn finish_extraction(&mut self, receipt: Receipt) {
        self.status = format!("Parsed {} items", receipt.items.len());
        self.receipt = Some(receipt);
        self.screen = Screen::Grid;
        self.row = 0;
        self.col = 0;
    }

    pub fn split_result(&self) -> Option<SplitResult> {
        let receipt = self.receipt.as_ref()?;
        Some(compute_split(
            receipt,
            &self.people,
            &self.assignments,
            self.tip_bps,
        ))
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            self.save_and_quit();
            return;
        }
        if self.mode == Mode::Insert {
            self.handle_insert(key);
            return;
        }
        if matches!(key.code, KeyCode::Char('q')) {
            self.save_and_quit();
            return;
        }
        match self.screen {
            Screen::Extracting => {
                if self.error.is_some() && matches!(key.code, KeyCode::Char('r')) {
                    self.start_extraction();
                }
            }
            Screen::Grid => self.handle_grid(key),
        }
    }

    fn handle_grid(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => self.move_grid(0, -1),
            KeyCode::Char('l') | KeyCode::Right => self.move_grid(0, 1),
            KeyCode::Char('j') | KeyCode::Down => self.move_grid(1, 0),
            KeyCode::Char('k') | KeyCode::Up => self.move_grid(-1, 0),
            KeyCode::Char('g') => self.row = 0,
            KeyCode::Char('G') => self.row = self.grid_rows().saturating_sub(1),
            KeyCode::Enter => self.grid_enter(),
            KeyCode::Char(' ') | KeyCode::Tab => self.grid_toggle(),
            KeyCode::Char('d') => self.grid_delete(),
            KeyCode::Char('a') => self.begin_grid_new_person(),
            KeyCode::Char('s') => self.save_state(),
            KeyCode::Char('m') => {
                let value = self.receipt.as_ref().map(|r| r.merchant.clone());
                if let Some(value) = value {
                    self.begin_edit(EditTarget::Merchant, value);
                }
            }
            KeyCode::Char('D') => {
                let value = self
                    .receipt
                    .as_ref()
                    .map(|r| r.transaction_date.format("%Y-%m-%d").to_string());
                if let Some(value) = value {
                    self.begin_edit(EditTarget::Date, value);
                }
            }
            KeyCode::Char('t') => {
                let value = self.receipt.as_ref().map(|r| money_input(r.tax_cents));
                if let Some(value) = value {
                    self.begin_edit(EditTarget::Tax, value);
                }
            }
            KeyCode::Char('S') => {
                let value = self
                    .receipt
                    .as_ref()
                    .map(|r| r.subtotal_cents.map(money_input).unwrap_or_default());
                if let Some(value) = value {
                    self.begin_edit(EditTarget::Subtotal, value);
                }
            }
            KeyCode::Char('T') => {
                let value = self.receipt.as_ref().map(|r| money_input(r.total_cents));
                if let Some(value) = value {
                    self.begin_edit(EditTarget::Total, value);
                }
            }
            KeyCode::Char('%') => {
                let value = format_percent_bps(self.tip_bps);
                self.begin_edit(EditTarget::TipPercent, value);
            }
            _ => {}
        }
    }

    fn handle_insert(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.adding_people = false;
                self.adding_items = false;
                self.mode = Mode::Normal;
                self.edit = None;
                self.input.clear();
            }
            KeyCode::Enter => self.commit_edit(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.clear();
            }
            KeyCode::Char(c) => self.input.push(c),
            _ => {}
        }
    }

    fn commit_edit(&mut self) {
        let Some(target) = self.edit.take() else {
            return;
        };
        let text = self.input.trim().to_string();
        self.mode = Mode::Normal;
        self.input.clear();

        match target {
            EditTarget::GridItemName(index) => {
                if let Some(item) = self.item_mut(index) {
                    item.name = text;
                }
            }
            EditTarget::GridItemQty(index) => {
                if let Ok(qty) = text.parse::<u32>() {
                    if let Some(item) = self.item_mut(index) {
                        item.qty = qty.max(1);
                    }
                }
            }
            EditTarget::GridItemTotal(index) => {
                if let Some(cents) = parse_cents(&text) {
                    if let Some(item) = self.item_mut(index) {
                        item.line_total_cents = cents;
                    }
                }
            }
            EditTarget::GridPersonName(index) => {
                if let Some(person) = self.people.get_mut(index) {
                    person.name = text;
                }
            }
            EditTarget::GridNewPerson => {
                if !text.is_empty() {
                    self.people.push(Person::new(text));
                    self.row = 0;
                    self.col = self.grid_cols() - 1;
                    if self.adding_people {
                        self.begin_grid_new_person();
                    }
                } else {
                    self.adding_people = false;
                }
            }
            EditTarget::GridNewItem => {
                if !text.is_empty() {
                    if let Some(receipt) = &mut self.receipt {
                        receipt.items.push(ReceiptItem::new(text));
                    }
                    self.row = self.grid_rows() - 1;
                    self.col = 0;
                    if self.adding_items {
                        self.begin_grid_new_item();
                    }
                } else {
                    self.adding_items = false;
                }
            }
            EditTarget::Merchant => {
                if let Some(receipt) = &mut self.receipt {
                    receipt.merchant = text;
                }
            }
            EditTarget::Date => {
                if let Ok(date) = NaiveDate::parse_from_str(&text, "%Y-%m-%d") {
                    if let Some(receipt) = &mut self.receipt {
                        receipt.transaction_date = date;
                    }
                }
            }
            EditTarget::Tax => {
                if let Some(cents) = parse_cents(&text) {
                    if let Some(receipt) = &mut self.receipt {
                        receipt.tax_cents = cents;
                    }
                }
            }
            EditTarget::Subtotal => {
                if text.is_empty() {
                    if let Some(receipt) = &mut self.receipt {
                        receipt.subtotal_cents = None;
                    }
                } else if let Some(cents) = parse_cents(&text) {
                    if let Some(receipt) = &mut self.receipt {
                        receipt.subtotal_cents = Some(cents);
                    }
                }
            }
            EditTarget::Total => {
                if let Some(cents) = parse_cents(&text) {
                    if let Some(receipt) = &mut self.receipt {
                        receipt.total_cents = cents;
                    }
                }
            }
            EditTarget::TipPercent => {
                if let Some(bps) = parse_percent_bps(&text) {
                    self.tip_bps = bps;
                }
            }
        }
    }

    fn begin_edit(&mut self, target: EditTarget, initial: String) {
        self.adding_people = false;
        self.adding_items = false;
        self.edit = Some(target);
        self.input = initial;
        self.mode = Mode::Insert;
    }

    fn begin_grid_new_person(&mut self) {
        self.row = 0;
        self.col = self.grid_cols().saturating_sub(1);
        self.begin_edit(EditTarget::GridNewPerson, String::new());
        self.adding_people = true;
    }

    fn begin_grid_new_item(&mut self) {
        self.row = self.grid_rows().saturating_sub(1);
        self.col = 0;
        self.begin_edit(EditTarget::GridNewItem, String::new());
        self.adding_items = true;
    }

    fn grid_enter(&mut self) {
        let items_len = self.item_count();
        if self.row == 0 {
            if let Some(person_index) = self.person_at_col(self.col) {
                let value = self.people.get(person_index).map(|p| p.name.clone());
                if let Some(value) = value {
                    self.begin_edit(EditTarget::GridPersonName(person_index), value);
                }
            } else if self.is_add_person_col(self.col) {
                self.begin_grid_new_person();
            }
        } else if self.row == items_len + 1 {
            self.begin_grid_new_item();
        } else if self.row >= 1 {
            let index = self.row - 1;
            match self.col {
                0 => {
                    let value = self.item(index).map(|item| item.name.clone());
                    if let Some(value) = value {
                        self.begin_edit(EditTarget::GridItemName(index), value);
                    }
                }
                1 => {
                    let value = self.item(index).map(|item| item.qty.to_string());
                    if let Some(value) = value {
                        self.begin_edit(EditTarget::GridItemQty(index), value);
                    }
                }
                2 => {
                    let value = self
                        .item(index)
                        .map(|item| money_input(item.line_total_cents));
                    if let Some(value) = value {
                        self.begin_edit(EditTarget::GridItemTotal(index), value);
                    }
                }
                _ => self.grid_toggle(),
            }
        }
    }

    fn grid_toggle(&mut self) {
        let items_len = self.item_count();
        if self.row < 1 || self.row > items_len {
            return;
        }
        let Some(person_index) = self.person_at_col(self.col) else {
            return;
        };
        let item_id = self.item(self.row - 1).map(|item| item.id.clone());
        let person_id = self.people.get(person_index).map(|p| p.id.clone());
        if let (Some(item_id), Some(person_id)) = (item_id, person_id) {
            self.assignments.toggle(&item_id, &person_id);
        }
    }

    fn grid_delete(&mut self) {
        let items_len = self.item_count();
        if self.row >= 1 && self.row <= items_len && self.col == 0 {
            self.delete_item(self.row - 1);
        } else if self.row == 0 {
            if let Some(person_index) = self.person_at_col(self.col) {
                self.delete_person(person_index);
            }
        }
    }

    fn delete_item(&mut self, index: usize) {
        let removed = if let Some(receipt) = &mut self.receipt {
            if index < receipt.items.len() {
                Some(receipt.items.remove(index).id)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(id) = removed {
            self.assignments.clear_item(&id);
            self.clamp_cursor();
        }
    }

    fn delete_person(&mut self, index: usize) {
        if index < self.people.len() {
            let removed = self.people.remove(index);
            self.assignments.remove_person(&removed.id);
            self.clamp_cursor();
        }
    }

    fn save_and_quit(&mut self) {
        if self.receipt.is_some() {
            self.save_state();
        }
        self.quit = true;
    }

    fn save_state(&mut self) {
        let Some(receipt) = self.receipt.clone() else {
            return;
        };
        let saved = state::SavedState {
            receipt,
            people: self.people.clone(),
            assignments: self.assignments.clone(),
            tip_bps: self.tip_bps,
        };
        match state::save(&self.args.state_dir, &saved, &self.args.image) {
            Ok(path) => {
                self.status = format!("Saved {}", path.display());
                self.saved_path = Some(path);
            }
            Err(error) => self.record_error(format!("failed to save state: {error:#}")),
        }
    }

    fn record_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.status = message.clone();
        self.errors.push(message);
    }

    /// Replay errors to stderr after the terminal is restored, so they persist
    /// in scrollback after quitting.
    pub fn print_terminal_messages(&self) {
        let mut lines: Vec<&str> = Vec::new();
        if let Some(error) = &self.error {
            lines.push(error.as_str());
        }
        for error in &self.errors {
            lines.push(error.as_str());
        }
        if lines.is_empty() {
            return;
        }
        eprintln!();
        for line in lines {
            eprintln!("split: {line}");
        }
    }

    fn move_grid(&mut self, row_delta: i64, col_delta: i64) {
        let rows = self.grid_rows();
        let cols = self.grid_cols();
        self.row = clamp_add(self.row, row_delta, rows);
        self.col = clamp_add(self.col, col_delta, cols);
    }

    fn clamp_cursor(&mut self) {
        self.row = self.row.min(self.grid_rows().saturating_sub(1));
        self.col = self.col.min(self.grid_cols().saturating_sub(1));
    }

    fn grid_rows(&self) -> usize {
        self.item_count() + 2
    }

    fn grid_cols(&self) -> usize {
        self.people.len() + ITEM_COLS + 1
    }

    fn person_at_col(&self, col: usize) -> Option<usize> {
        if col >= ITEM_COLS && col < ITEM_COLS + self.people.len() {
            Some(col - ITEM_COLS)
        } else {
            None
        }
    }

    fn is_add_person_col(&self, col: usize) -> bool {
        col == ITEM_COLS + self.people.len()
    }

    fn item(&self, index: usize) -> Option<&ReceiptItem> {
        self.receipt
            .as_ref()
            .and_then(|receipt| receipt.items.get(index))
    }

    fn item_mut(&mut self, index: usize) -> Option<&mut ReceiptItem> {
        self.receipt
            .as_mut()
            .and_then(|receipt| receipt.items.get_mut(index))
    }

    fn item_count(&self) -> usize {
        self.receipt.as_ref().map(|r| r.items.len()).unwrap_or(0)
    }

    pub fn edit_prompt(&self) -> &'static str {
        match &self.edit {
            Some(EditTarget::GridItemName(_)) => "ITEM NAME",
            Some(EditTarget::GridItemQty(_)) => "QTY",
            Some(EditTarget::GridItemTotal(_)) => "ITEM TOTAL",
            Some(EditTarget::GridPersonName(_)) => "PERSON NAME",
            Some(EditTarget::GridNewPerson) => "ADD PERSON (Enter for next, Esc done)",
            Some(EditTarget::GridNewItem) => "ADD ITEM (Enter for next, Esc done)",
            Some(EditTarget::Merchant) => "MERCHANT",
            Some(EditTarget::Date) => "DATE (YYYY-MM-DD)",
            Some(EditTarget::Tax) => "TAX",
            Some(EditTarget::Subtotal) => "SUBTOTAL",
            Some(EditTarget::Total) => "RECEIPT TOTAL",
            Some(EditTarget::TipPercent) => "TIP %",
            None => "EDIT",
        }
    }
}

fn clamp_add(value: usize, delta: i64, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let next = value as i64 + delta;
    next.clamp(0, len as i64 - 1) as usize
}

pub fn money(cents: i64) -> String {
    format!("${}", format_cents(cents))
}

fn money_input(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, (cents % 100).abs())
}
