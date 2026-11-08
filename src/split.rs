use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::money::allocate;
use crate::types::{Assignments, Person, Receipt};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonShare {
    pub person_id: String,
    pub name: String,
    pub items_cents: i64,
    pub tax_cents: i64,
    pub tip_cents: i64,
}

impl PersonShare {
    pub fn total_cents(&self) -> i64 {
        self.items_cents + self.tax_cents + self.tip_cents
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SplitResult {
    pub shares: Vec<PersonShare>,
    pub items_sum_cents: i64,
    pub assigned_sum_cents: i64,
    pub unassigned: Vec<String>,
    pub tax_cents: i64,
    pub tip_cents: i64,
    pub computed_total_cents: i64,
    pub receipt_total_cents: i64,
}

/// Compute each person's share of the bill.
///
/// - Each item's line total is split equally among the people checked for it.
/// - Tax and tip are allocated proportionally to each person's item subtotal.
/// - Tip is `subtotal * tip_bps / 10000`, rounded to the nearest cent.
/// - All allocations use the largest-remainder method so nothing is lost to rounding.
pub fn compute_split(
    receipt: &Receipt,
    people: &[Person],
    assignments: &Assignments,
    tip_bps: u32,
) -> SplitResult {
    let mut items_cents: BTreeMap<String, i64> =
        people.iter().map(|p| (p.id.clone(), 0)).collect();
    let mut unassigned: Vec<String> = Vec::new();

    for item in &receipt.items {
        let participants = assignments.participants(&item.id, people);
        if participants.is_empty() {
            unassigned.push(item.id.clone());
            continue;
        }
        let weights = vec![1_i64; participants.len()];
        for (person_id, amount) in participants.iter().zip(allocate(item.line_total_cents, &weights))
        {
            if let Some(slot) = items_cents.get_mut(*person_id) {
                *slot += amount;
            }
        }
    }

    let items_sum_cents: i64 = receipt.items.iter().map(|i| i.line_total_cents).sum();
    let assigned_sum_cents: i64 = items_cents.values().sum();
    let subtotal = receipt.subtotal_cents.unwrap_or(items_sum_cents);
    let tip_cents = ((subtotal as i128 * tip_bps as i128 + 5_000) / 10_000) as i64;

    let weights: Vec<i64> = people
        .iter()
        .map(|p| items_cents.get(&p.id).copied().unwrap_or(0))
        .collect();
    let tax_alloc = allocate(receipt.tax_cents, &weights);
    let tip_alloc = allocate(tip_cents, &weights);

    let shares = people
        .iter()
        .enumerate()
        .map(|(i, p)| PersonShare {
            person_id: p.id.clone(),
            name: p.name.clone(),
            items_cents: items_cents.get(&p.id).copied().unwrap_or(0),
            tax_cents: tax_alloc.get(i).copied().unwrap_or(0),
            tip_cents: tip_alloc.get(i).copied().unwrap_or(0),
        })
        .collect();

    SplitResult {
        shares,
        items_sum_cents,
        assigned_sum_cents,
        unassigned,
        tax_cents: receipt.tax_cents,
        tip_cents,
        computed_total_cents: items_sum_cents + receipt.tax_cents + tip_cents,
        receipt_total_cents: receipt.total_cents,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ReceiptItem, Person};
    use chrono::NaiveDate;

    fn item(id: &str, cents: i64) -> ReceiptItem {
        ReceiptItem {
            id: id.into(),
            name: id.into(),
            qty: 1,
            unit_price_cents: cents,
            line_total_cents: cents,
        }
    }

    fn receipt(items: Vec<ReceiptItem>, tax: i64, total: i64) -> Receipt {
        Receipt {
            merchant: "Test".into(),
            transaction_date: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            currency: "USD".into(),
            items,
            subtotal_cents: None,
            tax_cents: tax,
            total_cents: total,
        }
    }

    fn people(names: &[&str]) -> Vec<Person> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| Person {
                id: format!("p{i}"),
                name: (*n).into(),
            })
            .collect()
    }

    #[test]
    fn shared_item_splits_evenly() {
        let folks = people(&["Alice", "Bob"]);
        let r = receipt(vec![item("beer", 1000)], 0, 1000);
        let mut a = Assignments::default();
        a.set("beer", "p0", true);
        a.set("beer", "p1", true);
        let res = compute_split(&r, &folks, &a, 0);
        assert_eq!(res.shares[0].items_cents, 500);
        assert_eq!(res.shares[1].items_cents, 500);
        assert_eq!(res.unassigned.len(), 0);
    }

    #[test]
    fn indivisible_cents_are_preserved() {
        let folks = people(&["A", "B", "C"]);
        let r = receipt(vec![item("beers", 1000)], 0, 1000);
        let mut a = Assignments::default();
        for p in ["p0", "p1", "p2"] {
            a.set("beers", p, true);
        }
        let res = compute_split(&r, &folks, &a, 0);
        let sum: i64 = res.shares.iter().map(|s| s.items_cents).sum();
        assert_eq!(sum, 1000);
    }

    #[test]
    fn tax_and_tip_are_proportional_and_exact() {
        let folks = people(&["Alice", "Bob"]);
        // Alice orders 3000, Bob orders 1000, tax 400, tip 20% of 4000 = 800.
        let r = receipt(vec![item("steak", 3000), item("salad", 1000)], 400, 4200);
        let mut a = Assignments::default();
        a.set("steak", "p0", true);
        a.set("salad", "p1", true);
        let res = compute_split(&r, &folks, &a, 2000);
        assert_eq!(res.tip_cents, 800);
        assert_eq!(res.shares[0].tax_cents, 300);
        assert_eq!(res.shares[1].tax_cents, 100);
        assert_eq!(res.shares[0].tip_cents, 600);
        assert_eq!(res.shares[1].tip_cents, 200);
        let total: i64 = res.shares.iter().map(|s| s.total_cents()).sum();
        assert_eq!(total, res.computed_total_cents);
    }

    #[test]
    fn unassigned_items_are_reported() {
        let folks = people(&["Alice"]);
        let r = receipt(vec![item("fries", 500)], 0, 500);
        let a = Assignments::default();
        let res = compute_split(&r, &folks, &a, 0);
        assert_eq!(res.unassigned, vec!["fries".to_string()]);
        assert_eq!(res.assigned_sum_cents, 0);
    }
}
