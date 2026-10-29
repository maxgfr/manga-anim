//! `manga characters` : les persos détectés par Magi, recadrés par libellé dans
//! `characters/found/<libellé>/`. On en tire la banque `characters/<Nom>.png` (qui nomme
//! les locuteurs à l'analyse) et les références d'un spin-off (`manga story`).

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use serde::{Deserialize, Serialize};

use crate::analysis::{BBox, PageAnalysis, area};
use crate::home::{is_image, slugify};
use crate::project::Project;
use crate::render::crop_panel;

/// Un perso trouvé : `index.json` liste ce qu'il faut pour le reconnaître et le nommer.
#[derive(Debug, Serialize, Deserialize)]
pub struct Found {
    pub label: String,
    pub appearances: usize,
    /// Chemins relatifs au projet, plus grandes apparitions d'abord.
    pub crops: Vec<String>,
    /// Quelques répliques attribuées à ce perso.
    pub lines: Vec<String>,
}

const MAX_LINES: usize = 5;

pub fn run(project: &Project, per_character: usize) -> Result<Vec<Found>> {
    let mut boxes: BTreeMap<String, Vec<(usize, BBox)>> = BTreeMap::new();
    let mut lines: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (i, page) in project.cfg.pages.iter().enumerate() {
        let analysis = PageAnalysis::load(&project.analysis_path(page))?;
        for c in &analysis.characters {
            if let Some(name) = &c.name {
                boxes.entry(name.clone()).or_default().push((i, c.bbox));
            }
        }
        for t in analysis.texts.iter().filter(|t| t.essential) {
            if let Some(s) = &t.speaker {
                lines.entry(s.clone()).or_default().push(t.text.clone());
            }
        }
    }
    if boxes.is_empty() {
        bail!(
            "aucun personnage dans l'analyse : elle date d'avant `manga characters` ou vient du détecteur vision. \
             Relance `manga analyze {} --force` (Magi), en sachant que ça écrase les corrections faites dans la liseuse",
            project.cfg.name
        );
    }
    let dir = project.path("characters/found");
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    let mut pages: BTreeMap<usize, DynamicImage> = BTreeMap::new();
    let mut found = Vec::new();
    for (label, mut list) in boxes {
        let appearances = list.len();
        list.sort_by(|a, b| area(&b.1).total_cmp(&area(&a.1)));
        let sub = format!("characters/found/{}", slugify(&label));
        fs::create_dir_all(project.path(&sub))?;
        let mut crops = Vec::new();
        for (n, (page, bbox)) in list.into_iter().take(per_character).enumerate() {
            let img = match pages.entry(page) {
                Entry::Occupied(e) => e.into_mut(),
                Entry::Vacant(e) => {
                    let p = &project.cfg.pages[page];
                    e.insert(image::open(project.page_path(p)).with_context(|| format!("page illisible : {}", p.file))?)
                }
            };
            let rel = format!("{sub}/{:02}.png", n + 1);
            crop_panel(img, &bbox).save(project.path(&rel))?;
            crops.push(rel);
        }
        let lines = lines.remove(&label).unwrap_or_default().into_iter().take(MAX_LINES).collect();
        found.push(Found { label, appearances, crops, lines });
    }
    found.sort_by_key(|f| Reverse(f.appearances));
    fs::write(dir.join("index.json"), serde_json::to_string_pretty(&found)?)?;
    Ok(found)
}

/// Banque des persos nommés : `characters/<Nom>.png`, un portrait par perso.
pub fn bank(project: &Project) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = fs::read_dir(project.path("characters"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_image(p))
        .map(|p| (p.file_stem().unwrap().to_string_lossy().to_string(), p))
        .collect();
    out.sort();
    out
}

/// Lit `characters/found/index.json` s'il existe.
pub fn load_found(project: &Project) -> Vec<Found> {
    fs::read_to_string(project.path("characters/found/index.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}
