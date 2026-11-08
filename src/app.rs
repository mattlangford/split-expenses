use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Instant;

use anyhow::Result;
use chrono::NaiveDate;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::cli::Args;
use crate::money::{format_cents, format_percent_bps, parse_cents, parse_percent_bps, rate_bps};
use crate::split::{SplitResult, compute_split};
use crate::types::{Assignments, Person, Receipt, ReceiptItem};
use crate::{pi, state};

/// Columns before the people columns: item name, qty, line total.
pub const ITEM_COLS: usize = 3;
/// Receipt-level fields shown below the grid: merchant, date, tax, subtotal, total, tip %.
const DETAIL_ROWS: usize = 6;

const KEY_HELP: &str = "Keys\n  arrows       move between boxes\n  type         edit the highlighted box\n  Enter        edit the box, or toggle a share\n  Space        toggle a share\n  d / Delete   delete the focused item row (or person on a header), asks first\n  u / Ctrl+Z   undo\n  ?            this help\n  Ctrl+C       quit";

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
    TaxPercent,
    Subtotal,
    Total,
    TipPercent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteTarget {
    Item(usize),
    Person(usize),
}

/// A point-in-time copy of the document state, for undo.
#[derive(Clone)]
struct Snapshot {
    receipt: Option<Receipt>,
    people: Vec<Person>,
    assignments: Assignments,
    tip_bps: u32,
    row: usize,
    col: usize,
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
    /// Contextual info popup text, shown when `?` is pressed.
    pub help: Option<String>,
    /// Pending "are you sure" delete target.
    pub confirm_delete: Option<DeleteTarget>,
    undo: Vec<Snapshot>,
    /// When set, the next character typed replaces the seeded editor value.
    replace_on_type: bool,
    pub adding_people: bool,
    pub adding_items: bool,
    pub spinner: usize,
    pub extract_started: Instant,
    pub extract_status: String,
    pub extract_partial: String,
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
            help: None,
            confirm_delete: None,
            undo: Vec::new(),
            replace_on_type: false,
            adding_people: false,
            adding_items: false,
            spinner: 0,
            extract_started: Instant::now(),
            extract_status: String::new(),
            extract_partial: String::new(),
            errors: Vec::new(),
            rx,
        };
        if app.args.clean {
            app.status = "Clean run: re-parsing the receipt".to_string();
            app.start_extraction();
        } else {
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
        // Default the tip rate to whatever gratuity the receipt printed.
        if let Some(tip) = receipt.tip_cents {
            self.tip_bps = rate_bps(tip, receipt.subtotal_or_items_cents());
        }
        self.receipt = Some(receipt);
        self.screen = Screen::Grid;
        self.row = 0;
        self.col = 0;
        self.save_state();
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
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if control && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) {
            self.save_and_quit();
            return;
        }
        if self.confirm_delete.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.confirm_delete_now();
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm_delete = None;
                }
                _ => {}
            }
            return;
        }
        if self.help.is_some() {
            self.help = None;
            return;
        }
        if control && matches!(key.code, KeyCode::Char('z') | KeyCode::Char('Z')) {
            self.undo();
            return;
        }
        if self.mode == Mode::Insert {
            self.handle_insert(key);
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
            KeyCode::Up => self.move_grid(-1, 0),
            KeyCode::Down => self.move_grid(1, 0),
            KeyCode::Left => self.move_grid(0, -1),
            KeyCode::Right => self.move_grid(0, 1),
            KeyCode::Char(' ') => self.grid_toggle(),
            KeyCode::Enter => {
                if self.is_assignment_cell() {
                    self.grid_toggle();
                } else {
                    self.begin_edit_focused();
                }
            }
            KeyCode::Delete | KeyCode::Backspace | KeyCode::Char('d') => self.request_delete(),
            KeyCode::Char('u') => self.undo(),
            KeyCode::Char('?') => self.help = Some(self.help_text()),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.type_into_cell(c);
            }
            _ => {}
        }
    }

    /// Typing while a cell/field is highlighted starts editing it, replacing the
    /// value with the typed character.
    fn type_into_cell(&mut self, c: char) {
        let Some(target) = self.focused_target() else {
            return;
        };
        if is_numeric_target(&target) && !is_numeric_char(c) {
            return;
        }
        if matches!(target, EditTarget::Date) && !(c.is_ascii_digit() || c == '-' || c == '/') {
            return;
        }
        let new_person = matches!(target, EditTarget::GridNewPerson);
        let new_item = matches!(target, EditTarget::GridNewItem);
        self.begin_edit(target, c.to_string());
        self.adding_people = new_person;
        self.adding_items = new_item;
    }

    fn focused_target(&self) -> Option<EditTarget> {
        let items_len = self.item_count();
        let grid_rows = self.grid_row_count();
        if self.row < grid_rows {
            if self.row == 0 {
                if let Some(person_index) = self.person_at_col(self.col) {
                    return Some(EditTarget::GridPersonName(person_index));
                }
                if self.is_add_person_col(self.col) {
                    return Some(EditTarget::GridNewPerson);
                }
                return None;
            }
            if self.row == items_len + 1 {
                if self.col == 0 {
                    return Some(EditTarget::GridNewItem);
                }
                return None;
            }
            if self.row == items_len + 2 {
                return Some(EditTarget::TaxPercent);
            }
            if self.row == items_len + 3 {
                return Some(EditTarget::TipPercent);
            }
            if self.row == items_len + 4 {
                // "Owed" summary row: not editable.
                return None;
            }
            let index = self.row - 1;
            return match self.col {
                0 => Some(EditTarget::GridItemName(index)),
                1 => Some(EditTarget::GridItemQty(index)),
                2 => Some(EditTarget::GridItemTotal(index)),
                _ => None,
            };
        }
        match self.row - grid_rows {
            0 => Some(EditTarget::Merchant),
            1 => Some(EditTarget::Date),
            2 => Some(EditTarget::Subtotal),
            3 => Some(EditTarget::TaxPercent),
            4 => Some(EditTarget::Total),
            _ => Some(EditTarget::TipPercent),
        }
    }

    fn handle_insert(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.replace_on_type = false;
                self.adding_people = false;
                self.adding_items = false;
                self.mode = Mode::Normal;
                self.edit = None;
                self.input.clear();
            }
            KeyCode::Enter => self.commit_edit(),
            KeyCode::Up => self.commit_and_move(-1, 0),
            KeyCode::Down => self.commit_and_move(1, 0),
            KeyCode::Left => self.commit_and_move(0, -1),
            KeyCode::Right => self.commit_and_move(0, 1),
            KeyCode::Backspace => {
                self.replace_on_type = false;
                self.input.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.replace_on_type {
                    self.input.clear();
                    self.replace_on_type = false;
                }
                self.input.push(c)
            }
            _ => {}
        }
    }

    /// Commit the current edit and move the cursor (used when an arrow key is
    /// pressed while typing, so Enter is not required to leave the box).
    fn commit_and_move(&mut self, row_delta: i64, col_delta: i64) {
        self.adding_people = false;
        self.adding_items = false;
        self.commit_edit();
        self.move_grid(row_delta, col_delta);
    }

    fn commit_edit(&mut self) {
        let Some(target) = self.edit.take() else {
            return;
        };
        let text = self.input.trim().to_string();
        self.checkpoint();
        self.mode = Mode::Normal;
        self.replace_on_type = false;
        self.input.clear();

        match target {
            EditTarget::GridItemName(index) => {
                if let Some(item) = self.item_mut(index) {
                    item.name = text;
                }
            }
            EditTarget::GridItemQty(index) => {
                if let Ok(qty) = text.parse::<u32>()
                    && let Some(item) = self.item_mut(index)
                {
                    item.qty = qty.max(1);
                }
            }
            EditTarget::GridItemTotal(index) => {
                if let Some(cents) = parse_cents(&text)
                    && let Some(item) = self.item_mut(index)
                {
                    item.line_total_cents = cents;
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
                    self.row = self.item_count() + 1;
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
                if let Ok(date) = NaiveDate::parse_from_str(&text, "%Y-%m-%d")
                    && let Some(receipt) = &mut self.receipt
                {
                    receipt.transaction_date = date;
                }
            }
            EditTarget::TaxPercent => {
                if let Some(bps) = parse_percent_bps(&text)
                    && let Some(receipt) = &mut self.receipt
                {
                    receipt.tax_bps = bps;
                }
            }
            EditTarget::Subtotal => {
                let value = if text.is_empty() {
                    Some(None)
                } else {
                    parse_cents(&text).map(Some)
                };
                if let Some(value) = value
                    && let Some(receipt) = &mut self.receipt
                {
                    receipt.subtotal_cents = value;
                }
            }
            EditTarget::Total => {
                if let Some(cents) = parse_cents(&text)
                    && let Some(receipt) = &mut self.receipt
                {
                    receipt.total_cents = cents;
                }
            }
            EditTarget::TipPercent => {
                if let Some(bps) = parse_percent_bps(&text) {
                    self.tip_bps = bps;
                }
            }
        }
        self.save_state();
    }

    fn begin_edit(&mut self, target: EditTarget, initial: String) {
        self.adding_people = false;
        self.adding_items = false;
        self.replace_on_type = false;
        self.edit = Some(target);
        self.input = initial;
        self.mode = Mode::Insert;
    }

    /// Start editing the highlighted field via Enter, seeded with its current
    /// value. The first typed character replaces that value, exactly as if the
    /// user had started typing on the cell; Backspace edits the seeded value.
    fn begin_edit_focused(&mut self) {
        let Some(target) = self.focused_target() else {
            return;
        };
        let new_person = matches!(target, EditTarget::GridNewPerson);
        let new_item = matches!(target, EditTarget::GridNewItem);
        let initial = self.current_value(&target);
        self.begin_edit(target, initial);
        self.adding_people = new_person;
        self.adding_items = new_item;
        self.replace_on_type = true;
    }

    /// The committed value of an editable field, used to seed the editor.
    fn current_value(&self, target: &EditTarget) -> String {
        match target {
            EditTarget::GridItemName(index) => self
                .item(*index)
                .map(|item| item.name.clone())
                .unwrap_or_default(),
            EditTarget::GridItemQty(index) => self
                .item(*index)
                .map(|item| item.qty.to_string())
                .unwrap_or_default(),
            EditTarget::GridItemTotal(index) => self
                .item(*index)
                .map(|item| format_cents(item.line_total_cents))
                .unwrap_or_default(),
            EditTarget::GridPersonName(index) => self
                .people
                .get(*index)
                .map(|person| person.name.clone())
                .unwrap_or_default(),
            EditTarget::GridNewPerson | EditTarget::GridNewItem => String::new(),
            EditTarget::Merchant => self
                .receipt
                .as_ref()
                .map(|receipt| receipt.merchant.clone())
                .unwrap_or_default(),
            EditTarget::Date => self
                .receipt
                .as_ref()
                .map(|receipt| receipt.transaction_date.to_string())
                .unwrap_or_default(),
            EditTarget::Subtotal => self
                .receipt
                .as_ref()
                .and_then(|receipt| receipt.subtotal_cents)
                .map(format_cents)
                .unwrap_or_default(),
            EditTarget::Total => self
                .receipt
                .as_ref()
                .map(|receipt| format_cents(receipt.total_cents))
                .unwrap_or_default(),
            EditTarget::TaxPercent => self
                .receipt
                .as_ref()
                .map(|receipt| format_percent_bps(receipt.tax_bps))
                .unwrap_or_default(),
            EditTarget::TipPercent => format_percent_bps(self.tip_bps),
        }
    }

    fn begin_grid_new_person(&mut self) {
        self.row = 0;
        self.col = self.grid_cols().saturating_sub(1);
        self.begin_edit(EditTarget::GridNewPerson, String::new());
        self.adding_people = true;
    }

    fn begin_grid_new_item(&mut self) {
        self.row = self.item_count() + 1;
        self.col = 0;
        self.begin_edit(EditTarget::GridNewItem, String::new());
        self.adding_items = true;
    }

    /// True when the cursor is on an item row and a person column, where
    /// Space/Enter toggle that person's share of the item.
    fn is_assignment_cell(&self) -> bool {
        self.row >= 1 && self.row <= self.item_count() && self.person_at_col(self.col).is_some()
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
            self.checkpoint();
            self.assignments.toggle(&item_id, &person_id);
            self.save_state();
        }
    }

    fn request_delete(&mut self) {
        let items_len = self.item_count();
        if self.row >= 1 && self.row <= items_len {
            // Any column on an item row deletes that item.
            self.confirm_delete = Some(DeleteTarget::Item(self.row - 1));
        } else if self.row == 0
            && let Some(person_index) = self.person_at_col(self.col)
        {
            self.confirm_delete = Some(DeleteTarget::Person(person_index));
        }
    }

    fn confirm_delete_now(&mut self) {
        let Some(target) = self.confirm_delete.take() else {
            return;
        };
        self.checkpoint();
        match target {
            DeleteTarget::Item(index) => self.delete_item(index),
            DeleteTarget::Person(index) => self.delete_person(index),
        }
        self.save_state();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            receipt: self.receipt.clone(),
            people: self.people.clone(),
            assignments: self.assignments.clone(),
            tip_bps: self.tip_bps,
            row: self.row,
            col: self.col,
        }
    }

    /// Push the current state before a mutation so it can be undone.
    fn checkpoint(&mut self) {
        let snapshot = self.snapshot();
        self.undo.push(snapshot);
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
    }

    fn undo(&mut self) {
        let Some(snapshot) = self.undo.pop() else {
            self.status = "Nothing to undo".to_string();
            return;
        };
        self.restore(snapshot);
        self.status = "Undo".to_string();
        self.save_state();
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.receipt = snapshot.receipt;
        self.people = snapshot.people;
        self.assignments = snapshot.assignments;
        self.tip_bps = snapshot.tip_bps;
        self.row = snapshot.row;
        self.col = snapshot.col;
        self.mode = Mode::Normal;
        self.edit = None;
        self.replace_on_type = false;
        self.input.clear();
        self.adding_people = false;
        self.adding_items = false;
        self.clamp_cursor();
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
        if let Err(error) = state::save(&self.args.state_dir, &saved, &self.args.image) {
            self.record_error(format!("failed to save state: {error:#}"));
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
        let rows = self.total_rows();
        let cols = self.grid_cols();
        self.row = clamp_add(self.row, row_delta, rows);
        self.col = clamp_add(self.col, col_delta, cols);
    }

    fn clamp_cursor(&mut self) {
        self.row = self.row.min(self.total_rows().saturating_sub(1));
        self.col = self.col.min(self.grid_cols().saturating_sub(1));
    }

    /// header + items + "+ item" + "Tax" + "Tip" + "Owed".
    fn grid_row_count(&self) -> usize {
        self.item_count() + 5
    }

    fn total_rows(&self) -> usize {
        self.grid_row_count() + DETAIL_ROWS
    }

    fn grid_cols(&self) -> usize {
        self.people.len() + ITEM_COLS + 1
    }

    /// Which receipt-level detail row the cursor is on, if any.
    pub fn detail_focus(&self) -> Option<usize> {
        let grid = self.grid_row_count();
        (self.row >= grid).then(|| self.row - grid)
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

    /// Popup text: contextual info about the highlighted box plus a key reference.
    pub fn help_text(&self) -> String {
        format!("{}\n\n{KEY_HELP}", self.context_help())
    }

    /// Contextual text for the `?` popup, based on the highlighted box.
    fn context_help(&self) -> String {
        let Some(receipt) = &self.receipt else {
            return "Reading the receipt with pi…".to_string();
        };
        if let Some(detail) = self.detail_focus() {
            return match detail {
                0 => format!(
                    "Merchant\n\nNames the saved state for this image.\nCurrent: {}",
                    receipt.merchant
                ),
                1 => format!(
                    "Date\n\nTransaction date, stored with the merchant.\nCurrent: {}",
                    receipt.transaction_date
                ),
                2 => format!(
                    "Subtotal\n\nPre-tax total printed on the receipt.\nCurrent: {}",
                    receipt
                        .subtotal_cents
                        .map(money)
                        .unwrap_or_else(|| "(none)".to_string())
                ),
                _ => format!(
                    "Receipt total\n\nPrinted final amount. The receipt check at the bottom compares the parsed items+tax to it.\nCurrent: {}\nTotal includes tax: {}\nTotal includes tip: {}",
                    money(receipt.total_cents),
                    receipt.total_includes_tax,
                    receipt.total_includes_tip
                ),
            };
        }

        let items_len = self.item_count();
        if self.row == 0 {
            if let Some(person_index) = self.person_at_col(self.col)
                && let Some(split) = self.split_result()
                && let Some(share) = split.shares.get(person_index)
            {
                let mut text = format!("{}\n\nItems:\n", share.name);
                for item in &receipt.items {
                    if self.assignments.is_assigned(&item.id, &share.person_id) {
                        text.push_str(&format!(
                            "  {}  {}\n",
                            item.name,
                            money(item.line_total_cents)
                        ));
                    }
                }
                text.push_str(&format!(
                    "\nitems {}   tax {}   tip {}   owes {}",
                    money(share.items_cents),
                    money(share.tax_cents),
                    money(share.tip_cents),
                    money(share.total_cents())
                ));
                return text;
            }
            if self.is_add_person_col(self.col) {
                return "Add a person\n\nType a name and press Enter. Enter again adds another; Esc finishes.".to_string();
            }
            return format!(
                "{} column",
                match self.col {
                    0 => "Item",
                    1 => "Qty",
                    _ => "Total",
                }
            );
        }
        if self.row == items_len + 1 {
            return "Add an item\n\nType a name and press Enter. Enter again adds another; Esc finishes.".to_string();
        }
        if self.row == items_len + 2 {
            return format!(
                "Tax\n\nRate applied to the items.\nCurrent: {}%",
                format_percent_bps(receipt.tax_bps)
            );
        }
        if self.row == items_len + 3 {
            return format!(
                "Tip\n\nRate applied to the items.\nCurrent: {}%",
                format_percent_bps(self.tip_bps)
            );
        }
        if self.row == items_len + 4 {
            let owed: Vec<String> = self
                .split_result()
                .map(|split| {
                    split
                        .shares
                        .iter()
                        .map(|share| format!("{} {}", share.name, money(share.total_cents())))
                        .collect()
                })
                .unwrap_or_default();
            return format!(
                "Owed\n\nEach person's share of items + tax + tip.\n{}",
                owed.join("\n")
            );
        }
        if self.row >= 1 && self.row <= items_len {
            let item = &receipt.items[self.row - 1];
            if let Some(person_index) = self.person_at_col(self.col) {
                let person = &self.people[person_index];
                let assigned = self.assignments.is_assigned(&item.id, &person.id);
                let count = self
                    .assignments
                    .participants(&item.id, &self.people)
                    .len()
                    .max(1);
                let share = item.line_total_cents / count as i64;
                return format!(
                    "{}  {}\n\n{}\nShared by {} {}\n{}'s share of this item: {}",
                    item.name,
                    money(item.line_total_cents),
                    if assigned {
                        "Marked for this person."
                    } else {
                        "Not marked for this person."
                    },
                    count,
                    if count == 1 { "person" } else { "people" },
                    person.name,
                    money(share)
                );
            }
            return match self.col {
                0 => format!("Item: {}", item.name),
                1 => format!(
                    "Qty: {}\n\nQuantity is informational; the line total is what gets split.",
                    item.qty
                ),
                _ => format!(
                    "Total: {}\n\nSplit equally between everyone marked for this item.",
                    money(item.line_total_cents)
                ),
            };
        }

        "Move with the arrow keys. Type (or press Enter) to edit the highlighted box. Space toggles a share. Changes save automatically.".to_string()
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
            Some(EditTarget::TaxPercent) => "TAX %",
            Some(EditTarget::Subtotal) => "SUBTOTAL",
            Some(EditTarget::Total) => "RECEIPT TOTAL",
            Some(EditTarget::TipPercent) => "TIP %",
            None => "EDIT",
        }
    }
}

fn is_numeric_target(target: &EditTarget) -> bool {
    matches!(
        target,
        EditTarget::GridItemQty(_)
            | EditTarget::GridItemTotal(_)
            | EditTarget::TaxPercent
            | EditTarget::Subtotal
            | EditTarget::Total
            | EditTarget::TipPercent
    )
}

fn is_numeric_char(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '.' | '-' | ',' | '$' | '%')
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
