---
name: manga
description: Transformer des planches de manga en vidéo en local avec la CLI `manga` (manga-anim) — vidéo de lecture (OCR des bulles, voix de synthèse, sous-titres, mouvements de caméra) ou animation IA des cases (Wan2.2). À utiliser quand l'utilisateur veut lire, monter, doubler ou animer un manga/BD/webtoon, « de telle page à telle page », importer un CBZ/PDF/EPUB, chercher sur MangaDex, ou corriger l'OCR d'un projet.
---

# manga-anim

CLI `manga` (dépôt : ~/Downloads/manga-anim). Tout tourne en local sur le Mac.
Tout vit dans le dossier de l'app, y compris les modèles et les caches : `data/library/<titre>/<chapitre>/NNN.png`,
`data/projects/<nom>/`, `data/models/` et `data/cache/`. N'écris rien dans `~/.cache`.
Si la CLI est installée (Homebrew), ces dossiers sont dans `~/Library/Application Support/manga-anim/` :
`manga doctor` affiche l'emplacement réel.
`manga uninstall --yes` supprime tout, mais seulement si l'utilisateur le demande explicitement.
`manga --help` et `manga <commande> --help` font foi pour les options.

## Chaîne

1. **Planches dans la bibliothèque**
   - Fichiers de l'utilisateur : `manga import <fichier.cbz|.epub|.pdf|.cbr|dossier> --title "<Titre>" [--chapter N]`.
     Un dossier de CBZ devient une série (numéro de chapitre deviné depuis les noms).
   - MangaDex : `manga source search "<titre>" [--lang fr]` → `manga source chapters <id> --lang fr`
     → `manga source fetch <id> --chapters 101-110 --lang fr --title "<Titre>"`.
     Les titres sous licence (Vinland Saga, la plupart des grosses séries) n'ont **aucun** chapitre
     sur MangaDex. Dis-le, et propose `manga import` sur les fichiers de l'utilisateur.
     Ne cherche pas et n'utilise pas de sites de scans pirates.
   - `manga library` liste ce qui est disponible.
2. **Projet** sur une plage : `manga project new <nom> --title <titre> --from ch101:p1 --to ch120:p20 --lang <code> --direction rtl|ltr`.
   `ch101` seul = tout le chapitre. `rtl` pour le manga, `ltr` pour BD/comics/webtoons.
   Langues : fr, en, ja, es, it, pt, zh, hi ont des voix ; les autres (ko, de…) sont sous-titrées sans voix.
3. **Analyse** : `manga analyze <nom>` (Magi v2 : cases, bulles, ordre, locuteurs, onomatopées).
   `--detector vision` : plus rapide, sans locuteurs, pour les BD en couleur où Magi rate des bulles.
   Les pages déjà analysées sont sautées (`--force` écrase les corrections).
4. **Vérification** : lis `projects/<nom>/analysis/*.json` et corrige les erreurs d'OCR évidentes
   (champ `text`), mets `essential: false` sur le bruit (crédits des traducteurs, filigranes,
   onomatopées mal classées). Ou `manga read <nom>` pour la liseuse web (l'utilisateur corrige à la main).
   Pour nommer les personnages : déposer `projects/<nom>/characters/<Nom>.png` (un portrait par perso)
   puis `manga analyze <nom> --force`.
5. **Voix** : `manga voice <nom>`. Distribution dans `projects/<nom>/cast.toml`
   (`[voices] Thorfinn = "am_adam"`, `voix@0.9` = débit). Cache par réplique.
6. **Montage** : `manga timeline <nom>` → `timeline.json`. Réglages par plan, conservés à la régénération :
   `motion` (static, zoom_in, zoom_out, pan_left, pan_right, pan_up, pan_down), `skip: true`,
   `extra` (secondes en plus ou en moins), `mode` (motion ou animate), `prompt`.
   Ne modifie pas `duration` à la main : utilise `extra`.
7. **Rendu** : `manga render <nom> [--out fichier.mp4] [--speaker] [--no-captions] [--shots 3,7]`.
   `--shots` pour prévisualiser quelques plans. Le cache par plan rend les re-rendus rapides.

Raccourci : `manga run <nom>` enchaîne analyse, voix, timeline et rendu.

## Animation IA

- Nécessite `manga models install wan` (~34 Go à télécharger, ~15 Go installé). Toujours demander
  avant, et vérifier l'espace disque avec `manga doctor`. Sur 16 Go de RAM : `--bits 4`.
- `manga animate <nom> <plans…> --prompt "what moves, in English"` marque des plans, `--off` annule.
- Compter plusieurs minutes par clip de 2 s sur un Mac 16 Go. Choisis peu de plans : moments forts,
  cases larges avec personnages. Annonce le temps estimé avant de lancer.
- `manga render <nom> --no-animate` rend vite en ignorant l'animation.

## Bonnes pratiques

- Lance `manga doctor` en cas de doute sur l'installation.
- Après une correction d'analyse : `manga voice` puis `manga timeline` puis `manga render`.
- Indique toujours le chemin de la vidéo produite.
- Publication : la vidéo d'une œuvre sous copyright est pour un usage personnel. La mettre en ligne
  expose à un retrait (Content ID). Rappelle-le si l'utilisateur parle de publier.
