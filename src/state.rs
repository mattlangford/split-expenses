use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::types::{Assignments, Person, Receipt};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedState {
    pub receipt: Receipt,
    pub people: Vec<Person>,
    pub assignments: Assignments,
    pub tip_bps: u32,
}

/// Turn an arbitrary string into a filesystem-safe slug.
pub fn slug(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

/// /tmp/split-expense/<merchant>/<transaction-date>/
pub fn state_dir(root: &Path, receipt: &Receipt) -> PathBuf {
    root.join(slug(&receipt.merchant))
        .join(receipt.transaction_date.format("%Y-%m-%d").to_string())
}

pub fn save(root: &Path, state: &SavedState) -> Result<PathBuf> {
    let dir = state_dir(root, &state.receipt);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("state.json");
    let json = serde_json::to_string_pretty(state)?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

pub fn load(root: &Path, receipt: &Receipt) -> Result<Option<SavedState>> {
    let path = state_dir(root, receipt).join("state.json");
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
    Ok(Some(serde_json::from_str(&text)?))
}

/// Copy text to the system clipboard, falling back to a temp file.
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    const CANDIDATES: [(&str, &[&str]); 3] = [
        ("pbcopy", &[]),
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
    ];
    for (program, args) in CANDIDATES {
        let spawned = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = spawned {
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            return Ok(());
        }
    }
    let path = std::env::temp_dir().join("split-expense-summary.txt");
    std::fs::write(&path, text)?;
    anyhow::bail!("no clipboard tool found; wrote summary to {}", path.display())
}
