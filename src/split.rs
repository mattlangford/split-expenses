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
    /// Tax allocated to the split, from the tax rate applied to the items.
    pub tax_cents: i64,
    /// Tip allocated to the split, from the tip rate applied to the items.
    pub tip_cents: i64,
    /// Items + allocated tax.
    pub computed_pretip_cents: i64,
    /// Items + allocated tax + allocated tip: the amount actually being split.
    pub computed_total_cents: i64,
    pub receipt_total_cents: i64,
    pub receipt_includes_tax: bool,
    pub receipt_includes_tip: bool,
    /// The parsed receipt's own pretip amount, on the same basis as the receipt
    /// total. Used for the "did the extraction tie out" check.
    pub comparable_cents: i64,
    pub total_matches: bool,
    /// Sum of every person's share.
    pub owed_cents: i64,
    /// What the whole party should actually pay: the receipt total, plus tax/tip
    /// if the receipt total does not already include them.
    pub expected_charge_cents: i64,
    pub owed_matches: bool,
}

/// Apply a basis-point rate to cents, rounded to the nearest cent.
pub fn percent_of(total_cents: i64, bps: u32) -> i64 {
    ((total_cents as i128 * bps as i128 + 5_000) / 10_000) as i64
}

/// Compute each person's share of the bill.
///
/// - Each item's line total is split equally among the people checked for it.
/// - Tax is `items * receipt.tax_bps`; tip is `items * tip_bps`.
/// - Tax and tip are allocated proportionally to each person's item subtotal, and
///   only once items are assigned (an all-zero weight vector allocates nothing).
/// - The receipt check compares the **parsed** items + parsed tax/tip against the
///   parsed total, so it validates the extraction independently of the rates.
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
    // Rates apply to the receipt's subtotal (falling back to the items sum).
    let subtotal_cents = receipt.subtotal_cents.unwrap_or(items_sum_cents);

    let tax_cents = percent_of(subtotal_cents, receipt.tax_bps);
    let tip_cents = percent_of(subtotal_cents, tip_bps);

    let weights: Vec<i64> = people
        .iter()
        .map(|p| items_cents.get(&p.id).copied().unwrap_or(0))
        .collect();
    let tax_alloc = allocate(tax_cents, &weights);
    let tip_alloc = allocate(tip_cents, &weights);

    let shares: Vec<PersonShare> = people
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

    let owed_cents: i64 = shares.iter().map(|s| s.total_cents()).sum();
    let expected_charge_cents = receipt.total_cents
        + if receipt.total_includes_tax {
            0
        } else {
            tax_cents
        }
        + if receipt.total_includes_tip {
            0
        } else {
            tip_cents
        };
    let owed_matches = unassigned.is_empty() && owed_cents == expected_charge_cents;

    let computed_pretip_cents = items_sum_cents + tax_cents;
    let computed_total_cents = computed_pretip_cents + tip_cents;

    // The receipt check uses the parsed receipt numbers, not the chosen rates.
    let parsed_tip_cents = receipt.tip_cents.unwrap_or(0);
    let comparable_cents = match (receipt.total_includes_tax, receipt.total_includes_tip) {
        (true, true) => items_sum_cents + receipt.tax_cents + parsed_tip_cents,
        (true, false) => items_sum_cents + receipt.tax_cents,
        (false, true) => items_sum_cents + parsed_tip_cents,
        (false, false) => items_sum_cents,
    };
    let total_matches = comparable_cents == receipt.total_cents;

    SplitResult {
        shares,
        items_sum_cents,
        assigned_sum_cents,
        unassigned,
        tax_cents,
        tip_cents,
        computed_pretip_cents,
        computed_total_cents,
        receipt_total_cents: receipt.total_cents,
        receipt_includes_tax: receipt.total_includes_tax,
        receipt_includes_tip: receipt.total_includes_tip,
        comparable_cents,
        total_matches,
        owed_cents,
        expected_charge_cents,
        owed_matches,
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

    fn with_tax_bps(mut receipt: Receipt, bps: u32) -> Receipt {
        receipt.tax_bps = bps;
        receipt
    }

    fn receipt(items: Vec<ReceiptItem>, tax: i64, total: i64) -> Receipt {
        Receipt {
            merchant: "Test".into(),
            transaction_date: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            currency: "USD".into(),
            items,
            subtotal_cents: None,
            tax_cents: tax,
            tax_bps: 0,
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
        // 10% tax, 20% tip on 4000 items.
        let r = with_tax_bps(receipt(vec![item("steak", 3000), item("salad", 1000)], 400, 4400), 1000);
        let mut a = Assignments::default();
        a.toggle("steak", "p0");
        a.toggle("salad", "p1");
        let res = compute_split(&r, &folks, &a, 2000);
        assert_eq!(res.tax_cents, 400);
        assert_eq!(res.tip_cents, 800);
        assert_eq!(res.shares[0].tax_cents, 300);
        assert_eq!(res.shares[1].tax_cents, 100);
        assert_eq!(res.shares[0].tip_cents, 600);
        assert_eq!(res.shares[1].tip_cents, 200);
        let total: i64 = res.shares.iter().map(|s| s.total_cents()).sum();
        assert_eq!(total, res.computed_total_cents);
        assert!(res.total_matches);
        assert_eq!(res.expected_charge_cents, 5200);
        assert!(res.owed_matches);
    }

    #[test]
    fn printed_gratuity_is_used_and_total_reconciles() {
        let folks = people(&["Alice"]);
        let mut r = with_tax_bps(receipt(vec![item("meal", 2000)], 200, 2420), 1000);
        r.tip_cents = Some(220);
        r.total_includes_tip = true;
        let mut a = Assignments::default();
        a.toggle("meal", "p0");
        let res = compute_split(&r, &folks, &a, 1100);
        assert_eq!(res.tip_cents, 220);
        assert_eq!(res.computed_total_cents, 2420);
        assert!(res.total_matches);
    }

    #[test]
    fn receipt_total_without_tax_compares_to_subtotal() {
        let folks = people(&["Alice"]);
        let mut r = with_tax_bps(receipt(vec![item("meal", 2000)], 200, 2000), 1000);
        r.total_includes_tax = false;
        let mut a = Assignments::default();
        a.toggle("meal", "p0");
        let res = compute_split(&r, &folks, &a, 2000);
        assert_eq!(res.comparable_cents, 2000);
        assert!(res.total_matches);
    }

    #[test]
    fn rates_apply_to_the_subtotal() {
        let folks = people(&["Alice"]);
        let mut r = with_tax_bps(receipt(vec![item("meal", 2200)], 0, 2200), 700);
        r.subtotal_cents = Some(22000);
        let mut a = Assignments::default();
        a.toggle("meal", "p0");
        let res = compute_split(&r, &folks, &a, 0);
        assert_eq!(res.tax_cents, 1540);
    }

    #[test]
    fn unassigned_bill_has_no_owed() {
        let folks = people(&["Alice", "Bob"]);
        let r = with_tax_bps(receipt(vec![item("fries", 500)], 50, 550), 1000);
        let a = Assignments::default();
        let res = compute_split(&r, &folks, &a, 2000);
        for share in &res.shares {
            assert_eq!(share.total_cents(), 0);
        }
        assert!(!res.owed_matches);
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
