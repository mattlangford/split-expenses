/// Format cents as a grouped decimal string, e.g. 123456 -> "1,234.56".
pub fn format_cents(cents: i64) -> String {
    let negative = cents < 0;
    let abs = cents.unsigned_abs();
    let dollars = abs / 100;
    let remainder = abs % 100;
    let digits = dollars.to_string();
    let mut grouped = String::new();
    let bytes = digits.as_bytes();
    for (i, byte) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(*byte as char);
    }
    format!(
        "{}{}.{:02}",
        if negative { "-" } else { "" },
        grouped,
        remainder
    )
}

/// Parse a user- or model-supplied money string into cents.
/// Accepts "$1,234.56", "1234.5", "12", "-3.25".
pub fn parse_cents(input: &str) -> Option<i64> {
    let cleaned: String = input
        .trim()
        .chars()
        .filter(|c| !matches!(c, '$' | ',' | ' ' | '\u{00a0}'))
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    let (negative, body) = match cleaned.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, cleaned.as_str()),
    };
    let mut parts = body.splitn(2, '.');
    let int_part = parts.next().unwrap_or("0");
    let frac_part = parts.next().unwrap_or("");
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    let int_value: i64 = if int_part.is_empty() {
        0
    } else {
        int_part.parse().ok()?
    };
    let mut frac = frac_part.to_string();
    frac.truncate(2);
    while frac.len() < 2 {
        frac.push('0');
    }
    let frac_value: i64 = if frac.is_empty() {
        0
    } else {
        frac.parse().ok()?
    };
    let total = int_value.checked_mul(100)?.checked_add(frac_value)?;
    Some(if negative { -total } else { total })
}

/// Parse a percentage string into basis points (hundredths of a percent).
/// "20" -> 2000, "18.5%" -> 1850.
pub fn parse_percent_bps(input: &str) -> Option<u32> {
    let cleaned = input.trim().trim_end_matches('%').trim();
    if cleaned.is_empty() {
        return None;
    }
    let value: f64 = cleaned.parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    Some((value * 100.0).round() as u32)
}

/// Convert a part/whole pair of cent amounts into basis points, rounded to the
/// nearest basis point. E.g. (165, 2000) -> 825 (8.25%).
pub fn rate_bps(part_cents: i64, whole_cents: i64) -> u32 {
    if whole_cents <= 0 {
        return 0;
    }
    ((part_cents as i128 * 10_000 + whole_cents as i128 / 2) / whole_cents as i128) as u32
}

/// Format basis points as a percentage, e.g. 2000 -> "20", 1850 -> "18.5".
pub fn format_percent_bps(bps: u32) -> String {
    if bps.is_multiple_of(100) {
        format!("{}", bps / 100)
    } else if bps.is_multiple_of(10) {
        format!("{}.{}", bps / 100, (bps % 100) / 10)
    } else {
        format!("{}.{:02}", bps / 100, bps % 100)
    }
}

/// Split `total` among `weights` using the largest-remainder method so the
/// returned amounts always sum exactly to `total`.
pub fn allocate(total: i64, weights: &[i64]) -> Vec<i64> {
    let n = weights.len();
    if n == 0 {
        return Vec::new();
    }
    let weight_sum: i128 = weights.iter().map(|&w| w.max(0) as i128).sum();
    if weight_sum == 0 {
        // Nothing to weight by (e.g. no items assigned yet): allocate nothing.
        return vec![0_i64; n];
    }

    let mut shares = vec![0_i64; n];
    let mut remainders: Vec<(i128, usize)> = Vec::with_capacity(n);
    let mut assigned: i128 = 0;
    for (i, &w) in weights.iter().enumerate() {
        let w = w.max(0) as i128;
        let numerator = (total as i128) * w;
        let quotient = numerator.div_euclid(weight_sum);
        let remainder = numerator.rem_euclid(weight_sum);
        shares[i] = quotient as i64;
        remainders.push((remainder, i));
        assigned += quotient;
    }

    let mut leftover = total as i128 - assigned;
    remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut idx = 0;
    while leftover > 0 {
        shares[remainders[idx % n].1] += 1;
        leftover -= 1;
        idx += 1;
    }
    shares
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_money() {
        assert_eq!(format_cents(0), "0.00");
        assert_eq!(format_cents(5), "0.05");
        assert_eq!(format_cents(1234), "12.34");
        assert_eq!(format_cents(123456), "1,234.56");
        assert_eq!(format_cents(-1234), "-12.34");
    }

    #[test]
    fn parses_money() {
        assert_eq!(parse_cents("12.34"), Some(1234));
        assert_eq!(parse_cents("$1,234.56"), Some(123456));
        assert_eq!(parse_cents("12"), Some(1200));
        assert_eq!(parse_cents("12.3"), Some(1230));
        assert_eq!(parse_cents("-3.25"), Some(-325));
        assert_eq!(parse_cents("abc"), None);
    }

    #[test]
    fn parses_percent() {
        assert_eq!(parse_percent_bps("20"), Some(2000));
        assert_eq!(parse_percent_bps("18.5%"), Some(1850));
        assert_eq!(format_percent_bps(2000), "20");
        assert_eq!(format_percent_bps(1850), "18.5");
        assert_eq!(format_percent_bps(1855), "18.55");
    }

    #[test]
    fn derives_rate_from_amounts() {
        assert_eq!(rate_bps(165, 2000), 825);
        assert_eq!(rate_bps(0, 2000), 0);
        assert_eq!(rate_bps(5, 0), 0);
    }

    #[test]
    fn allocate_is_exact() {
        assert_eq!(allocate(100, &[1, 1, 1]), vec![34, 33, 33]);
        assert_eq!(allocate(100, &[1, 2]), vec![33, 67]);
        let parts = allocate(999, &[5, 3, 2]);
        assert_eq!(parts.iter().sum::<i64>(), 999);
    }

    #[test]
    fn allocate_zero_weights_allocates_nothing() {
        assert_eq!(allocate(10, &[0, 0, 0]), vec![0, 0, 0]);
    }
}
