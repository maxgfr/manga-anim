//! Sous-titres rendus en PNG transparents, superposés par ffmpeg (le ffmpeg Homebrew
//! n'a ni libass ni drawtext). Une seule police, Arial Unicode, couvre toutes les langues.

use std::path::Path;

use ab_glyph::{FontVec, PxScale};
use anyhow::{Context, Result};
use image::{Rgba, RgbaImage};
use imageproc::drawing::{draw_filled_rect_mut, draw_text_mut, text_size};
use imageproc::rect::Rect;

pub struct CaptionStyle {
    pub font: FontVec,
    pub size: f32,
    pub max_width: u32,
    pub show_speaker: bool,
}

impl CaptionStyle {
    pub fn load(font_path: &Path, frame_height: u32, frame_width: u32, show_speaker: bool) -> Result<Self> {
        let bytes = std::fs::read(font_path).with_context(|| format!("police introuvable : {} (MANGA_FONT=…)", font_path.display()))?;
        Ok(Self {
            font: FontVec::try_from_vec(bytes).context("police illisible")?,
            size: frame_height as f32 * 0.042,
            max_width: (frame_width as f32 * 0.8) as u32,
            show_speaker,
        })
    }
}

/// Mots pour le retour à la ligne. Le CJK n'a pas d'espaces : chaque caractère est un mot.
fn tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let mut current = String::new();
        for c in word.chars() {
            if is_cjk(c) {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                out.push(c.to_string());
            } else {
                current.push(c);
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
        out.push(" ".into());
    }
    out.pop();
    out
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3000..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

pub fn wrap(text: &str, max_width: u32, measure: impl Fn(&str) -> u32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for tok in tokens(text) {
        let candidate = format!("{line}{tok}");
        if !line.trim().is_empty() && measure(candidate.trim_end()) > max_width {
            lines.push(line.trim().to_string());
            line = if tok == " " { String::new() } else { tok };
        } else {
            line = candidate;
        }
    }
    if !line.trim().is_empty() {
        lines.push(line.trim().to_string());
    }
    lines
}

/// Rend une réplique. Renvoie l'image (fond semi-transparent ajusté au texte).
pub fn render(style: &CaptionStyle, text: &str, speaker: Option<&str>) -> RgbaImage {
    let scale = PxScale::from(style.size);
    let measure = |s: &str| text_size(scale, &style.font, s).0;
    let mut lines = wrap(text, style.max_width, measure);
    let speaker = speaker.filter(|s| style.show_speaker && !s.starts_with("perso-"));
    if lines.is_empty() {
        lines.push(String::new());
    }
    let line_h = (style.size * 1.25) as u32;
    let pad_x = (style.size * 0.7) as u32;
    let pad_y = (style.size * 0.35) as u32;
    let label_scale = PxScale::from(style.size * 0.7);
    let label_h = if speaker.is_some() { (style.size * 0.95) as u32 } else { 0 };
    let text_w = lines.iter().map(|l| measure(l)).max().unwrap_or(0);
    let label_w = speaker.map(|s| text_size(label_scale, &style.font, s).0).unwrap_or(0);
    let w = text_w.max(label_w) + 2 * pad_x;
    let h = label_h + line_h * lines.len() as u32 + 2 * pad_y;

    let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
    draw_filled_rect_mut(&mut img, Rect::at(0, 0).of_size(w, h), Rgba([12, 12, 16, 185]));
    if let Some(s) = speaker {
        draw_text_mut(&mut img, Rgba([255, 214, 102, 255]), pad_x as i32, pad_y as i32, label_scale, &style.font, s);
    }
    for (i, line) in lines.iter().enumerate() {
        let lw = measure(line);
        let x = (w - lw) / 2;
        let y = pad_y + label_h + i as u32 * line_h + (line_h - style.size as u32) / 2;
        draw_text_mut(&mut img, Rgba([255, 255, 255, 255]), x as i32, y as i32, scale, &style.font, line);
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_words_and_cjk() {
        let measure = |s: &str| s.chars().count() as u32 * 10;
        assert_eq!(wrap("un deux trois quatre", 100, measure), ["un deux", "trois", "quatre"]);
        assert_eq!(wrap("あいうえおかきくけこ", 50, measure), ["あいうえお", "かきくけこ"]);
        assert_eq!(wrap("motimmensequidepasse", 50, measure), ["motimmensequidepasse"]);
    }
}
