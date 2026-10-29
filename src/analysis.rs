//! Schéma de `analysis/NNNN.json`, écrit par le worker `analyze` et corrigé par la liseuse.
//! Coordonnées en pixels de la page d'origine : `[x1, y1, x2, y2]`.

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
    /// Cases, dans l'ordre de lecture.
    pub panels: Vec<Panel>,
    /// Bulles et cartouches, dans l'ordre de lecture.
    pub texts: Vec<TextBox>,
    /// Personnages détectés (Magi), pour `manga cast`. Absent des anciennes analyses.
    #[serde(default)]
    pub characters: Vec<CharacterBox>,
    #[serde(default)]
    pub detector: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterBox {
    pub bbox: BBox,
    /// Nom de la banque `characters/`, sinon cluster Magi : `perso-0.3` (passe 0, cluster 3).
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Panel {
    pub bbox: BBox,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextBox {
    /// Identifiant stable dans le projet : `0003_t2`.
    pub id: String,
    pub bbox: BBox,
    /// Index de la case qui contient la bulle, si elle est dans une case.
    pub panel: Option<usize>,
    pub text: String,
    pub speaker: Option<String>,
    /// Faux pour les onomatopées et le texte de décor : ni lu, ni sous-titré.
    #[serde(default = "yes")]
    pub essential: bool,
}

fn yes() -> bool {
    true
}

impl PageAnalysis {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path).with_context(|| format!("analyse absente : {} (manga analyze …)", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("analyse invalide : {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

pub fn area(b: &BBox) -> f64 {
    (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0)
}
