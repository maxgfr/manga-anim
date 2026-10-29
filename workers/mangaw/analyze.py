"""Analyse des pages : cases, bulles, ordre de lecture, locuteurs, OCR.

Job (écrit par `manga analyze`) :
{
  "lang": "fr", "direction": "rtl", "detector": "magi" | "vision",
  "character_bank": [{"name": "Thorfinn", "image": "/…/characters/Thorfinn.png"}],
  "pages": [{"image": "/…/pages/0001.png", "out": "/…/analysis/0001.json", "prefix": "0001"}]
}
"""

from __future__ import annotations

import json
import sys
import time
from pathlib import Path

import numpy as np
from PIL import Image

from .ocr import Ocr, plausible, vision_observations

CHAPTER_BATCH = 24  # pages par passe Magi : assez pour regrouper les persos, assez peu pour 16 Go


def rows_in_order(boxes: list[list[float]], rtl: bool) -> list[int]:
    """Ordre de lecture par rangées : haut → bas, puis droite → gauche (rtl) ou l'inverse."""
    idx = sorted(range(len(boxes)), key=lambda i: boxes[i][1])
    rows: list[list[int]] = []
    for i in idx:
        b = boxes[i]
        h = b[3] - b[1]
        for row in rows:
            r = boxes[row[0]]
            overlap = min(b[3], r[3]) - max(b[1], r[1])
            if overlap > 0.5 * min(h, r[3] - r[1]):
                row.append(i)
                break
        else:
            rows.append([i])
    order = []
    for row in rows:
        row.sort(key=lambda i: -boxes[i][2] if rtl else boxes[i][0])
        order.extend(row)
    return order


def containing_panel(box: list[float], panels: list[list[float]]) -> int | None:
    cx, cy = (box[0] + box[2]) / 2, (box[1] + box[3]) / 2
    best, best_area = None, None
    for i, p in enumerate(panels):
        if p[0] <= cx <= p[2] and p[1] <= cy <= p[3]:
            a = (p[2] - p[0]) * (p[3] - p[1])
            if best_area is None or a < best_area:
                best, best_area = i, a
    return best


def order_texts(texts: list[dict], panels: list[list[float]], rtl: bool) -> list[dict]:
    by_panel: dict[int | None, list[dict]] = {}
    for t in texts:
        by_panel.setdefault(t["panel"], []).append(t)
    out = []
    for key in sorted(by_panel, key=lambda k: (k is None, k if k is not None else 0)):
        group = by_panel[key]
        order = rows_in_order([t["bbox"] for t in group], rtl)
        out.extend(group[i] for i in order)
    return out


# --- Détecteur de repli : OpenCV pour les cases, Vision pour le texte -----------------


def detect_panels_cv(gray: np.ndarray) -> list[list[float]]:
    import cv2

    h, w = gray.shape
    _, ink = cv2.threshold(gray, 235, 255, cv2.THRESH_BINARY_INV)
    ink = cv2.morphologyEx(ink, cv2.MORPH_CLOSE, np.ones((7, 7), np.uint8))
    contours, _ = cv2.findContours(ink, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    panels = []
    for c in contours:
        x, y, bw, bh = cv2.boundingRect(c)
        if bw * bh > 0.03 * w * h and bw > 0.1 * w and bh > 0.08 * h:
            panels.append([float(x), float(y), float(x + bw), float(y + bh)])
    # Une case englobant toutes les autres (cadre de page) n'en est pas une.
    if len(panels) > 1:
        panels = [p for p in panels if (p[2] - p[0]) * (p[3] - p[1]) < 0.9 * w * h]
    return panels or [[0.0, 0.0, float(w), float(h)]]


def group_lines(obs: list[tuple[str, tuple]]) -> list[list[float]]:
    """Fusionne les lignes Vision proches en bulles."""
    boxes = [list(b) for _, b in obs]
    merged = True
    while merged:
        merged = False
        out: list[list[float]] = []
        for b in boxes:
            lh = b[3] - b[1]
            for m in out:
                mh = m[3] - m[1]
                pad = 0.8 * min(lh, mh) if mh else lh
                if b[0] - pad < m[2] and m[0] - pad < b[2] and b[1] - pad < m[3] and m[1] - pad < b[3]:
                    m[:] = [min(m[0], b[0]), min(m[1], b[1]), max(m[2], b[2]), max(m[3], b[3])]
                    merged = True
                    break
            else:
                out.append(b[:])
        boxes = out
    return boxes


def analyze_vision(pages: list[dict], lang: str, rtl: bool, ocr: Ocr) -> None:
    for job in pages:
        img = Image.open(job["image"]).convert("RGB")
        gray = np.array(img.convert("L"))
        panels = detect_panels_cv(gray)
        panels = [panels[i] for i in rows_in_order(panels, rtl)]
        lines = [o for o in vision_observations(img, lang) if o[0].strip()]
        texts = [{"bbox": b, "panel": containing_panel(b, panels), "speaker": None, "essential": True} for b in group_lines(lines)]
        write_page(job, img, panels, order_texts(texts, panels, rtl), ocr, "vision")


# --- Magi v2 ----------------------------------------------------------------------------


def load_magi():
    import torch
    from transformers import AutoModel

    model = AutoModel.from_pretrained("ragavsachdeva/magiv2", trust_remote_code=True).eval()
    device = "mps" if torch.backends.mps.is_available() else "cpu"
    return model.to(device)


def analyze_magi(pages: list[dict], bank: list[dict], rtl: bool, ocr: Ocr) -> None:
    import torch

    model = load_magi()
    character_bank = {
        "images": [np.array(Image.open(c["image"]).convert("L").convert("RGB")) for c in bank],
        "names": [c["name"] for c in bank],
    }
    for start in range(0, len(pages), CHAPTER_BATCH):
        batch = pages[start : start + CHAPTER_BATCH]
        images = [Image.open(j["image"]).convert("RGB") for j in batch]
        arrays = [np.array(im.convert("L").convert("RGB")) for im in images]
        t0 = time.time()
        with torch.no_grad():
            results = model.do_chapter_wide_prediction(arrays, character_bank, use_tqdm=False, do_ocr=False)
        print(f"magi : pages {start + 1}-{start + len(batch)} en {time.time() - t0:.0f}s", file=sys.stderr)
        for job, img, res in zip(batch, images, results):
            panels = [list(map(float, p)) for p in res["panels"]]
            if not rtl:
                panels = [panels[i] for i in rows_in_order(panels, rtl)]
            labels = character_labels(res, start // CHAPTER_BATCH)
            speakers = speaker_names(res, labels)
            characters = [{"bbox": list(map(float, b)), "name": n} for b, n in zip(res.get("characters") or [], labels)]
            texts = []
            for i, box in enumerate(res["texts"]):
                box = list(map(float, box))
                texts.append({
                    "bbox": box,
                    "panel": containing_panel(box, panels),
                    "speaker": speakers.get(i),
                    "essential": bool(res["is_essential_text"][i]) if i < len(res["is_essential_text"]) else True,
                })
            # Magi ordonne déjà les bulles pour le manga (rtl) ; en ltr on réordonne.
            if not rtl:
                texts = order_texts(texts, panels, rtl)
            write_page(job, img, panels, texts, ocr, "magiv2", characters)


def character_labels(res: dict, batch: int) -> list[str | None]:
    """Nom de chaque perso détecté. Sans banque, son cluster : perso-<passe>.<cluster>, car Magi
    ne regroupe les persos qu'à l'intérieur d'une passe de CHAPTER_BATCH pages."""
    names = res.get("character_names") or []
    clusters = res.get("character_cluster_labels") or []
    out = []
    for i in range(max(len(res.get("characters") or []), len(clusters), len(names))):
        name = names[i] if i < len(names) else None
        if not name or name.lower() in ("other", "unsure"):
            name = f"perso-{batch}.{clusters[i]}" if i < len(clusters) else None
        out.append(name)
    return out


def speaker_names(res: dict, labels: list[str | None]) -> dict[int, str]:
    """Bulle → nom du perso qui parle."""
    out = {}
    for text_idx, char_idx in res.get("text_character_associations") or []:
        if char_idx < len(labels) and labels[char_idx]:
            out[int(text_idx)] = labels[char_idx]
    return out


# --- Sortie --------------------------------------------------------------------------


def write_page(job: dict, img: Image.Image, panels: list, texts: list[dict], ocr: Ocr, detector: str, characters: list[dict] | None = None) -> None:
    w, h = img.size
    out_texts = []
    for i, t in enumerate(texts):
        x1, y1, x2, y2 = t["bbox"]
        pad = 4
        crop = img.crop((max(0, int(x1) - pad), max(0, int(y1) - pad), min(w, int(x2) + pad), min(h, int(y2) + pad)))
        # On lit aussi les onomatopées : la liseuse permet de les réactiver.
        text = ocr.read(crop)
        out_texts.append({
            "id": f"{job['prefix']}_t{i}",
            "bbox": [x1, y1, x2, y2],
            "panel": t["panel"],
            "text": text,
            "speaker": t["speaker"],
            # Bulle sans texte lisible : rien à dire.
            "essential": t["essential"] and plausible(text, ocr.lang),
        })
    result = {
        "page": Path(job["image"]).name,
        "width": w,
        "height": h,
        "panels": [{"bbox": p} for p in panels],
        "texts": out_texts,
        "characters": characters or [],
        "detector": detector,
    }
    Path(job["out"]).parent.mkdir(parents=True, exist_ok=True)
    Path(job["out"]).write_text(json.dumps(result, ensure_ascii=False, indent=2))
    n = sum(1 for t in out_texts if t["essential"])
    print(f"{result['page']} : {len(panels)} cases, {n} bulles", file=sys.stderr)


def main() -> None:
    job = json.loads(Path(sys.argv[1]).read_text())
    rtl = job["direction"] == "rtl"
    ocr = Ocr(job["lang"])
    if job["detector"] == "vision":
        analyze_vision(job["pages"], job["lang"], rtl, ocr)
    else:
        analyze_magi(job["pages"], job.get("character_bank", []), rtl, ocr)


if __name__ == "__main__":
    main()
