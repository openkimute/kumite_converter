# OpenKumite: one converter for everything, the plan

**Status (2026-10-17):** steps 0–7c done and merged. `kumite_convert --t3 BIN [--t2 BIN] [--cs GAME_DIR] OUT`
runs with no outside program; fingerprints T3 only `59656217…`, T3 + CS + T2 `3f41b981…`, the same on macOS,
Linux x86_64, Node WASI, and in the browser (`web/convert.html`: headless Chrome, both; Firefox, T3 only). The
game runs from the browser's pack (Chrome, Firefox; Safari boots it). Left: Safari and Edge conversions by hand, iOS
(7d), compacting the pack (4.1 GB written for 3.3 GB of files with both extras), the rest of step 8.

**Goal.** One Rust tool, `kumite_convert`, converts everything in one run.

| Input | Status | What it gives |
|---|---|---|
| Tekken 3 disc | required | the whole game |
| Tekken 2 disc | optional | the Tekken 2 cast in the roster |
| Counter-Strike game folder | optional | its levels as arenas, and two extra Tekken Force stages |

With the Tekken 3 disc alone, the result is a complete game with Tekken 3 assets only. The engine works both with
and without the extras.

**The user's decisions:**
1. The tool links the existing tools as **crates, statically**, and runs **no external program**: no ffmpeg, no
   other binary. It will later be compiled to **WebAssembly**, to convert in a browser.
2. `--cs` takes the **Counter-Strike game folder** itself. The levels are converted from its `.bsp` and `.wad` files.
3. When `--cs` is given, the two extra Tekken Force stages are always added.
4. It must work in **all modern browsers**: current Chrome, Edge, Firefox and Safari, on desktop and, where the
   platform allows, on iOS, iPadOS and Android.

This plan comes from reading the tools (this repository) and the engine (OpenKumite), from measurements, and from a
wasm32 build of the current crates. Line references are as of commit a3bacb3. Guesses are marked as such.

## 1. Where things stand

### How the extras get into the game today

Each extra is a separate tool that edits a Tekken 3 `assets/` folder in place, after the Tekken 3 conversion.

**`t2_convert transplant BIN assets`** needs the Tekken 3 conversion: each Tekken 2 fighter gets a Tekken 3 donor.

- **Reads from the Tekken 3 output:** the roster, the donor's moveset and glb (its animations go into the Tekken 2
  model), `reactions.json`, `shared.json`, `sound.json`, `music.json`, `theater.json`, and the audio and picture
  indexes.
- **Reads from Tekken 2's own output:** the ending movies, at a hard-coded `assets/../assets_t2/media/video`
  (`transplant.rs:1476`).
- **Writes:** `characters/model_9NN.glb` (900–946), `moveset_t2_*.json`, `stages/stage_{30,31,60–82}.glb`,
  `audio/sfx/m9NN/`, `t2_NN.mp3`, `video/1NN.*`, portraits, faces and Theater thumbnails.
- **Edits in place:** the roster, the movesets index, `stages.json` (appended, not sorted), `sound.json`,
  `music.json`, `theater.json`, `reactions.json`, the audio and picture indexes, and the lists the engine draws
  fighters from or shades them by: `ai.json` (the arcade's), `screens.json` (Survival's pools), `shadows.json`.

**`cs_levels install LEVEL.glb assets [ID]`** writes `stages/stage_ID.glb`, adds a `"menu": "CS LEVELS"` entry to
`stages.json` (sorted by id) and refreshes the fingerprint.

**`cs_levels force_stage assets ID BASE`** builds a Tekken Force script from the Tekken 3 stage `BASE` and appends it
to `force.json`'s `extra_stages`, so their order is the run order. Its boss pool is fixed: `[13, 14, 20, 23, 33]`.

The level `.glb` files come from an outside Python script (`levels/tools/bsp2glb.py`, in the Counter-Strike
project).

### Problems with that

- **Re-converting drops the extras.** `export_gamedata` rewrites the roster, stages, music, sound, reactions, force
  and the movesets index; the theater, pictures and media steps rewrite theirs.
- **Orphan files stay behind.** No converter deletes anything. After a Tekken 3-only re-run, the Tekken 2 models,
  movesets and the extra stages remain, and the fingerprint still hashes them, so the folder no longer matches a fresh
  Tekken 3 conversion online.
- **The fingerprint depends on run order** (append versus sort in `stages.json`). The current `assets/` was made
  T3 → CS → T2.
- **IDs can collide.** CS level ids are free, and from 60 up they overwrite Tekken 2 stages.
- **Many manual steps.**
- **Things that rule out the browser:**
  - ffmpeg runs as a subprocess. It also *decodes* the PS1 formats (XA audio, STR/MDEC video), not only encodes.
  - Apple Accelerate is called through FFI for numpy-identical floats, and the platform libm for sin/cos/atan2.
  - `std::fs`, temp folders, `Instant`/`Local::now` and threads all panic or fail on `wasm32-unknown-unknown`.

### The engine already copes

Every reader goes through `kumite_fs::read` and tolerates absent data:

| Reader | Where |
|---|---|
| roster, movesets index, stages, CS list | `menu.rs` |
| sound banks | `kumite_game/sound.rs` |
| Theater (Tekken 2 parts optional) | `app/theater.rs` |
| Force extra stages | `forcerun.rs`, `force.rs` |
| reactions | `kumite_sim` |
| picture sets | `art.rs` |

Tekken 2 ids appear only through roster lookups:
- `T2_BOSSES = [23, 33]`;
- the tag arcade's boss pairs;
- `id >= 23` meaning a Tekken 2 fighter;
- model records 900+.

Pairs and bosses that are not in the roster are skipped. The Force boss pool is filtered by the roster. The CS LEVELS
menu shows only when there are levels.

**No engine change is needed for the extras**, as long as two things hold:
1. The Tekken 2 numbering stays as the transplant makes it (Kazuya 23, Devil 33, models 900 + block).
2. Tekken 3's 20 stages stay first in `stages.json`: the menu takes the first 15.

The engine changes this plan does need come from the media formats (§4).

**Byte-identical one-player modes** hold on a pure Tekken 3 conversion only. With extras, by design:
- the arcade's stages 5–8 and Survival include Tekken 2 fighters;
- Tekken Force gains extra stages.

`--hash-log` comparisons therefore use a Tekken 3-only conversion.

## 2. The design

**An orchestrator crate over the existing ones; the extras keep editing in place.** The orchestrator, in
`convert`, links `t3_converter`, `t2_converter` and `cs_levels`. It cleans the output first, runs the steps
in a fixed order and computes the fingerprint once.

- No pack or overlay system. It was rejected because:
  - the Tekken 2 data holds absolute indexes into Tekken 3's arrays (throw victims, sound script indexes, sfx codes
    from 212, movie and BGM indexes);
  - a pack would still contain Tekken 3 data (the donors' animations);
  - about 12 engine readers would change, including the simulation's.
- Its only gain would be switching packs on and off without converting again.

```sh
kumite_convert --t3 "Tekken_3_(USA)_(Track 1).bin" \
               [--t2 "Tekken 2 (USA) (v1.1) (Track 1).bin"] \
               [--cs "PATH/TO/Half-Life"] \
               OUT
```

The CD-audio tracks (Track 2, Track 3) are inputs too. The CLI finds them beside track 1 as now; the library takes
them explicitly.

### The run

1. **Clean.** Empty `OUT/characters`, `OUT/stages` and `OUT/gamedata`, so no orphans survive.
2. **Tekken 3.** Every step, without the fingerprint yet.
3. **Counter-Strike** (with `--cs`):
   1. Find the game folder (§5).
   2. Convert each level's `.bsp` in memory, with no intermediate file.
   3. Install the levels with fixed ids from 40: the 17 known levels in the README's order (40–56), any others after
      them, sorted by name. More than 20 levels is refused, because id 60 and up belong to Tekken 2.
   4. Add the two Force stages: `de_dust → 41, base 1`, then `cs_assault → 40, base 0`.
4. **Tekken 2** (with `--t2`): its media step for the ending movies (into a scratch area of `OUT`), then the
   transplant.
5. **The manifest.**
   - The fingerprint, computed once at the end.
   - What went in: `"packs": {"t2": {"file", "sha1_16mb"}, "cs": [level names]}`. The engine reads only
     `fingerprint`, so the extra key is safe.
   - The manifest is written last. A folder without one counts as not converted, so an interrupted browser run is
     safe.

`t3_convert`, `t2_convert`, `cs_levels` and `serve` keep working on their own, for single steps and debugging. The
CLIs keep `std::fs`.

### The library's shape

```rust
pub trait DiscFile { fn name(&self) -> String; fn open(&self) -> Result<Box<dyn ReadSeek>>; } // opened per step
pub trait Vfs { fn read, write, exists, list, remove_all }          // the output tree; write creates folders
pub struct Disc { name: String, tracks: Vec<Box<dyn DiscFile>> }    // track 1 first, then CD-audio tracks
pub struct Inputs { t3: Disc, t2: Option<Disc>, cs: Option<Box<dyn Vfs>> /* the game folder, read only */ }
pub fn convert(inputs: &Inputs, out: &dyn Vfs, now: &str, progress: &dyn Fn(Progress)) -> Result<Manifest>
```

- **Implementations:** natively, `DiscFile` and `Vfs` are thin wrappers over `std::fs`, so the CLI's output stays the
  same. In the browser they sit over `File` objects and OPFS (§6).
- **Each step splits** into `run(ctx, …)` and a thin CLI `main(args)`. About 120 `std::fs` call sites change, all
  mechanically.
- **Logging and time:** `println!` becomes the `progress` callback. Timings move to the CLI. The manifest's timestamp
  comes in as `now`.
- **No renames:** the media step's `media/sfx → audio/sfx` rename becomes a path map, because OPFS cannot rename
  folders.
- **Threads** (`export_gltf`, Tekken 2 `export_characters`) run inline on wasm or with `--jobs 1`. The output does
  not depend on the thread count today; a test keeps it that way.

### Where the shared code lives (decision for the user)

The rule: Tekken 2 tooling never imports Tekken 3's. It holds as long as `t2_converter` does not depend on
`t3_converter`, and only the orchestrator depends on both.

New code both converters need, with no game knowledge in it:
- the I/O traits;
- the XA and STR/MDEC decoders;
- the FLAC and AVI writers;
- the portable maths.

| | Option |
|---|---|
| **A (proposed)** | Small neutral crates, e.g. `conv_io` and `psx_media`, used by both converters. Neither is Tekken 3 tooling. |
| B | Copy the code into both converters, as `ps1.rs` and `pyjson.rs` are copied today. |

## 3. Identical results on every platform (portable maths)

A conversion in the browser must give the same online fingerprint as one on a desktop, and so must macOS, Linux and
Windows. Today they differ:
- the float maths goes through Accelerate on macOS and through nalgebra or plain loops elsewhere;
- `sin`, `cos`, `acos` and `atan2` call each platform's libm.

Measured: the SVD and the 4×4 inverse differ from Accelerate on every input, and some matrix products do too.

**The change:** one implementation everywhere.

| What | How |
|---|---|
| `sin`, `cos`, `acos`, `atan2`, `powf` | the `libm` crate (musl's code, the same Rust's std uses on wasm) |
| `sqrt`, `mul_add` | already correctly rounded everywhere; unchanged |
| matrix and vector products | fixed loop orders, keeping those that already match today's results where they do |
| 3×3 SVD | Jacobi, using only + − × ÷ and `sqrt` (nalgebra's goes through the platform's `hypot`) |
| 4×4 inverse | the LU inverse Tekken 2 already has, shared |
| Accelerate, nalgebra | removed |

The call sites that reach fingerprinted files:
- **Tekken 3:** `skel.rs` (skeletons, animations), `export_gltf.rs` (bind matrices: every model) and
  `export_stages.rs` (the sky drum).
- **Tekken 2:** `np.rs`, via the transplant and stages.
- **cs_levels:** `install.rs:487`.

**Consequences:**
- **A new fingerprint, once.** Every player converts again.
- **The fight may change.** The simulation reads skeletons and animations from the glb (`kumite_game/src/lib.rs:222`), so
  the last bits can reach the fight. The `--hash-log` one-player baselines must be compared before and after. Any
  difference is investigated until it is understood: a borderline hit check flipping against the original would
  be a bug.
- **Checked on:** macOS = Linux = a `wasm32-wasip1` build of the CLI run under Node's WASI (Node 22, already used by
  `headless.mjs`). That one run also covers 32-bit `usize` and the single-thread paths.

## 4. Media without ffmpeg

What ffmpeg does today, and what replaces it:

| Output | Today (ffmpeg) | Read by | New |
|---|---|---|---|
| music `audio/music/NN` | decodes XA (37.8 kHz stereo), encodes MP3 192k, resampled to 32 kHz | engine: rodio/symphonia natively, `<audio>` in the browser | our **XA decoder** → **FLAC** at 37.8 kHz (`flacenc`, pure Rust, Apache-2.0, without its thread feature) |
| CD audio `cdda_track2` | raw 44.1 kHz → MP3 | engine | **FLAC** 44.1 kHz |
| movies `video/NN.avi` | decodes STR (MDEC v3, XA audio), encodes MJPEG AVI | engine: `kumite_assets` AviIndex, zune-jpeg | our **MDEC decoder** → JPEG (`image` crate, `jpeg` feature) → our **AVI writer** |
| movie sound `video/NN.mp3` | XA → MP3 | engine | **FLAC** |
| `video/NN.mp4` | x264 + AAC | the viewers only | **dropped** |
| sound effects WAV | already Rust | engine | unchanged |

- **No crate exists for the PS1 formats:**
  - the XA decoder is about 60 lines, with the same filter as the sound effects' ADPCM;
  - the STR/MDEC decoder is about 500 lines, written from the psx-spx documentation, not from ffmpeg's LGPL `mdec.c`;
  - the AVI writer is about 80 lines.
- **Why FLAC:**
  - it is the smallest engine change: rodio's `flac` feature (symphonia, pure Rust), and browsers play FLAC in
    `<audio>`;
  - the alternatives fail: no pure-Rust MP3 encoder exists under a usable licence (shine-rs is LGPL), and Vorbis
    has no pure-Rust encoder;
  - **its cost is size:** Tekken 3's music grows from about 155 MB to about 680 MB, and all music with Tekken 2's
    reaches about 1.3 GB (measured on the decoded streams).
- **Opus is rejected.** Opus (`opus-rs`, BSD-3) would be smaller than today, but Safari plays Ogg Opus only from 18.4,
  so it would cut off iOS 17. It would also need a resampler, an Ogg writer and an Opus decoder in the engine.
  FLAC at 37.8 kHz was tested in Chrome, Firefox and Safari, as `<audio>` and through `decodeAudioData`.
- **Engine changes:**
  - rodio's `flac` feature;
  - `movie.rs` looks for `NN.flac`, falling back to `.mp3` so older conversions still play;
  - `serve` gets the `audio/flac` type.
- **Viewer changes.** The viewers lose MP4 and get a small AVI frame player instead (about 40 lines of JavaScript).
- **Fingerprint.** It changes, because `music.json` and `attract.json` name the files. That happens together with §3:
  one fingerprint change for both.
- **Checks:**
  - every music file decodes to its expected length (the existing rodio test);
  - movie frames are compared against ffmpeg's decode by PSNR, in a developer-only test (ffmpeg as a reference, never
    needed to convert);
  - the movies play natively and in the browser.

## 5. Counter-Strike from the game folder

`bsp2glb.py` (484 lines, numpy and Pillow) is ported into `cs_levels` as three modules, about 650 lines.

**`bsp`** reads BSP version 30:
- the entities, planes, textures, vertices, texinfo, faces, edges, surfedges and models lumps (no lighting: the
  levels are unlit);
- faces whose texture name starts with `sky`, `clip`, `trigger`, `null`, `origin`, `hint`, `skip`, `bevel`,
  `noclip`, `noshow` or `aaatrigger` are skipped;
- UVs from the texinfo vectors, triangle fans, winding flipped by the Newell normal against the plane, axes
  `(x, z, −y) × 0.0254`;
- a brush entity's `origin` is added to its vertices.

**`wad`** reads WAD3:
- a texture embedded in the BSP wins; otherwise the texture comes from the WADs in worldspawn's `"wad"` key;
- each WAD is looked up by its basename, case-insensitively (`itsitaly.wad` is `itsItaly.wad` on disc), in
  `cstrike/` first, then `valve/`;
- across WADs, the first texture of a name that parses wins;
- a missing WAD (`cs_siege.wad`) is skipped; a missing texture gets the placeholder and a warning (the Python
  crashed there).

**`level_glb`** builds the same glTF as the script, in the same order:
- images first;
- one mesh per brush model, worldspawn first, one primitive per texture (sorted by name);
- unlit materials, with `alphaMode: MASK` for `{` textures (palette index 255 transparent);
- one sampler;
- `info_player*` nodes, and the scene named after the map.

The result goes straight into the installer, which already works on the glb in memory. The `source` names stay
`<map>.glb`, as they appear in `stages.json` and `force.json`.

**The game folder:**

| Folder given | Found by | Then |
|---|---|---|
| a Half-Life root (the installed or unpacked game) | `cstrike/maps/*.bsp` | used directly |
| the `cstrike/` folder itself | `maps/*.bsp` | natively, `../valve` beside it; in the browser the root is required |
| the installer CD | `cstrike/counter-strike.exe` without `maps/` | refused: "install or unpack it first" |

Without `valve/halflife.wad`, most textures are missing, and the error lists them.

The 17 levels read about 32 MB of BSP and 60 MB of WAD. Other copies of the game (Steam, other versions) work, but
give other arenas and another fingerprint.

**Verified so far** (by the research):
- The Python's 17 glbs regenerated equal `levels/` structurally.
- Today's Rust `install` reproduces `stage_40`, `stage_41` and `stage_48` byte for byte.
- A plain left-to-right sum gives numpy's UVs exactly over all faces of all 17 maps.
- The normals' division is not yet confirmed bit for bit.

**The check for OpenKumite:** each new stage glb's `canonical_glb` equals today's `assets/stages/stage_40..56.glb`.
Before the maths change, the fingerprint is compared on a copy of `assets/`.

**One string to settle.** `force.json`'s `source` reads `"tools/cs_levels/force_stage.py: …"` in today's Rust and
`"cs_levels/force_stage.py: …"` in the current `assets/`. It changed when the folders were reorganised. It is
fingerprinted, so a new value is set once, with the §3 change.

## 6. The browser (later phase)

Checked on 2026-10-17 by a test page in Chrome 155, Firefox 156 and Safari 27 on macOS, and against wpt.fyi, MDN,
caniuse and the WebKit blog and bug tracker. Edge is Chromium. iOS, iPadOS and Android were not tested (no device).

### What works everywhere, and the design built on it

**The converter** runs in a dedicated Web Worker, single-threaded (no SharedArrayBuffer: that needs cross-origin
isolation).
- **Inputs:** the disc images are picked with `<input type=file>` and read with `FileReaderSync` on `File.slice`, in
  1 MB chunks: about 50 ms per 64 MB in all three browsers. The Counter-Strike folder comes from
  `<input webkitdirectory>` (every desktop browser; iOS/iPadOS from 18.4) or a dropped folder.
  `showDirectoryPicker` is Chromium only: it picks the output folder there (below).
- **Output:** OPFS, written through `createSyncAccessHandle` (1.9–6 GB/s), one open handle at a time. Safari refuses
  a second handle on the same file.
- **Panics:** a panic hook in the worker reports the message, and the page shows it.
- **Safety at start:** the converter checks that OPFS is there, that sync handles work, that `storage.estimate()`
  leaves room for the output (about 2.6 GB for Tekken 3 with FLAC, a guess), and that the window is not private.
  Otherwise it says what is missing.
- **When a write fails with `QuotaExceededError`:** the converter shows "not enough space: needs X, has Y" and
  removes its partial output.
- **Resuming:** a progress file in OPFS lists the finished steps, so an interrupted conversion resumes. The Screen
  Wake Lock (all browsers) keeps the screen on. `manifest.json` is still written last.
- **After success:** `navigator.storage.persist()`, from the button's click, because Firefox shows a prompt.

**One pack file, not a tree of files.** In OPFS only reading and writing through an open handle is synchronous;
creating a file or folder (`getFileHandle`, `getDirectoryHandle`) is async, and the converter writes about 4,700 files
synchronously. So the browser's `Vfs` is a pack:
- `assets.pack`: the files' bytes, appended through one sync access handle opened before the conversion starts. A
  file written again (an `index.json`) is appended again; the index points at the latest copy.
- `assets.idx`: JSON, `{"version": 1, "files": {"path": [offset, length], ...}, "done": [finished steps]}`, rewritten
  through its own sync handle after every step (that is also the resume point) and at the end.
- `remove_all` drops paths from the index. Reads come from the pack at the index's offsets.
- The pack logic is plain Rust in `conv_io` (`PackVfs` over a "read/write at offset" store), so it is tested natively
  over a std file: a conversion into a pack, unpacked, must equal a normal conversion byte for byte.

**The game reads the pack without a service worker.**
- A service worker does not see synchronous XHR from the page in Chrome, Edge or Safari: the wpt test fails there
  and whatwg/xhr#397 proposes making that the rule. `kumite_fs` falls back to synchronous XHR for files it has not
  prefetched, so a service worker is not an option.
- **The way that works in all three:**
  - The page's `__okfs` gets an OPFS mode. At start-up it reads `assets.idx` and gets the pack as a `File`
    (`getFile()`): every URL maps to a slice of it.
  - Prefetching reads `slice.arrayBuffer()`, or `slice.stream()` for movies, into the cache the game already reads.
  - A file not prefetched gets a `blob:` URL of its slice, and kumite_fs's existing synchronous XHR reads that. (Tested
    with an OPFS file's blob URL: 16 MB in 50–60 ms.)
  - Audio plays the same blob URLs.
  - `exists` asks the map: a blob URL answers only GET.
- **Code changes:** about 15 lines in kumite_fs (two optional JS hooks; without them today's behaviour stays), about 5 in
  `audio_web.rs`, and the OPFS backend in `index.html`.
- **The game and the converter never run at once.** Converting is its own screen, and the game starts after a
  reload. That keeps memory low, which matters on iOS.

### Chrome and Edge: a folder on the disk

Where the browser has `showDirectoryPicker` and JSPI (WebAssembly JS Promise Integration, Chrome 137+), the
conversion goes into a folder the user picks instead of OPFS, the same tree `kumite_convert` writes, and the game
reads it from there.
- **The converter writes the folder through JSPI.** Writing into a picked folder is async only (`createWritable`;
  sync access handles are OPFS's alone), and the converter writes synchronously. wasm-bindgen 0.2.129 supports JSPI:
  the exports are `#[wasm_bindgen(jspi)]` and the folder's functions `#[wasm_bindgen(suspending)]`
  (`convert_web/src/dir.rs`), so each `Vfs` call waits on the worker's async function without blocking anything. The
  JSPI build needs exnref, which Safari before 18.4 cannot compile, so `build.sh` makes two builds and the worker
  loads `convert_web_jspi.js` only where `WebAssembly.Suspending` exists.
- **No resuming:** as the command line, a run starts clean (removing `characters/`, `stages/`, `gamedata/`), and no
  progress file is left in the folder. The page refuses a folder holding anything but what the converter writes.
- **The handle is kept in IndexedDB** (`openkumite` / `handles` / `folder`), for the game page.
- **The game** (`index.html`) walks the folder at start-up and gets a `File` for each file (lazy: a `File` reads from
  the disk only when read). Those are the same Blobs the pack's slices are, so kumite_fs's hooks stay as they were:
  prefetching reads them, a synchronous read or a sound takes a blob: URL of one. A remembered folder needs its
  access asked again on a later visit, from a click (**Open it**); **Open converted game folder…** opens any
  converted folder, also the command line's. First found wins: the folder, the OPFS pack, the server.
- **Measured** (Chrome 155, macOS, M-series; headless, Tekken 3 only, 1,601 files): into the folder 325 s, into OPFS
  293 s (the command line, `--jobs 1`: 86 s); peak resident memory of all Chrome's processes 2.0 GB and 1.9 GB.
  The fingerprint is the command line's, 59656217…, and the folder equals the command line's by `diff -r` but for
  the manifest's times and four FLAC files whose encoding differs (the same samples decoded: flacenc's float
  choices differ between wasm and native; audio is not fingerprinted). With both extras (4,729 files): 680 s, 2.2
  GB, the fingerprint 3f41b981…, the same `diff -r` result (15 FLAC files). The game's start-up walk: 1,601 files
  in about 200 ms, 4,729 in about 500 ms.
- **Tested headless** by a folder dropped through CDP (`Input.dispatchDragEvent`: `getAsFileSystemHandle()` gives a
  real directory handle) standing in for the picker, which a headless Chrome cannot show; the write access is
  granted beforehand in the test profile's preferences.

### Limits by browser

| | Chrome / Edge | Firefox | Safari 17+ / iOS 17+ |
|---|---|---|---|
| OPFS with sync handles | 102+ | 111+ | 15.2+ (bugs fixed by 17; 17 is the minimum, refused below) |
| Quota per site | 60 % of the disk | min(10 % of the disk, 10 GiB) best-effort; 50 % after `persist()` | about 60 % of the disk |
| FLAC in `<audio>`, 37.8 kHz | yes | yes | yes |
| Folder pick | yes | yes | yes (iOS from 18.4) |
| wasm memory to 4 GB | yes | yes | yes on macOS; on iOS the process is killed instead of `memory.grow` failing |

- **Safari before 17** (1 GB quota, then a prompt) and **private windows** are refused, with a message.
- **Firefox on disks under about 30 GB** needs `persist()`, which prompts, to get past 10 % of the disk.
- **iOS and iPadOS cannot fully match the desktop:**
  - **Storage can be wiped.** In a Safari tab, a site's storage (OPFS included) is deleted after 7 days of Safari use
    without a visit. Home-screen web apps are not subject to that, and their storage is separate from Safari's (to
    verify). So on iOS the page asks to "Add to Home Screen" first and converts inside the installed app.
  - **Conversion pauses.** Safari suspends JavaScript, workers included, when the user leaves the app or locks the
    screen. The conversion resumes on return, thanks to the progress file.
  - **Memory is tight.** iOS kills tabs that use too much, and available memory shrinks across reloads. The converter
    keeps its peak low (a target of about 500 MB, a guess to measure) and streams every step.
  - **Older iOS:** before 18.4 there is no folder picker, so Counter-Strike cannot be added there (it is optional; the
    screen says so). Chrome, Firefox and Edge on iOS use WebKit, so they behave the same.
  - If the converted game is gone, the page says "the browser removed the converted game: convert again".

### Test matrix

| Platform | Browsers | Checks |
|---|---|---|
| macOS | Chrome, Edge, Firefox (Playwright), Safari (by hand, results posted back) | fingerprint = desktop CLI's; the game starts; a synchronous-read fallback; music; movies |
| Windows 11 | Edge, Chrome, Firefox | the same; fingerprint = macOS's |
| Android | Chrome, Firefox | quota, background, memory peak |
| iPhone (4 GB RAM) and iPad, iOS 17 and 18.4+ | Safari tab and home-screen app | a 700 MB file pick, resuming after a lock, memory (Web Inspector), FLAC at 37.8 kHz, blob-URL reads, storage after install |
| any browser | a low-disk profile | the `QuotaExceededError` path |

## 7. Steps

Each step is finished, checked and committed on its own.

| # | Step | Check |
|---|---|---|
| 0 | **Baseline:** a fresh conversion T3-only, and T3 → CS → T2 with today's tools, into scratch folders | record both fingerprints and file trees; OpenKumite's tests on the T3-only one; `--hash-log 30` of arcade, VS, Force, Survival |
| 1 | **I/O and context:** `DiscFile` / `Vfs` traits, the step `run(ctx)` / `main(args)` split, the progress callback, no timers or temp folders, `--jobs 1`, explicit CD-audio tracks, the Tekken 2 video folder parameter | `cmp -r` against step 0: byte-identical, with any `--jobs` |
| 2 | **Library APIs:** step tables into the libraries, a `cs_levels` lib, the fingerprint over a `Vfs` | as step 1 |
| 3 | **CS from the game folder:** `bsp`, `wad`, `level_glb`, folder detection; `cs_levels bsp2glb` for debugging | the Python's 17 glbs structurally equal; every installed stage canonically equal to `assets/stages/stage_40..56` |
| 4 | **`kumite_convert`, natively:** clean, T3, CS, T2, manifest | T3-only = step 0's fingerprint and tree; all three = step 0's T3 → CS → T2 fingerprint; re-running gives the same; the game plays both |
| 5 | **Portable maths** (with the `force.json` source string) | the new fingerprint is the same on macOS, Linux and Node WASI; port tests; `--hash-log` against step 0, any difference understood |
| 6 | **Media without ffmpeg:** XA, MDEC, FLAC, AVI; engine `flac`; viewers' AVI player | music lengths; movie PSNR against ffmpeg; plays natively and in the browser; no `ffmpeg` left in the converter |
| 7a | **Browser checks and the convert screen:** OPFS, sync handles, quota, private window, what is missing | the screen's verdicts in all four browsers and on iOS |
| 7b | **The converter in a worker:** `FileReaderSync` inputs, OPFS output, quota errors, resume, Wake Lock, `persist()` | Chrome, Edge, Firefox and Safari give step 5's fingerprint |
| 7c | **The game reads OPFS:** `__okfs`'s OPFS mode, blob URLs for synchronous reads and audio, the kumite_fs and `audio_web.rs` hooks | the game, music and movies from a browser conversion, in all four |
| 7d | **iOS:** "Add to Home Screen" first, convert there, "convert again" when storage was wiped | the iOS rows of §6's matrix |
| 7e | **Chrome and Edge: a folder on the disk** (§6): the converter writes it through JSPI, the game reads it | the folder = the command line's (`diff -r`), its fingerprint; the game, music and movies from it with nothing on the server |
| 8 | **Docs:** one command in getting started, converter.md, the READMEs; drop the "extras gone after converting again" row | link check, the reviewers |

Steps 1–4 change no output. Steps 5 and 6 change the fingerprint, and ship together as one "convert again" change.

Engine improvements alongside:
- the app's tests read `OPENKUMITE_ASSETS` instead of the hard-coded `../../assets`;
- a test that a Tekken 3-only conversion starts every mode.

## 8. Risks and decisions

- **Decision:** neutral shared crates (`conv_io`, `psx_media`) or copies in each converter (§2).
- **Settled:** FLAC (works in every browser tested); Opus would cut off iOS 17 (§4).
- **Settled:** no service worker; the game reads OPFS through `__okfs` and blob URLs (§6).
- **Risk:** the maths change could alter a fight in its last bits. That is caught by the `--hash-log` comparison in
  step 5.
- **Risk:** the Counter-Strike result depends on the user's copy of the game.
- **Risk:** iOS can wipe a Safari tab's storage and suspends work in the background; home-screen install and
  resuming answer that, but iOS is untested (§6).
