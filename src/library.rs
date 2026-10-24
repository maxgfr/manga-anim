//! Bibliothèque locale : `library/<titre>/<chapitre>/<NNN>.<ext>`.
//!
//! Tout ce qui entre (CBZ, EPUB, PDF, CBR, dossiers, MangaDex) finit dans ce format,
//! ce qui permet au reste de la chaîne d'ignorer d'où viennent les planches.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::home::{is_image, library_dir, natural_key, slugify};

pub fn title_dir(title: &str) -> PathBuf {
    library_dir().join(slugify(title))
}

/// Numéro de chapitre normalisé pour un nom de dossier (`101`, `101.5`).
pub fn chapter_dir_name(chapter: f64) -> String {
    if chapter.fract() == 0.0 {
        format!("{}", chapter as i64)
    } else {
        format!("{chapter}")
    }
}

/// Trouve un numéro de chapitre dans un nom de fichier : `Vinland Saga - Ch. 101.cbz` → 101.
pub fn guess_chapter(name: &str) -> Option<f64> {
    let lower = name.to_lowercase();
    for marker in ["chapitre", "chapter", "chap", "ch.", "ch", "c", "tome", "vol.", "vol", "t", "v"] {
        if let Some(pos) = lower.rfind(marker) {
            let rest = lower[pos + marker.len()..].trim_start_matches([' ', '.', '_', '-', '#']);
            if let Some(n) = leading_number(rest) {
                return Some(n);
            }
        }
    }
    let digits: Vec<f64> = lower
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .filter_map(|s| s.trim_matches('.').parse().ok())
        .collect();
    digits.last().copied()
}

fn leading_number(s: &str) -> Option<f64> {
    let end = s.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(s.len());
    s[..end].trim_end_matches('.').parse().ok()
}

/// Chapitres présents dans la bibliothèque pour un titre, triés.
pub fn chapters(title: &str) -> Result<Vec<(f64, PathBuf)>> {
    let dir = title_dir(title);
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("titre absent de la bibliothèque : {}", dir.display()))? {
        let path = entry?.path();
        if let Some(n) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.parse::<f64>().ok()) {
            out.push((n, path));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(out)
}

pub fn chapter_pages(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut pages: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| is_image(p))
        .collect();
    pages.sort_by_key(|p| natural_key(&p.file_name().unwrap().to_string_lossy()));
    Ok(pages)
}

pub fn list_titles() -> Result<Vec<(String, usize)>> {
    let dir = library_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let n = chapters(&name).map(|c| c.len()).unwrap_or(0);
            out.push((name, n));
        }
    }
    out.sort();
    Ok(out)
}

/// Importe un fichier ou un dossier. Renvoie les chapitres créés.
///
/// Un dossier d'images est un chapitre ; un dossier qui contient des archives ou des
/// sous-dossiers est une série, chaque entrée devenant un chapitre.
pub fn import(src: &Path, title: &str, chapter: Option<f64>) -> Result<Vec<(f64, usize)>> {
    if src.is_dir() {
        let mut entries: Vec<PathBuf> = fs::read_dir(src)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort_by_key(|p| natural_key(&p.file_name().unwrap().to_string_lossy()));
        if entries.iter().any(|p| is_image(p)) {
            let n = chapter.or_else(|| guess_chapter(&src.file_name().unwrap().to_string_lossy())).unwrap_or(1.0);
            let images: Vec<PathBuf> = entries.into_iter().filter(|p| is_image(p)).collect();
            return Ok(vec![(n, store_files(title, n, &images)?)]);
        }
        let mut out = Vec::new();
        for (i, entry) in entries.iter().filter(|p| p.is_dir() || is_archive(p)).enumerate() {
            let name = entry.file_stem().unwrap().to_string_lossy();
            let n = guess_chapter(&name).unwrap_or((i + 1) as f64);
            out.extend(import(entry, title, Some(n))?);
        }
        if out.is_empty() {
            bail!("aucune image ni archive dans {}", src.display());
        }
        return Ok(out);
    }

    let n = chapter.or_else(|| guess_chapter(&src.file_stem().unwrap().to_string_lossy())).unwrap_or(1.0);
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let count = match ext.as_str() {
        "cbz" | "zip" | "epub" => import_zip(src, title, n)?,
        "pdf" => import_pdf(src, title, n)?,
        "cbr" | "rar" | "cb7" | "7z" => import_with_unarchiver(src, title, n)?,
        _ if is_image(src) => store_files(title, n, &[src.to_path_buf()])?,
        _ => bail!("format non géré : {} (attendu : cbz, zip, epub, pdf, cbr, dossier d'images)", src.display()),
    };
    Ok(vec![(n, count)])
}

fn is_archive(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("cbz" | "zip" | "epub" | "pdf" | "cbr" | "rar" | "cb7" | "7z")
    )
}

fn fresh_chapter_dir(title: &str, n: f64) -> Result<PathBuf> {
    let dir = title_dir(title).join(chapter_dir_name(n));
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn store_files(title: &str, n: f64, images: &[PathBuf]) -> Result<usize> {
    let dir = fresh_chapter_dir(title, n)?;
    for (i, img) in images.iter().enumerate() {
        let ext = img.extension().unwrap().to_string_lossy().to_ascii_lowercase();
        fs::copy(img, dir.join(format!("{:03}.{ext}", i + 1)))?;
    }
    Ok(images.len())
}

/// Pages d'un EPUB dans l'ordre de lecture : spine de l'OPF → balises img des XHTML.
fn epub_image_order(archive: &mut zip::ZipArchive<fs::File>) -> Option<Vec<String>> {
    let container = read_entry(archive, "META-INF/container.xml")?;
    let opf_path = attr_after(&container, "full-path=")?;
    let opf = read_entry(archive, &opf_path)?;
    let base = Path::new(&opf_path).parent().unwrap_or(Path::new(""));

    let mut manifest = std::collections::HashMap::new();
    for item in opf.split("<item ").skip(1) {
        if let (Some(id), Some(href)) = (attr_after(item, "id="), attr_after(item, "href=")) {
            manifest.insert(id, href);
        }
    }
    let mut images = Vec::new();
    for itemref in opf.split("<itemref ").skip(1) {
        let href = manifest.get(&attr_after(itemref, "idref=")?)?;
        let doc_path = join_zip_path(base, href);
        let doc = read_entry(archive, &doc_path)?;
        let doc_base = Path::new(&doc_path).parent().unwrap_or(Path::new("")).to_path_buf();
        for tag in doc.split("<img").skip(1).chain(doc.split("<image").skip(1)) {
            if let Some(src) = attr_after(tag, "src=").or_else(|| attr_after(tag, "xlink:href=")).or_else(|| attr_after(tag, "href=")) {
                images.push(join_zip_path(&doc_base, &src));
            }
        }
    }
    (!images.is_empty()).then_some(images)
}

fn read_entry(archive: &mut zip::ZipArchive<fs::File>, name: &str) -> Option<String> {
    let mut s = String::new();
    archive.by_name(name).ok()?.read_to_string(&mut s).ok()?;
    Some(s)
}

fn attr_after(s: &str, key: &str) -> Option<String> {
    let rest = &s[s.find(key)? + key.len()..];
    let quote = rest.chars().next()?;
    let rest = &rest[1..];
    Some(rest[..rest.find(quote)?].to_string())
}

fn join_zip_path(base: &Path, href: &str) -> String {
    let mut parts: Vec<String> = base.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    for seg in href.split('#').next().unwrap_or("").split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s.to_string()),
        }
    }
    parts.join("/")
}

fn import_zip(src: &Path, title: &str, n: f64) -> Result<usize> {
    let mut archive = zip::ZipArchive::new(fs::File::open(src)?).with_context(|| format!("archive illisible : {}", src.display()))?;
    let mut names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().and_then(|f| f.name().ok().map(|n| n.to_string())))
        .filter(|n| is_image(Path::new(n)) && !n.starts_with("__MACOSX"))
        .collect();
    let is_epub = src.extension().is_some_and(|e| e.eq_ignore_ascii_case("epub"));
    if let Some(order) = is_epub.then(|| epub_image_order(&mut archive)).flatten() {
        names = order.into_iter().filter(|n| names.contains(n)).collect();
        names.dedup();
    } else {
        names.sort_by_key(|n| natural_key(n));
    }
    if names.is_empty() {
        bail!("aucune image dans {}", src.display());
    }
    let dir = fresh_chapter_dir(title, n)?;
    for (i, name) in names.iter().enumerate() {
        let ext = Path::new(name).extension().unwrap().to_string_lossy().to_ascii_lowercase();
        let mut buf = Vec::new();
        archive.by_name(name)?.read_to_end(&mut buf)?;
        fs::write(dir.join(format!("{:03}.{ext}", i + 1)), buf)?;
    }
    Ok(names.len())
}

fn import_pdf(src: &Path, title: &str, n: f64) -> Result<usize> {
    let dir = fresh_chapter_dir(title, n)?;
    let status = Command::new("pdftoppm")
        .args(["-r", "150", "-png"])
        .arg(src)
        .arg(dir.join("p"))
        .status()
        .context("pdftoppm introuvable (brew install poppler)")?;
    if !status.success() {
        bail!("pdftoppm a échoué sur {}", src.display());
    }
    renumber(&dir)
}

fn import_with_unarchiver(src: &Path, title: &str, n: f64) -> Result<usize> {
    let tmp = tempdir_in_library()?;
    let ok = Command::new("unar").args(["-q", "-o"]).arg(&tmp).arg(src).status().map(|s| s.success()).unwrap_or(false)
        || Command::new("7z").arg("x").arg(format!("-o{}", tmp.display())).arg(src).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        fs::remove_dir_all(&tmp).ok();
        bail!("impossible d'extraire {} : installe `unar` ou `7z` (brew install unar)", src.display());
    }
    let mut images: Vec<PathBuf> = walkdir::WalkDir::new(&tmp)
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.into_path())
        .filter(|p| is_image(p))
        .collect();
    images.sort_by_key(|p| natural_key(&p.to_string_lossy()));
    let count = store_files(title, n, &images);
    fs::remove_dir_all(&tmp).ok();
    count
}

fn tempdir_in_library() -> Result<PathBuf> {
    let dir = library_dir().join(format!(".tmp-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Renomme les images d'un dossier en `001.ext`, `002.ext`… dans l'ordre naturel.
fn renumber(dir: &Path) -> Result<usize> {
    let pages = chapter_pages(dir)?;
    let staged: Vec<PathBuf> = pages
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let tmp = dir.join(format!(".{i}.tmp"));
            fs::rename(p, &tmp).map(|_| tmp)
        })
        .collect::<std::io::Result<_>>()?;
    for (i, (tmp, orig)) in staged.iter().zip(&pages).enumerate() {
        let ext = orig.extension().unwrap().to_string_lossy().to_ascii_lowercase();
        fs::rename(tmp, dir.join(format!("{:03}.{ext}", i + 1)))?;
    }
    Ok(pages.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapter_guess() {
        assert_eq!(guess_chapter("Vinland Saga - Ch. 101"), Some(101.0));
        assert_eq!(guess_chapter("vinland_saga_c101.5"), Some(101.5));
        assert_eq!(guess_chapter("Vinland Saga Tome 12"), Some(12.0));
        assert_eq!(guess_chapter("Chapitre 7 - Le retour"), Some(7.0));
        assert_eq!(guess_chapter("sans numero"), None);
    }

    #[test]
    fn chapter_names() {
        assert_eq!(chapter_dir_name(101.0), "101");
        assert_eq!(chapter_dir_name(101.5), "101.5");
    }

    #[test]
    fn zip_paths() {
        assert_eq!(join_zip_path(Path::new("OEBPS/text"), "../images/p1.jpg"), "OEBPS/images/p1.jpg");
        assert_eq!(attr_after(r#"<rootfile full-path="OEBPS/content.opf" />"#, "full-path="), Some("OEBPS/content.opf".into()));
    }
}
