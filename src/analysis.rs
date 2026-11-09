//! Schema of `analysis/NNNN.json`, written by the `analyze` worker and corrected in the reader.
//! Coordinates are in pixels of the original page: `[x1, y1, x2, y2]`.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub type BBox = [f64; 4];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageAnalysis {
    pub page: String,
    pub width: u32,
    pub height: u32,
    /// Panels, in reading order.
    pub panels: Vec<Panel>,
    /// Speech bubbles and caption boxes, in reading order.
    pub texts: Vec<TextBox>,
    /// Detected characters (Magi), for `manga cast`. Missing from older analyses.
    #[serde(default)]
    pub characters: Vec<CharacterBox>,
    #[serde(default)]
    pub detector: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterBox {
    pub bbox: BBox,
    /// Name from the `characters/` bank, otherwise a Magi cluster: `character-0.3` (pass 0, cluster 3).
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Panel {
    pub bbox: BBox,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextBox {
    /// Stable identifier within the project: `0003_t2`.
    pub id: String,
    pub bbox: BBox,
    /// Index of the panel containing the bubble, if it is inside a panel.
    pub panel: Option<usize>,
    pub text: String,
    pub speaker: Option<String>,
    /// False for sound effects and background text: neither voiced nor captioned.
    #[serde(default = "yes")]
    pub essential: bool,
}

fn yes() -> bool {
    true
}

impl PageAnalysis {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path).with_context(|| format!("analysis missing: {} (manga analyze …)", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("invalid analysis: {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

pub fn area(b: &BBox) -> f64 {
    (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0)
}
