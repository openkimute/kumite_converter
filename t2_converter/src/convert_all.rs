//! Convert a Tekken 2 disc (USA v1.1, SLUS-00213) for the port and the Tekken 2 viewer, apart from Tekken 3's
//! converter (t3_converter/).
//!
//! usage: t2_convert "Tekken 2 (USA) (v1.1) (Track 1).bin" [OUTDIR] [characters,stages,media] [--jobs N]
//!
//!   OUTDIR/characters/  the 47 character blocks as glTF (export_characters)
//!   OUTDIR/stages/      the 23 arenas as glTF (export_stages)
//!   OUTDIR/media/       sounds, music, movies, pictures (export_media)
//! OUTDIR defaults to assets_t2/ at the repository's root (t2_viewer/ links to it).

use crate::{export_characters, export_media, export_stages};
use anyhow::{Result, bail};
use conv_io::{Ctx, DiscFile, StdDisc, StdFs};
use std::path::PathBuf;

pub fn main(args: &[String]) -> Result<()> {
    let (jobs, args) = export_characters::take_jobs(args);
    let Some(src) = args.first() else { bail!("usage: t2_convert BIN [OUTDIR] [characters,stages,media] [--jobs N]") };
    let out = args.get(1).map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../assets_t2"), PathBuf::from);
    let what: Vec<&str> = args.get(2).map_or(vec!["characters", "stages", "media"], |s| s.split(',').collect());
    let ctx = Ctx { out: &StdFs(out), log: &conv_io::print_line, jobs };
    run(&ctx, &StdDisc(src.into()), &export_media::cdda_beside(src), &what)
}

/// The parts (`what` of characters, stages, media) in their folders of the output.
pub fn run(ctx: &Ctx, src: &dyn DiscFile, cdda: &[Option<Box<dyn DiscFile>>; 2], what: &[&str]) -> Result<()> {
    for part in ["characters", "stages", "media"] {
        if what.contains(&part) {
            ctx.log(&format!("== {part}"));
            match part {
                "characters" => export_characters::run(ctx, src, part, None)?,
                "stages" => export_stages::run(ctx, src, part, None)?,
                _ => export_media::run(ctx, src, cdda, part, &["sfx", "music", "video", "pictures"])?,
            }
        }
    }
    Ok(())
}
