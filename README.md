# manga-anim

CLI pour transformer des planches de manga en vidéo, en local sur Mac (Apple Silicon).

- **Vidéo de lecture** : détection des cases et des bulles, OCR multilingue, voix de synthèse par
  personnage. Par défaut la page en pleine largeur défile de case en case au fil de la lecture
  (`--style panels` : une case à la fois, zoom ou panoramique, sous-titres).
- **Animation** : les cases choisies sont animées par un modèle image→vidéo open source
  (Wan2.2 TI2V-5B via MLX).
- **Spin-off** : un nouveau manga avec les persos d'un projet. Claude (skill) écrit le scénario,
  FLUX.2 [klein] 4B dessine les cases avec les persos en référence, la CLI compose les planches et
  les bulles. Le résultat se lit, se double et se rend comme les autres projets.
- **Liseuse web** locale pour vérifier et corriger l'OCR, l'ordre de lecture et les locuteurs.
- **Skill Claude Code** (`skills/manga`) pour piloter toute la chaîne en langage naturel.

## Installation

```sh
brew install ffmpeg uv poppler        # poppler pour les PDF ; `brew install unar` pour les CBR
cargo build --release
ln -s "$PWD/target/release/manga" ~/.cargo/bin/manga      # lien, pas de copie
(cd workers && uv sync --extra ja)    # workers ML (+ manga-ocr pour le japonais)
ln -s "$PWD/skills/manga" ~/.claude/skills/manga
manga doctor
```

Les modèles d'analyse (Magi v2, ~4 Go) et de voix (Kokoro, ~350 Mo) se téléchargent au premier usage.

### Tout est dans ce dossier

Code, venv Python (`workers/.venv`), planches, projets, vidéos, modèles et caches (`data/`) :
rien n'est écrit dans `~/.cache` ni ailleurs. Les workers reçoivent `HF_HOME`, `TORCH_HOME`,
`UV_CACHE_DIR`, `XDG_CACHE_HOME` et `MPLCONFIGDIR` pointés vers `data/cache/`.
Les deux seuls éléments hors du dossier sont des liens symboliques (binaire et skill).

`manga uninstall --yes` supprime le dossier et ces deux liens. Sans `--yes`, la commande affiche
seulement ce qui serait supprimé. `MANGA_HOME` permet de mettre `data/` ailleurs.

### Installation hors du dépôt (Homebrew, préfixe)

Le binaire détecte s'il est installé : `<préfixe>/bin/manga` à côté de
`<préfixe>/share/manga-anim/{workers,skills}` (la disposition d'une formule Homebrew). Il prend
alors ses workers dans le préfixe et met tout le reste (venv, planches, projets, modèles, caches)
dans `~/Library/Application Support/manga-anim`. `manga uninstall --yes` vide ce dossier ; le
binaire se retire avec le gestionnaire de paquets.

```sh
P=/opt/manga-anim   # ou le préfixe d'une formule
cargo install --path . --root "$P"
mkdir -p "$P/share/manga-anim" && cp -R workers skills "$P/share/manga-anim/"
manga doctor        # affiche la commande uv sync à lancer pour créer le venv
```

L'animation est optionnelle : `manga models install wan --bits 4` (~34 Go téléchargés, convertis dans `data/models/`, puis la source est supprimée).

## Exemple

```sh
manga import ~/Mangas/Vinland\ Saga/ --title "Vinland Saga"        # dossier de CBZ par chapitre
manga project new vs-s3 --title "Vinland Saga" --from ch101:p1 --to ch105 --lang fr
manga analyze vs-s3          # cases, bulles, OCR, locuteurs
manga read vs-s3             # liseuse : vérifier, corriger
manga voice vs-s3            # voix (cast.toml : une voix par personnage)
manga timeline vs-s3         # timeline.json = le montage, éditable
manga render vs-s3           # → data/projects/vs-s3/out/vs-s3.mp4
```

Pour animer quelques cases : `manga animate vs-s3 12 27 --prompt "snow falling, cape flapping"`
puis `manga render vs-s3`.

### Spin-off

```sh
manga characters vs-s3                 # persos détectés par Magi → characters/found/
cp …/characters/found/perso-0-2/01.png …/characters/Thorfinn.png   # nommer (la skill le fait)
manga story new vs-spin --from vs-s3   # story.json d'exemple, persos copiés dans refs/
# écrire story.json : pages, cases (prompt, persos, seed), bulles, narration
manga story build vs-spin --no-draw    # mise en page seule, cases grises
manga models install flux2             # une fois : ~15 Go téléchargés, ~5 Go gardés (4 bits)
manga story build vs-spin              # dessine les cases qui ont changé, compose les planches
manga voice vs-spin && manga timeline vs-spin && manga render vs-spin
```

Le dessin a son propre projet uv (`workers/draw`, venv `workers/draw/.venv`) : mflux demande
transformers 5, Magi transformers 4. `uv` le crée au premier `manga story build`.

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
| `workers/draw/mangadraw` (Python, uv, venv à part) | `draw` (FLUX.2 [klein] via mflux), `models` |

Rust et Python échangent des fichiers JSON (`projects/<nom>/work/*_job.json` → `analysis/*.json`,
`audio/`, `clips/`). Chaque étape est mise en cache et reprenable.

## Tests

```sh
cargo test                       # tests unitaires
cargo test -- --ignored          # E2E : CBZ synthétique → vidéo (macOS, workers installés)
```

## Modèles et licences

Le code de manga-anim est sous licence [MIT](LICENSE). Les modèles ont leur propre licence :

- [Magi v2](https://huggingface.co/ragavsachdeva/magiv2) : licence recherche, usage personnel uniquement.
- [Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M) : Apache 2.0.
- [Wan2.2](https://huggingface.co/Wan-AI/Wan2.2-TI2V-5B) : Apache 2.0, via [mlx-video](https://github.com/Blaizzy/mlx-video) (MIT).
- [FLUX.2 [klein] 4B](https://huggingface.co/black-forest-labs/FLUX.2-klein-4B) : Apache 2.0, via [mflux](https://github.com/filipstrand/mflux) (MIT).
