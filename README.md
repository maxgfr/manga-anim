# manga-anim

CLI pour transformer des planches de manga en vidéo, en local sur Mac (Apple Silicon).

- **Vidéo de lecture** : détection des cases et des bulles, OCR multilingue, voix de synthèse par
  personnage, sous-titres, mouvements de caméra case par case.
- **Animation** : les cases choisies sont animées par un modèle image→vidéo open source
  (Wan2.2 TI2V-5B via MLX).
- **Liseuse web** locale pour vérifier et corriger l'OCR, l'ordre de lecture et les locuteurs.
- **Skill Claude Code** (`skills/manga`) pour piloter toute la chaîne en langage naturel.

## Installation

```sh
brew install ffmpeg uv poppler        # poppler pour les PDF ; `brew install unar` pour les CBR
cargo install --path .                # binaire `manga`
(cd workers && uv sync --extra ja)    # workers ML (+ manga-ocr pour le japonais)
ln -s "$PWD/skills/manga" ~/.claude/skills/manga
manga doctor
```

Les modèles d'analyse (Magi v2, ~4 Go) et de voix (Kokoro, ~350 Mo) se téléchargent au premier usage.
L'animation est optionnelle : `manga models install wan --bits 4` (~34 Go téléchargés, convertis puis supprimés).

## Exemple

```sh
manga import ~/Mangas/Vinland\ Saga/ --title "Vinland Saga"        # dossier de CBZ par chapitre
manga project new vs-s3 --title "Vinland Saga" --from ch101:p1 --to ch105 --lang fr
manga analyze vs-s3          # cases, bulles, OCR, locuteurs
manga read vs-s3             # liseuse : vérifier, corriger
manga voice vs-s3            # voix (cast.toml : une voix par personnage)
manga timeline vs-s3         # timeline.json = le montage, éditable
manga render vs-s3           # → ~/.manga-anim/projects/vs-s3/out/vs-s3.mp4
```

Pour animer quelques cases : `manga animate vs-s3 12 27 --prompt "snow falling, cape flapping"`
puis `manga render vs-s3`.

## Sources

- **Fichiers locaux** : CBZ, ZIP, EPUB (ordre du spine), PDF, CBR/7z, dossiers d'images.
- **MangaDex** (API publique) : `manga source search|chapters|fetch`. Les titres sous licence
  officielle n'y ont pas de chapitres ; la CLI le signale.

Le projet ne contient aucun scraper de sites de scans non autorisés. Les vidéos tirées d'œuvres
sous copyright sont pour un usage personnel.

## Architecture

| Partie | Rôle |
|---|---|
| `src/` (Rust) | CLI, bibliothèque, sources, projet, timeline, rendu ffmpeg, sous-titres PNG, liseuse axum |
| `workers/mangaw` (Python, uv) | `analyze` (Magi v2 / Vision + OpenCV), `ocr` (Apple Vision, manga-ocr), `tts` (Kokoro MLX), `animate` (mlx-video Wan2.2), `models` |

Rust et Python échangent des fichiers JSON (`projects/<nom>/work/*_job.json` → `analysis/*.json`,
`audio/`, `clips/`). Chaque étape est mise en cache et reprenable.

## Tests

```sh
cargo test                       # tests unitaires
cargo test -- --ignored          # E2E : CBZ synthétique → vidéo (macOS, workers installés)
```

## Modèles et licences

- [Magi v2](https://huggingface.co/ragavsachdeva/magiv2) : licence recherche, usage personnel uniquement.
- [Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M) : Apache 2.0.
- [Wan2.2](https://huggingface.co/Wan-AI/Wan2.2-TI2V-5B) : Apache 2.0, via [mlx-video](https://github.com/Blaizzy/mlx-video) (MIT).
