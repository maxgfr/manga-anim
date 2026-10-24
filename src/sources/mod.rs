//! Sources en ligne. Chacune renvoie des séries et des chapitres, puis télécharge un
//! chapitre dans la bibliothèque locale.

pub mod mangadex;

use std::path::Path;

use anyhow::{Result, bail};

#[derive(Debug, Clone)]
pub struct SeriesHit {
    pub id: String,
    pub title: String,
    pub year: Option<i64>,
    pub status: String,
    pub languages: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ChapterInfo {
    pub id: String,
    pub chapter: Option<f64>,
    pub volume: Option<String>,
    pub title: String,
    pub pages: u32,
    pub group: String,
    /// Chapitre publié ailleurs (éditeur officiel) : pas de pages téléchargeables.
    pub external_url: Option<String>,
}

pub trait Source {
    fn search(&self, query: &str, lang: Option<&str>) -> Result<Vec<SeriesHit>>;
    fn chapters(&self, series_id: &str, lang: &str) -> Result<Vec<ChapterInfo>>;
    fn series_title(&self, series_id: &str) -> Result<String>;
    /// Télécharge les pages dans `dest` (créé vide). Renvoie le nombre de pages.
    fn fetch(&self, chapter: &ChapterInfo, dest: &Path) -> Result<usize>;
}

pub fn by_name(name: &str) -> Result<Box<dyn Source>> {
    match name {
        "mangadex" => Ok(Box::new(mangadex::MangaDex::new()?)),
        other => bail!("source inconnue : {other} (disponible : mangadex)"),
    }
}

/// `101-110`, `1,3,5-7`, `all`.
pub fn parse_chapter_spec(spec: &str) -> Result<Vec<(f64, f64)>> {
    if spec == "all" {
        return Ok(vec![(f64::MIN, f64::MAX)]);
    }
    spec.split(',')
        .map(|part| {
            let part = part.trim();
            let (a, b) = part.split_once('-').unwrap_or((part, part));
            match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
                (Ok(a), Ok(b)) if a <= b => Ok((a, b)),
                _ => bail!("plage de chapitres invalide : {part}"),
            }
        })
        .collect()
}

pub fn in_spec(spec: &[(f64, f64)], n: f64) -> bool {
    spec.iter().any(|&(a, b)| n >= a && n <= b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapter_spec() {
        let s = parse_chapter_spec("1,3,5-7").unwrap();
        assert!(in_spec(&s, 1.0) && in_spec(&s, 6.0) && in_spec(&s, 7.0));
        assert!(!in_spec(&s, 2.0) && !in_spec(&s, 8.0));
        assert!(parse_chapter_spec("9-3").is_err());
        assert!(in_spec(&parse_chapter_spec("all").unwrap(), 101.5));
    }
}
