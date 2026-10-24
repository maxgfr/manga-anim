//! Chaîne complète sur des planches synthétiques : import CBZ → projet → analyse Vision
//! → timeline → rendu. Lent et dépendant de macOS (Vision) et des workers uv, donc
//! ignoré par défaut : `cargo test -- --ignored`.

use std::io::Write;
use std::path::Path;
use std::process::Command;

use ab_glyph::{FontVec, PxScale};
use image::{Rgb, RgbImage};
use imageproc::drawing::{draw_filled_rect_mut, draw_hollow_rect_mut, draw_text_mut};
use imageproc::rect::Rect;

const FONT: &str = "/System/Library/Fonts/Supplemental/Arial Unicode.ttf";

fn page(lines: &[&str], font: &FontVec) -> Vec<u8> {
    let mut img = RgbImage::from_pixel(1000, 1400, Rgb([255, 255, 255]));
    for (k, text) in lines.iter().enumerate() {
        let y = 40 + k as i32 * 660;
        // Case : cadre noir épais, un décor gris, une bulle blanche avec le texte.
        for d in 0..6 {
            draw_hollow_rect_mut(&mut img, Rect::at(40 + d, y + d).of_size(920 - 2 * d as u32, 620 - 2 * d as u32), Rgb([0, 0, 0]));
        }
        draw_filled_rect_mut(&mut img, Rect::at(120, y + 300).of_size(760, 260), Rgb([150, 150, 150]));
        draw_text_mut(&mut img, Rgb([0, 0, 0]), 140, y + 120, PxScale::from(56.0), font, text);
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}

fn manga(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_manga")).args(args).env("MANGA_HOME", home).output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "manga {args:?} a échoué :\n{text}");
    text
}

#[test]
#[ignore]
fn cbz_to_video() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let font = FontVec::try_from_vec(std::fs::read(FONT).unwrap()).unwrap();

    let cbz = dir.path().join("Demo - Ch. 7.cbz");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&cbz).unwrap());
    let pages = [["Bonjour le monde", "Une seconde case"], ["La suite arrive", "Fin du chapitre"]];
    for (i, lines) in pages.iter().enumerate() {
        zip.start_file(format!("p{}.png", i + 1), zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(&page(lines, &font)).unwrap();
    }
    zip.finish().unwrap();

    assert!(manga(&home, &["import", cbz.to_str().unwrap(), "--title", "Demo"]).contains("ch 7 : 2 pages"));
    manga(&home, &["project", "new", "demo", "--title", "demo", "--from", "ch7:p1", "--to", "ch7:p2", "--lang", "fr", "--direction", "ltr"]);
    manga(&home, &["analyze", "demo", "--detector", "vision"]);

    let analysis = std::fs::read_to_string(home.join("projects/demo/analysis/0001.json")).unwrap();
    assert!(analysis.contains("Bonjour le monde"), "OCR : {analysis}");

    manga(&home, &["timeline", "demo"]);
    let out = manga(&home, &["render", "demo", "--jobs", "2"]);
    assert!(out.contains("vidéo :"));

    let video = home.join("projects/demo/out/demo.mp4");
    let probe = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type:format=duration", "-of", "csv=p=0"])
        .arg(&video)
        .output()
        .unwrap();
    let probe = String::from_utf8_lossy(&probe.stdout);
    assert!(probe.contains("video") && probe.contains("audio"), "{probe}");
    let duration: f64 = probe.lines().last().unwrap().trim().parse().unwrap();
    assert!(duration > 6.0, "durée {duration}");
}
