# kumite_converter

Converts your own Tekken 3 disc (PlayStation, USA, SLUS-00402) into the game data OpenKumite plays (OpenKumite: an
open-source 3D fighting game engine, a separate project). Optionally adds the Tekken 2 fighters from your Tekken 2
disc and Counter-Strike levels from your Counter-Strike game. Runs on the command line or in a browser.

> **Unofficial.** kumite_converter is not affiliated with, endorsed or sponsored by Bandai Namco Entertainment.
> TEKKEN is a trademark of Bandai Namco Entertainment Inc.; PlayStation is a trademark of Sony Interactive
> Entertainment; Counter-Strike and Half-Life are trademarks of Valve Corporation. This repository contains no game
> data: you need your own legally obtained copies, which the converter reads on your computer. Do not ask for, or
> share, disc images or converted files.

## Quick start

You need [Rust](https://rustup.rs) and the game's disc image as a raw `.bin` (`Tekken_3_(USA)_(Track 1).bin`; a
`.chd`: `chdman extractcd` first; a 2048-byte `.iso` lacks the sound and movies and does not work).

```sh
cargo build --release
target/release/kumite_convert --t3 "Tekken_3_(USA)_(Track 1).bin" OUT   # once, about 2 minutes
```

OUT is any folder; OpenKumite finds it without being told when it is the per-user folder OpenKumite looks in:

```sh
target/release/kumite_convert --t3 "Tekken_3_(USA)_(Track 1).bin" ~/Library/Application\ Support/OpenKumite/assets  # macOS
target/release/kumite_convert --t3 "Tekken_3_(USA)_(Track 1).bin" ~/.local/share/openkumite/assets                  # Linux
target\release\kumite_convert --t3 "Tekken_3_(USA)_(Track 1).bin" %APPDATA%\OpenKumite\assets                      # Windows (cmd; PowerShell: $env:APPDATA)
```

(or `assets/` beside OpenKumite's program, or its repository's `assets/`; any other folder is given to it with
`--assets DIR` or `OPENKUMITE_ASSETS`: OpenKumite's getting-started guide lists where it looks). With your Tekken 2 (USA
v1.1) disc image and your Counter-Strike game, the same run adds the Tekken 2 fighters and the Counter-Strike levels;
both are optional:

```sh
target/release/kumite_convert --t3 "Tekken_3_(USA)_(Track 1).bin" \
    --t2 "Tekken 2 (USA) (v1.1) (Track 1).bin" --cs "PATH/TO/Half-Life" OUT
```

`--cs` takes the folder that holds `cstrike/` and `valve/`. The CD-audio tracks (`Track 2`, `Track 3`) are found
beside each track 1. The output is about 1.4 GB (3.1 GB with both extras), with `manifest.json` (what was converted,
and a fingerprint online play compares).

In a browser: `sh convert_web/build.sh`, then `target/release/serve 8770 web` and open
<http://127.0.0.1:8770/convert.html>.

## Repository

| | |
|---|---|
| `convert/` | `kumite_convert`: the converter, everything in one run |
| `t3_converter/` | the Tekken 3 converter (`t3_convert`: its steps one at a time, a developer tool) |
| `t2_converter/` | the Tekken 2 converter (`t2_convert`: `assets_t2/` for the Tekken 2 viewer, a developer tool) |
| `cs_levels/` | Counter-Strike levels as arenas (`cs_levels`: one level at a time, a developer tool) |
| `convert_web/`, `web/` | the converter in the browser (`web/convert.html`; `convert_web/build.sh`) |
| `serve/` | `serve`: the local web server of the viewers and the browser page |
| `t3_viewer/`, `t2_viewer/` | the browser viewers' pages (`target/release/serve 8765 t3_viewer`) |

Documentation, and how to check and test changes here: [docs/converter.md](docs/converter.md); the reference notes (the original games and their disc formats):
[docs/reference/](docs/reference/README.md); the universal converter plan: [docs/UNIVERSAL.md](docs/UNIVERSAL.md).
`sh check.sh` runs the checks (formatting, clippy, tests, cargo deny; the git hooks run it). A tag `vX.Y.Z` (the
version in `Cargo.toml`) publishes a release: one standalone `kumite_convert` for Linux (static), Windows and macOS,
each on amd64 and arm64, a WASI `.wasm`, and the browser converter (docs/converter.md, Releases).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option. Third-party work in the repository: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as
defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
