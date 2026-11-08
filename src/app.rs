use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use anyhow::Result;
use chrono::NaiveDate;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::cli::Args;
use crate::money::{format_cents, format_percent_bps, parse_cents, parse_percent_bps};
use crate::split::{SplitResult, compute_split};
use crate::types::{Assignments, Person, Receipt, ReceiptItem};
use crate::pi;
use crate::state::{self, SavedState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Extracting,
    Receipt,
    People,
    Assign,
    Summary,
}

impl Screen {
    pub fn index(self) -> usize {
        match self {
            Screen::Extracting => 0,
            Screen::Receipt => 1,
            Screen::People => 2,
            Screen::Assign => 3,
            Screen::Summary => 4,
        }
    }

    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Screen::Extracting,
            1 => Screen::Receipt,
            2 => Screen::People,
            3 => Screen::Assign,
            _ => Screen::Summary,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Screen::Extracting => "Extract",
            Screen::Receipt => "Receipt",
            Screen::People => "People",
            Screen::Assign => "Assign",
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
    NewItemName,
    NewPersonName,
    RenamePerson(usize),
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
    pub spinner: usize,
    pub saved_path: Option<PathBuf>,
    rx: Receiver<Result<Receipt, String>>,
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
            spinner: 0,
            saved_path: None,
            rx,
        };
        app.start_extraction();
        app
    }

    fn start_extraction(&mut self) {
        self.error = None;
        self.screen = Screen::Extracting;
        self.status = "Extracting receipt…".to_string();
        let (tx, rx) = channel();
        self.rx = rx;
        let args = self.args.clone();
        std::thread::spawn(move || {
            let result = pi::extract_receipt(&args.pi_bin, &args.model, &args.image)
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(result);
        });
    }

    pub fn tick(&mut self) {
        if self.screen != Screen::Extracting || self.error.is_some() {
            return;
        }
        match self.rx.try_recv() {
            Ok(Ok(receipt)) => {
                let resumed = state::load(&self.args.state_dir, &receipt).ok().flatten();
                if let Some(saved) = resumed {
                    self.people = saved.people;
                    self.assignments = saved.assignments;
                    self.tip_bps = saved.tip_bps;
                    self.status = format!("Resumed saved split for {}", receipt.merchant);
                } else {
                    self.status = format!("Parsed {} items", receipt.items.len());
                }
                self.receipt = Some(receipt);
                self.screen = Screen::Receipt;
                self.row = 0;
                self.col = 0;
            }
            Ok(Err(message)) => self.error = Some(message),
            Err(TryRecvError::Empty) => self.spinner = self.spinner.wrapping_add(1),
            Err(TryRecvError::Disconnected) => {
                self.error = Some("extraction ended unexpectedly".to_string());
            }
        }
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
            Screen::Receipt => self.handle_receipt(key),
            Screen::People => self.handle_people(key),
            Screen::Assign => self.handle_assign(key),
            Screen::Summary => self.handle_summary(key),
        }
    }

    fn handle_receipt(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1, 0),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1, 0),
            KeyCode::Char('g') => self.row = 0,
            KeyCode::Char('G') => self.row = self.item_count().saturating_sub(1),
            KeyCode::Char('l') | KeyCode::Tab => self.next_screen(),
            KeyCode::Char('h') | KeyCode::BackTab => self.prev_screen(),
            KeyCode::Char('s') => {
                let _ = self.save_state();
            }
            KeyCode::Char('a') => self.begin_edit(EditTarget::NewItemName, String::new()),
            KeyCode::Char('d') => self.delete_item(),
            KeyCode::Char('i') => {
                let value = self.item(self.row).map(|item| item.name.clone());
                if let Some(value) = value {
                    self.begin_edit(EditTarget::ItemName(self.row), value);
                }
            }
            KeyCode::Char('e') => {
                let value = self.item(self.row).map(|item| money_input(item.unit_price_cents));
                if let Some(value) = value {
                    self.begin_edit(EditTarget::ItemUnitPrice(self.row), value);
                }
            }
            KeyCode::Char('u') => {
                let value = self.item(self.row).map(|item| item.qty.to_string());
                if let Some(value) = value {
                    self.begin_edit(EditTarget::ItemQty(self.row), value);
                }
            }
            KeyCode::Char('L') => {
                let value = self.item(self.row).map(|item| money_input(item.line_total_cents));
                if let Some(value) = value {
                    self.begin_edit(EditTarget::ItemLineTotal(self.row), value);
                }
            }
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

    fn handle_people(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1, 0),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1, 0),
            KeyCode::Char('l') | KeyCode::Tab => self.next_screen(),
            KeyCode::Char('h') | KeyCode::BackTab => self.prev_screen(),
            KeyCode::Char('s') => {
                let _ = self.save_state();
            }
            KeyCode::Char('a') => self.begin_edit(EditTarget::NewPersonName, String::new()),
            KeyCode::Char('i') => {
                let value = self.people.get(self.row).map(|person| person.name.clone());
                if let Some(value) = value {
                    self.begin_edit(EditTarget::RenamePerson(self.row), value);
                }
            }
            KeyCode::Char('d') => self.delete_person(),
            _ => {}
        }
    }

    fn handle_assign(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => self.move_cursor(0, -1),
            KeyCode::Char('l') | KeyCode::Right => self.move_cursor(0, 1),
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1, 0),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1, 0),
            KeyCode::Char(' ') => self.toggle_cell(),
            KeyCode::Char('A') => self.toggle_row_all(),
            KeyCode::Char('o') => self.solo_cell(),
            KeyCode::Char('c') => self.clear_row(),
            KeyCode::Char('n') | KeyCode::Tab => self.next_screen(),
            KeyCode::Char('p') | KeyCode::BackTab => self.prev_screen(),
            KeyCode::Char('s') => {
                let _ = self.save_state();
            }
            _ => {}
        }
    }

    fn handle_summary(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('l') | KeyCode::Tab => self.next_screen(),
            KeyCode::Char('h') | KeyCode::BackTab => self.prev_screen(),
            KeyCode::Char('s') => {
                let _ = self.save_state();
            }
            KeyCode::Char('y') => self.copy_summary(),
            _ => {}
        }
    }

    fn handle_insert(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
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
            EditTarget::NewItemName => {
                if !text.is_empty() {
                    if let Some(receipt) = &mut self.receipt {
                        receipt.items.push(ReceiptItem::new(text));
                    }
                    self.row = self.item_count().saturating_sub(1);
                }
            }
            EditTarget::NewPersonName => {
                if !text.is_empty() {
                    self.people.push(Person::new(text));
                    self.row = self.people.len().saturating_sub(1);
                }
            }
            EditTarget::RenamePerson(index) => {
                if let Some(person) = self.people.get_mut(index) {
                    person.name = text;
                }
            }
        }
    }

    fn begin_edit(&mut self, target: EditTarget, initial: String) {
        self.edit = Some(target);
        self.input = initial;
        self.mode = Mode::Insert;
    }

    fn save_and_quit(&mut self) {
        if self.receipt.is_some() {
            let _ = self.save_state();
        }
        self.quit = true;
    }

    pub fn save_state(&mut self) -> Result<()> {
        let Some(receipt) = self.receipt.clone() else {
            return Ok(());
        };
        let saved = SavedState {
            receipt,
            people: self.people.clone(),
            assignments: self.assignments.clone(),
            tip_bps: self.tip_bps,
        };
        let path = state::save(&self.args.state_dir, &saved)?;
        self.status = format!("Saved {}", path.display());
        self.saved_path = Some(path);
        Ok(())
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
            Err(error) => self.status = format!("{error}"),
        }
    }

    fn next_screen(&mut self) {
        let next = (self.screen.index() + 1).min(4);
        self.go_to(Screen::from_index(next));
    }

    fn prev_screen(&mut self) {
        let floor = if self.receipt.is_some() { 1 } else { 0 };
        let previous = self.screen.index().saturating_sub(1).max(floor);
        self.go_to(Screen::from_index(previous));
    }

    fn go_to(&mut self, screen: Screen) {
        self.screen = screen;
        self.row = 0;
        self.col = 0;
    }

    fn move_cursor(&mut self, row_delta: i64, col_delta: i64) {
        match self.screen {
            Screen::Receipt => self.row = clamp_add(self.row, row_delta, self.item_count()),
            Screen::People => self.row = clamp_add(self.row, row_delta, self.people.len()),
            Screen::Assign => {
                self.row = clamp_add(self.row, row_delta, self.item_count());
                self.col = clamp_add(self.col, col_delta, self.people.len());
            }
            _ => {}
        }
    }

    fn delete_item(&mut self) {
        let removed = if let Some(receipt) = &mut self.receipt {
            if self.row < receipt.items.len() {
                Some(receipt.items.remove(self.row).id)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(id) = removed {
            self.assignments.clear_item(&id);
            self.row = self.row.min(self.item_count().saturating_sub(1));
        }
    }

    fn delete_person(&mut self) {
        if self.row < self.people.len() {
            let removed = self.people.remove(self.row);
            self.assignments.remove_person(&removed.id);
            self.row = self.row.min(self.people.len().saturating_sub(1));
            self.col = self.col.min(self.people.len().saturating_sub(1));
        }
    }

    fn toggle_cell(&mut self) {
        let item = self.item(self.row).map(|item| item.id.clone());
        let person = self.people.get(self.col).map(|person| person.id.clone());
        if let (Some(item), Some(person)) = (item, person) {
            self.assignments.toggle(&item, &person);
        }
    }

    fn toggle_row_all(&mut self) {
        let Some(item_id) = self.item(self.row).map(|item| item.id.clone()) else {
            return;
        };
        let all = self
            .people
            .iter()
            .all(|person| self.assignments.is_assigned(&item_id, &person.id));
        if all {
            self.assignments.clear_item(&item_id);
        } else {
            for person in &self.people {
                self.assignments.set(&item_id, &person.id, true);
            }
        }
    }

    fn solo_cell(&mut self) {
        let Some(item_id) = self.item(self.row).map(|item| item.id.clone()) else {
            return;
        };
        let Some(person_id) = self.people.get(self.col).map(|person| person.id.clone()) else {
            return;
        };
        self.assignments.clear_item(&item_id);
        self.assignments.set(&item_id, &person_id, true);
    }

    fn clear_row(&mut self) {
        if let Some(item_id) = self.item(self.row).map(|item| item.id.clone()) {
            self.assignments.clear_item(&item_id);
        }
    }

    fn item(&self, index: usize) -> Option<&ReceiptItem> {
        self.receipt.as_ref().and_then(|receipt| receipt.items.get(index))
    }

    fn item_mut(&mut self, index: usize) -> Option<&mut ReceiptItem> {
        self.receipt
            .as_mut()
            .and_then(|receipt| receipt.items.get_mut(index))
    }

    fn item_count(&self) -> usize {
        self.receipt.as_ref().map(|r| r.items.len()).unwrap_or(0)
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
