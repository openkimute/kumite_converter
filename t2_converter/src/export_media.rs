//! Tekken 2 (USA v1.1, SLUS-00213) sounds, music, movies and pictures to browser-friendly files.
//!
//! usage: t2_convert export_media "Tekken 2 (USA) (v1.1) (Track 1).bin" OUTDIR [sfx,music,video,pictures]
//!
//!   OUTDIR/sfx/<bank>/NN.wav   VAB banks: records 0 + 1 and 2 + 3 (header, body), the one bank at the start of
//!                              record 5, and each character block's +3 (header) + +6 (body); SPU ADPCM to WAV
//!                              at the rate the tones play it (ps1::export_bank).
//!   OUTDIR/music/NN.flac       the 56 music streams of the EXE's XA table (0x80011490: first, last sector,
//!                              channel), named by the BGM table 0x80011888; plus CD-DA tracks 2 and 3.
//!   OUTDIR/video/NN.avi, .flac the 28 STR movies in TEKKEN2.XAS (video and XA audio on channel 1), each starting at
//!                              frame 1 chunk 0, named by the title (record 17) and ending (record 20) tables:
//!                              Motion-JPEG frames and the soundtrack (psx_media), as Tekken 3's.
//!   OUTDIR/pictures/rNNN/*.png every TIM in the EXE and the system records 0-23 (plain, and the LZ streams
//!                              0x8006C46C unpacks) with its own CLUT, and each stage's panorama
//!                              (export_stages::strip), and each block's hit spark frames ("nlz ").
//!   OUTDIR/index.json          what the Tekken 2 viewer lists.

use crate::export_characters::{self as t2, Disc, block_name, load_json, write_json};
use crate::export_stages as st;
use crate::ps1::{
    Iso, LZ_LIMIT, RAW, Rgba, XA_SAMPLES_PER_SECTOR, export_bank, find_movies, i16_, i32_, lz, parse_vab_header,
    py_round, render, scan_lz_tims, u32_, walk_tims,
};
use crate::{pil, pyjson};
use anyhow::{Context, Result, bail};
use conv_io::{Ctx, DiscFile, StdDisc, StdFs, Vfs, join};
use psx_media::{Pcm, decode_xa, flac, str_to_avi};
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Read;

// ---- sounds ---------------------------------------------------------------------------------------
fn export_sfx(disc: &Disc, names: &HashMap<i64, String>, out: &dyn Vfs, outdir: &str) -> Result<Vec<Value>> {
    let mut banks = Vec::new();
    for (name, key, h, b) in
        [("System bank A (records 0, 1)", "r000", 0, 1), ("System bank B (records 2, 3)", "r002", 2, 3)]
    {
        banks.push(export_bank(name, disc.rec(h)?, disc.rec(b)?, out, outdir, key)?);
    }
    let x = disc.rec(5)?;
    if let Some(i) = x.windows(4).position(|w| w == b"pBAV") {
        let (_, sizes, hlen) = parse_vab_header(&x[i..])?;
        let body = crate::ps1::py_slice(x, i + hlen, i + hlen + sizes.iter().sum::<usize>());
        banks.push(export_bank("Record 5 bank", &x[i..], body, out, outdir, "r005")?);
    }
    for b in 0..t2::BLOCKS {
        let base = t2::BLOCK0 + 8 * b;
        let (hdr, body) = (disc.rec(base + 3)?, disc.rec(base + 6)?);
        if hdr.get(..4) != Some(b"pBAV") {
            continue;
        }
        banks.push(export_bank(&block_name(b, names), hdr, body, out, outdir, &format!("block_{b:02}"))?);
    }
    Ok(banks)
}

// ---- music and movies -----------------------------------------------------------------------------
const XA_TABLE: u32 = 0x80011490; // 84 x {s32 first, s32 last, s32 channel} (XAS-relative sectors; then the movies)
const XA_MUSIC: u32 = 56;

/// (channel, first, last) of the music streams, from the EXE's XA table.
pub fn xa_streams(disc: &Disc) -> Result<Vec<(i32, i32, i32)>> {
    (0..XA_MUSIC)
        .map(|k| Ok((disc.i32(XA_TABLE + 12 * k + 8)?, disc.i32(XA_TABLE + 12 * k)?, disc.i32(XA_TABLE + 12 * k + 4)?)))
        .collect()
}

pub const BGM_TABLE: u32 = 0x80011888; // 30 BGMs x 2 sets (0x800A3701) x {u8 1 = CD audio, u8 volume, s16 XA stream} (0x80012E50)
pub const CHAR_INFO: u32 = 0x800A39CC; // per block: pointer to {..., +6 home stage, +7 BGM}

fn bgm_use(bgm: u32) -> String {
    match bgm {
        15 => "BGM 15 (0x80093424)".into(),
        16 => "BGM 16 (record 20)".into(),
        17 => "BGM 17 (record 21)".into(),
        18 => "BGM 18 (record 21)".into(),
        _ => format!("BGM {bgm}"),
    }
}

/// A BGM table entry (set v): (CD audio, volume, XA stream or CD track - 2).
pub fn bgm_entry(disc: &Disc, bgm: u32, v: u32) -> Result<(u8, u8, i16)> {
    let a = BGM_TABLE + 8 * bgm + 4 * v;
    Ok((disc.u8(a)?, disc.u8(a + 1)?, disc.i16(a + 2)?))
}

/// XA stream / CD track -> what plays it: the stage BGM of the characters whose +7 names it, per set. Keys:
/// (CD audio, stream or track).
fn music_names(disc: &Disc) -> Result<HashMap<(bool, i64), Vec<String>>> {
    let stage_name: HashMap<usize, String> = st::stages(disc)?.into_iter().map(|(k, _, n)| (k, n)).collect();
    let mut users: HashMap<u8, BTreeSet<usize>> = HashMap::new();
    for b in 0..48 {
        let p = disc.u32(CHAR_INFO + 4 * b)?;
        let (stage, bgm) = (disc.u8(p + 6)?, disc.u8(p + 7)?);
        users.entry(bgm).or_default().insert(stage as usize);
    }
    let mut out: HashMap<(bool, i64), Vec<String>> = HashMap::new();
    for bgm in 0..30u32 {
        let what = users
            .get(&(bgm as u8))
            .map(|s| {
                s.iter()
                    .map(|k| stage_name.get(k).cloned().with_context(|| format!("stage {k} has no home character")))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .map(|v| v.join(" / "))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| bgm_use(bgm));
        for v in 0..2 {
            let (cd, vol, xa) = bgm_entry(disc, bgm, v)?;
            let key = if cd != 0 { (true, xa as i64 + 2) } else { (false, xa as i64) };
            if xa >= 0 && (cd == 0 || vol != 0) {
                out.entry(key).or_default().push(format!("{what} (set {})", v + 1));
            }
        }
    }
    Ok(out)
}

fn raw_sector(iso: &mut Iso, lba: u64) -> Result<Vec<u8>> {
    let s = iso.at(lba * RAW, RAW)?;
    if s.len() < 32 {
        bail!("sector {lba} is past the end of the image");
    }
    Ok(s)
}

/// One XA music stream (channel, first, last sector) as a FLAC; its sector count.
pub fn export_stream(
    iso: &mut Iso,
    xas_lba: u32,
    (ch, a, b): (i32, i32, i32),
    out: &dyn Vfs,
    path: &str,
) -> Result<usize> {
    let mut secs = Vec::new();
    for i in a..=b {
        let s = raw_sector(iso, (xas_lba as i64 + i as i64) as u64)?;
        if s[17] as i32 == ch && s[18] & 4 != 0 {
            secs.push(s);
        }
    }
    out.write(path, &flac(&decode_xa(&secs)?)?)?;
    Ok(secs.len())
}

fn export_music(
    ctx: &Ctx,
    iso: &mut Iso,
    xas_lba: u32,
    cdda: &[Option<Box<dyn DiscFile>>; 2],
    outdir: &str,
    disc: &Disc,
    names: &HashMap<(bool, i64), Vec<String>>,
) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for (k, (ch, a, b)) in xa_streams(disc)?.into_iter().enumerate() {
        let n = export_stream(iso, xas_lba, (ch, a, b), ctx.out, &join(outdir, &format!("{k:02}.flac")))?;
        let use_ = names.get(&(false, k as i64)).map_or("not played by the BGM table".to_string(), |v| v.join(", "));
        out.push(json!({"name": format!("{use_} · XA {:02} (channel {ch}, sectors {a}-{b})", k + 1), "file": format!("{k:02}.flac"),
                        "seconds": py_round((n * XA_SAMPLES_PER_SECTOR) as f64 / 37800.0, 1)}));
        ctx.log(&format!("music {k} {ch} {a} {b}"));
    }
    for (n, track) in [2, 3].into_iter().zip(cdda) {
        if let Some(track) = track {
            let mut pcm = Vec::new();
            track.open()?.read_to_end(&mut pcm).with_context(|| format!("cannot read {}", track.name()))?;
            let file = format!("cdda_track{n}.flac");
            let samples = pcm.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
            ctx.out.write(&join(outdir, &file), &flac(&Pcm { rate: 44100, channels: 2, samples })?)?;
            let size = pcm.len() as f64;
            out.push(json!({"name": format!("{} · CD audio track {n}", names.get(&(true, n)).map_or(String::new(), |v| v.join(", "))),
                            "file": file, "seconds": py_round(size / RAW as f64 * 588.0 / 44100.0, 1)}));
        }
    }
    Ok(out)
}

pub const OVERLAY_BASE: u32 = 0x801BDFD8; // where records 5, 17, 19, 20, 21 run
const TITLE_MOVIES: (usize, u32) = (17, 0x801E9F10); // record 17: s32 [2][2] movie by (title part, music set 0x800A3701)
pub const ENDING_MOVIES: (usize, u32) = (20, 0x801D00E4); // record 20: s16 [48] ending movie by block

/// The ending movie of a block (record 20's table).
pub fn ending_movie(disc: &Disc, block: usize) -> Result<i16> {
    let (r, a) = ENDING_MOVIES;
    i16_(disc.rec(r)?, (a - OVERLAY_BASE) as usize + 2 * block)
}

fn movie_names(disc: &Disc) -> Result<HashMap<i64, Vec<String>>> {
    let names = disc.names()?;
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    let (r, a) = TITLE_MOVIES;
    for part in 0..2 {
        for v in 0..2 {
            let m = i32_(disc.rec(r)?, (a - OVERLAY_BASE) as usize + 8 * part + 4 * v)?;
            out.entry(m as i64).or_default().push(format!(
                "{} (music set {})",
                if part == 0 { "Opening" } else { "Title movie 2" },
                v + 1
            ));
        }
    }
    for b in 0..t2::BLOCKS {
        let m = ending_movie(disc, b)?;
        let bn = block_name(b, &names);
        let n = format!("Ending: {}", bn.split(" (").next().unwrap_or(""));
        let e = out.entry(m as i64).or_default();
        if !e.contains(&n) {
            e.push(n);
        }
    }
    Ok(out)
}

fn movie_name(k: usize, names: &HashMap<i64, Vec<String>>) -> String {
    let mut n = names.get(&(k as i64)).cloned().unwrap_or_else(|| vec![format!("Movie {}", k + 1)]);
    let head = |s: &str| s.split(" (music").next().unwrap_or("").to_string();
    if n.len() == 2 && head(&n[0]) == head(&n[1]) {
        n = vec![head(&n[0])]; // the same movie for both music sets
    }
    n.join(" / ")
}

fn export_video(
    ctx: &Ctx,
    iso: &mut Iso,
    xas_lba: u32,
    nsec: u32,
    outdir: &str,
    names: &HashMap<i64, Vec<String>>,
) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for (k, (a, b)) in find_movies(iso, xas_lba, nsec)?.into_iter().enumerate() {
        let (mut secs, mut frames, mut audio) = (Vec::new(), HashSet::new(), 0usize);
        for i in a..=b {
            let s = raw_sector(iso, (xas_lba + i) as u64)?;
            if s[17] != 1 {
                continue;
            }
            if s[18] & 4 != 0 {
                audio += 1;
            } else if s[24..28] == [0x60, 0x01, 0x01, 0x80] {
                frames.insert(u32_(&s, 32)?);
            }
            secs.push(s);
        }
        let seconds = (audio * XA_SAMPLES_PER_SECTOR) as f64 / 37800.0;
        let rate = frames.len() as f64 / seconds.max(1e-6);
        let fps = [15, 20, 30]
            .into_iter()
            .fold(15, |b, r| if (r as f64 - rate).abs() < (b as f64 - rate).abs() { r } else { b });
        ctx.out.write(&join(outdir, &format!("{k:02}.avi")), &str_to_avi(&secs, fps, 85)?)?;
        ctx.out.write(&join(outdir, &format!("{k:02}.flac")), &flac(&decode_xa(&secs)?)?)?;
        let name = movie_name(k, names);
        out.push(json!({"name": name, "file": format!("{k:02}.avi"), "audio_file": format!("{k:02}.flac"),
                        "fps": fps, "frames": frames.len(), "seconds": py_round(seconds, 1)}));
        ctx.log(&format!("video {k:02} sectors {a}-{b} frames {} fps {fps} {seconds:.1}s", frames.len()));
    }
    Ok(out)
}

// ---- pictures -------------------------------------------------------------------------------------
/// A block's stage panorama, one period of its rows as the game lays them (export_stages::strip).
fn panorama(disc: &Disc, block: usize) -> Result<Option<Rgba>> {
    let base = t2::BLOCK0 + 8 * block;
    let info = st::describe(disc.rec(base + 1)?)?;
    Ok(st::strip(&info.rows, &mut st::Pages::new(st::vram(disc.rec(base)?)?))?.map(|s| s.0))
}

struct Saver<'a> {
    out: &'a dyn Vfs,
    outdir: &'a str,
    seen: HashSet<Vec<u8>>,
}

impl Saver<'_> {
    fn save(&mut self, key: &str, n: &str, rgba: &Rgba, meta: Value) -> Result<Option<Value>> {
        if !self.seen.insert(rgba.px.clone()) {
            return Ok(None);
        }
        let file = format!("{key}/{n}.png");
        pil::save_png(rgba, self.out, &join(self.outdir, &file))?;
        let mut e = Map::new();
        e.insert("file".into(), json!(file));
        e.insert("w".into(), json!(rgba.w));
        e.insert("h".into(), json!(rgba.h));
        if let Value::Object(m) = meta {
            e.extend(m);
        }
        Ok(Some(Value::Object(e)))
    }
}

fn export_pictures(ctx: &Ctx, disc: &Disc, names: &HashMap<i64, String>, outdir: &str) -> Result<Vec<Value>> {
    let mut sv = Saver { out: ctx.out, outdir, seen: HashSet::new() };
    let mut sets = Vec::new();
    for r in std::iter::once(None).chain((0..t2::BLOCK0).map(Some)) {
        let d = match r {
            None => &disc.exe[..],
            Some(r) => disc.rec(r)?,
        };
        let key = r.map_or("exe".to_string(), |r| format!("r{r:03}"));
        let mut imgs = Vec::new();
        let raw = walk_tims(d).into_iter().map(|t| (t, false));
        let packed = scan_lz_tims(d).into_iter().map(|(_, _, t)| (t, true));
        for (t, lz) in raw.chain(packed).filter(|(t, _)| t.img.is_some()) {
            let Some(rgba) = render(&t)? else { continue };
            if rgba.w * rgba.h < 16 * 16 || !rgba.px.as_chunks::<4>().0.iter().any(|p| p[3] != 0) {
                continue;
            }
            let img = t.img.as_ref().context("a TIM without its image")?;
            let meta = json!({"bpp": t.bpp, "vram": [img.x, img.y], "packed": lz});
            if let Some(e) = sv.save(&key, &format!("{:03}", imgs.len()), &rgba, meta)? {
                imgs.push(e);
            }
        }
        let n = imgs.len();
        if !imgs.is_empty() {
            sets.push(json!({"name": r.map_or("EXE".to_string(), |r| format!("Record {r}")), "images": imgs}));
        }
        ctx.log(&format!("pictures record {} {n}", r.map_or("exe".to_string(), |r| r.to_string())));
    }
    let mut imgs = Vec::new();
    for (k, b, name) in st::stages(disc)? {
        if let Some(p) = panorama(disc, b)?
            && let Some(e) = sv.save("stages", &format!("stage_{k:02}"), &p, json!({"name": name}))?
        {
            imgs.push(e);
        }
    }
    sets.push(json!({"name": "Stage panoramas", "images": imgs}));
    let mut imgs = Vec::new();
    for b in 0..t2::BLOCKS {
        let frames = nlz_frames(disc.rec(t2::BLOCK0 + 8 * b + 4)?)?;
        if frames.is_empty() {
            continue;
        }
        // np.concatenate(frames, 1)
        let h = frames[0].h;
        if frames.iter().any(|f| f.h != h) {
            bail!("block {b}'s spark frames differ in height");
        }
        let mut all = Rgba::new(frames.iter().map(|f| f.w).sum(), h);
        let mut x = 0;
        for f in &frames {
            all.paste(x, 0, f.w, h, f, 0, 0)?;
            x += f.w;
        }
        let meta =
            json!({"name": format!("{} · {} frames", block_name(b, names), frames.len()), "frames": frames.len()});
        if let Some(e) = sv.save("sparks", &format!("block_{b:02}"), &all, meta)? {
            imgs.push(e);
        }
    }
    sets.push(json!({"name": "Hit sparks (block +4, \"nlz \")", "images": imgs}));
    Ok(sets)
}

/// Block +4: "nlz ", u32 size, an LZ stream (0x8006C46C) of 32 x 32 4-bit TIMs: the character's hit spark frames,
/// uploaded one by one by 0x80087280 / 0x800873B8 / 0x80087518 at the offsets they are given.
fn nlz_frames(rec: &[u8]) -> Result<Vec<Rgba>> {
    if rec.get(..4) != Some(b"nlz ") {
        return Ok(vec![]);
    }
    let (out, _) = lz(rec, 4, LZ_LIMIT).map_err(|_| anyhow::anyhow!("an \"nlz \" stream runs past its record"))?;
    walk_tims(&out.unwrap_or_default())
        .iter()
        .filter(|t| t.img.is_some())
        .map(|t| render(t)?.context("a 24-bit spark frame"))
        .collect()
}

/// The CD-audio tracks 2 and 3 beside track 1, when they are there.
pub fn cdda_beside(src: &str) -> [Option<Box<dyn DiscFile>>; 2] {
    [2, 3].map(|n| {
        let track = src.replace("Track 1", &format!("Track {n}"));
        (track != src && std::path::Path::new(&track).exists())
            .then(|| Box::new(StdDisc(track.into())) as Box<dyn DiscFile>)
    })
}

pub fn main(args: &[String]) -> Result<()> {
    let [src, out, rest @ ..] = args else {
        bail!("usage: t2_convert export_media BIN OUTDIR [sfx,music,video,pictures]")
    };
    let what: Vec<&str> = match rest.first() {
        Some(s) => s.split(',').collect(),
        None => vec!["sfx", "music", "video", "pictures"],
    };
    let ctx = Ctx { out: &StdFs(out.into()), log: &conv_io::print_line, jobs: 1 };
    run(&ctx, &StdDisc(src.into()), &cdda_beside(src), "", &what)
}

/// Sounds, music (with the CD-audio tracks 2 and 3), movies and pictures (`what` of those) in `dir` of the output,
/// with index.json.
pub fn run(
    ctx: &Ctx,
    src: &dyn DiscFile,
    cdda: &[Option<Box<dyn DiscFile>>; 2],
    dir: &str,
    what: &[&str],
) -> Result<()> {
    let ipath = join(dir, "index.json");
    let mut index = if ctx.out.exists(&ipath) { load_json(ctx.out, &ipath)? } else { json!({}) };
    let idx = index.as_object_mut().context("index.json is not an object")?;
    let disc = Disc::open(src)?;
    let names = disc.names()?;
    let mut iso = Iso::open(src)?;
    let (xas_lba, xas_size) = iso.find("TEKKEN2/TEKKEN2.XAS")?;
    let nsec = xas_size / 2048;
    if what.contains(&"sfx") {
        let banks = export_sfx(&disc, &names, ctx.out, &join(dir, "sfx"))?;
        let n: usize = banks.iter().map(|b| b["samples"].as_array().map_or(0, Vec::len)).sum();
        ctx.log(&format!("sfx banks {} samples {n}", banks.len()));
        idx.insert("sfx".into(), json!(banks));
    }
    if what.contains(&"music") {
        let music = export_music(ctx, &mut iso, xas_lba, cdda, &join(dir, "music"), &disc, &music_names(&disc)?)?;
        idx.insert("music".into(), json!(music));
    }
    if what.contains(&"video") {
        idx.insert(
            "video".into(),
            json!(export_video(ctx, &mut iso, xas_lba, nsec, &join(dir, "video"), &movie_names(&disc)?)?),
        );
    }
    if what.contains(&"pictures") {
        idx.insert("pictures".into(), json!(export_pictures(ctx, &disc, &names, &join(dir, "pictures"))?));
    }
    write_json(ctx.out, &ipath, &index, pyjson::INDENT1)
}
