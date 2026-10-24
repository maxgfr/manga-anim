"""OCR local : Apple Vision pour toutes les langues, manga-ocr pour le japonais."""

from __future__ import annotations

import io
import re
import sys

from PIL import Image

VISION_LANGS = {
    "fr": "fr-FR", "en": "en-US", "ja": "ja-JP", "ko": "ko-KR", "zh": "zh-Hans",
    "zh-hant": "zh-Hant", "es": "es-ES", "de": "de-DE", "it": "it-IT", "pt": "pt-BR",
    "ru": "ru-RU", "uk": "uk-UA", "vi": "vi-VT", "th": "th-TH", "ar": "ar-SA",
}


def vision_observations(image: Image.Image, lang: str) -> list[tuple[str, tuple[float, float, float, float]]]:
    """Lignes reconnues par Vision : (texte, bbox en pixels x1, y1, x2, y2)."""
    import Vision
    from Foundation import NSData

    buf = io.BytesIO()
    image.convert("RGB").save(buf, format="PNG")
    data = NSData.dataWithBytes_length_(buf.getvalue(), len(buf.getvalue()))
    handler = Vision.VNImageRequestHandler.alloc().initWithData_options_(data, None)
    request = Vision.VNRecognizeTextRequest.alloc().init()
    request.setRecognitionLevel_(Vision.VNRequestTextRecognitionLevelAccurate)
    request.setUsesLanguageCorrection_(True)
    request.setRecognitionLanguages_([VISION_LANGS.get(lang, lang)])
    ok, err = handler.performRequests_error_([request], None)
    if not ok:
        raise RuntimeError(f"Vision a échoué : {err}")
    w, h = image.size
    out = []
    for obs in request.results() or []:
        cand = obs.topCandidates_(1)
        if not cand:
            continue
        box = obs.boundingBox()
        x, y, bw, bh = box.origin.x, box.origin.y, box.size.width, box.size.height
        # Vision : origine en bas à gauche, coordonnées normalisées.
        out.append((str(cand[0].string()), (x * w, (1 - y - bh) * h, (x + bw) * w, (1 - y) * h)))
    return out


def join_lines(lines: list[str]) -> str:
    text = ""
    for line in lines:
        line = line.strip()
        if not line:
            continue
        if text.endswith("-") and len(text) > 1 and text[-2].isalpha():
            text = text[:-1] + line
        elif text:
            text += " " + line
        else:
            text = line
    return re.sub(r"\s+", " ", text).strip()


LATIN_LANGS = {"fr", "en", "es", "de", "it", "pt", "vi"}

# Le lettrage manga en capitales perd souvent l'apostrophe à l'OCR.
EN_CONTRACTIONS = {
    "DONT": "DON'T", "IVE": "I'VE", "IM": "I'M", "ILL": "I'LL", "CANT": "CAN'T", "DIDNT": "DIDN'T",
    "ISNT": "ISN'T", "WASNT": "WASN'T", "COULDNT": "COULDN'T", "WOULDNT": "WOULDN'T",
    "SHOULDNT": "SHOULDN'T", "DOESNT": "DOESN'T", "ARENT": "AREN'T", "HAVENT": "HAVEN'T",
    "HASNT": "HASN'T", "THATS": "THAT'S", "WHATS": "WHAT'S", "YOURE": "YOU'RE", "THEYRE": "THEY'RE",
    "YOUVE": "YOU'VE", "WEVE": "WE'VE", "THEYVE": "THEY'VE", "YOULL": "YOU'LL", "WERENT": "WEREN'T",
}


def clean(text: str, lang: str) -> str:
    """Corrige les erreurs typiques de l'OCR sur du lettrage en capitales."""
    letters = [c for c in text if c.isalpha()]
    if letters and sum(c.isupper() for c in letters) / len(letters) > 0.7:
        # « YoU », « You » : dans un texte en capitales, les minuscules sont des erreurs d'OCR.
        text = text.upper()
        if lang == "en":
            text = re.sub(r"\b[A-Z]+\b", lambda m: EN_CONTRACTIONS.get(m.group(0), m.group(0)), text)
    return text


def plausible(text: str, lang: str) -> bool:
    """Faux pour un texte dont l'écriture ne correspond pas à la langue (bruit d'OCR)."""
    letters = [c for c in text if c.isalpha()]
    if not letters:
        return False
    if lang in LATIN_LANGS:
        latin = sum(1 for c in letters if c.isascii() or "\u00c0" <= c <= "\u024f")
        return latin / len(letters) >= 0.6 and len(letters) >= 2
    return True


class Ocr:
    def __init__(self, lang: str):
        self.lang = lang
        self.manga_ocr = None
        if lang == "ja":
            try:
                from manga_ocr import MangaOcr

                self.manga_ocr = MangaOcr()
            except ImportError:
                print("manga-ocr absent (uv sync --extra ja) : Vision utilisé pour le japonais", file=sys.stderr)

    def read(self, crop: Image.Image) -> str:
        if self.manga_ocr is not None:
            return self.manga_ocr(crop).strip()
        # Vision lit mal le texte minuscule : on agrandit les petites bulles.
        if min(crop.size) < 160:
            scale = 160 / min(crop.size)
            crop = crop.resize((int(crop.width * scale), int(crop.height * scale)), Image.LANCZOS)
        obs = vision_observations(crop, self.lang)
        obs.sort(key=lambda o: (o[1][1], o[1][0]))
        return clean(join_lines([t for t, _ in obs]), self.lang)
