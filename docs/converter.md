# The converter and tools

```
kumite_converter/      the tools (Rust workspace): build once with cargo build --release
  convert/             kumite_convert: the converter, everything in one run (also the browser's pack)
  t3_converter/        the Tekken 3 converter: disc → assets/ (the game's data); t3_convert, a developer tool
  t2_converter/        the Tekken 2 converter: fighters into assets/; t2_convert, a developer tool (assets_t2/)
  cs_levels/           Counter-Strike levels installed as arenas of the engine; cs_levels, a developer tool
  psx_media/           PS1 XA sound and STR movies → FLAC and Motion-JPEG AVI (shared, no game in it)
  convert_web/         the converter in the browser (web/convert.html; convert_web/build.sh)
  serve/               serve: the local web server of the viewers and the browser page
  t3_viewer/           a browser viewer for the converted characters, stages, pictures, sound and movies
  t2_viewer/           the viewer for Tekken 2's converted files
```

The tools run no outside program: the PS1 sound and movies are decoded by `psx_media` (XA ADPCM, STR/MDEC) and
written as FLAC and Motion-JPEG AVI. They were ported from Python tools (in git history before their removal) and still
write those tools' exact output, so their code reproduces Python's and numpy's arithmetic where the output depends
on it (`pyjson`, `np`, the `*_as_python` tests). A change to a tool's output changes the online fingerprint: every
player converts again. Tekken 2's tools never use Tekken 3's (they copy what they need).

The binaries below are in `target/release/`.

## Converting: `kumite_convert`

The one command a player needs: the Tekken 3 conversion, and with `--t2` and `--cs` the Tekken 2 fighters and the
Counter-Strike levels, into one folder, the fingerprint once at the end.

```sh
kumite_convert --t3 BIN [--t2 BIN] [--cs GAME_DIR] [--jobs N] OUT   # or --pack DIR: the browser's pack
kumite_convert unpack DIR OUT                                       # a pack's files as a folder
```

OUT can be the per-user folder OpenKumite looks in, so it finds the game without `--assets` or `OPENKUMITE_ASSETS`:
`~/Library/Application Support/OpenKumite/assets` (macOS), `~/.local/share/openkumite/assets` (Linux;
`$XDG_DATA_HOME/openkumite/assets`), `%APPDATA%\OpenKumite\assets` (Windows).

Each run starts clean (it empties `characters/`, `stages/` and `gamedata/`), so a folder converted again with other
extras keeps nothing of the earlier ones. The CD-audio tracks are found beside each track 1.

## Developer tools

`t3_convert`, `t2_convert` and `cs_levels` run the converters' parts one at a time, for working on them: a step
again, a module by hand, the Tekken 2 viewer's files, one level. A conversion to play is `kumite_convert`'s: the
parts edit a converted folder in place, and the order they are run in changes the fingerprint.

| Only here | Command |
|---|---|
| one Tekken 3 step again, in place | `t3_convert BIN assets --only STEPS` |
| the fingerprint after editing a converted folder | `t3_convert --fingerprint assets` |
| the camera tests' golden data | `t3_convert camera_golden assets/tests/camera_golden.json` |
| checks and prototypes (`qa_glb`, `attackpath`, `camera`, `extract_models`, `t3m`, `texrender`) | `t3_convert MODULE ARGS...` |
| the T3 viewer's sound and movies | `t3_convert export_media BIN t3_viewer/media` |
| the Tekken 2 viewer's files (`assets_t2/`) | `t2_convert BIN assets_t2` |
| some Tekken 2 fighters only | `t2_convert transplant BIN assets BLOCKS` |
| a level from a glb, with an id of one's choosing | `cs_levels install LEVEL.glb assets [ID]` |
| another level as a Tekken Force stage | `cs_levels force_stage assets ID BASE [NAME]` |
| one level's glb, to look at | `cs_levels bsp2glb MAP.bsp OUT.glb [GAME_DIR]` |

### Tekken 3: `t3_convert`

```sh
t3_convert "Tekken_3_(USA)_(Track 1).bin" assets [--only characters,stages,...] [--jobs N]  # N threads (1: none)
t3_convert --fingerprint assets            # only the manifest's fingerprint, again
t3_convert MODULE ARGS...                  # one module by hand (t3_convert lists them)
```

| `--only` name | Module (`src/`) | Writes |
|---|---|---|
| characters | `export_gltf` | `characters/model_NNN.glb`: skinned glTF, every animation at 60 fps (own and shared moves), the embu's |
| stages | `export_stages` | `stages/stage_NN.glb`: arenas, Tekken Force and Ball stages, sky, walls |
| pictures | `export_pictures` | `pictures/`: the 2D art as PNG, index maps and palettes |
| media | `export_media` | `audio/` (WAV sounds, FLAC music), `video/` (Motion-JPEG AVI + FLAC soundtrack) |
| gamedata | `export_gamedata` | `gamedata/*.json`: movesets, commands, hit paths, reactions, rules, roster, stages, music, AI, camera, HUD layout |
| effects | `export_effects` | `pictures/effects/`: hit sparks and effects in the game's palettes (reads pictures and gamedata) |
| theater | `export_theater` | `gamedata/theater.json`, `pictures/theater/` |
| shadows | `export_shadows` | `gamedata/shadows.json` |

`manifest.json` records what was converted, when, from which image, and a fingerprint of everything a fight reads
(online play refuses computers whose fingerprints differ).

Shared modules: `disc` (ISO 9660, the BNS archive), `tim` (PS1 pictures), `t3m` / `skel` / `skin` / `anim`
(models and motions), `gamedata_*` (the EXE's tables), `mips` (runs a routine's code to read the arguments of its
draw calls: the screens' layouts come from the code, not typed in), `extract_models`, `assemble`, `atlas`,
`texrender`, `embu`, `camera_motion`; `pyjson` writes JSON exactly as Python's `json` does, `np` does numpy's float arithmetic in fixed
orders (the SVD and 4x4 inverse in `np_portable`, shared with the Tekken 2 converter). Other steps: `extract_models` (OBJ models), `qa_glb` (checks
exported models for stretched triangles). `reference/` holds integer-exact prototypes of tricky routines (the
camera, attack paths): `t3_convert camera_golden assets/tests/camera_golden.json` writes the camera tests' golden data. Tools that read the
disc on their own take it from `T3_BIN`, else `Tekken_3_(USA)_(Track 1).bin` at this repository's root.

To redo a step use `--only`: a module run by hand gets none of convert_all's extra arguments (`export_gltf` needs
`--with-common --with-embu`) and leaves the fingerprint as it was (`t3_convert --fingerprint assets` after).

**Adding data:** find it in the EXE (the reference notes: [reference/](reference/README.md)), export it in the step that owns it
(usually `gamedata_tables`, as JSON with the RAM address of each table), then read it in OpenKumite
(`kumite_sim::data` or `kumite_assets`). The engine never reads the disc.

### Tekken 2: `t2_convert`

A Tekken 2 disc (USA v1.1, SLUS-00213) into `assets_t2/`, and Tekken 2 fighters (Kazuya, Jun, ...) into the Tekken 3
conversion's `assets/`:

```sh
t2_convert "Tekken 2 (USA) (v1.1) (Track 1).bin" assets_t2         # characters, stages, media
t2_convert transplant "Tekken 2 (USA) (v1.1) (Track 1).bin" assets  # then: t3_convert --fingerprint assets
```

Its modules: [t2_converter/README.md](../t2_converter/README.md). Formats:
[reference/tekken2/engine.md](reference/tekken2/engine.md). The transplant reads `assets_t2/media/video/` beside `assets/` (`t2_convert`'s media first). Re-running Tekken 3's
`gamedata` step rewrites `roster.json`, `stages.json`, `force.json`, `ai.json` and `screens.json` (its `shadows`
step `shadows.json`), which drops what the transplant and `cs_levels` added: run them again after (or convert
again with `kumite_convert`).

### Counter-Strike levels: `cs_levels`

```sh
cs_levels from_game GAME_DIR assets              # all the game's levels and the two Force stages (as --cs)
cs_levels install LEVEL.glb assets [ID]          # an arena of the CS LEVELS menu
cs_levels force_stage assets ID BASE [NAME]      # an installed level as an extra Tekken Force stage
cs_levels bsp2glb MAP.bsp OUT.glb [GAME_DIR]     # one level's glb, to look at
```

All but `bsp2glb` refresh the manifest's fingerprint. Details: [cs_levels/README.md](../cs_levels/README.md).

## In the browser: `web/convert.html`

The site <https://openkimute.github.io/kumite_converter/> is `web/` from main (`.github/workflows/pages.yml`).
`sh convert_web/build.sh` builds it into `web/pkg/`; `target/release/serve 8770 web` serves it at
<http://127.0.0.1:8770/convert.html>. Nothing leaves the user's computer. Dev: `node web/pack_assets.mjs ASSETS OUT`
makes a converted folder into the browser's pack, and `web/import_pack.html` copies a served `web/pack/` (a link to
OUT) into the site's OPFS (`?clear` removes it).

- **Pick:** the Tekken 3 image's track 1 (and track 2 for the CD music); optionally the Tekken 2 tracks; optionally
  the Counter-Strike folder (the one holding `cstrike/` and `valve/`), picked or dropped.
- **Start, then Cancel:** the bar follows each step's share of the time (`WEIGHT` in `convert.html`, measured with
  `kumite_convert --jobs 1`: the browser's converter is one thread too). Cancel stops the worker: the browser's
  storage keeps the finished steps (Start goes on from there), a folder is converted again from the beginning.
  The background, `web/background.webp`, is in Git LFS (`git lfs install` before cloning, or `git lfs pull`).
- **Chrome and Edge (137 or newer): a folder on the disk.** **Start** asks for a folder (an empty
  one, or one converted into before) and writes the game there as `kumite_convert` does: `characters/`,
  `stages/`, `gamedata/`, `audio/`, `video/`, `pictures/`, `manifest.json` last. Closed or reloaded midway, the
  conversion starts again from the beginning when run again. OpenKumite's browser page opens such a folder (**Open
  converted game folder…**), also one written by the command-line converter, and remembers the one this page wrote
  into when served from the same site.
- **Firefox and Safari: the browser's storage.** The page checks the space before it starts (about 1.5 GB for
  Tekken 3, up to about 4.2 GB with both extras). If the page is closed or reloaded, the conversion resumes from the
  last finished step. **Delete it** removes it.
- **Time:** about 5 minutes for Tekken 3, 10 with both extras (on a recent desktop).
- The browser's storage is the site's: OpenKumite's browser page reads it only when served from the same site. To
  play such a conversion, copy `web/convert.html`, `web/convert_worker.js`, `web/background.webp` and `web/pkg/convert_web*` into
  OpenKumite's `web/` and `web/pkg/` (no name clashes) and serve them together.
- Needs a current Chrome, Edge, Firefox or Safari (17 or newer); not a private window.
- **iPhone and iPad:** first add the page to the Home Screen and convert inside that app. Safari may delete a web
  page's storage after 7 days without a visit; a Home Screen app keeps it. Locking the screen pauses the conversion.
  The Counter-Strike folder needs iOS 18.4 or newer.

## Working on the tools

- `sh check.sh commit` (the pre-commit hook): `cargo fmt` (120 columns, `rustfmt.toml`), clippy with warnings as
  errors (`unwrap` and `expect` denied, tests too), the tests; `sh check.sh push` (the pre-push hook, and the
  default) adds `cargo deny` (`deny.toml`).
- **main** takes changes only through pull requests (the repository's rulesets): `.github/workflows/ci.yml` runs
  `sh check.sh push` on Linux and the tests on macOS and Windows, and a pull request merges when they pass and its
  conversations are resolved, by squash or rebase (no merge commits). Only admins create, move or delete `v*` tags.
- **Releases:** set `version` in `Cargo.toml`'s `[workspace.package]` in a pull request, merge it, then push a tag of
  that version on the merged commit: `git pull && git tag v1.2.3 && git push origin v1.2.3` (`v1.2.3-rc.1`, with that version, makes a pre-release).
  `.github/workflows/release.yml` builds each platform on its own runner and publishes the release: archives with
  `kumite_convert`, the README, the licenses and the notices, for Linux (static musl, with mimalloc: musl's allocator
  serializes the threads, the conversion ran 3x slower), Windows (the C runtime linked in) and macOS (universal), on
  amd64 and arm64, plus WASI, the browser converter and `SHA256SUMS`. Profiles: `dist` (LTO, stripped), `web` (the
  browser's build). Every one of them gives the same fingerprint (checked on macOS arm64 and x86_64, Linux arm64
  and amd64, Windows amd64, Node 24's WASI; Node 22's WASI crashes on Linux arm64).
- `sh dist.sh` (on macOS) makes the same files into `dist/` locally, for testing; its Windows ones are cross-built
  through cargo-xwin and convert 2.7x slower than the release's (Microsoft's libraries cargo-xwin downloads).
- The tests that compare with the disc skip silently unless pointed at it:
  ```sh
  T3_BIN="$PWD/Tekken_3_(USA)_(Track 1).bin" T3_GAMEDATA=<a fresh conversion>/gamedata cargo test --release
  T3_FINGERPRINT_DIR=assets cargo test --release -- --ignored fingerprint_of
  ```
- A change to what a tool writes changes the online fingerprint: every player converts again. Check a change
  against the previous build's conversion (`cmp`, PNGs by pixels; the fingerprint).
- Floats: every platform (macOS, Linux, Windows, wasm) must give the same bits, so the fingerprint: only + - * /,
  `sqrt` in fixed orders (`np`, `np_portable`), `np_portable::fma` (the CPU's FMA, else `libm::fma`), and `sin`,
  `cos`, `acos`, `atan2`, `pow` through the `libm` crate, never `f64::mul_add`, `f64::sin` and the like (the
  platform's libm: Windows x64's static C runtime gets `fma` wrong on a quarter of inputs, and `mul_add` calls it
  where the CPU target has no FMA). `cs_levels/src/fingerprint.rs` is a copy of `convert_all`'s fingerprint (no
  shared crate): change both.

## Viewers

```sh
serve 8765 t3_viewer      # http://127.0.0.1:8765 (the defaults)
serve 8767 t2_viewer      # http://127.0.0.1:8767
```

They read the converted files through links next to them (not in git): in `t3_viewer/`,
`models -> ../assets/characters`, `stages -> ../assets/stages`, `pictures -> ../assets/pictures`; in
`t2_viewer/`, `models`, `stages`, `media` into `../assets_t2/`. Create them with `ln -s`. The T3 viewer's
sound and movies read the export's own layout: `t3_convert export_media BIN t3_viewer/media`.
