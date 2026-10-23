//! The Counter-Strike game folder (Half-Life with cstrike/, read only): its levels converted in memory (bsp2glb.py's
//! work) and installed as the CS LEVELS arenas 40.. and two extra Tekken Force stages.
//!
//! `cs_levels from_game GAME_DIR ASSETS`
//! `cs_levels bsp2glb MAP.bsp OUT.glb [GAME_DIR]`
//!
//! GAME_DIR is the Half-Life folder (cstrike/maps/*.bsp, the WADs in cstrike/ and valve/) or its cstrike/ folder
//! (valve/ then beside it). File names are matched in any case (the maps name `itsitaly.wad`, the disc has
//! `itsItaly.wad`). The installer CD (cstrike/counter-strike.exe, no maps) is refused.

use crate::bsp::Bsp;
use crate::wad::{self, Miptex};
use crate::{fingerprint, force_stage, install, level_glb};
use anyhow::{Context, Result, bail};
use conv_io::{Ctx, StdFs, Vfs, join};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The levels of Counter-Strike 1.x (Sierra's CD), in the order of the stage ids 40.. (the README's); others follow
/// them, sorted by name.
pub const LEVELS: [&str; 17] = [
    "cs_assault",
    "de_dust",
    "de_train",
    "as_oilrig",
    "cs_747",
    "cs_backalley",
    "cs_italy",
    "cs_militia",
    "cs_office",
    "cs_siege",
    "de_aztec",
    "de_cbble",
    "de_prodigy",
    "tr_1",
    "tr_1a",
    "tr_2",
    "tr_3",
];
const FIRST_ID: i64 = 40;
/// Stage ids from 60 are Tekken 2's.
const MAX_LEVELS: usize = 20;
/// The extra Tekken Force stages: the level, the original stage whose script it plays.
const FORCE: [(&str, usize); 2] = [("de_dust", 1), ("cs_assault", 0)];

/// The entry of `dir` named `name` in any case: its path.
fn find(game: &dyn Vfs, dir: &str, name: &str, is_dir: bool) -> Result<Option<String>> {
    let e = game.list(dir)?.into_iter().find(|e| e.is_dir == is_dir && e.name.eq_ignore_ascii_case(name));
    Ok(e.map(|e| join(dir, &e.name)))
}

/// The folders whose WADs a level uses: the game's (`cs`), then valve/ (those present).
fn wad_dirs(game: &dyn Vfs, cs: &str) -> Result<Vec<String>> {
    Ok([find(game, "", cs, true)?, find(game, "", "valve", true)?].into_iter().flatten().collect())
}

/// `load_wad_textures` over the WADs a map names, each looked up by its file name in `dirs` (a missing one skipped).
fn wad_textures(
    game: &dyn Vfs,
    dirs: &[String],
    refs: &[String],
    log: &dyn Fn(&str),
) -> Result<HashMap<String, Miptex>> {
    let mut found = HashMap::new();
    for r in refs {
        let mut path = None;
        for d in dirs {
            path = find(game, d, r, false)?;
            if path.is_some() {
                break;
            }
        }
        match path {
            Some(p) => wad::add_textures(&game.read(&p)?, &mut found),
            None => log(&format!("  WAD not found: {r}")),
        }
    }
    Ok(found)
}

/// The level `name` (its BSP's bytes) as a glb's JSON and binary, its textures from the WADs in `dirs` of `game`.
pub fn level(game: &dyn Vfs, dirs: &[String], name: &str, bsp: &[u8], log: &dyn Fn(&str)) -> Result<(Value, Vec<u8>)> {
    let bsp = Bsp::read(bsp).with_context(|| format!("{name}.bsp"))?;
    let wads = wad_textures(game, dirs, &bsp.wads(), log)?;
    level_glb::build(name, &bsp, &wads, log).with_context(|| format!("{name}.bsp"))
}

/// Installs every level of the game folder `game` into the assets (`ctx.out`): the arenas (ids 40.., `LEVELS`
/// first), then the two Force stages (the manifest is the caller's: its fingerprint once, at the end).
pub fn install_game(ctx: &Ctx, game: &dyn Vfs) -> Result<()> {
    let cs = find(game, "", "cstrike", true)?;
    let maps = match &cs {
        Some(cs) => find(game, cs, "maps", true)?,
        None => None,
    };
    let Some(maps) = maps else {
        if let Some(cs) = &cs
            && find(game, cs, "counter-strike.exe", false)?.is_some()
        {
            bail!("this is the Counter-Strike installer CD: install or unpack the game first, then give its folder");
        }
        if find(game, "", "maps", true)?.is_some() {
            bail!("this is a cstrike folder: give the Half-Life folder that holds it (cstrike/ and valve/)");
        }
        bail!("no cstrike/maps in the Counter-Strike folder: give the Half-Life folder (with cstrike/ and valve/)");
    };
    let mut levels: Vec<(String, String)> = game
        .list(&maps)?
        .into_iter()
        .filter(|e| !e.is_dir)
        .filter_map(|e| {
            let (stem, ext) = e.name.rsplit_once('.')?;
            ext.eq_ignore_ascii_case("bsp").then(|| (stem.to_string(), join(&maps, &e.name)))
        })
        .collect();
    let order = |n: &str| LEVELS.iter().position(|l| l.eq_ignore_ascii_case(n)).unwrap_or(LEVELS.len());
    levels.sort_by(|a, b| (order(&a.0), &a.0).cmp(&(order(&b.0), &b.0)));
    if levels.is_empty() {
        bail!("no levels (.bsp) in {maps}");
    }
    if levels.len() > MAX_LEVELS {
        bail!("{} levels in {maps}: at most {MAX_LEVELS} fit (stage ids 40..59)", levels.len());
    }
    let dirs = wad_dirs(game, "cstrike")?;
    let mut ids = HashMap::new();
    for (i, (name, path)) in levels.iter().enumerate() {
        let sid = FIRST_ID + i as i64;
        let glb = level(game, &dirs, name, &game.read(path)?, ctx.log)?;
        install::install(ctx, glb, &format!("{name}.glb"), sid).with_context(|| format!("{name}.bsp"))?;
        ids.insert(name.to_lowercase(), sid);
    }
    for (name, base) in FORCE {
        match ids.get(name) {
            Some(&sid) => force_stage::force_stage(ctx, sid, base, None).with_context(|| format!("{name}.bsp"))?,
            None => ctx.log(&format!("{name}: not in the game folder, no Tekken Force stage")),
        }
    }
    Ok(())
}

/// The Half-Life folder and the game's folder name in it for GAME_DIR (a cstrike folder: the one above it).
fn game_root(dir: &Path) -> Result<(PathBuf, String)> {
    if find(&StdFs(dir.into()), "", "maps", true)?.is_some() {
        let dir = std::path::absolute(dir)?;
        if let (Some(root), Some(name)) = (dir.parent(), dir.file_name()) {
            return Ok((root.into(), name.to_string_lossy().into_owned()));
        }
    }
    Ok((dir.into(), "cstrike".into()))
}

/// `cs_levels from_game GAME_DIR ASSETS`
pub fn main(args: &[String]) -> Result<()> {
    let [game, assets, ..] = args else { bail!("usage: cs_levels from_game GAME_DIR ASSETS") };
    let (root, _) = game_root(Path::new(game))?;
    let out = StdFs(assets.into());
    install_game(&Ctx { out: &out, log: &conv_io::print_line, jobs: 1 }, &StdFs(root))?;
    fingerprint::print_manifest(&out, assets)
}

/// `cs_levels bsp2glb MAP.bsp OUT.glb [GAME_DIR]`: GAME_DIR by default the folder above the map's game folder
/// (MAP.bsp in GAME/maps/, as bsp2glb.py looks).
pub fn bsp2glb_main(args: &[String]) -> Result<()> {
    let [map, out, ..] = args else { bail!("usage: cs_levels bsp2glb MAP.bsp OUT.glb [GAME_DIR]") };
    let (root, cs) = match args.get(2) {
        Some(g) => game_root(Path::new(g))?,
        None => game_root(
            std::path::absolute(map)?.parent().and_then(Path::parent).context("MAP.bsp is not in a maps folder")?,
        )?,
    };
    let game = StdFs(root);
    let name = Path::new(map).file_stem().map_or(String::new(), |n| n.to_string_lossy().into_owned());
    let data = std::fs::read(map).with_context(|| format!("cannot read {map}"))?;
    let (j, bin) = level(&game, &wad_dirs(&game, &cs)?, &name, &data, &conv_io::print_line)?;
    std::fs::write(out, install::write_glb(&j, &bin)).with_context(|| format!("cannot write {out}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn refuses_other_folders() {
        let root = std::env::temp_dir().join(format!("cs_levels_test_{}", std::process::id()));
        let fs = StdFs(root.clone());
        let ctx = Ctx { out: &fs, log: &|_| {}, jobs: 1 };
        let err = |dir: &str| install_game(&ctx, &StdFs(root.join(dir))).unwrap_err().to_string();
        fs.write("cd/CSTRIKE/Counter-Strike.exe", b"").unwrap();
        fs.write("bare/MAPS/x.txt", b"").unwrap();
        fs.write("empty/Cstrike/Maps/readme.txt", b"").unwrap();
        assert!(err("cd").contains("installer CD"));
        assert!(err("bare").contains("cstrike folder"));
        assert!(err("empty").contains("no levels"));
        assert!(err("nowhere").contains("no cstrike/maps"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
