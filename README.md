# manga-anim

CLI that turns manga pages into a voiced reading video or a full anime episode, locally on a Mac
(Apple Silicon) or with your own API keys: voices, animation, drawing and proofreading can run
through [OpenRouter](#generation-backends) or [other providers](#other-providers-through-llm-connect)
instead of local models.

**[See what it does, with real examples](https://maxgfr.github.io/manga-anim/)**

- **Reading video**: panel and speech bubble detection, multilingual OCR, a synthetic voice per
  character. By default the full-width page scrolls from panel to panel as the text is read
  (`--style panels`: one panel at a time, with zoom or pan, and captions).
- **Animation**: selected panels are animated by an open source image→video model
  (Wan2.2 TI2V-5B via MLX).
- **Anime**: `manga anime` repaints each panel as a clean 16:9 key image, animates it for as long
  as its shot lasts, and renders a full-frame episode with the voices and subtitles.
- **Spin-off**: a new manga with the characters of a project. Claude (skill) writes the script,
  FLUX.2 [klein] 4B draws the panels using the characters as references, and the CLI lays out the
  pages and bubbles. The result can be read, dubbed and rendered like any other project.
- **Three ways to analyze**: local (Magi v2 or Apple Vision), proofread by an API (Claude,
  OpenRouter, OpenAI, Google), or corrected by Claude Code without an API key.
- **Local web reader** to check and fix the OCR, reading order and speakers.
- **Any model with your own keys**: `manga models search` lists current models and prices, through
  [llm-models](https://github.com/maxgfr/llm-models).
- **Claude Code skill** (`skills/manga`) to drive the whole pipeline in natural language, including
  model choice and budget.

## What's new

- `manga anime`: from an analyzed and voiced project to a full-frame episode, in color or black and
  white, with `--dry-run`, `--max-seconds` and `--keyframes-only` to control the spending.
- A vision model can write each shot's motion prompt (`--direct-provider`, `--direct-model`).
- The skill installs with `npx skills add maxgfr/manga-anim --skill manga -a claude-code`.
- A clip is no longer billed one second too long (a 3 s clip was requested as 4 s).
- Mouths move on talking shots, in the same generic prompt for every manga, and the model no longer voices the line under the dub.
- Sound effects: `manga anime` keeps the sound the video model generates with the picture, guided by `--sound`, under the voices, and ends by listing the shots that have none.
- A style bible: `anime.toml` pins one art style, a reference image and one remembered face per character, across runs and episodes (`--style-from`).
- Vertical output for TikTok, Shorts and Reels, with a maximum length and numbered parts: `--format vertical --max-duration 60 --split`, on `anime` and `render`.

## Commands

| Command | What it does |
|---|---|
| `manga doctor` | Checks the tools, models and API keys. |
| `manga source search\|chapters\|fetch` | Finds and downloads chapters from MangaDex. |
| `manga import` | Imports CBZ, EPUB, PDF, CBR or a folder of images or chapters. |
| `manga library` | Lists the local library. |
| `manga project new\|list\|show` | Creates and inspects projects. |
| `manga analyze` | Detects panels and bubbles, reads the text, attributes speakers (`--detector magi\|vision\|api`). |
| `manga read` | Opens the web reader to check and correct the analysis. |
| `manga voice` | Generates one voice per character from `cast.toml`. |
| `manga timeline` | Builds `timeline.json`, the editable cut. |
| `manga animate` | Marks shots for AI animation, with a motion prompt, sound effects and the characters in view. |
| `manga render` | Renders the reading video (`--style scroll\|panels`, `--format vertical`, `--max-duration`, `--split`). |
| `manga anime` | Renders an anime episode: clean key images, clips with sound effects and a full-frame edit, landscape or vertical. |
| `manga run` | Runs analyze, voice, timeline and render in sequence. |
| `manga characters` | Crops the characters found by Magi, to name and reuse them. |
| `manga story new\|build` | Writes and draws a spin-off with a project's characters. |
| `manga models search\|install` | Searches models and prices, or installs the local ones. |
| `manga uninstall` | Removes the app folder and its links. |

## Installation

```sh
brew install ffmpeg uv poppler        # poppler for PDFs; `brew install unar` for CBR
cargo build --release
ln -s "$PWD/target/release/manga" ~/.cargo/bin/manga      # a link, not a copy
(cd workers && uv sync --extra ja)    # ML workers (+ manga-ocr for Japanese)
ln -s "$PWD/skills/manga" ~/.claude/skills/manga
manga doctor
```

To install only the Claude Code skill, with the [skills CLI](https://github.com/vercel-labs/skills):

```sh
npx skills add maxgfr/manga-anim --skill manga -a claude-code
```

The skill does not contain the `manga` binary: if it is missing, the skill offers to install it.
The `ln -s` above is for working on the skill from a clone.

The analysis models (Magi v2, ~4 GB) and voice models (Kokoro, ~350 MB) are downloaded on first use.

### Everything lives in this folder

Code, Python venv (`workers/.venv`), pages, projects, videos, models and caches (`data/`):
nothing is written to `~/.cache` or anywhere else. Workers get `HF_HOME`, `TORCH_HOME`,
`UV_CACHE_DIR`, `XDG_CACHE_HOME` and `MPLCONFIGDIR` pointed at `data/cache/`.
The only two items outside the folder are symlinks (binary and skill).

`manga uninstall --yes` deletes the folder and those two links. Without `--yes`, the command only
shows what would be deleted. `MANGA_HOME` lets you put `data/` somewhere else.

### Installing outside the repo (Homebrew, prefix)

The binary detects whether it is installed: `<prefix>/bin/manga` next to
`<prefix>/share/manga-anim/{workers,skills}` (the layout of a Homebrew formula). It then
takes its workers from the prefix and puts everything else (venv, pages, projects, models, caches)
in `~/Library/Application Support/manga-anim`. `manga uninstall --yes` empties that folder; the
binary is removed with the package manager.

```sh
P=/opt/manga-anim   # or a formula's prefix
cargo install --path . --root "$P"
mkdir -p "$P/share/manga-anim" && cp -R workers skills "$P/share/manga-anim/"
manga doctor        # prints the uv sync command to run to create the venv
```

Animation is optional and needs the `animate` extra first, otherwise the install fails with
`No module named 'mlx_video'`:

```sh
(cd workers && uv sync --extra ja --extra animate)   # keep --extra ja: syncing without it removes it
manga models install wan --bits 4                    # ~34 GB downloaded, converted in data/models/, then the source is deleted
```

## Example

```sh
manga import ~/Mangas/Vinland\ Saga/ --title "Vinland Saga"        # folder with one CBZ per chapter
manga project new vs-s3 --title "Vinland Saga" --from ch101:p1 --to ch105 --lang fr
manga analyze vs-s3          # panels, bubbles, OCR, speakers
manga read vs-s3             # reader: check and fix
manga voice vs-s3            # voices (cast.toml: one voice per character)
manga timeline vs-s3         # timeline.json = the edit, can be changed by hand
manga render vs-s3           # → data/projects/vs-s3/out/vs-s3.mp4
```

To animate a few panels: `manga animate vs-s3 12 27 --prompt "snow falling, cape flapping"`
then `manga render vs-s3`.

For TikTok, Shorts or Reels: `manga render vs-s3 --format vertical --max-duration 60 --split` writes
numbered parts of at most 60 s, ending each on a whole shot.

### Anime

Once a project has its analysis, voices and timeline:

```sh
manga models search image                       # a model that accepts reference images
manga models search video                       # a model that starts from a first frame
manga anime vs-s3 --backend openrouter --image-model <id> --video-model <id> --dry-run
manga anime vs-s3 --backend openrouter --image-model <id> --keyframes-only     # check keyframes/ first
manga anime vs-s3 --backend openrouter --image-model <id> --video-model <id> --max-seconds 60
# → data/projects/vs-s3/out/vs-s3-anime.mp4
```

| Option | Effect |
|---|---|
| `--style color\|bw` | Full-color anime stills, or clean black and white. Remembered. |
| `--art "..."` | Replaces the art style description used in every prompt. Remembered. |
| `--look "..."` | Appearance notes added to every key image. Remembered. |
| `--style-from <project>` | Reuses another project's style, reference image and faces, for the next episode. |
| `--no-sfx` | Silent clips. |
| `--format vertical` | Generates images and clips in 9:16, for TikTok, Shorts and Reels. |
| `--max-duration 60` | Keeps at most that many seconds, ending on a whole shot. Only what is kept is generated. |
| `--split` | With `--max-duration`: every shot, as numbered parts of at most that length. |
| `--direct-provider`, `--direct-model` | A vision model writes the missing motion prompts, sound effects and characters (`openrouter`, `openai`, `google`). What was written by hand is kept. |
| `--shots 3,4` | Only these shots. |
| `--key-shots-only` | Only the shots marked with `manga animate` move; the others keep their key image and a camera move. |
| `--keyframes-only` | Stop after the images, to check them before paying for clips. |
| `--max-seconds` | Refuse to start above this many seconds of video. |
| `--dry-run` | Show what would be generated, without calling a model. |

Key images and clips are cached, so a rerun only pays for what is missing. Key images reuse the
drawing backends and clips the animation backends, so `--backend local` and `connect` work too.

**Sound effects** come from the video model, generated with the picture, so they land on the action.
The analysis filters out onomatopoeia, so give each shot its sounds:
`manga animate vs-s3 12 --sound "knuckles knocking on a door" --characters Thorfinn`. A run ends by
listing the shots without sound effects (a still image, or a clip the model returned silent) and the
command to fix each one. A local model makes no sound, and neither does a model whose
`generate_audio` is false: the run refuses it before billing, unless you pass `--no-sfx`.

**The look stays the same.** `anime.toml` is the project's style bible. It pins the art style to one
era in every image and clip prompt, and the first shot drawn becomes the reference image for all the
others. Each character with a `characters/<Name>.png` portrait gets one remembered face, drawn once in
`anime/<style>/faces/`. The panel still decides the composition and expressions. Delete
`anime/<style>/reference.png` to choose another reference.

Mouths follow timing cues in the prompt, which is not exact lip sync, and there is no music yet.
In a motion prompt, say whose lips move by how they look and where they are, and avoid "talks" or "speaks": a video model knows no names and would voice the line under the dub.

### Spin-off

```sh
manga characters vs-s3                 # characters detected by Magi → characters/found/
cp …/characters/found/character-0-2/01.png …/characters/Thorfinn.png   # name them (the skill does this)
manga story new vs-spin --from vs-s3   # sample story.json, characters copied to refs/
# write story.json: pages, panels (prompt, characters, seed), bubbles, narration
manga story build vs-spin --no-draw    # layout only, gray panels
manga models install flux2             # once: ~15 GB downloaded, ~5 GB kept (4-bit)
manga story build vs-spin              # draws the panels that changed, lays out the pages
manga voice vs-spin && manga timeline vs-spin && manga render vs-spin
```

Drawing has its own uv project (`workers/draw`, venv `workers/draw/.venv`): mflux requires
transformers 5, Magi transformers 4. `uv` creates it on the first `manga story build`.

## Three analysis modes

| Mode | Command | What happens |
|---|---|---|
| **local** (default) | `manga analyze vs-s3` or `--detector vision` | Magi v2 or Apple Vision: nothing leaves the Mac |
| **api** | `manga analyze vs-s3 --detector api [--provider openrouter --model <id>]` | Vision segments the page, an API model (Anthropic by default) reviews each page: text, order, speakers, noise |
| **claude-code** | the `manga` skill, in natural language | Vision segments the page, Claude Code reviews the pages and fixes `analysis/*.json`, no API key needed |

The `api` and `claude-code` modes send the pages to Anthropic. The `api` mode requires
`ant auth login` or `ANTHROPIC_API_KEY` and is billed by usage; it does not need the Magi model.
Voices, drawing, animation, editing and rendering stay local in all three modes, unless you
choose a backend below.

`--detector api` can also go through another provider, with any model that reads images:

```sh
manga models search vision flash --provider openrouter      # free: ids and prices
export OPENROUTER_API_KEY=...                               # or OPENAI_API_KEY / GEMINI_API_KEY
manga analyze vs-s3 --detector api --provider openrouter --model <id>
```

`--provider` is `anthropic` (the default), `openrouter`, `openai` or `google`, and `--model` is
required for the last three. **The pages and their OCR text are sent to that provider (and, for
OpenRouter, to the company serving the model), and each page is billed.** The review contract is the
same: a forgotten bubble keeps its OCR, an invented id is ignored, and a refused or truncated page
keeps its OCR. A model that cannot do structured output is asked for plain JSON; if the reply still
cannot be read the run stops with a one-line error and the finished pages are kept.
`--force` is needed to redo pages already reviewed by another provider.

## Generation backends

Voices, animation, drawing and anime run on local models by default. With an `OPENROUTER_API_KEY`, each
of them can run on [OpenRouter](https://openrouter.ai) instead, with no large model to download.

> **OpenRouter is billed per use and sends your data to third parties**: panels for animation,
> prompts and character reference images for drawing, the dialogue text for voices, to OpenRouter
> and the provider serving the model. Do not use it for pages you cannot share, and check the
> model's price first. Nothing is sent unless you pass `--backend openrouter`.

| Step | Command | Model ids |
|---|---|---|
| Animation | `manga render <name> --backend openrouter --model <id>` | [video models](https://openrouter.ai/models?output_modalities=video), which must accept a first frame |
| Voices | `manga voice <name> --backend openrouter --model <id>` | [speech models](https://openrouter.ai/models?output_modalities=speech), which must output MP3 |
| Drawing | `manga story build <name> --backend openrouter --model <id>` | [image models](https://openrouter.ai/models?output_modalities=image), with `input_references` if panels use character refs |
| Anime | `manga anime <name> --backend openrouter --image-model <id> --video-model <id>` | an image model with `input_references` and a video model that accepts a first frame |

```sh
export OPENROUTER_API_KEY=...         # read from the environment only, never written to job files
```

### Other providers through llm-connect

`--backend connect --provider <p> --model <id>` reaches a provider directly through
[llm-connect](https://github.com/maxgfr/llm-models/tree/main/packages/llm-connect), in a small Node
worker. Node 22.12 or later is then required (optional otherwise; `manga doctor` reports it), and
the locked packages are installed with `npm ci` on first use.

| Step | Providers | Key variable |
|---|---|---|
| Animation (`manga render`) | fal, google, xai, replicate | see below |
| Voices (`manga voice`) | fal, openai, google, elevenlabs | see below |
| Drawing (`manga story build`) | fal, openai, google, xai, replicate | see below |

Keys come only from the environment, under the names llm-connect documents: `FAL_KEY`,
`OPENAI_API_KEY`, `GEMINI_API_KEY`, `XAI_API_KEY`, `REPLICATE_API_TOKEN`, `ELEVENLABS_API_KEY`.

> **`--backend connect` is billed per use by the provider you pick, and sends it your data**: panels
> for animation, prompts and character reference images for drawing, the dialogue text for voices.
> Nothing is sent unless you pass `--backend connect`.

Pick a model with the free catalog search (it shells out to the `llm-models` CLI, or `npx llm-models`):

```sh
manga models search video wan --provider fal
manga models search speech --provider openai
manga models search image --provider google
manga models search vision flash
```

Prices are shown when the catalog publishes them (`?` otherwise). `--model latest` does not exist:
`llm-models latest` only ranks text tiers per endpoint, so it cannot pick an image-to-video, image or
speech model, and the choice of model is also the choice of what you pay.

What each step does with `--backend connect` matches the OpenRouter path (cropped clips without
audio, 24 kHz WAV, PNG panels, cache keys including backend, provider and model), with these limits:

- **Capabilities** come from the free `llm-models` catalog: duration, ratio, resolution and seed are
  only sent when the model declares them, and a model listed as text-to-video only, or without image
  editing when panels use character refs, is refused before anything is billed. A model the catalog
  does not list, or does not describe, runs with its provider's defaults and a warning, and its
  references cannot be checked.
- **Voices** are sent as is from `cast.toml`, which must hold ids of that model. Speed is applied
  locally. Audio must arrive in a container (MP3, WAV...), which is what these providers return.
- **Drawing**: fal needs a model that takes several images, such as `fal-ai/flux-2/edit`, when a
  panel has more than one reference; Replicate takes one reference, or eight with a
  `black-forest-labs/flux-2-*` model.

`--model` is required: there is no default, because the model decides what you pay. The clips,
WAV files and panels produced are the same as with the local models, and the cache key includes
the backend and the model, so switching never reuses the wrong result (and local caches stay valid).
A run stopped by a missing key, a rejected key (401), a lack of credits (402) or a failed job
prints a one-line error; what was finished stays cached, so rerunning does not pay for it again.

- **Voices**: `cast.toml` still maps characters to voices, and each value is sent as is in the
  `voice` field of the request, so use ids of the chosen model (such as `alloy` or `Kore`) rather
  than Kokoro names, which are refused before any call. `voice@0.9` still means speed 0.9, applied
  locally with ffmpeg, so a voice id cannot contain `@`. Audio is requested as MP3 and converted to
  the 24 kHz WAV the pipeline uses; models that only output PCM are not supported, as the API does
  not state the sample rate of its raw PCM.
- **Animation**: duration, aspect ratio and resolution are chosen from what the model declares, then
  the clip is cropped to the panel's shape and its audio dropped. `--steps` has no effect.
- **Drawing**: characters are sent as reference images; a model that does not accept them, or
  not that many, stops the build before anything is billed. `--steps` has no effect, and `seed` is
  only sent when the model supports it.

## Sources

- **Local files**: CBZ, ZIP, EPUB (spine order), PDF, CBR/7z, image folders.
- **MangaDex** (public API): `manga source search|chapters|fetch`. Officially licensed titles
  have no chapters there; the CLI tells you so.

The project contains no scraper for unauthorized scan sites. Videos made from copyrighted works
are for personal use.

## Architecture

| Part | Role |
|---|---|
| `src/` (Rust) | CLI, library, sources, project, timeline, anime, ffmpeg rendering, PNG captions, axum reader |
| `workers/mangaw` (Python, uv) | `analyze` (Magi v2 / Vision + OpenCV), `review` (API review), `direct` (anime motion prompts), `ocr` (Apple Vision, manga-ocr), `tts` (Kokoro MLX, OpenRouter or llm-connect), `animate` (mlx-video Wan2.2, OpenRouter or llm-connect), `draw_openrouter`, `draw_connect`, `openrouter` (HTTP client), `connect` (drives the Node worker), `models` |
| `workers/connect` (Node, optional) | `run.mjs`: one image, speech line or clip through llm-connect |
| `workers/draw/mangadraw` (Python, uv, separate venv) | `draw` (FLUX.2 [klein] via mflux), `models` |

Rust and Python exchange JSON files (`projects/<name>/work/*_job.json` → `analysis/*.json`,
`audio/`, `clips/`). Every step is cached and can be resumed.

## Tests

```sh
cargo test                       # unit tests
cargo test -- --ignored          # E2E: synthetic CBZ → video (macOS, workers installed)
workers/.venv/bin/python -m unittest discover -s workers/tests   # API paths against a mock server
(cd workers/connect && npm ci && npm test)                       # the llm-connect worker against a mock server
```

`MANGA_OPENROUTER_BASE_URL` and `MANGA_CONNECT_BASE_URL` point the workers at those mocks; they exist
for tests only and accept loopback addresses only. No test calls a real API or uses a real key.

## Models and licenses

manga-anim's code is under the [MIT](LICENSE) license. The models have their own licenses:

- [Magi v2](https://huggingface.co/ragavsachdeva/magiv2): research license, personal use only.
- [Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M): Apache 2.0.
- [Wan2.2](https://huggingface.co/Wan-AI/Wan2.2-TI2V-5B): Apache 2.0, via [mlx-video](https://github.com/Blaizzy/mlx-video) (MIT).
- [FLUX.2 [klein] 4B](https://huggingface.co/black-forest-labs/FLUX.2-klein-4B): Apache 2.0, via [mflux](https://github.com/filipstrand/mflux) (MIT).
