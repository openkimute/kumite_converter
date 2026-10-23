//! export_media.py: Tekken 3 (USA, SLUS-00402) sound effects, music and movies to browser-friendly files.
//!
//! usage: t3_convert export_media "Tekken_3_(USA)_(Track 1).bin" OUTDIR [sfx,music,video]
//!
//!   OUTDIR/sfx/<bank>/NN.wav   VAB sound banks: each character's header (BNS record m+1) with its
//!                              sample body (part 2 of the texture container m+2), plus the one-sample
//!                              bank in record 301. PS1 SPU ADPCM decoded to 16-bit mono WAV, at the
//!                              rate the game plays it: libsnd keys note = tone + 60 with
//!                              fine 0, so rate = 44100 * 2^((key - centre + shift/128) / 12) (shift = tone fine-tune).
//!                              The system SFX and announcer banks are uncompressed inside the EXE (header 0x80025850 /
//!                              0x80026A70, bodies 0x800D366C / 0x8011001C).
//!   OUTDIR/music/NN.flac       the 50 XA streams listed in the EXE (0x15578: start, end, channel;
//!                              XAS-relative sectors; 37.8 kHz) plus CD-DA track 2 (track 3 is silence).
//!   OUTDIR/video/NN.avi, .flac the 22 STR movies in TEKKEN3.XAS (MDEC video + XA audio on channel 1), found by
//!                              scanning for STR frames whose frame counter restarts: Motion-JPEG frames and the
//!                              soundtrack (psx_media). The frame rate (15, 20 or 30) is frames / audio length.
//!   OUTDIR/index.json          what the viewer lists; each SFX sample also lists the VAB tones that play it
//!                              ([program, min key, max key]) so engines can map SePlay codes to files.

use crate::disc::{BNS_COUNT, BNS_TABLE_OFF, Iso, py_slice, u32le};
use crate::export_gltf::NAMES;
use crate::pyjson;
use anyhow::{Context, Result, bail};
use conv_io::{Ctx, DiscFile, StdDisc, StdFs, Vfs, join, print_line};
use psx_media::{Pcm, decode_xa, flac, str_to_avi};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::io::Read;

/// raw CD sector
const RAW: u64 = 2352;
/// EXE file offsets
const XA_TABLE: usize = 0x15578;
const XA_COUNT: usize = 50;
/// 37800 Hz stereo 4-bit: 18 groups x 112 sample pairs
const XA_SAMPLES_PER_SECTOR: f64 = 2016.0;
/// identified visually
const MOVIE_NAMES: [&str; 22] = [
    "Paul ending",
    "Forest Law ending",
    "Lei Wulong ending",
    "King ending",
    "Yoshimitsu ending",
    "Nina ending",
    "Hwoarang ending",
    "Ling Xiaoyu ending",
    "Eddy Gordo ending",
    "Jin Kazama ending",
    "Julia Chang ending",
    "Kuma / Panda ending",
    "Bryan Fury ending",
    "Heihachi ending",
    "Ogre / True Ogre ending",
    "Mokujin ending",
    "Gun Jack ending",
    "Anna Williams ending",
    "Gon ending",
    "Tiger Jackson ending",
    "Opening",
    "Prologue (Jin / Jun / Kazuya / Ogre)",
];

/// Python's `round(x, n)` (correctly rounded, ties to even, as Rust's formatting).
pub fn py_round(x: f64, n: usize) -> f64 {
    format!("{x:.n$}").parse().unwrap_or(x)
}

fn u16le(d: &[u8], o: usize) -> Result<u16> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).context("a VAB header is cut short")
}

// ---- sound effects ---------------------------------------------------------------------------
const ADPCM_K: [(i32, i32); 5] = [(0, 0), (60, 0), (115, -52), (98, -55), (122, -60)];

/// PS1 SPU ADPCM (16-byte blocks: shift/filter, flags, 14 data bytes) -> int16 samples.
pub fn spu_adpcm(b: &[u8]) -> Vec<i16> {
    let mut out = Vec::new();
    let (mut s1, mut s2) = (0i32, 0i32);
    for (n, blk) in b.as_chunks::<16>().0.iter().enumerate() {
        let (sh, f, flag) = (blk[0] & 15, ((blk[0] >> 4) & 7).min(4), blk[1]);
        let (k0, k1) = ADPCM_K[f as usize];
        for &byte in &blk[2..] {
            for nib in [byte & 15, byte >> 4] {
                let v = ((nib as i32) << 12) as i16 as i32;
                let s = ((v >> sh) + (s1 * k0 + s2 * k1 + 32).div_euclid(64)).clamp(-32768, 32767);
                out.push(s as i16);
                (s2, s1) = (s1, s);
            }
        }
        if flag & 1 != 0 && n != 0 && flag & 2 == 0 {
            break; // end of a one-shot sample
        }
    }
    out
}

/// One VAB tone: (program, min key, centre, fine, vag index, max key, tone index in the program: what SePlay's code
/// names, SsUtKeyOnV 0x800885D8).
type Tone = (usize, u8, u8, u8, usize, u8, usize);

/// pBAV v7 header -> (tones, sample sizes, header length).
fn parse_vab_header(d: &[u8]) -> Result<(Vec<Tone>, Vec<usize>, usize)> {
    let (ps, vs) = (u16le(d, 18)? as usize, u16le(d, 22)? as usize);
    let o = 32 + 2048;
    let mut tones = Vec::new();
    for p in 0..ps {
        for t in 0..16 {
            let a = d.get(o + (p * 16 + t) * 32..o + (p * 16 + t) * 32 + 24).context("a VAB header is cut short")?;
            let vag = i16::from_le_bytes([a[22], a[23]]);
            if vag > 0 {
                tones.push((p, a[6], a[4], a[5], vag as usize, a[7], t));
            }
        }
    }
    let at = o + ps * 512;
    let mut sizes = Vec::new();
    for i in 1..(vs + 1).min(256) {
        sizes.push(u16le(d, at + 2 * i)? as usize * 8);
    }
    u16le(d, at + 510)?; // struct.unpack_from('<256H') needs the whole table
    Ok((tones, sizes, at + 512))
}

/// What Python's `wave` module writes for 16-bit mono.
fn write_wav(out: &dyn Vfs, path: &str, x: &[i16], rate: f64) -> Result<()> {
    let n = (x.len() * 2) as u32;
    let rate = rate.round_ties_even() as u32;
    let mut w = Vec::with_capacity(44 + n as usize);
    w.extend(b"RIFF");
    w.extend((36 + n).to_le_bytes());
    w.extend(b"WAVEfmt ");
    w.extend(16u32.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(rate.to_le_bytes());
    w.extend((rate * 2).to_le_bytes());
    w.extend(2u16.to_le_bytes());
    w.extend(16u16.to_le_bytes());
    w.extend(b"data");
    w.extend(n.to_le_bytes());
    for s in x {
        w.extend(s.to_le_bytes());
    }
    out.write(path, &w)
}

fn export_bank(name: &str, header: &[u8], body: &[u8], out: &dyn Vfs, outdir: &str, key: &str) -> Result<Value> {
    let (tones, sizes, _) = parse_vab_header(header)?;
    let mut offs = vec![0usize];
    for s in &sizes {
        offs.push(offs[offs.len() - 1] + s);
    }
    let mut rate_of = std::collections::HashMap::new();
    let mut keys_of: std::collections::HashMap<usize, Vec<Value>> = std::collections::HashMap::new();
    for &(p, k, centre, fine, vag, kmax, t) in &tones {
        rate_of.entry(vag).or_insert_with(|| {
            44100.0 * libm::pow(2.0, ((k as i32 - centre as i32) as f64 + fine as f64 / 128.0) / 12.0)
        });
        keys_of.entry(vag).or_default().push(json!([p, k, kmax, t]));
    }
    let mut samples = Vec::new();
    for i in 0..sizes.len() {
        let x = spu_adpcm(py_slice(body, offs[i], offs[i + 1]));
        if x.len() < 32 || x.iter().map(|s| s.wrapping_abs()).max() == Some(0) {
            continue;
        }
        let rate = rate_of.get(&(i + 1)).copied().unwrap_or(22050.0);
        let fn_ = format!("{key}/{:02}.wav", i + 1);
        write_wav(out, &join(outdir, &fn_), &x, rate)?;
        // tones: [program, min key, max key, tone index] that play this sample (SePlay's code names the program and
        // the tone index; its key note = tone + 60)
        samples.push(json!({"file": fn_, "rate": rate.round_ties_even() as i64,
            "seconds": py_round(x.len() as f64 / rate, 2), "tones": keys_of.get(&(i + 1)).cloned().unwrap_or_default()}));
    }
    Ok(json!({"name": name, "samples": samples}))
}

const EXE_BASE: usize = 0x80010000 - 0x800;
const SYSTEM_BANKS: [(&str, &str, usize, usize); 2] =
    [("System sounds (hits, menus)", "sys", 0x80025850, 0x800D366C), ("Announcer", "ann", 0x80026A70, 0x8011001C)];

fn export_sfx(rec: &[Vec<u8>], out: &dyn Vfs, outdir: &str, exe: &[u8]) -> Result<Vec<Value>> {
    let mut banks = Vec::new();
    for (name, key, h, b) in SYSTEM_BANKS {
        let hdr = py_slice(exe, h - EXE_BASE, usize::MAX);
        let (_, sizes, _) = parse_vab_header(hdr)?;
        let body = py_slice(exe, b - EXE_BASE, b - EXE_BASE + sizes.iter().sum::<usize>());
        banks.push(export_bank(name, hdr, body, out, outdir, key)?);
    }
    for m in (71..276).step_by(4) {
        let (hdr, tex) = (&rec[m + 1], &rec[m + 2]);
        if !hdr.starts_with(b"pBAV") {
            continue;
        }
        if tex.len() < 28 {
            bail!("record {}'s container is cut short", m + 2);
        }
        let (o, z) = (u32le(tex, 20) as usize, u32le(tex, 24) as usize);
        let body = py_slice(tex, o, o.saturating_add(z)); // part 2: (offset, size) pairs
        let name = NAMES.get(&m).cloned().unwrap_or_else(|| format!("Model {m}"));
        banks.push(export_bank(&name, hdr, body, out, outdir, &format!("m{m:03}"))?);
    }
    let d = &rec[301];
    if let Some(i) = d.windows(4).position(|w| w == b"pBAV") {
        let (_, sizes, hlen) = parse_vab_header(&d[i..])?;
        let body = py_slice(d, i + hlen, i + hlen + sizes.iter().sum::<usize>());
        banks.push(export_bank("Record 301 (single sample)", &d[i..], body, out, outdir, "r301")?);
    }
    Ok(banks)
}

// ---- music & movies --------------------------------------------------------------------------
/// One raw sector of the image (short or empty past its end).
fn sector(iso: &mut Iso, lba: u64) -> Result<Vec<u8>> {
    iso.raw(lba, 1)
}

fn export_music(
    iso: &mut Iso,
    xas_lba: u64,
    exe: &[u8],
    track2: Option<&dyn DiscFile>,
    out: &dyn Vfs,
    outdir: &str,
) -> Result<Vec<Value>> {
    let mut music = Vec::new();
    for k in 0..XA_COUNT {
        let at = XA_TABLE + 12 * k;
        if at + 12 > exe.len() {
            bail!("the EXE is too short for its XA table");
        }
        let (a, b, ch) = (u32le(exe, at), u32le(exe, at + 4), u32le(exe, at + 8));
        let mut secs = Vec::new();
        for i in a as u64..=b as u64 {
            let s = sector(iso, xas_lba + i)?;
            if s.len() < 19 {
                bail!("the XA stream {k} runs past the disc image");
            }
            if s[17] as u32 == ch && s[18] & 4 != 0 {
                secs.push(s);
            }
        }
        out.write(&join(outdir, &format!("{k:02}.flac")), &flac(&decode_xa(&secs)?)?)?;
        music.push(json!({"name": format!("XA stream {:02} (channel {ch})", k + 1), "file": format!("{k:02}.flac"),
            "seconds": py_round(secs.len() as f64 * XA_SAMPLES_PER_SECTOR / 37800.0, 1)}));
    }
    if let Some(track2) = track2 {
        let mut pcm = Vec::new();
        track2.open()?.read_to_end(&mut pcm).with_context(|| format!("cannot read {}", track2.name()))?;
        let n = pcm.len() as u64 / RAW;
        let samples = pcm.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
        out.write(&join(outdir, "cdda_track2.flac"), &flac(&Pcm { rate: 44100, channels: 2, samples })?)?;
        music.push(json!({"name": "CD audio track 2", "file": "cdda_track2.flac",
            "seconds": py_round(n as f64 * 588.0 / 44100.0, 1)}));
    }
    Ok(music)
}

/// STR movies: each starts at a video sector with frame 1, chunk 0; ends where the next starts.
fn find_movies(iso: &mut Iso, xas_lba: u64, nsec: u64) -> Result<Vec<(u64, u64)>> {
    let (mut starts, mut last) = (Vec::new(), None);
    for i in 0..nsec {
        let s = sector(iso, xas_lba + i)?;
        let Some(h) = s.get(16..40) else { continue };
        if h[2] & 4 != 0 || h[8..12] != [0x60, 0x01, 0x01, 0x80] {
            continue;
        }
        let (chunk, frame) = (u16::from_le_bytes([h[12], h[13]]), u32le(h, 16));
        if chunk == 0 && frame == 1 {
            starts.push(i);
        }
        last = Some(i);
    }
    Ok(starts.iter().enumerate().map(|(j, &s)| (s, starts.get(j + 1).map_or(last.unwrap_or(s), |n| n - 1))).collect())
}

fn export_video(ctx: &Ctx, iso: &mut Iso, xas_lba: u64, nsec: u64, outdir: &str) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for (k, (a, b)) in find_movies(iso, xas_lba, nsec)?.into_iter().enumerate() {
        let (mut secs, mut frames, mut audio) = (Vec::new(), BTreeSet::new(), 0u32);
        for i in a..=b {
            let s = sector(iso, xas_lba + i)?;
            if s.len() < 36 {
                bail!("movie {k} runs past the disc image");
            }
            if s[17] != 1 {
                continue; // movie video + audio live on channel 1
            }
            if s[18] & 4 != 0 {
                audio += 1;
            } else if s[24..28] == [0x60, 0x01, 0x01, 0x80] {
                frames.insert(u32le(&s, 32));
            }
            secs.push(s);
        }
        let seconds = audio as f64 * XA_SAMPLES_PER_SECTOR / 37800.0;
        let rate = frames.len() as f64 / seconds.max(1e-6);
        let mut fps = 15;
        for r in [20, 30] {
            if (r as f64 - rate).abs() < (fps as f64 - rate).abs() {
                fps = r;
            }
        }
        // the engine's copy: Motion-JPEG frames in an AVI (decoded by the engine itself, natively and in a
        // browser) and the soundtrack
        ctx.out.write(&join(outdir, &format!("{k:02}.avi")), &str_to_avi(&secs, fps, 85)?)?;
        ctx.out.write(&join(outdir, &format!("{k:02}.flac")), &flac(&decode_xa(&secs)?)?)?;
        let name = MOVIE_NAMES.get(k).map_or(format!("Movie {}", k + 1), |n| n.to_string());
        out.push(json!({"name": name, "file": format!("{k:02}.avi"), "audio_file": format!("{k:02}.flac"),
            "fps": fps, "frames": frames.len(), "seconds": py_round(seconds, 1)}));
        ctx.log(&format!("video {k:02} sectors {a}-{b} frames {} fps {fps} {seconds:.1}s", frames.len()));
    }
    Ok(out)
}

pub fn main(args: &[String]) -> Result<()> {
    let [src, out, rest @ ..] = args else {
        bail!("usage: t3_convert export_media \"Tekken_3_(USA)_(Track 1).bin\" OUTDIR [sfx,music,video]")
    };
    let what: Vec<&str> = rest.first().map_or(vec!["sfx", "music", "video"], |w| w.split(',').collect());
    let track2 = StdDisc(src.replace("Track 1", "Track 2").into());
    let track2 = track2.0.exists().then_some(&track2 as &dyn DiscFile);
    let ctx = Ctx { out: &StdFs(out.into()), log: &print_line, jobs: 1 };
    run(&ctx, &StdDisc(src.into()), track2, &what, &|p: &str| p.to_string())
}

/// Writes `what` (sfx, music, video) and index.json, each top-level name (sfx, music, video, index.json) at
/// `place(name)`; `track2`: the CD-audio track 2, if there.
pub fn run(
    ctx: &Ctx,
    disc: &dyn DiscFile,
    track2: Option<&dyn DiscFile>,
    what: &[&str],
    place: &dyn Fn(&str) -> String,
) -> Result<()> {
    let ipath = place("index.json");
    let mut index: Map<String, Value> = match ctx.out.read(&ipath) {
        Ok(d) => serde_json::from_slice(&d).with_context(|| format!("{ipath} is not a JSON object"))?,
        Err(_) => Map::new(),
    };
    let mut iso = Iso::open(disc)?;
    let exe = iso.read("TEKKEN3/SLUS_004.02")?;
    let (xas_lba, xas_size) = iso.find("TEKKEN3/TEKKEN3.XAS")?;
    let nsec = xas_size as u64 / 2048;
    if what.contains(&"sfx") {
        let bns = iso.read("TEKKEN3/TEKKEN3.BNS")?;
        let mut rec = Vec::with_capacity(BNS_COUNT);
        for i in 0..BNS_COUNT {
            let at = BNS_TABLE_OFF + 8 * i;
            if at + 8 > exe.len() {
                bail!("the EXE is too short for its BNS table");
            }
            let (s, z) = (u32le(&exe, at) as usize * 2048, u32le(&exe, at + 4) as usize);
            rec.push(py_slice(&bns, s, s + z).to_vec());
        }
        let banks = export_sfx(&rec, ctx.out, &place("sfx"), &exe)?;
        let n: usize = banks.iter().map(|b| b["samples"].as_array().map_or(0, Vec::len)).sum();
        ctx.log(&format!("sfx banks {} samples {n}", banks.len()));
        index.insert("sfx".into(), Value::Array(banks));
    }
    if what.contains(&"music") {
        let music = export_music(&mut iso, xas_lba as u64, &exe, track2, ctx.out, &place("music"))?;
        ctx.log(&format!("music tracks {}", music.len()));
        index.insert("music".into(), Value::Array(music));
    }
    if what.contains(&"video") {
        index.insert("video".into(), Value::Array(export_video(ctx, &mut iso, xas_lba as u64, nsec, &place("video"))?));
    }
    ctx.out.write(&ipath, pyjson::indent1(&Value::Object(index)).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_as_python() {
        // round(x, 2) / round(x, 1) as Python gives them
        assert_eq!(py_round(0.125, 2), 0.12);
        assert_eq!(py_round(2.675, 2), 2.67);
        assert_eq!(py_round(0.25, 1), 0.2);
        assert_eq!(py_round(0.05, 1), 0.1);
    }

    #[test]
    fn adpcm_one_shot_end() {
        // filter 0, shift 0: a nibble is the sample << 12; the second block flags the end (1, not loop 2)
        let mut b = vec![0u8; 48];
        b[2] = 0x21;
        b[17] = 1;
        let x = spu_adpcm(&b);
        assert_eq!(x.len(), 56);
        assert_eq!(&x[..2], &[4096, 8192]);
    }
}
