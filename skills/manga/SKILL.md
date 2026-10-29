---
name: manga
description: Transformer des planches de manga en vidéo en local avec la CLI `manga` (manga-anim) — vidéo de lecture (OCR des bulles, voix de synthèse, sous-titres, mouvements de caméra) ou animation IA des cases (Wan2.2) — et créer un nouveau manga (spin-off) dessiné avec les persos d'un manga existant (FLUX.2). À utiliser quand l'utilisateur veut lire, monter, doubler ou animer un manga/BD/webtoon, « de telle page à telle page », importer un CBZ/PDF/EPUB, chercher sur MangaDex, corriger l'OCR d'un projet, ou inventer une suite/une histoire avec ses personnages.
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
   Pour nommer les personnages : déposer `projects/<nom>/characters/<Nom>.png` (un portrait par perso,
   `manga characters <nom>` en propose) puis `manga analyze <nom> --force`.
5. **Voix** : `manga voice <nom>`. Distribution dans `projects/<nom>/cast.toml`
   (`[voices] Thorfinn = "am_adam"`, `voix@0.9` = débit). Cache par réplique.
6. **Montage** : `manga timeline <nom>` → `timeline.json`. Réglages par plan, conservés à la régénération :
   `motion` (static, zoom_in, zoom_out, pan_left, pan_right, pan_up, pan_down), `skip: true`,
   `extra` (secondes en plus ou en moins), `mode` (motion ou animate), `prompt`.
   Ne modifie pas `duration` à la main : utilise `extra`.
7. **Rendu** : `manga render <nom> [--out fichier.mp4] [--style scroll|panels] [--speaker] [--captions|--no-captions] [--shots 3,7]`.
   `scroll` (défaut) : la page en pleine largeur défile de case en case pendant la lecture, sans
   sous-titres (`--captions` pour les ajouter). `panels` : une case à la fois sur fond flou, avec
   `motion` et sous-titres. `--page-intro 0` sur `manga timeline` pour un défilement sans pause
   en haut de page. `--shots` pour prévisualiser quelques plans. Le cache par plan rend les re-rendus rapides.

Raccourci : `manga run <nom>` enchaîne analyse, voix, timeline et rendu.

## Animation IA

- Nécessite `manga models install wan` (~34 Go à télécharger, ~15 Go installé). Toujours demander
  avant, et vérifier l'espace disque avec `manga doctor`. Sur 16 Go de RAM : `--bits 4`.
- `manga animate <nom> <plans…> --prompt "what moves, in English"` marque des plans, `--off` annule.
- Compter plusieurs minutes par clip de 2 s sur un Mac 16 Go. Choisis peu de plans : moments forts,
  cases larges avec personnages. Annonce le temps estimé avant de lancer.
- `manga render <nom> --no-animate` rend vite en ignorant l'animation.

## Spin-off : un nouveau manga avec les persos d'un projet

Toi, tu écris et tu relis ; le seul modèle local dessine les cases (FLUX.2 [klein] 4B, Apache 2.0).
Le spin-off devient un projet normal : `read`, `voice`, `timeline`, `render`, `animate` marchent dessus.

1. **Persos** : sur un projet source analysé avec Magi, `manga characters <src>` recadre chaque perso
   détecté dans `characters/found/<libellé>/NN.png` et liste dans `characters/found/index.json`
   ses apparitions et quelques répliques. Les libellés `perso-<passe>.<cluster>` ne valent que dans une
   passe de 24 pages : regarde les crops (Read), fusionne ceux qui sont le même perso, ignore les ratés,
   et pour chaque perso retenu copie son meilleur crop (visage net, de face, grand) en
   `characters/<Nom>.png`. Cette banque nomme aussi les locuteurs si on relance `manga analyze <src> --force`.
   Une analyse ancienne ou faite avec `--detector vision` n'a pas de persos : il faut relancer Magi,
   ce qui écrase les corrections de la liseuse. Demande avant.
2. **Projet** : `manga story new <nom> --from <src> [--lang fr] [--direction rtl|ltr]` crée
   `projects/<nom>/story.json` (exemple d'une page) et copie la banque dans `refs/`.
3. **Scénario** : réécris `story.json` en entier.
   - `characters.<Nom>` : `refs` (1 à 3 images, relatives au projet ou absolues, la meilleure d'abord)
     et `look`, une description visuelle en anglais (coiffure, vêtements, signes distinctifs).
   - `pages[].layout` : rangées de haut en bas, chaque rangée liste la largeur relative de ses cases
     (`[[1], [2, 1], [1, 1, 1]]`) ; `heights` (optionnel) : hauteurs relatives des rangées.
     3 à 6 cases par page, une grande case pour les moments forts.
   - `panels[]`, dans l'ordre de lecture et autant que de cases : `prompt` en anglais (cadrage, action,
     décor, émotion, sans décrire les persos déjà dans `look`), `characters` (au plus 2 ou 3),
     `seed`, `bubbles` (`speaker`, `text` dans la langue du projet, `pos` facultatif : top-left,
     top-right, bottom-left, bottom-right, top, bottom), `narration` (cartouche).
   - `style` en tête de chaque prompt ; le texte n'est jamais dessiné par le modèle, toutes les
     bulles sont posées par la CLI.
   - Bulles courtes (une ou deux phrases) : elles doivent tenir dans la case.
4. **Mise en page d'abord** : `manga story build <nom> --no-draw` compose les planches avec des cases
   grises, sans le modèle. Regarde `pages/*.png` et corrige layout et bulles avant de dessiner.
5. **Dessin** : nécessite `manga models install flux2` (~15 Go téléchargés, ~5 Go gardés ; demander
   avant et vérifier le disque avec `manga doctor`). `manga story build <nom> [--pages 1,2]` ne dessine
   que les cases qui ont changé (cache par prompt, refs, taille et seed). Compte de l'ordre de la minute
   par case, plus avec plusieurs références ; le worker affiche le temps restant. Commence par une page.
6. **Relecture** : regarde chaque case dessinée (`pages/*.png`) à côté des refs de ses persos. Pour une
   case ratée (perso méconnaissable, texte parasite, anatomie), change son `seed` ou précise son `prompt`,
   puis relance `build` : seules ces cases sont redessinées.
7. Puis `manga voice <nom>`, `manga timeline <nom>`, `manga render <nom>` comme pour tout projet.
   `story.json` est la source : `build` réécrit `pages/` et `analysis/`, donc corrige les dialogues
   dans `story.json`, pas dans la liseuse.

Un spin-off d'une œuvre sous copyright est une fan-fiction : usage personnel.

## Bonnes pratiques

- Lance `manga doctor` en cas de doute sur l'installation.
- Après une correction d'analyse : `manga voice` puis `manga timeline` puis `manga render`.
- Indique toujours le chemin de la vidéo produite.
- Publication : la vidéo d'une œuvre sous copyright est pour un usage personnel. La mettre en ligne
  expose à un retrait (Content ID). Rappelle-le si l'utilisateur parle de publier.
