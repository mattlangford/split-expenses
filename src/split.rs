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
    /// Items plus tax.
    pub computed_pretip_cents: i64,
    /// Items plus tax plus tip: the amount actually being split.
    pub computed_total_cents: i64,
    pub receipt_total_cents: i64,
    pub receipt_includes_tax: bool,
    pub receipt_includes_tip: bool,
    /// Our computed amount on the same basis as the receipt total (matching
    /// whatever tax/tip composition the receipt total claims to have).
    pub comparable_cents: i64,
    pub total_matches: bool,
}

/// Compute each person's share of the bill.
///
/// - Each item's line total is split equally among the people checked for it.
/// - Tax and tip are allocated proportionally to each person's item subtotal.
/// - Tip is the printed gratuity if the receipt has one, otherwise
///   `subtotal * tip_bps / 10000`, rounded to the nearest cent.
/// - Whether the printed total includes tax/tip is taken into account so nothing
///   is double counted.
/// - All allocations use the largest-remainder method so nothing is lost.
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

    let tip_cents = match receipt.tip_cents {
        Some(printed) => printed,
        None => ((items_sum_cents as i128 * tip_bps as i128 + 5_000) / 10_000) as i64,
    };

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

    let computed_pretip_cents = items_sum_cents + receipt.tax_cents;
    let computed_total_cents = computed_pretip_cents + tip_cents;

    // Compare like with like: pick the computed amount whose composition matches
    // what the receipt total claims to include.
    let comparable_cents = match (receipt.total_includes_tax, receipt.total_includes_tip) {
        (true, true) => computed_total_cents,
        (true, false) => computed_pretip_cents,
        (false, true) => computed_total_cents - receipt.tax_cents,
        (false, false) => items_sum_cents,
    };
    let total_matches = comparable_cents == receipt.total_cents;

    SplitResult {
        shares,
        items_sum_cents,
        assigned_sum_cents,
        unassigned,
        tax_cents: receipt.tax_cents,
        tip_cents,
        computed_pretip_cents,
        computed_total_cents,
        receipt_total_cents: receipt.total_cents,
        receipt_includes_tax: receipt.total_includes_tax,
        receipt_includes_tip: receipt.total_includes_tip,
        comparable_cents,
        total_matches,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Person, ReceiptItem};
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
            tip_cents: None,
            total_cents: total,
            total_includes_tax: true,
            total_includes_tip: false,
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
        a.toggle("beer", "p0");
        a.toggle("beer", "p1");
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
            a.toggle("beers", p);
        }
        let res = compute_split(&r, &folks, &a, 0);
        let sum: i64 = res.shares.iter().map(|s| s.items_cents).sum();
        assert_eq!(sum, 1000);
    }

    #[test]
    fn tax_and_tip_are_proportional_and_exact() {
        let folks = people(&["Alice", "Bob"]);
        // Alice orders 3000, Bob 1000; tax 400; tip 20% of 4000 = 800.
        let r = receipt(vec![item("steak", 3000), item("salad", 1000)], 400, 4400);
        let mut a = Assignments::default();
        a.toggle("steak", "p0");
        a.toggle("salad", "p1");
        let res = compute_split(&r, &folks, &a, 2000);
        assert_eq!(res.tip_cents, 800);
        assert_eq!(res.shares[0].tax_cents, 300);
        assert_eq!(res.shares[1].tax_cents, 100);
        assert_eq!(res.shares[0].tip_cents, 600);
        assert_eq!(res.shares[1].tip_cents, 200);
        let total: i64 = res.shares.iter().map(|s| s.total_cents()).sum();
        assert_eq!(total, res.computed_total_cents);
        assert!(res.total_matches);
    }

    #[test]
    fn printed_gratuity_is_used_and_total_reconciles() {
        let folks = people(&["Alice"]);
        let mut r = receipt(vec![item("meal", 2000)], 200, 2420);
        r.tip_cents = Some(220);
        r.total_includes_tip = true;
        let mut a = Assignments::default();
        a.toggle("meal", "p0");
        let res = compute_split(&r, &folks, &a, 2000);
        assert_eq!(res.tip_cents, 220);
        assert_eq!(res.computed_total_cents, 2420);
        assert!(res.total_matches);
    }

    #[test]
    fn receipt_total_without_tax_compares_to_subtotal() {
        let folks = people(&["Alice"]);
        let mut r = receipt(vec![item("meal", 2000)], 200, 2000);
        r.total_includes_tax = false;
        let mut a = Assignments::default();
        a.toggle("meal", "p0");
        let res = compute_split(&r, &folks, &a, 2000);
        assert_eq!(res.comparable_cents, 2000);
        assert!(res.total_matches);
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
