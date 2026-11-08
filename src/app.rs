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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Extracting,
    Receipt,
    Grid,
    Summary,
}

impl Screen {
    pub fn index(self) -> usize {
        match self {
            Screen::Extracting => 0,
            Screen::Receipt => 1,
            Screen::Grid => 2,
            Screen::Summary => 3,
        }
    }

    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Screen::Extracting,
            1 => Screen::Receipt,
            2 => Screen::Grid,
            _ => Screen::Summary,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Screen::Extracting => "Extract",
            Screen::Receipt => "Receipt",
            Screen::Grid => "Grid",
            Screen::Summary => "Summary",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditTarget {
    // Receipt screen (footer input)
    ItemName(usize),
    ItemQty(usize),
    ItemUnitPrice(usize),
    ItemLineTotal(usize),
    Merchant,
    Date,
    Tax,
    Subtotal,
    Total,
    TipPercent,
    // Grid screen (inline cell input)
    GridItemName(usize),
    GridPersonName(usize),
    GridNewPerson,
    GridNewItem,
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
    /// Cursor. On Receipt this is an item row; on Grid it is (grid row, grid col).
    pub row: usize,
    pub col: usize,
    pub edit: Option<EditTarget>,
    pub input: String,
    pub adding_people: bool,
    pub adding_items: bool,
    /// True when the cursor has moved up into the window/tab bar.
    pub window_focus: bool,
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
            window_focus: false,
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
                app.screen = Screen::Receipt;
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
        self.screen = Screen::Receipt;
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
        // Ctrl+C exits from any mode (raw mode delivers it as a key event, not SIGINT).
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
        if self.window_focus {
            self.handle_window_focus(key);
            return;
        }
        match self.screen {
            Screen::Extracting => {
                if self.error.is_some() && matches!(key.code, KeyCode::Char('r')) {
                    self.start_extraction();
                }
            }
            Screen::Receipt => self.handle_receipt(key),
            Screen::Grid => self.handle_grid(key),
            Screen::Summary => self.handle_summary(key),
        }
    }

    fn handle_receipt(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_receipt(1),
            KeyCode::Char('k') | KeyCode::Up => {
                if self.row == 0 {
                    self.window_focus = true;
                } else {
                    self.move_receipt(-1);
                }
            }
            KeyCode::Char('h') | KeyCode::Left => self.move_receipt_col(-1),
            KeyCode::Char('l') | KeyCode::Right => self.move_receipt_col(1),
            KeyCode::Char('g') => self.row = 0,
            KeyCode::Char('G') => self.row = self.item_count().saturating_sub(1),
            KeyCode::Enter => self.begin_receipt_edit(self.col),
            KeyCode::Char('s') => self.save_state(),
            KeyCode::Char('a') => {
                if let Some(receipt) = &mut self.receipt {
                    receipt.items.push(ReceiptItem::new("New item"));
                }
                self.row = self.item_count().saturating_sub(1);
                self.col = 0;
            }
            KeyCode::Char('d') => self.delete_item(self.row),
            KeyCode::Char('i') => self.begin_receipt_edit(0),
            KeyCode::Char('e') => self.begin_receipt_edit(2),
            KeyCode::Char('u') => self.begin_receipt_edit(1),
            KeyCode::Char('L') => self.begin_receipt_edit(3),
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

    fn handle_grid(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => self.move_grid(0, -1),
            KeyCode::Char('l') | KeyCode::Right => self.move_grid(0, 1),
            KeyCode::Char('j') | KeyCode::Down => self.move_grid(1, 0),
            KeyCode::Char('k') | KeyCode::Up => {
                if self.row == 0 {
                    self.window_focus = true;
                } else {
                    self.move_grid(-1, 0);
                }
            }
            KeyCode::Char('g') => {
                self.row = 0;
            }
            KeyCode::Char('G') => {
                self.row = self.grid_rows().saturating_sub(1);
            }
            KeyCode::Enter => self.grid_enter(),
            KeyCode::Char(' ') => self.grid_toggle(),
            KeyCode::Char('d') => self.grid_delete(),
            KeyCode::Char('s') => self.save_state(),
            _ => {}
        }
    }

    fn handle_summary(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('k') | KeyCode::Up => self.window_focus = true,
            KeyCode::Char('s') => self.save_state(),
            KeyCode::Char('y') => self.copy_summary(),
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
            EditTarget::ItemName(index) => {
                if let Some(item) = self.item_mut(index) {
                    item.name = text;
                }
            }
            EditTarget::ItemQty(index) => {
                if let Ok(qty) = text.parse::<u32>() {
                    if let Some(item) = self.item_mut(index) {
                        item.qty = qty.max(1);
                    }
                }
            }
            EditTarget::ItemUnitPrice(index) => {
                if let Some(cents) = parse_cents(&text) {
                    if let Some(item) = self.item_mut(index) {
                        item.unit_price_cents = cents;
                        item.line_total_cents = cents * item.qty as i64;
                    }
                }
            }
            EditTarget::ItemLineTotal(index) => {
                if let Some(cents) = parse_cents(&text) {
                    if let Some(item) = self.item_mut(index) {
                        item.line_total_cents = cents;
                    }
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
            EditTarget::GridItemName(index) => {
                if let Some(item) = self.item_mut(index) {
                    item.name = text;
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
        let people_len = self.people.len();
        if self.row == 0 {
            if self.col >= 1 && self.col <= people_len {
                let index = self.col - 1;
                let value = self.people.get(index).map(|p| p.name.clone());
                if let Some(value) = value {
                    self.begin_edit(EditTarget::GridPersonName(index), value);
                }
            } else if self.col == people_len + 1 {
                self.begin_grid_new_person();
            }
        } else if self.row == items_len + 1 {
            self.begin_grid_new_item();
        } else if self.row >= 1 {
            let index = self.row - 1;
            if self.col == 0 {
                let value = self.item(index).map(|item| item.name.clone());
                if let Some(value) = value {
                    self.begin_edit(EditTarget::GridItemName(index), value);
                }
            } else if self.col <= people_len {
                self.grid_toggle();
            }
        }
    }

    fn grid_toggle(&mut self) {
        let items_len = self.item_count();
        let people_len = self.people.len();
        if self.row >= 1 && self.row <= items_len && self.col >= 1 && self.col <= people_len {
            let item_id = self.item(self.row - 1).map(|item| item.id.clone());
            let person_id = self.people.get(self.col - 1).map(|p| p.id.clone());
            if let (Some(item_id), Some(person_id)) = (item_id, person_id) {
                self.assignments.toggle(&item_id, &person_id);
            }
        }
    }

    fn grid_delete(&mut self) {
        let items_len = self.item_count();
        let people_len = self.people.len();
        if self.row >= 1 && self.row <= items_len && self.col == 0 {
            self.delete_item(self.row - 1);
        } else if self.row == 0 && self.col >= 1 && self.col <= people_len {
            self.delete_person(self.col - 1);
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

    fn copy_summary(&mut self) {
        let Some(split) = self.split_result() else {
            return;
        };
        let merchant = self
            .receipt
            .as_ref()
            .map(|r| format!("{} — {}", r.merchant, r.transaction_date))
            .unwrap_or_default();
        let mut text = format!("{merchant}\n");
        for share in &split.shares {
            text.push_str(&format!(
                "{:<16} items {:>10}  tax {:>8}  tip {:>8}  owes {:>10}\n",
                share.name,
                money(share.items_cents),
                money(share.tax_cents),
                money(share.tip_cents),
                money(share.total_cents()),
            ));
        }
        text.push_str(&format!(
            "computed {} / receipt {}\n",
            money(split.computed_total_cents),
            money(split.receipt_total_cents),
        ));
        match state::copy_to_clipboard(&text) {
            Ok(()) => self.status = "Summary copied to clipboard".to_string(),
            Err(error) => self.record_error(format!("clipboard: {error:#}")),
        }
    }

    fn handle_window_focus(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => self.switch_screen(-1),
            KeyCode::Char('l') | KeyCode::Right => self.switch_screen(1),
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Esc | KeyCode::Enter => {
                self.window_focus = false;
            }
            KeyCode::Char('k') | KeyCode::Up => {}
            KeyCode::Char('s') => self.save_state(),
            _ => {}
        }
    }

    /// Switch screens while staying in window focus.
    fn switch_screen(&mut self, delta: i64) {
        let floor = if self.receipt.is_some() { 1_i64 } else { 0 };
        let current = self.screen.index() as i64;
        let next = (current + delta).clamp(floor, 3) as usize;
        self.screen = Screen::from_index(next);
        self.row = 0;
        self.col = 0;
    }

    fn move_receipt(&mut self, delta: i64) {
        self.row = clamp_add(self.row, delta, self.item_count());
    }

    fn move_receipt_col(&mut self, delta: i64) {
        self.col = (self.col as i64 + delta).clamp(0, 3) as usize;
    }

    fn begin_receipt_edit(&mut self, col: usize) {
        let Some(item) = self.item(self.row) else {
            return;
        };
        let (target, initial) = match col {
            0 => (EditTarget::ItemName(self.row), item.name.clone()),
            1 => (EditTarget::ItemQty(self.row), item.qty.to_string()),
            2 => (
                EditTarget::ItemUnitPrice(self.row),
                money_input(item.unit_price_cents),
            ),
            _ => (
                EditTarget::ItemLineTotal(self.row),
                money_input(item.line_total_cents),
            ),
        };
        self.col = col.min(3);
        self.begin_edit(target, initial);
    }

    fn move_grid(&mut self, row_delta: i64, col_delta: i64) {
        let rows = self.grid_rows();
        let cols = self.grid_cols();
        self.row = clamp_add(self.row, row_delta, rows);
        self.col = clamp_add(self.col, col_delta, cols);
    }

    fn clamp_cursor(&mut self) {
        match self.screen {
            Screen::Grid => {
                self.row = self.row.min(self.grid_rows().saturating_sub(1));
                self.col = self.col.min(self.grid_cols().saturating_sub(1));
            }
            Screen::Receipt => {
                self.row = self.row.min(self.item_count().saturating_sub(1));
                self.col = self.col.min(3);
            }
            _ => {}
        }
    }

    fn grid_rows(&self) -> usize {
        self.item_count() + 2
    }

    fn grid_cols(&self) -> usize {
        self.people.len() + 2
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
            Some(EditTarget::ItemName(_)) => "ITEM NAME",
            Some(EditTarget::ItemQty(_)) => "QTY",
            Some(EditTarget::ItemUnitPrice(_)) => "UNIT PRICE",
            Some(EditTarget::ItemLineTotal(_)) => "LINE TOTAL",
            Some(EditTarget::Merchant) => "MERCHANT",
            Some(EditTarget::Date) => "DATE (YYYY-MM-DD)",
            Some(EditTarget::Tax) => "TAX",
            Some(EditTarget::Subtotal) => "SUBTOTAL",
            Some(EditTarget::Total) => "TOTAL",
            Some(EditTarget::TipPercent) => "TIP %",
            Some(EditTarget::GridItemName(_)) => "ITEM NAME",
            Some(EditTarget::GridPersonName(_)) => "PERSON NAME",
            Some(EditTarget::GridNewPerson) => "ADD PERSON (Enter for next, Esc done)",
            Some(EditTarget::GridNewItem) => "ADD ITEM (Enter for next, Esc done)",
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
