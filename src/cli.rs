use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "split", version, about = "Split a restaurant bill from a receipt image")]
pub struct Args {
    /// Path to the receipt image.
    pub image: PathBuf,

    /// pi model to use for extraction; must support images.
    #[arg(long, default_value = "openrouter/google/gemini-2.5-flash")]
    pub model: String,

    /// Root directory for saved state.
    #[arg(long, default_value = "/tmp/split-expense")]
    pub state_dir: PathBuf,

    /// pi executable to shell out to.
    #[arg(long, default_value = "pi")]
    pub pi_bin: String,

    /// Ignore any saved split for this image and re-parse / recalculate from scratch.
    #[arg(long)]
    pub clean: bool,
}
