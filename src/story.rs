//! Spin-off : un nouveau manga avec les persos d'un projet existant.
//!
//! `story.json` (écrit par Claude via la skill) décrit pages, cases, prompts et bulles.
//! `manga story build` fait dessiner les cases par FLUX.2 [klein] (worker `draw`), compose les
//! planches et écrit leur analyse : le projet se lit, se double et se rend comme un autre.

use std::collections::BTreeMap;
use std::f64::consts::SQRT_2;
use std::fs;
use std::path::{Path, PathBuf};

use ab_glyph::{FontVec, PxScale};
use anyhow::{Context, Result, bail, ensure};
use image::imageops::{self, FilterType};
use image::{DynamicImage, Rgb, RgbImage};
use imageproc::drawing::{draw_filled_ellipse_mut, draw_filled_rect_mut, draw_hollow_rect_mut, draw_polygon_mut, draw_text_mut, text_size};
use imageproc::point::Point;
use imageproc::rect::Rect;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::analysis::{BBox, PageAnalysis, Panel, TextBox};
use crate::captions::wrap;
use crate::home::{caption_font, data_dir, slugify};
use crate::project::{Direction, PageRef, Project, ProjectConfig};
use crate::{characters, worker};

pub const PAGE_W: u32 = 1500;
pub const PAGE_H: u32 = 2120;
const MARGIN: f64 = 70.0;
const GUTTER: f64 = 28.0;
const BORDER: u32 = 4;
const FONT_PX: f32 = 32.0;
/// Écart entre une bulle et le bord de sa case.
const INSET: f64 = 16.0;
/// Surface de dessin d'une case : ~1 Mpx, la taille native du modèle.
const DRAW_AREA: f64 = 1024.0 * 1024.0;
/// Références par case : chacune allonge la génération et la mémoire.
const MAX_REFS: usize = 4;
const NO_TEXT: &str = "No text, no letters, no speech bubbles, no sound effects, no panel borders";
pub const DEFAULT_STYLE: &str = "Black and white manga panel, clean ink line art, screentone shading, expressive faces, dynamic composition";

const WHITE: Rgb<u8> = Rgb([255, 255, 255]);
const BLACK: Rgb<u8> = Rgb([0, 0, 0]);
const PENDING: Rgb<u8> = Rgb([215, 215, 215]);

pub fn model_dir() -> PathBuf {
    data_dir().join("models/flux2-klein-4b-q4")
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Story {
    pub title: String,
    pub lang: String,
    pub direction: Direction,
    /// Style graphique, en tête de chaque prompt.
    #[serde(default)]
    pub style: String,
    #[serde(default)]
    pub characters: BTreeMap<String, Character>,
    pub pages: Vec<StoryPage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Character {
    /// Images de référence, relatives au projet (ou absolues).
    #[serde(default)]
    pub refs: Vec<String>,
    /// Description visuelle en anglais, ajoutée au prompt des cases où il apparaît.
    #[serde(default)]
    pub look: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StoryPage {
    /// Rangées de haut en bas ; chaque rangée liste le poids (largeur relative) de ses cases.
    pub layout: Vec<Vec<f64>>,
    /// Hauteurs relatives des rangées (vide = égales).
    #[serde(default)]
    pub heights: Vec<f64>,
    /// Dans l'ordre de lecture, autant que de cases dans `layout`.
    pub panels: Vec<StoryPanel>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StoryPanel {
    /// Ce que montre la case, en anglais.
    pub prompt: String,
    #[serde(default)]
    pub characters: Vec<String>,
    #[serde(default)]
    pub seed: u64,
    #[serde(default)]
    pub bubbles: Vec<Bubble>,
    /// Cartouche du narrateur, en haut de la case côté début de lecture.
    #[serde(default)]
    pub narration: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Bubble {
    pub speaker: Option<String>,
    pub text: String,
    /// Défaut : les coins dans l'ordre de lecture.
    #[serde(default)]
    pub pos: Option<Pos>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pos {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Top,
    Bottom,
}

impl Story {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path).with_context(|| format!("scénario absent : {} (manga story new …)", path.display()))?;
        let story: Story = serde_json::from_str(&text).with_context(|| format!("scénario invalide : {}", path.display()))?;
        story.validate()?;
        Ok(story)
    }

    fn validate(&self) -> Result<()> {
        ensure!(!self.pages.is_empty(), "le scénario n'a aucune page");
        for (i, page) in self.pages.iter().enumerate() {
            let n = i + 1;
            let cells: usize = page.layout.iter().map(Vec::len).sum();
            ensure!(cells > 0, "page {n} : layout vide");
            ensure!(cells == page.panels.len(), "page {n} : {cells} cases dans layout, {} dans panels", page.panels.len());
            ensure!(
                page.layout.iter().all(|r| !r.is_empty()) && page.layout.iter().flatten().all(|w| *w > 0.0),
                "page {n} : rangée vide ou poids nul dans layout"
            );
            ensure!(
                page.heights.is_empty() || page.heights.len() == page.layout.len(),
                "page {n} : {} hauteurs pour {} rangées",
                page.heights.len(),
                page.layout.len()
            );
            ensure!(page.heights.iter().all(|h| *h > 0.0), "page {n} : hauteur nulle dans heights");
            for (k, panel) in page.panels.iter().enumerate() {
                for c in &panel.characters {
                    ensure!(self.characters.contains_key(c), "page {n}, case {} : perso inconnu « {c} » (à déclarer dans characters)", k + 1);
                }
            }
        }
        Ok(())
    }

    /// Références d'une case : une de chaque perso à tour de rôle, au plus MAX_REFS.
    fn panel_refs(&self, panel: &StoryPanel, root: &Path) -> Vec<(String, PathBuf)> {
        let lists: Vec<(&String, &Vec<String>)> = panel.characters.iter().filter_map(|c| self.characters.get(c).map(|ch| (c, &ch.refs))).collect();
        let mut out = Vec::new();
        let mut i = 0;
        while out.len() < MAX_REFS && lists.iter().any(|(_, l)| i < l.len()) {
            for (name, refs) in &lists {
                if let Some(r) = refs.get(i).filter(|_| out.len() < MAX_REFS) {
                    out.push(((*name).clone(), root.join(r)));
                }
            }
            i += 1;
        }
        out
    }

    /// Style, contenu de la case, persos (avec leur image de référence) et interdiction du texte :
    /// toutes les bulles sont posées par la composition.
    pub fn panel_prompt(&self, panel: &StoryPanel, refs: &[(String, PathBuf)]) -> String {
        let style = if self.style.trim().is_empty() { DEFAULT_STYLE } else { self.style.trim() };
        let mut parts = vec![style.to_string(), panel.prompt.trim().to_string()];
        for name in &panel.characters {
            let mut s = name.clone();
            let images: Vec<String> = refs.iter().enumerate().filter(|(_, (n, _))| n == name).map(|(i, _)| format!("image {}", i + 1)).collect();
            if !images.is_empty() {
                s += &format!(" (as in {})", images.join(", "));
            }
            let look = self.characters[name].look.trim();
            if !look.is_empty() {
                s += &format!(": {}", look.trim_end_matches('.'));
            }
            parts.push(s);
        }
        parts.push(NO_TEXT.into());
        parts.iter().map(|p| p.trim_end_matches('.')).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(". ") + "."
    }
}

/// Identifiant d'une case, le même que celui du plan dans la timeline : `p0002_k1`.
pub fn panel_id(page: usize, panel: usize) -> String {
    format!("p{:04}_k{panel}", page + 1)
}

/// Cases de la page dans l'ordre de lecture : rangées de haut en bas, et dans chaque rangée de
/// droite à gauche (rtl) ou de gauche à droite (ltr).
pub fn layout(page: &StoryPage, direction: Direction) -> Vec<BBox> {
    let (w, h) = (PAGE_W as f64, PAGE_H as f64);
    let rows = page.layout.len();
    let heights = if page.heights.is_empty() { vec![1.0; rows] } else { page.heights.clone() };
    let total_h: f64 = heights.iter().sum();
    let usable_h = h - 2.0 * MARGIN - GUTTER * (rows - 1) as f64;
    let mut out = Vec::new();
    let mut y = MARGIN;
    for (row, rh) in page.layout.iter().zip(&heights) {
        let ph = usable_h * rh / total_h;
        let total_w: f64 = row.iter().sum();
        let usable_w = w - 2.0 * MARGIN - GUTTER * (row.len() - 1) as f64;
        let mut x = MARGIN;
        for cw in row {
            let pw = usable_w * cw / total_w;
            let (x1, x2) = match direction {
                Direction::Ltr => (x, x + pw),
                // Même découpe en miroir : la première case est à droite.
                Direction::Rtl => (w - x - pw, w - x),
            };
            out.push([x1.round(), y.round(), x2.round(), (y + ph).round()]);
            x += pw + GUTTER;
        }
        y += ph + GUTTER;
    }
    out
}

/// Taille de dessin d'une case : son ratio, ~1 Mpx, multiples de 16.
pub fn draw_size(bbox: &BBox) -> (u32, u32) {
    let (w, h) = (bbox[2] - bbox[0], bbox[3] - bbox[1]);
    let s = (DRAW_AREA / (w * h)).sqrt();
    let snap = |x: f64| ((x / 16.0).round() as u32 * 16).clamp(256, 2048);
    (snap(w * s), snap(h * s))
}

fn default_pos(i: usize, direction: Direction) -> Pos {
    use Pos::*;
    let order = match direction {
        Direction::Rtl => [TopRight, TopLeft, BottomRight, BottomLeft, Top, Bottom],
        Direction::Ltr => [TopLeft, TopRight, BottomLeft, BottomRight, Top, Bottom],
    };
    order[i % order.len()]
}

/// Ellipse d'une bulle autour d'un bloc de texte tw×th : centre et rayons. Placée dans la case
/// selon `pos`, centrée si elle n'y tient pas, et toujours dans la page.
pub fn bubble_shape(tw: f64, th: f64, panel: &BBox, pos: Pos) -> ((f64, f64), f64, f64) {
    // Une ellipse qui contient un rectangle a des rayons √2 fois ses demi-côtés.
    let rx = tw / 2.0 * SQRT_2 + 10.0;
    let ry = th / 2.0 * SQRT_2 + 10.0;
    let (left, right) = (panel[0] + INSET + rx, panel[2] - INSET - rx);
    let (top, bottom) = (panel[1] + INSET + ry, panel[3] - INSET - ry);
    let mid_x = (panel[0] + panel[2]) / 2.0;
    let mid_y = (panel[1] + panel[3]) / 2.0;
    let mut cx = match pos {
        Pos::TopLeft | Pos::BottomLeft => left,
        Pos::TopRight | Pos::BottomRight => right,
        Pos::Top | Pos::Bottom => mid_x,
    };
    let mut cy = match pos {
        Pos::TopLeft | Pos::TopRight | Pos::Top => top,
        _ => bottom,
    };
    if left > right {
        cx = mid_x;
    }
    if top > bottom {
        cy = mid_y;
    }
    let cx = cx.clamp(rx.min(PAGE_W as f64 / 2.0), (PAGE_W as f64 - rx).max(PAGE_W as f64 / 2.0));
    let cy = cy.clamp(ry.min(PAGE_H as f64 / 2.0), (PAGE_H as f64 - ry).max(PAGE_H as f64 / 2.0));
    ((cx, cy), rx, ry)
}

fn overlaps(a: &BBox, b: &BBox) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// Décale verticalement une bulle qui en chevauche une déjà posée dans la case : sous elle si
/// la bulle est en haut, au-dessus si elle est en bas. Renvoie le nouveau centre vertical.
pub fn avoid(center: (f64, f64), rx: f64, ry: f64, panel: &BBox, placed: &[BBox]) -> f64 {
    let (cx, mut cy) = center;
    let down = cy <= (panel[1] + panel[3]) / 2.0;
    for _ in 0..placed.len() {
        let me = [cx - rx, cy - ry, cx + rx, cy + ry];
        let Some(other) = placed.iter().find(|p| overlaps(&me, p)) else { break };
        cy = if down { other[3] + 8.0 + ry } else { other[1] - 8.0 - ry };
    }
    cy
}

/// Queue de bulle : un triangle qui part du centre et pointe vers le milieu de la case.
fn tail(center: (f64, f64), rx: f64, ry: f64, panel: &BBox, shrink: f64) -> Vec<Point<i32>> {
    let (cx, cy) = center;
    let (mut dx, mut dy) = ((panel[0] + panel[2]) / 2.0 - cx, (panel[1] + panel[3]) / 2.0 - cy);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        (dx, dy) = (0.0, 1.0);
    } else {
        (dx, dy) = (dx / len, dy / len);
    }
    // Distance du centre au bord de l'ellipse dans cette direction.
    let edge = 1.0 / ((dx / rx).powi(2) + (dy / ry).powi(2)).sqrt();
    let reach = edge + 0.6 * ry.min(rx).max(30.0) - shrink * 2.0;
    let tip = ((cx + dx * reach).clamp(panel[0], panel[2]), (cy + dy * reach).clamp(panel[1], panel[3]));
    let half = 0.3 * rx.min(ry).max(30.0) - shrink;
    let base = (cx + dx * edge * 0.5, cy + dy * edge * 0.5);
    let p = |x: f64, y: f64| Point::new(x.round() as i32, y.round() as i32);
    vec![p(base.0 - dy * half, base.1 + dx * half), p(tip.0, tip.1), p(base.0 + dy * half, base.1 - dx * half)]
}

struct Lettering<'a> {
    font: &'a FontVec,
    scale: PxScale,
}

impl Lettering<'_> {
    fn width(&self, s: &str) -> u32 {
        text_size(self.scale, self.font, s).0
    }

    fn line_h(&self) -> f64 {
        FONT_PX as f64 * 1.2
    }

    fn lines(&self, text: &str, max_w: f64) -> (Vec<String>, f64, f64) {
        let lines = wrap(text, max_w as u32, |s| self.width(s));
        let tw = lines.iter().map(|l| self.width(l)).max().unwrap_or(0) as f64;
        let th = self.line_h() * lines.len().max(1) as f64;
        (lines, tw, th)
    }
}

fn draw_bubble(canvas: &mut RgbImage, l: &Lettering, text: &str, panel: &BBox, pos: Pos, placed: &[BBox]) -> BBox {
    let (lines, tw, th) = l.lines(text, ((panel[2] - panel[0]) * 0.42).clamp(160.0, 460.0));
    let ((cx, cy), rx, ry) = bubble_shape(tw, th, panel, pos);
    let cy = avoid((cx, cy), rx, ry, panel, placed);
    let c = (cx.round() as i32, cy.round() as i32);
    // Contour noir, puis intérieur blanc : la queue se fond dans la bulle.
    let stroke = 3.0;
    draw_polygon_mut(canvas, &tail((cx, cy), rx, ry, panel, 0.0), BLACK);
    draw_filled_ellipse_mut(canvas, c, (rx + stroke) as i32, (ry + stroke) as i32, BLACK);
    draw_polygon_mut(canvas, &tail((cx, cy), rx, ry, panel, stroke), WHITE);
    draw_filled_ellipse_mut(canvas, c, rx as i32, ry as i32, WHITE);
    for (i, line) in lines.iter().enumerate() {
        let x = cx - l.width(line) as f64 / 2.0;
        let y = cy - th / 2.0 + i as f64 * l.line_h();
        draw_text_mut(canvas, BLACK, x as i32, y as i32, l.scale, l.font, line);
    }
    [cx - rx, cy - ry, cx + rx, cy + ry]
}

/// Cartouche collé au coin de la case où commence la lecture.
fn draw_narration(canvas: &mut RgbImage, l: &Lettering, text: &str, panel: &BBox, direction: Direction) -> BBox {
    let pad = 14.0;
    let (lines, tw, th) = l.lines(text, ((panel[2] - panel[0]) * 0.5).clamp(200.0, 600.0));
    let (w, h) = (tw + 2.0 * pad, th + 2.0 * pad);
    let x = match direction {
        Direction::Rtl => panel[2] - w,
        Direction::Ltr => panel[0],
    };
    let y = panel[1];
    draw_filled_rect_mut(canvas, Rect::at(x as i32, y as i32).of_size(w as u32, h as u32), BLACK);
    draw_filled_rect_mut(canvas, Rect::at(x as i32 + 3, y as i32 + 3).of_size(w as u32 - 6, h as u32 - 6), WHITE);
    for (i, line) in lines.iter().enumerate() {
        draw_text_mut(canvas, BLACK, (x + pad) as i32, (y + pad + i as f64 * l.line_h()) as i32, l.scale, l.font, line);
    }
    [x, y, x + w, y + h]
}

/// Compose une planche : cases (dessinées, ou grises si pas encore), cadres, cartouches, bulles.
/// Renvoie l'image et son analyse, qui est exacte puisqu'on a tout posé soi-même.
pub fn compose(page: &StoryPage, rects: &[BBox], images: &[Option<DynamicImage>], direction: Direction, file: &str, font: &FontVec) -> (RgbImage, PageAnalysis) {
    let l = Lettering { font, scale: PxScale::from(FONT_PX) };
    let stem = crate::project::stem(file);
    let mut canvas = RgbImage::from_pixel(PAGE_W, PAGE_H, WHITE);
    let mut texts = Vec::new();
    for (k, ((panel, rect), img)) in page.panels.iter().zip(rects).zip(images).enumerate() {
        let (x, y) = (rect[0] as u32, rect[1] as u32);
        let (w, h) = ((rect[2] - rect[0]) as u32, (rect[3] - rect[1]) as u32);
        match img {
            Some(img) => imageops::overlay(&mut canvas, &img.resize_to_fill(w, h, FilterType::Lanczos3).to_rgb8(), x as i64, y as i64),
            None => draw_filled_rect_mut(&mut canvas, Rect::at(x as i32, y as i32).of_size(w, h), PENDING),
        }
        for b in 0..BORDER {
            draw_hollow_rect_mut(&mut canvas, Rect::at((x - b) as i32, (y - b) as i32).of_size(w + 2 * b, h + 2 * b), BLACK);
        }
        let mut placed = Vec::new();
        let mut push = |bbox: BBox, text: &str, speaker: Option<String>, placed: &mut Vec<BBox>| {
            placed.push(bbox);
            texts.push(TextBox { id: format!("{stem}_t{}", texts.len()), bbox, panel: Some(k), text: text.into(), speaker, essential: true });
        };
        if let Some(n) = panel.narration.as_deref().filter(|n| !n.trim().is_empty()) {
            push(draw_narration(&mut canvas, &l, n, rect, direction), n, None, &mut placed);
        }
        for (j, b) in panel.bubbles.iter().enumerate() {
            let pos = b.pos.unwrap_or_else(|| default_pos(j, direction));
            let bbox = draw_bubble(&mut canvas, &l, &b.text, rect, pos, &placed);
            push(bbox, &b.text, b.speaker.clone(), &mut placed);
        }
    }
    let analysis = PageAnalysis {
        page: file.into(),
        width: PAGE_W,
        height: PAGE_H,
        panels: rects.iter().map(|r| Panel { bbox: *r }).collect(),
        texts,
        characters: Vec::new(),
        detector: "story".into(),
    };
    (canvas, analysis)
}

pub fn load_font() -> Result<FontVec> {
    let path = caption_font();
    let bytes = fs::read(&path).with_context(|| format!("police introuvable : {} (MANGA_FONT=…)", path.display()))?;
    FontVec::try_from_vec(bytes).context("police illisible")
}

/// Crée le spin-off : `story.json` d'exemple, persos de `source` copiés dans `refs/`.
/// La banque nommée (`characters/<Nom>.png`) d'abord, sinon les persos de `manga characters`.
pub fn new(name: &str, source: &Project, lang: Option<String>, direction: Option<Direction>) -> Result<PathBuf> {
    let root = Project::dir(name);
    let path = root.join("story.json");
    ensure!(root != source.root, "le spin-off doit être un nouveau projet, distinct de {}", source.cfg.name);
    ensure!(!path.exists(), "{} existe déjà : modifie-le, ou supprime-le pour repartir de zéro", path.display());
    fs::create_dir_all(root.join("refs"))?;
    let mut cast = BTreeMap::new();
    let mut add = |name: String, src: &Path| -> Result<()> {
        let file = format!("refs/{}.png", slugify(&name));
        image::open(src).with_context(|| format!("image illisible : {}", src.display()))?.save(root.join(&file))?;
        cast.insert(name, Character { refs: vec![file], look: String::new() });
        Ok(())
    };
    let bank = characters::bank(source);
    for (name, img) in &bank {
        add(name.clone(), img)?;
    }
    if bank.is_empty() {
        for f in characters::load_found(source).into_iter().take(6) {
            if let Some(crop) = f.crops.first() {
                add(f.label, &source.path(crop))?;
            }
        }
    }
    let names: Vec<String> = cast.keys().take(2).cloned().collect();
    let panel = |prompt: &str, characters: Vec<String>, bubbles: Vec<Bubble>, narration: Option<&str>| StoryPanel {
        prompt: prompt.into(),
        characters,
        seed: 1,
        bubbles,
        narration: narration.map(Into::into),
    };
    let say = |who: Option<&String>, text: &str| Bubble { speaker: who.cloned(), text: text.into(), pos: None };
    let story = Story {
        title: name.into(),
        lang: lang.unwrap_or_else(|| source.cfg.lang.clone()),
        direction: direction.unwrap_or(source.cfg.direction),
        style: DEFAULT_STYLE.into(),
        characters: cast,
        pages: vec![StoryPage {
            layout: vec![vec![1.0], vec![1.0, 1.0]],
            heights: vec![1.2, 1.0],
            panels: vec![
                panel("Wide establishing shot of a quiet village at dawn", Vec::new(), Vec::new(), Some("…")),
                panel("Close-up, determined expression", names.iter().take(1).cloned().collect(), vec![say(names.first(), "…")], None),
                panel("Medium shot, surprised", names.iter().skip(1).take(1).cloned().collect(), vec![say(names.get(1), "…")], None),
            ],
        }],
    };
    fs::write(&path, serde_json::to_string_pretty(&story)?)?;
    Ok(path)
}

pub struct BuildOptions {
    /// Ne dessine que ces pages (1 = première) ; vide = toutes. Les autres restent en l'état.
    pub pages: Vec<usize>,
    /// Faux : compose sans appeler le modèle, les cases pas encore dessinées restent grises.
    pub draw: bool,
    pub steps: u32,
}

#[derive(Serialize)]
struct DrawItem {
    id: String,
    prompt: String,
    refs: Vec<String>,
    width: u32,
    height: u32,
    seed: u64,
    steps: u32,
    out: String,
}

#[derive(Serialize)]
struct DrawJob {
    model_dir: String,
    items: Vec<DrawItem>,
}

/// Dessine les cases qui manquent (cache par contenu dans `panels/`), compose les planches,
/// écrit `pages/`, `analysis/` et `project.toml`. Renvoie le projet et le nombre de cases grises.
pub fn build(name: &str, opts: &BuildOptions) -> Result<(Project, usize)> {
    let root = Project::dir(name);
    let story = Story::load(&root.join("story.json"))?;
    let mut plans = Vec::new();
    let mut items = Vec::new();
    for (i, page) in story.pages.iter().enumerate() {
        let rects = layout(page, story.direction);
        let mut files = Vec::new();
        for (k, (panel, rect)) in page.panels.iter().zip(&rects).enumerate() {
            let id = panel_id(i, k);
            let refs = story.panel_refs(panel, &root);
            let prompt = story.panel_prompt(panel, &refs);
            let (width, height) = draw_size(rect);
            let mut h = Sha256::new();
            h.update(format!("{prompt}|{width}x{height}|{}|{}", panel.seed, opts.steps));
            for (_, r) in &refs {
                h.update(fs::read(r).with_context(|| format!("{id} : référence introuvable {}", r.display()))?);
            }
            let out = root.join(format!("panels/{}.png", &hex::encode(h.finalize())[..16]));
            if opts.draw && !out.exists() && (opts.pages.is_empty() || opts.pages.contains(&(i + 1))) {
                items.push(DrawItem {
                    id,
                    prompt,
                    refs: refs.iter().map(|(_, r)| r.to_string_lossy().into()).collect(),
                    width,
                    height,
                    seed: panel.seed,
                    steps: opts.steps,
                    out: out.to_string_lossy().into(),
                });
            }
            files.push(out);
        }
        plans.push((rects, files));
    }
    if !items.is_empty() {
        let model = model_dir();
        if !model.exists() {
            bail!(
                "modèle de dessin absent ({}). Installe-le avec `manga models install flux2` (~15 Go téléchargés, ~5 Go gardés), \
                 ou compose sans dessiner avec --no-draw",
                model.display()
            );
        }
        println!("{} cases à dessiner avec FLUX.2 [klein]", items.len());
        let job = DrawJob { model_dir: model.to_string_lossy().into(), items };
        worker::run_in(worker::Env::Draw, "draw", &job, &root.join("work/draw_job.json"))?;
    }

    // Les planches et leur analyse se déduisent entièrement du scénario : on les réécrit.
    let font = load_font()?;
    for dir in ["pages", "analysis"] {
        if root.join(dir).exists() {
            fs::remove_dir_all(root.join(dir))?;
        }
        fs::create_dir_all(root.join(dir))?;
    }
    let mut pages = Vec::new();
    let mut pending = 0;
    for (i, (page, (rects, files))) in story.pages.iter().zip(plans).enumerate() {
        let images: Vec<Option<DynamicImage>> = files.iter().map(|f| f.exists().then(|| image::open(f)).transpose()).collect::<Result<_, _>>()?;
        pending += images.iter().filter(|img| img.is_none()).count();
        let file = format!("{:04}.png", i + 1);
        let (canvas, analysis) = compose(page, &rects, &images, story.direction, &file, &font);
        canvas.save(root.join("pages").join(&file))?;
        analysis.save(&root.join("analysis").join(format!("{:04}.json", i + 1)))?;
        pages.push(PageRef { file, chapter: 1.0, page: i + 1 });
    }
    let project = Project {
        cfg: ProjectConfig {
            name: slugify(name),
            title: story.title.clone(),
            lang: story.lang.clone(),
            direction: story.direction,
            from: "ch1:p1".into(),
            to: format!("ch1:p{}", pages.len()),
            pages,
        },
        root,
    };
    project.save()?;
    if !project.path("cast.toml").exists() {
        fs::write(project.path("cast.toml"), crate::voice::default_cast(&story.lang))?;
    }
    Ok((project, pending))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(layout: Vec<Vec<f64>>) -> StoryPage {
        let n = layout.iter().map(Vec::len).sum();
        let panels = (0..n).map(|_| StoryPanel { prompt: "x".into(), characters: Vec::new(), seed: 0, bubbles: Vec::new(), narration: None }).collect();
        StoryPage { layout, heights: Vec::new(), panels }
    }

    fn story(json: &str) -> Result<Story> {
        let s: Story = serde_json::from_str(json)?;
        s.validate()?;
        Ok(s)
    }

    #[test]
    fn layout_fills_page_in_reading_order() {
        let p = page(vec![vec![1.0], vec![2.0, 1.0]]);
        let ltr = layout(&p, Direction::Ltr);
        let rtl = layout(&p, Direction::Rtl);
        assert_eq!(ltr.len(), 3);
        // Rangée pleine largeur, de marge à marge.
        assert_eq!(ltr[0], [MARGIN, MARGIN, PAGE_W as f64 - MARGIN, ltr[0][3]]);
        assert_eq!(ltr[2][3], PAGE_H as f64 - MARGIN);
        // ltr : la première case de la rangée est à gauche ; rtl : à droite, même largeur.
        assert!(ltr[1][0] < ltr[2][0] && rtl[1][0] > rtl[2][0]);
        assert_eq!(ltr[1][2] - ltr[1][0], rtl[1][2] - rtl[1][0]);
        assert!(ltr[1][2] - ltr[1][0] > ltr[2][2] - ltr[2][0]);
        // Gouttières entre rangées et entre cases.
        assert_eq!(ltr[1][1] - ltr[0][3], GUTTER);
        assert_eq!((ltr[2][0] - ltr[1][2]).round(), GUTTER);
    }

    #[test]
    fn draw_sizes() {
        for bbox in [[0.0, 0.0, 1360.0, 900.0], [0.0, 0.0, 400.0, 1500.0], [0.0, 0.0, 1360.0, 200.0]] {
            let (w, h) = draw_size(&bbox);
            assert!(w % 16 == 0 && h % 16 == 0 && (256..=2048).contains(&w) && (256..=2048).contains(&h));
            assert_eq!(w > h, bbox[2] > bbox[3]);
        }
    }

    #[test]
    fn bubbles_stay_in_panel() {
        let panel = [100.0, 100.0, 800.0, 700.0];
        let ((cx, cy), rx, ry) = bubble_shape(200.0, 80.0, &panel, Pos::TopRight);
        assert!(cx + rx <= panel[2] && cx - rx >= panel[0] && cy - ry >= panel[1]);
        assert!(cx > 450.0 && cy < 400.0);
        let ((cx, cy), _, _) = bubble_shape(200.0, 80.0, &panel, Pos::BottomLeft);
        assert!(cx < 450.0 && cy > 400.0);
        // Trop large pour la case : centrée (ici plus large que la page : au milieu), toujours en haut.
        let ((cx, cy), _, ry) = bubble_shape(1400.0, 80.0, &[0.0, 0.0, 300.0, 300.0], Pos::TopLeft);
        assert_eq!(cx, PAGE_W as f64 / 2.0);
        assert_eq!(cy - ry, INSET);
        // Trop haute : centrée verticalement dans la case.
        let ((_, cy), _, _) = bubble_shape(100.0, 400.0, &[0.0, 1000.0, 600.0, 1300.0], Pos::Top);
        assert_eq!(cy, 1150.0);
    }

    #[test]
    fn bubbles_do_not_overlap() {
        let panel = [0.0, 0.0, 400.0, 900.0];
        let first = [200.0, 16.0, 390.0, 120.0];
        // En haut : descend sous la bulle déjà posée.
        let cy = avoid((200.0, 70.0), 120.0, 54.0, &panel, &[first]);
        assert_eq!(cy - 54.0, first[3] + 8.0);
        // En bas : remonte au-dessus.
        let low = [100.0, 700.0, 300.0, 880.0];
        let cy = avoid((200.0, 800.0), 90.0, 50.0, &panel, &[low]);
        assert_eq!(cy + 50.0, low[1] - 8.0);
        // Rien à éviter : inchangé.
        assert_eq!(avoid((200.0, 400.0), 90.0, 50.0, &panel, &[first, low]), 400.0);
    }

    #[test]
    fn validation_errors() {
        let ok = r#"{"title":"t","lang":"fr","direction":"rtl","characters":{"A":{"refs":[]}},
            "pages":[{"layout":[[1],[1,1]],"panels":[{"prompt":"a"},{"prompt":"b","characters":["A"]},{"prompt":"c","bubbles":[{"speaker":"A","text":"Salut","pos":"top-left"}]}]}]}"#;
        story(ok).unwrap();
        let err = story(&ok.replace("[[1],[1,1]]", "[[1],[1]]")).unwrap_err().to_string();
        assert!(err.contains("2 cases dans layout, 3 dans panels"), "{err}");
        let err = story(&ok.replace(r#"["A"]"#, r#"["B"]"#)).unwrap_err().to_string();
        assert!(err.contains("perso inconnu « B »"), "{err}");
        assert!(story(&ok.replace("top-left", "middle")).is_err());
    }

    #[test]
    fn prompt_names_reference_images() {
        let s = story(
            r#"{"title":"t","lang":"fr","direction":"rtl","style":"Ink manga.",
            "characters":{"A":{"refs":["a1.png","a2.png"],"look":"red scarf."},"B":{"refs":["b1.png"]}},
            "pages":[{"layout":[[1]],"panels":[{"prompt":"They fight.","characters":["A","B"]}]}]}"#,
        )
        .unwrap();
        let panel = &s.pages[0].panels[0];
        let refs = s.panel_refs(panel, Path::new("/p"));
        let names: Vec<&str> = refs.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["A", "B", "A"]);
        assert_eq!(refs[1].1, Path::new("/p/b1.png"));
        let prompt = s.panel_prompt(panel, &refs);
        assert_eq!(prompt, format!("Ink manga. They fight. A (as in image 1, image 3): red scarf. B (as in image 2). {NO_TEXT}."));
    }

    #[test]
    fn compose_writes_matching_analysis() {
        let Ok(font) = load_font() else { return };
        let mut p = page(vec![vec![1.0], vec![1.0, 1.0]]);
        p.panels[0].narration = Some("Il était une fois".into());
        p.panels[1].bubbles = vec![Bubble { speaker: Some("A".into()), text: "Bonjour, comment vas-tu ?".into(), pos: None }];
        let rects = layout(&p, Direction::Rtl);
        let images = vec![Some(DynamicImage::new_rgb8(64, 64)), None, None];
        let (img, analysis) = compose(&p, &rects, &images, Direction::Rtl, "0001.png", &font);
        assert_eq!((img.width(), img.height()), (PAGE_W, PAGE_H));
        assert_eq!(analysis.panels.len(), 3);
        let ids: Vec<&str> = analysis.texts.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["0001_t0", "0001_t1"]);
        assert_eq!(analysis.texts[1].speaker.as_deref(), Some("A"));
        assert_eq!(analysis.texts[1].panel, Some(1));
        for t in &analysis.texts {
            let r = rects[t.panel.unwrap()];
            assert!(t.bbox[0] >= r[0] && t.bbox[2] <= r[2] && t.bbox[1] >= r[1] && t.bbox[3] <= r[3], "{t:?} hors de {r:?}");
        }
        // La case dessinée est noire, la case en attente grise, le fond blanc.
        let at = |b: BBox| *img.get_pixel(((b[0] + b[2]) / 2.0) as u32, (b[3] - 20.0) as u32);
        assert_eq!(at(rects[0]), BLACK);
        assert_eq!(at(rects[2]), PENDING);
        assert_eq!(*img.get_pixel(5, 5), WHITE);
        // L'analyse se relit comme celle de Magi.
        let dir = tempfile::tempdir().unwrap();
        analysis.save(&dir.path().join("0001.json")).unwrap();
        PageAnalysis::load(&dir.path().join("0001.json")).unwrap();
    }
}
