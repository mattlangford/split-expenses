use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use chrono::{Local, NaiveDate};
use serde_json::Value;
use uuid::Uuid;

use crate::money::{parse_cents, rate_bps};
use crate::types::{Receipt, ReceiptItem};

const EXTRACTION_PROMPT: &str = r#"You are a precise receipt OCR engine. Look at the receipt image and reply with ONLY a single minified JSON object. Transcribe only values printed on the receipt; do not compute, total, or infer anything. Do not include markdown fences or any commentary.

Schema:
{"merchant":string,"date":"YYYY-MM-DD","currency":"USD","items":[{"name":string,"qty":number,"unit_price":number,"line_total":number}],"subtotal":number|null,"tax":number,"tip":number|null,"total":number}

Rules:
- All monetary values are decimal dollars exactly as printed, e.g. 12.34 (never include a currency symbol).
- qty defaults to 1 when not shown.
- line_total is the amount printed for the line, after any discounts or comps.
- unit_price is the printed per-unit price, or null if the receipt does not show one.
- Include every purchased line item, including drinks and shared plates.
- subtotal is the pre-tax total if printed, otherwise null.
- tax is the printed tax amount, or 0 if no tax line is shown.
- tip is any tip or gratuity printed on the receipt, otherwise null.
- total is the final amount printed on the receipt.
- date is the transaction date printed on the receipt, or null if unknown."#;

/// Progress emitted while `pi` reads the receipt.
#[derive(Debug, Clone)]
pub enum ExtractProgress {
    /// A short human-readable stage message.
    Status(String),
    /// Assistant text accumulated so far.
    Partial(String),
}

/// Run `pi` headlessly to OCR the receipt with a vision model.
///
/// Uses `--mode json` so events stream back while pi works; `on_progress` is
/// called with stage updates and partial assistant output. Returns the parsed
/// receipt once pi exits.
pub fn extract_receipt(
    pi_bin: &str,
    model: &str,
    image: &Path,
    mut on_progress: impl FnMut(ExtractProgress),
) -> Result<Receipt> {
    if !image.exists() {
        bail!("receipt image not found: {}", image.display());
    }

    on_progress(ExtractProgress::Status(format!("launching {pi_bin}")));

    let mut child = Command::new(pi_bin)
        .arg("--mode")
        .arg("json")
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
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run `{pi_bin}` (is pi installed and on PATH?)"))?;

    let stdout = child.stdout.take().context("pi stdout unavailable")?;
    let stderr = child.stderr.take().context("pi stderr unavailable")?;
    let stderr_handle = std::thread::spawn(move || {
        let mut buffer = String::new();
        let mut reader = BufReader::new(stderr);
        let _ = reader.read_to_string(&mut buffer);
        buffer
    });

    let mut assistant = String::new();
    for line in BufReader::new(stdout).lines() {
        let line = line.context("reading pi output")?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(kind) = event.get("type").and_then(Value::as_str) else {
            continue;
        };
        match kind {
            "agent_start" => on_progress(ExtractProgress::Status("pi agent started".to_string())),
            "turn_start" => on_progress(ExtractProgress::Status(
                "model is reading the receipt…".to_string(),
            )),
            "message_update" | "message_end" => {
                let text = assistant_text(&event);
                if !text.is_empty() && text != assistant {
                    assistant = text.clone();
                    on_progress(ExtractProgress::Partial(text));
                }
            }
            "agent_end" => {
                if let Some(messages) = event.get("messages").and_then(Value::as_array) {
                    for message in messages.iter().rev() {
                        if message.get("role").and_then(Value::as_str) == Some("assistant") {
                            let text = message_text(message);
                            if !text.is_empty() && text != assistant {
                                assistant = text.clone();
                                on_progress(ExtractProgress::Partial(text));
                            }
                            break;
                        }
                    }
                }
            }
            "agent_settled" => on_progress(ExtractProgress::Status("finishing up…".to_string())),
            "tool_execution_start" => {
                on_progress(ExtractProgress::Status("model used a tool".to_string()))
            }
            _ => {}
        }
    }

    let status = child.wait().context("waiting for pi")?;
    let stderr = stderr_handle.join().unwrap_or_default();

    if assistant.is_empty() {
        if !status.success() {
            bail!("pi exited with {}: {}", status, stderr.trim());
        }
        bail!("pi returned no response. stderr: {}", stderr.trim());
    }

    let json = extract_json(&assistant).with_context(|| {
        format!(
            "model did not return a JSON object. Output:\n{}",
            assistant.trim()
        )
    })?;
    let value: Value = serde_json::from_str(json).context("model returned invalid JSON")?;
    normalize(&value)
}

/// Extract the assistant text from a pi JSON event, if it is an assistant message.
fn assistant_text(event: &Value) -> String {
    let Some(message) = event.get("message") else {
        return String::new();
    };
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return String::new();
    }
    message_text(message)
}

fn message_text(message: &Value) -> String {
    let Some(content) = message.get("content").and_then(Value::as_array) else {
        return String::new();
    };
    let mut text = String::new();
    for part in content {
        if part.get("type").and_then(Value::as_str) == Some("text")
            && let Some(chunk) = part.get("text").and_then(Value::as_str)
        {
            text.push_str(chunk);
        }
    }
    text
}

fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

fn normalize(value: &Value) -> Result<Receipt> {
    let obj = value.as_object().context("expected a JSON object")?;
    let cents = |key: &str| obj.get(key).and_then(value_to_cents);

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
            let unit_price_cents = raw.get("unit_price").and_then(value_to_cents).unwrap_or(0);
            let line_total_cents = raw
                .get("line_total")
                .and_then(value_to_cents)
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
    let subtotal_cents = cents("subtotal");
    let base = subtotal_cents.unwrap_or(items_sum);
    let parsed_tax = cents("tax").unwrap_or(0);
    let parsed_tip = cents("tip");
    let total_cents = cents("total").unwrap_or_else(|| base + parsed_tax);

    // The model only reports what is printed; deciding how the printed total is
    // composed (does it already include tax/tip?) is Rust's job.
    let (total_includes_tax, total_includes_tip) =
        infer_inclusions(base, parsed_tax, parsed_tip, total_cents);

    // Recover tax/tip from the printed total when the receipt does not itemise them.
    let mut tax_cents = parsed_tax;
    let mut tip_cents = parsed_tip;
    if total_includes_tax && tax_cents == 0 {
        let included_tip = if total_includes_tip {
            tip_cents.unwrap_or(0)
        } else {
            0
        };
        tax_cents = (total_cents - base - included_tip).max(0);
    }
    if total_includes_tip && tip_cents.is_none() {
        let included_tax = if total_includes_tax { tax_cents } else { 0 };
        tip_cents = Some((total_cents - base - included_tax).max(0));
    }
    let tax_bps = rate_bps(tax_cents, base);

    Ok(Receipt {
        merchant,
        transaction_date,
        currency,
        items,
        subtotal_cents,
        tax_cents,
        tax_bps,
        tip_cents,
        total_cents,
        total_includes_tax,
        total_includes_tip,
    })
}

/// Pick the tax/tip-inclusion interpretation whose components best reconcile
/// with the printed total. Ties prefer folding tax in and, when a tip was
/// printed, accounting for it.
fn infer_inclusions(
    base_cents: i64,
    tax_cents: i64,
    tip_cents: Option<i64>,
    total_cents: i64,
) -> (bool, bool) {
    let tip = tip_cents.unwrap_or(0);
    let candidates = [
        (true, true, base_cents + tax_cents + tip),
        (true, false, base_cents + tax_cents),
        (false, true, base_cents + tip),
        (false, false, base_cents),
    ];
    candidates
        .iter()
        .min_by_key(|(includes_tax, includes_tip, candidate)| {
            let distance = (candidate - total_cents).abs();
            let tip_mismatch = i64::from(tip_cents.is_some() != *includes_tip);
            let tax_mismatch = i64::from(!*includes_tax);
            (distance, tip_mismatch, tax_mismatch)
        })
        .map(|(includes_tax, includes_tip, _)| (*includes_tax, *includes_tip))
        .unwrap_or((false, false))
}

fn value_to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn value_to_cents(value: &Value) -> Option<i64> {
    match value {
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

    #[test]
    fn derives_tax_from_total_and_subtotal() {
        let value = json!({
            "items": [{"name": "meal", "line_total": 20.0}],
            "subtotal": 20.0,
            "total": 21.65
        });
        let receipt = normalize(&value).unwrap();
        assert_eq!(receipt.tax_cents, 165);
        assert_eq!(receipt.tax_bps, 825);
    }

    #[test]
    fn infers_tax_inclusive_total() {
        // subtotal + tax == printed total.
        assert_eq!(infer_inclusions(2000, 165, None, 2165), (true, false));
    }

    #[test]
    fn infers_tax_exclusive_total() {
        // Printed total is the pre-tax amount; tax is added on top.
        assert_eq!(infer_inclusions(2000, 165, None, 2000), (false, false));
    }

    #[test]
    fn infers_printed_tip() {
        // subtotal + tax + gratuity == printed total.
        assert_eq!(infer_inclusions(2000, 200, Some(220), 2420), (true, true));
        // Gratuity printed but the total leaves it out.
        assert_eq!(infer_inclusions(2000, 200, Some(220), 2200), (true, false));
    }

    #[test]
    fn normalize_uses_printed_gratuity() {
        let value = json!({
            "items": [{"name": "meal", "line_total": 20.0}],
            "subtotal": 20.0,
            "tax": 2.0,
            "tip": 2.2,
            "total": 24.2
        });
        let receipt = normalize(&value).unwrap();
        assert!(receipt.total_includes_tax);
        assert!(receipt.total_includes_tip);
        assert_eq!(receipt.tip_cents, Some(220));
    }

    #[test]
    fn reads_assistant_text_from_events() {
        let event = json!({
            "type": "message_update",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "{\"merchant\":"},
                    {"type": "text", "text": "\"X\"}"}
                ]
            }
        });
        assert_eq!(assistant_text(&event), "{\"merchant\":\"X\"}");
    }
}
