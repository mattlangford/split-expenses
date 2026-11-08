use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::{Local, NaiveDate};
use serde_json::Value;
use uuid::Uuid;

use crate::money::parse_cents;
use crate::types::{Receipt, ReceiptItem};

const EXTRACTION_PROMPT: &str = r#"You are a precise receipt parser. Look at the receipt image and reply with ONLY a single minified JSON object. Do not include markdown fences or any commentary.

Schema:
{"merchant":string,"date":"YYYY-MM-DD","currency":"USD","items":[{"name":string,"qty":number,"unit_price":number,"line_total":number}],"subtotal":number,"tax":number,"total":number}

Rules:
- All monetary values are decimal dollars, e.g. 12.34 (never include a currency symbol).
- qty defaults to 1 when not shown.
- line_total is the amount printed for the line, after any discounts or comps.
- Include every purchased line item, including drinks and shared plates.
- subtotal is the pre-tax total if printed, otherwise null.
- tax is the tax amount; total is the final amount charged.
- date is the transaction date printed on the receipt, or null if unknown."#;

/// Run `pi` headlessly to OCR the receipt with a vision model.
pub fn extract_receipt(pi_bin: &str, model: &str, image: &Path) -> Result<Receipt> {
    if !image.exists() {
        bail!("receipt image not found: {}", image.display());
    }

    let output = Command::new(pi_bin)
        .arg("--print")
        .arg("--model")
        .arg(model)
        .arg("--no-session")
        .arg("--no-tools")
        .arg("--no-extensions")
        .arg("--no-skills")
        .arg("--no-context-files")
        .arg("--no-prompt-templates")
        .arg("--no-themes")
        .arg(format!("@{}", image.display()))
        .arg(EXTRACTION_PROMPT)
        .output()
        .with_context(|| format!("failed to run `{pi_bin}` (is pi installed and on PATH?)"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("pi exited with {}: {}", output.status, stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json = extract_json(&stdout)
        .with_context(|| format!("model did not return a JSON object. Output:\n{}", stdout.trim()))?;
    let value: Value = serde_json::from_str(json).context("model returned invalid JSON")?;
    normalize(&value)
}

fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

fn normalize(value: &Value) -> Result<Receipt> {
    let obj = value.as_object().context("expected a JSON object")?;

    let merchant = obj
        .get("merchant")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("Unknown")
        .to_string();

    let transaction_date = obj
        .get("date")
        .and_then(Value::as_str)
        .and_then(parse_date)
        .unwrap_or_else(|| Local::now().date_naive());

    let currency = obj
        .get("currency")
        .and_then(Value::as_str)
        .unwrap_or("USD")
        .to_string();

    let mut items = Vec::new();
    if let Some(array) = obj.get("items").and_then(Value::as_array) {
        for raw in array {
            let name = raw
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Item")
                .to_string();
            let qty = raw
                .get("qty")
                .and_then(value_to_f64)
                .map(|q| q.round().max(1.0) as u32)
                .unwrap_or(1);
            let unit_price_cents = raw.get("unit_price").and_then(|v| value_to_cents(Some(v))).unwrap_or(0);
            let line_total_cents = raw
                .get("line_total")
                .and_then(|v| value_to_cents(Some(v)))
                .unwrap_or(unit_price_cents * qty as i64);
            items.push(ReceiptItem {
                id: Uuid::new_v4().to_string(),
                name,
                qty,
                unit_price_cents,
                line_total_cents,
            });
        }
    }
    if items.is_empty() {
        bail!("model returned no line items");
    }

    let items_sum: i64 = items.iter().map(|i| i.line_total_cents).sum();
    let subtotal_cents = obj.get("subtotal").and_then(|v| value_to_cents(Some(v)));
    let tax_cents = obj.get("tax").and_then(|v| value_to_cents(Some(v))).unwrap_or(0);
    let total_cents = obj
        .get("total")
        .and_then(|v| value_to_cents(Some(v)))
        .unwrap_or_else(|| subtotal_cents.unwrap_or(items_sum) + tax_cents);

    Ok(Receipt {
        merchant,
        transaction_date,
        currency,
        items,
        subtotal_cents,
        tax_cents,
        total_cents,
    })
}

fn value_to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn value_to_cents(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(n) => n.as_f64().map(|f| (f * 100.0).round() as i64),
        Value::String(s) => parse_cents(s),
        _ => None,
    }
}

fn parse_date(input: &str) -> Option<NaiveDate> {
    const FORMATS: [&str; 8] = [
        "%Y-%m-%d",
        "%m/%d/%Y",
        "%m/%d/%y",
        "%d/%m/%Y",
        "%b %d, %Y",
        "%B %d, %Y",
        "%Y/%m/%d",
        "%d-%m-%Y",
    ];
    let trimmed = input.trim();
    FORMATS
        .iter()
        .find_map(|fmt| NaiveDate::parse_from_str(trimmed, fmt).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_json_from_prose() {
        let text = "Sure! Here is the data:\n{\"a\":1}\nHope that helps.";
        assert_eq!(extract_json(text), Some("{\"a\":1}"));
    }

    #[test]
    fn normalizes_money_and_items() {
        let value = json!({
            "merchant": "Joe's Diner",
            "date": "2024-03-05",
            "items": [
                {"name": "Burger", "qty": 2, "unit_price": 12.5, "line_total": 25.0},
                {"name": "Fries", "unit_price": "$4.50"}
            ],
            "subtotal": 29.5,
            "tax": 2.36,
            "total": 31.86
        });
        let receipt = normalize(&value).unwrap();
        assert_eq!(receipt.merchant, "Joe's Diner");
        assert_eq!(receipt.items.len(), 2);
        assert_eq!(receipt.items[0].line_total_cents, 2500);
        assert_eq!(receipt.items[1].line_total_cents, 450);
        assert_eq!(receipt.tax_cents, 236);
        assert_eq!(receipt.total_cents, 3186);
        assert_eq!(
            receipt.transaction_date.format("%Y-%m-%d").to_string(),
            "2024-03-05"
        );
    }

    #[test]
    fn missing_total_falls_back_to_subtotal_plus_tax() {
        let value = json!({"items":[{"name":"x","line_total":10}], "tax": 1});
        let receipt = normalize(&value).unwrap();
        assert_eq!(receipt.total_cents, 1100);
    }
}
