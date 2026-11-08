use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReceiptItem {
    pub id: String,
    pub name: String,
    pub qty: u32,
    pub unit_price_cents: i64,
    pub line_total_cents: i64,
}

impl ReceiptItem {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            qty: 1,
            unit_price_cents: 0,
            line_total_cents: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Receipt {
    pub merchant: String,
    pub transaction_date: NaiveDate,
    pub currency: String,
    pub items: Vec<ReceiptItem>,
    pub subtotal_cents: Option<i64>,
    pub tax_cents: i64,
    pub total_cents: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Person {
    pub id: String,
    pub name: String,
}

impl Person {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
        }
    }
}

/// Which people shared which item. Absent item => nobody assigned yet.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Assignments {
    pub map: BTreeMap<String, BTreeSet<String>>,
}

impl Assignments {
    pub fn is_assigned(&self, item: &str, person: &str) -> bool {
        self.map.get(item).is_some_and(|set| set.contains(person))
    }

    pub fn toggle(&mut self, item: &str, person: &str) {
        let set = self.map.entry(item.to_string()).or_default();
        if !set.remove(person) {
            set.insert(person.to_string());
        }
        if set.is_empty() {
            self.map.remove(item);
        }
    }

    pub fn set(&mut self, item: &str, person: &str, on: bool) {
        let set = self.map.entry(item.to_string()).or_default();
        if on {
            set.insert(person.to_string());
        } else {
            set.remove(person);
        }
        if set.is_empty() {
            self.map.remove(item);
        }
    }

    pub fn clear_item(&mut self, item: &str) {
        self.map.remove(item);
    }

    pub fn remove_person(&mut self, person: &str) {
        for set in self.map.values_mut() {
            set.remove(person);
        }
        self.map.retain(|_, set| !set.is_empty());
    }

    /// People (in the given order) assigned to `item`.
    pub fn participants<'a>(&'a self, item: &str, people: &'a [Person]) -> Vec<&'a str> {
        people
            .iter()
            .filter(|p| self.is_assigned(item, &p.id))
            .map(|p| p.id.as_str())
            .collect()
    }
}
