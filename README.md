# manga-anim

CLI that turns manga pages into video, locally on a Mac (Apple Silicon). Voices, animation and
drawing can optionally run through [OpenRouter](#generation-backends) instead of local models.

- **Reading video**: panel and speech bubble detection, multilingual OCR, a synthetic voice per
  character. By default the full-width page scrolls from panel to panel as the text is read
  (`--style panels`: one panel at a time, with zoom or pan, and captions).
- **Animation**: selected panels are animated by an open source image→video model
  (Wan2.2 TI2V-5B via MLX).
- **Spin-off**: a new manga with the characters of a project. Claude (skill) writes the script,
  FLUX.2 [klein] 4B draws the panels using the characters as references, and the CLI lays out the
  pages and bubbles. The result can be read, dubbed and rendered like any other project.
- **Local web reader** to check and fix the OCR, reading order and speakers.
- **Claude Code skill** (`skills/manga`) to drive the whole pipeline in natural language.

## Installation

```sh
brew install ffmpeg uv poppler        # poppler for PDFs; `brew install unar` for CBR
cargo build --release
ln -s "$PWD/target/release/manga" ~/.cargo/bin/manga      # a link, not a copy
(cd workers && uv sync --extra ja)    # ML workers (+ manga-ocr for Japanese)
ln -s "$PWD/skills/manga" ~/.claude/skills/manga
manga doctor
```

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
| **api** | `manga analyze vs-s3 --detector api` | Vision segments the page, the Claude API reviews each page: text, order, speakers, noise |
| **claude-code** | the `manga` skill, in natural language | Vision segments the page, Claude Code reviews the pages and fixes `analysis/*.json`, no API key needed |

The `api` and `claude-code` modes send the pages to Anthropic. The `api` mode requires
`ant auth login` or `ANTHROPIC_API_KEY` and is billed by usage; it does not need the Magi model.
Voices, drawing, animation, editing and rendering stay local in all three modes, unless you
choose the OpenRouter backend below.

## Generation backends

Voices, animation and drawing run on local models by default. With an `OPENROUTER_API_KEY`, each
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

```sh
export OPENROUTER_API_KEY=...         # read from the environment only, never written to job files
```

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
| `src/` (Rust) | CLI, library, sources, project, timeline, ffmpeg rendering, PNG captions, axum reader |
| `workers/mangaw` (Python, uv) | `analyze` (Magi v2 / Vision + OpenCV), `ocr` (Apple Vision, manga-ocr), `tts` (Kokoro MLX or OpenRouter), `animate` (mlx-video Wan2.2 or OpenRouter), `draw_openrouter`, `openrouter` (HTTP client), `models` |
| `workers/draw/mangadraw` (Python, uv, separate venv) | `draw` (FLUX.2 [klein] via mflux), `models` |

Rust and Python exchange JSON files (`projects/<name>/work/*_job.json` → `analysis/*.json`,
`audio/`, `clips/`). Every step is cached and can be resumed.

## Tests

```sh
cargo test                       # unit tests
cargo test -- --ignored          # E2E: synthetic CBZ → video (macOS, workers installed)
workers/.venv/bin/python -m unittest discover -s workers/tests   # OpenRouter paths against a mock server
```

`MANGA_OPENROUTER_BASE_URL` points the workers at that mock; it exists for tests only and accepts
loopback addresses only.

## Models and licenses

manga-anim's code is under the [MIT](LICENSE) license. The models have their own licenses:

- [Magi v2](https://huggingface.co/ragavsachdeva/magiv2): research license, personal use only.
- [Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M): Apache 2.0.
- [Wan2.2](https://huggingface.co/Wan-AI/Wan2.2-TI2V-5B): Apache 2.0, via [mlx-video](https://github.com/Blaizzy/mlx-video) (MIT).
- [FLUX.2 [klein] 4B](https://huggingface.co/black-forest-labs/FLUX.2-klein-4B): Apache 2.0, via [mflux](https://github.com/filipstrand/mflux) (MIT).
