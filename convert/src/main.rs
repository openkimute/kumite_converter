//! `kumite_convert --t3 BIN [--t2 BIN] [--cs GAME_DIR] [--jobs N] OUT`: everything OpenKumite needs, in one run.
//! The CD-audio tracks are found beside each track 1 ("Track 1" → "Track 2", "Track 3"). `--pack DIR` instead of
//! OUT writes the browser's pack (DIR/assets.pack, DIR/assets.idx; an interrupted run resumes);
//! `kumite_convert unpack DIR OUT` writes a pack's files out as a folder; `kumite_convert --version`.

use anyhow::{Context, Result, bail};
use conv_io::{Ctx, DiscFile, PackVfs, StdDisc, StdFs, StdStore, Vfs, join};
use convert::{Disc, Inputs};
use std::path::{Path, PathBuf};

#[cfg(target_env = "musl")]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

const USAGE: &str = "usage: kumite_convert --t3 BIN [--t2 BIN] [--cs GAME_DIR] [--jobs N] (OUT | --pack DIR)\n       \
                     kumite_convert unpack DIR OUT\n       kumite_convert --version";

/// track 1 and the tracks beside it that exist
fn disc(track1: &str, tracks: usize) -> Result<Disc> {
    if !std::path::Path::new(track1).is_file() {
        bail!("no disc image at {track1}");
    }
    let beside = |n: usize| -> Option<Box<dyn DiscFile>> {
        let p = track1.replace("Track 1", &format!("Track {n}"));
        (p != track1 && n <= tracks + 1 && std::path::Path::new(&p).is_file())
            .then(|| Box::new(StdDisc(PathBuf::from(p))) as Box<dyn DiscFile>)
    };
    Ok(Disc { track1: Box::new(StdDisc(track1.into())), cdda: [beside(2), beside(3)] })
}

/// DIR/assets.pack and DIR/assets.idx, made if not there
fn open_pack(dir: &str) -> Result<PackVfs> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {dir}"))?;
    let store = |name: &str| StdStore::open(&Path::new(dir).join(name)).map(|s| Box::new(s) as Box<dyn conv_io::Store>);
    PackVfs::open(store("assets.pack")?, store("assets.idx")?)
}

/// Every file under `dir` of `from` into `to`.
fn copy_tree(from: &dyn Vfs, to: &dyn Vfs, dir: &str) -> Result<()> {
    for e in from.list(dir)? {
        let path = join(dir, &e.name);
        if e.is_dir { copy_tree(from, to, &path)? } else { to.write(&path, &from.read(&path)?)? }
    }
    Ok(())
}

fn run(args: &[String]) -> Result<()> {
    if let [flag] = args
        && flag == "--version"
    {
        println!("kumite_convert {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if let [cmd, pack, out] = args
        && cmd == "unpack"
    {
        return copy_tree(&open_pack(pack)?, &StdFs(out.into()), "");
    }
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let t3 = opt("--t3").context(USAGE)?;
    let mut rest = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !a.starts_with("--") && !args.get(i.wrapping_sub(1)).is_some_and(|p| p.starts_with("--")));
    let pack = opt("--pack");
    let out = match &pack {
        Some(dir) => dir.clone(),
        None => rest.next().map(|(_, a)| a.clone()).context(USAGE)?,
    };
    let cs = opt("--cs").map(|d| StdFs(d.into()));
    let inputs = Inputs {
        t3: disc(&t3, 1)?,
        t2: opt("--t2").map(|p| disc(&p, 2)).transpose()?,
        cs: cs.as_ref().map(|c| c as &dyn conv_io::Vfs),
    };
    let fs: Box<dyn Vfs> = match &pack {
        Some(dir) => Box::new(open_pack(dir)?),
        None => Box::new(StdFs(out.clone().into())),
    };
    let log = |line: &str| conv_io::print_line(line);
    let ctx = Ctx { out: fs.as_ref(), log: &log, jobs: conv_io::jobs_from(args) };
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let t = std::time::Instant::now();
    let manifest = convert::convert(&ctx, &inputs, &now)?;
    println!(
        "{out}: fingerprint {} in {:.0} s",
        manifest["fingerprint"].as_str().unwrap_or("?"),
        t.elapsed().as_secs_f64()
    );
    Ok(())
}
