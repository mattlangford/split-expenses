use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

/// Stable content hash of the receipt image, so re-passing the same image
/// reloads its saved split.
pub fn image_hash(image: &Path) -> Result<String> {
    let bytes = std::fs::read(image)?;
    let digest = Sha256::digest(&bytes);
    let mut out = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        out.push_str(&format!("{byte:02x}"));
    }
    Ok(out)
}

/// /tmp/split-expense/<merchant>/<transaction-date>/
pub fn state_dir(root: &Path, receipt: &Receipt) -> PathBuf {
    root.join(slug(&receipt.merchant))
        .join(receipt.transaction_date.format("%Y-%m-%d").to_string())
}

fn image_index_path(root: &Path, image: &Path) -> Result<PathBuf> {
    Ok(root.join("by-image").join(image_hash(image)?))
}

pub fn save(root: &Path, state: &SavedState, image: &Path) -> Result<()> {
    let dir = state_dir(root, &state.receipt);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("state.json");
    let json = serde_json::to_string_pretty(state)?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;

    // Index the canonical file by image content so the same photo reloads it.
    if let Ok(index) = image_index_path(root, image) {
        if let Some(parent) = index.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&index, path.to_string_lossy().as_bytes());
    }
    Ok(())
}

pub fn load_by_image(root: &Path, image: &Path) -> Result<Option<SavedState>> {
    let Ok(index) = image_index_path(root, image) else {
        return Ok(None);
    };
    if !index.exists() {
        return Ok(None);
    }
    let target = std::fs::read_to_string(&index)?;
    let target = PathBuf::from(target.trim());
    if !target.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&target)?;
    Ok(Some(serde_json::from_str(&text)?))
}
