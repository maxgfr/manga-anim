//! Projet = une plage de pages d'un titre de la bibliothèque, plus tout ce qu'on en
//! dérive (analyse, voix, timeline, vidéos) dans `projects/<nom>/`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::home::{projects_dir, slugify};
use crate::library;

/// Sens de lecture : `rtl` pour le manga japonais, `ltr` pour comics et webtoons.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Rtl,
    Ltr,
}

/// Position dans un titre : `ch101:p3`. `ch101` seul désigne tout le chapitre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PagePos {
    pub chapter: f64,
    pub page: Option<usize>,
}

impl std::str::FromStr for PagePos {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim().to_lowercase();
        let (ch, page) = match s.split_once(':') {
            Some((c, p)) => (c, Some(p)),
            None => (s.as_str(), None),
        };
        let chapter = ch
            .trim_start_matches("ch")
            .trim_start_matches('c')
            .parse::<f64>()
            .with_context(|| format!("position invalide « {s} » (attendu ch101 ou ch101:p3)"))?;
        let page = page
            .map(|p| p.trim_start_matches('p').parse::<usize>().with_context(|| format!("page invalide dans « {s} »")))
            .transpose()?;
        if page == Some(0) {
            bail!("les pages commencent à 1");
        }
        Ok(Self { chapter, page })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageRef {
    /// Nom dans `pages/` : `0001.png`.
    pub file: String,
    pub chapter: f64,
    pub page: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    pub title: String,
    /// Code langue ISO 639-1 du texte des planches (`fr`, `en`, `ja`…).
    pub lang: String,
    pub direction: Direction,
    pub from: String,
    pub to: String,
    pub pages: Vec<PageRef>,
}

pub struct Project {
    pub root: PathBuf,
    pub cfg: ProjectConfig,
}

impl Project {
    pub fn dir(name: &str) -> PathBuf {
        projects_dir().join(slugify(name))
    }

    pub fn open(name: &str) -> Result<Self> {
        let root = Self::dir(name);
        let text = fs::read_to_string(root.join("project.toml"))
            .with_context(|| format!("projet introuvable : {} (manga project new …)", root.display()))?;
        Ok(Self { cfg: toml::from_str(&text)?, root })
    }

    pub fn save(&self) -> Result<()> {
        fs::write(self.root.join("project.toml"), toml::to_string_pretty(&self.cfg)?)?;
        Ok(())
    }

    pub fn path(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.root.join(rel)
    }

    pub fn page_path(&self, page: &PageRef) -> PathBuf {
        self.root.join("pages").join(&page.file)
    }

    pub fn analysis_path(&self, page: &PageRef) -> PathBuf {
        self.root.join("analysis").join(format!("{}.json", stem(&page.file)))
    }

    /// Crée le projet en copiant les pages de la plage `from..=to`.
    pub fn create(name: &str, title: &str, from: PagePos, to: PagePos, lang: &str, direction: Direction) -> Result<Self> {
        let title = slugify(title);
        let chapters = library::chapters(&title)?;
        let mut selected = Vec::new();
        for (n, dir) in &chapters {
            if *n < from.chapter || *n > to.chapter {
                continue;
            }
            let pages = library::chapter_pages(dir)?;
            for (i, path) in pages.iter().enumerate() {
                let p = i + 1;
                if *n == from.chapter && from.page.is_some_and(|f| p < f) {
                    continue;
                }
                if *n == to.chapter && to.page.is_some_and(|t| p > t) {
                    continue;
                }
                selected.push((*n, p, path.clone()));
            }
        }
        if selected.is_empty() {
            let have: Vec<String> = chapters.iter().map(|(n, _)| library::chapter_dir_name(*n)).collect();
            bail!("aucune page dans cette plage. Chapitres disponibles pour {title} : {}", have.join(", "));
        }

        let root = Self::dir(name);
        let pages_dir = root.join("pages");
        if pages_dir.exists() {
            fs::remove_dir_all(&pages_dir)?;
        }
        fs::create_dir_all(&pages_dir)?;
        let mut pages = Vec::new();
        for (i, (chapter, page, src)) in selected.into_iter().enumerate() {
            let ext = src.extension().unwrap().to_string_lossy().to_ascii_lowercase();
            let file = format!("{:04}.{ext}", i + 1);
            fs::copy(&src, pages_dir.join(&file))?;
            pages.push(PageRef { file, chapter, page });
        }
        let fmt = |p: PagePos| match p.page {
            Some(pg) => format!("ch{}:p{pg}", library::chapter_dir_name(p.chapter)),
            None => format!("ch{}", library::chapter_dir_name(p.chapter)),
        };
        let project = Self {
            cfg: ProjectConfig {
                name: slugify(name),
                title,
                lang: lang.to_string(),
                direction,
                from: fmt(from),
                to: fmt(to),
                pages,
            },
            root,
        };
        project.save()?;
        if !project.path("cast.toml").exists() {
            fs::write(project.path("cast.toml"), crate::voice::default_cast(lang))?;
        }
        Ok(project)
    }
}

pub fn stem(file: &str) -> &str {
    file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_pos() {
        let p: PagePos = "ch101:p3".parse().unwrap();
        assert_eq!(p, PagePos { chapter: 101.0, page: Some(3) });
        let p: PagePos = "CH101.5".parse().unwrap();
        assert_eq!(p, PagePos { chapter: 101.5, page: None });
        let p: PagePos = "12:4".parse().unwrap();
        assert_eq!(p, PagePos { chapter: 12.0, page: Some(4) });
        assert!("ch1:p0".parse::<PagePos>().is_err());
        assert!("page".parse::<PagePos>().is_err());
    }
}
