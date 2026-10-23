#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use std::process::Command;

/// A raw sector with `data` at +24 and the subheader's submode and coding bytes.
fn sector(submode: u8, coding: u8, data: &[u8]) -> Vec<u8> {
    let mut s = vec![0u8; 2352];
    (s[18], s[19]) = (submode, coding);
    s[24..24 + data.len()].copy_from_slice(data);
    s
}

#[test]
fn xa_nibbles_and_channels() {
    // filter 0, range 0: a sample is its nibble << 12; units alternate left and right
    let mut g = vec![0u8; 18 * 128];
    g[4..12].fill(0); // parameters
    g[16] = 0x21; // word 0: unit 0 nibble 1, unit 1 nibble 2
    g[16 + 4] = 0xf0; // word 1: unit 1 nibble -1
    let pcm = decode_xa(&[sector(4, 1, &g), sector(0, 0, &[])]).unwrap();
    assert_eq!((pcm.rate, pcm.channels, pcm.samples.len()), (37800, 2, 2016 * 2));
    assert_eq!(&pcm.samples[..4], &[4096, 8192, 0, -4096]);
    // filter 1 (60/64 of the last sample), mono 18.9 kHz: a 1 then zeros decays
    g[4] = 0x10;
    let pcm = decode_xa(&[sector(4, 4, &g)]).unwrap();
    assert_eq!((pcm.rate, pcm.channels), (18900, 1));
    assert_eq!(&pcm.samples[..3], &[4096, 3840, 3600]);
    assert!(decode_xa(&[sector(4, 1, &g), sector(4, 0, &g)]).is_err());
    assert!(decode_xa(&[vec![0u8; 100]]).unwrap().samples.is_empty()); // too short to be audio: skipped
}

#[test]
fn ac_codes_are_a_whole_prefix_code() {
    let codes: Vec<&str> = mdec_codes();
    for (i, a) in codes.iter().enumerate() {
        for b in &codes[i + 1..] {
            assert!(!a.starts_with(b) && !b.starts_with(a), "{a} {b}");
        }
    }
    // with the sign, end of block and escape, all but the 12-zero prefix (in units of 2^-17)
    let used: usize = codes.iter().map(|c| 1 << (17 - c.len())).sum();
    assert_eq!(used, (1 << 17) - (1 << 5));
}

fn mdec_codes() -> Vec<&'static str> {
    // the table's codes with either sign, as strings
    let signed: Vec<String> = mdec::AC.iter().flat_map(|(c, _, _)| [format!("{c}0"), format!("{c}1")]).collect();
    let mut v: Vec<&'static str> = signed.into_iter().map(|s| &*s.leak()).collect();
    v.extend(["10", "000001"]);
    v
}

#[test]
fn mdec_flat_frame() {
    // a 16x16 version 2 frame: every block only its DC (10 bits) and the end of block "10"
    let dcs = [64i32, -64, 100, 100, -100, 0]; // Cr, Cb, Y0-Y3
    let mut bits = String::new();
    for dc in dcs {
        bits += &format!("{:010b}10", dc as u32 & 0x3ff);
    }
    while !bits.len().is_multiple_of(16) {
        bits.push('0');
    }
    let mut d = vec![0, 0, 0, 0x38, 1, 0, 2, 0];
    for w in bits.as_bytes().chunks(16) {
        let v = w.iter().fold(0u16, |a, &c| a * 2 + (c - b'0') as u16);
        d.extend(v.to_le_bytes());
    }
    let [y, cb, cr] = mdec::decode_planes(&mdec::ac_table(), &d, 16, 16).unwrap();
    // DC·2/8 + 128
    assert_eq!((y[0], y[8], y[8 * 16], y[255]), (153, 153, 103, 128));
    assert_eq!((cr[0], cb[63]), (144, 112));
    // a frame in a sector, then an audio sector
    let mut h = vec![0x60, 0x01, 0x01, 0x80, 0, 0, 1, 0, 7, 0, 0, 0, 0, 0, 0, 0, 16, 0, 16, 0];
    h.resize(32, 0);
    h.extend(&d);
    let mut frames = Vec::new();
    decode_str(&[sector(8, 0, &h), sector(4, 1, &[])], |f| {
        frames.push(f);
        Ok(())
    })
    .unwrap();
    assert_eq!((frames.len(), frames[0].width, frames[0].rgb.len()), (1, 16, 16 * 16 * 3));
    assert!(decode_str(&[sector(8, 0, &h[..40])], |_| Ok(())).is_err()); // the bitstream ends early
}

/// The engine's AviIndex (OpenKumite's crates/kumite_assets/src/avi.rs), its essentials: avih's µs per frame and frame count, the
/// '##dc' chunks of the lists in order.
fn parse_avi(d: &[u8]) -> (u32, Option<usize>, Vec<(usize, usize)>) {
    let u32_at = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    assert!(&d[0..4] == b"RIFF" && &d[8..12] == b"AVI ");
    let (mut micros, mut total, mut frames) = (50_000, None, Vec::new());
    let mut stack = vec![(12usize, u32_at(4).unwrap() as usize + 8)];
    while let Some(&(at, end)) = stack.last() {
        let body = at + 8;
        if body > end {
            stack.pop();
            continue;
        }
        let (id, size) = (&d[at..at + 4], u32_at(at + 4).unwrap() as usize);
        let next = body + size + (size & 1);
        stack.last_mut().unwrap().0 = next;
        if id == b"LIST" {
            stack.push((body + 4, next.min(end)));
            continue;
        }
        assert!(body + size <= d.len());
        if id == b"avih" {
            micros = u32_at(body).unwrap();
            total = u32_at(body + 16).map(|n| n as usize).filter(|&n| n > 0);
        } else if matches!(id, [_, _, b'd', b'c' | b'b']) && size > 0 {
            frames.push((body, size));
        }
    }
    (micros, total, frames)
}

#[test]
fn avi_as_the_engine_reads_it() {
    let frames = vec![b"abc".to_vec(), b"defg".to_vec()];
    let a = avi(&frames, 320, 240, 15);
    let (micros, total, found) = parse_avi(&a);
    assert_eq!((micros, total), (66_666, Some(2)));
    assert_eq!(found.iter().map(|&(o, n)| &a[o..o + n]).collect::<Vec<_>>(), [&b"abc"[..], b"defg"]);
    assert_eq!(u32::from_le_bytes(a[64..68].try_into().unwrap()), 320); // avih width at +32 (body at +32)
}

#[test]
fn flac_round_trip() {
    use symphonia::core::{codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream, probe::Hint};
    let samples: Vec<i16> = (0..50_000).map(|i: i32| ((i * 37 % 2000 - 1000) * (i % 7)) as i16).collect();
    let f = flac(&Pcm { rate: 37800, channels: 2, samples: samples.clone() }).unwrap();
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(f)), Default::default());
    let mut probed = symphonia::default::get_probe()
        .format(&Hint::new(), mss, &FormatOptions::default(), &Default::default())
        .unwrap();
    let track = probed.format.default_track().unwrap();
    assert_eq!(track.codec_params.sample_rate, Some(37800));
    let mut dec = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).unwrap();
    let mut got = Vec::new();
    while let Ok(p) = probed.format.next_packet() {
        let buf = dec.decode(&p).unwrap();
        let mut s = symphonia::core::audio::SampleBuffer::<i16>::new(buf.capacity() as u64, *buf.spec());
        s.copy_interleaved_ref(buf);
        got.extend_from_slice(s.samples());
    }
    assert_eq!(got, samples);
}

/// T3_BIN (Tekken 3, or Tekken 2) and ffmpeg on PATH: the disc's STR movies (channel 1, from a frame 1 to the next)
/// decoded as ffmpeg does: pictures by PSNR, sound to within 1. `which` picks the movies.
fn compare_movies(which: impl Fn(usize) -> bool) {
    let Ok(bin) = std::env::var("T3_BIN") else { return };
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        return;
    }
    let disc = std::fs::read(bin).unwrap();
    let sectors = disc.as_chunks::<2352>().0;
    let mut starts = Vec::new();
    for (i, s) in sectors.iter().enumerate() {
        if s[18] & 4 == 0 && s[24..28] == [0x60, 0x01, 0x01, 0x80] && s[28..30] == [0, 0] && s[32..36] == [1, 0, 0, 0] {
            starts.push(i);
        }
    }
    for (k, &a) in starts.iter().enumerate().filter(|(k, _)| which(*k)) {
        let b = starts.get(k + 1).copied().unwrap_or(sectors.len());
        let secs: Vec<Vec<u8>> = sectors[a..b].iter().filter(|s| s[17] == 1).map(|s| s.to_vec()).collect();
        let tmp = std::env::temp_dir().join(format!("psx_media_test_{}_{k}.str", std::process::id()));
        let n: usize = secs.iter().map(Vec::len).sum();
        let mut cdxa =
            [&b"RIFF"[..], &(n as u32 + 36).to_le_bytes(), b"CDXAfmt ", &16u32.to_le_bytes(), &[0; 16]].concat();
        cdxa.extend([&b"data"[..], &(n as u32).to_le_bytes()].concat());
        cdxa.extend(secs.concat());
        std::fs::write(&tmp, cdxa).unwrap();
        let ff = |args: &[&str]| {
            let o = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&tmp).args(args).arg("-").output().unwrap();
            o.stdout
        };
        let yuv = ff(&["-map", "0:v", "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "yuvj420p"]);
        let pcm = ff(&["-map", "0:a", "-f", "s16le", "-ar", "37800"]);
        let _ = std::fs::remove_file(&tmp);

        let (mut worst, mut sum, mut count, mut at) = (f64::MAX, 0.0, 0, 0);
        decode_str(&secs, |f| {
            let (w, h) = (f.width as usize, f.height as usize);
            let n = w * h * 3 / 2;
            let theirs = yuv.get(at..at + n).expect("ffmpeg gave fewer frames");
            at += n;
            let (y, c) = theirs.split_at(w * h);
            let (cb, cr) = c.split_at(w * h / 4);
            let want = mdec::rgb(y, cb, cr, w, w, h);
            let mse =
                want.iter().zip(&f.rgb).map(|(&a, &b)| (a as f64 - b as f64).powi(2)).sum::<f64>() / want.len() as f64;
            let psnr = if mse == 0.0 { 99.0 } else { 10.0 * (255.0 * 255.0 / mse).log10() };
            (worst, sum, count) = (worst.min(psnr), sum + psnr, count + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(at, yuv.len(), "movie {k}: ffmpeg gave more frames");
        let ours = decode_xa(&secs).unwrap();
        let theirs: Vec<i16> = pcm.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
        let diff =
            |d: i32| ours.samples.iter().zip(&theirs).filter(|(a, b)| (**a as i32 - **b as i32).abs() > d).count();
        let off = diff(1);
        println!(
            "movie {k}: {count} frames, PSNR mean {:.1} dB, worst {worst:.1} dB; {} samples, {} differ, {off} by >1",
            sum / count as f64,
            ours.samples.len(),
            diff(0)
        );
        assert!(worst > 40.0, "movie {k}: a frame at {worst:.1} dB");
        assert_eq!((ours.samples.len(), off), (theirs.len(), 0), "movie {k}'s sound");
    }
}

#[test]
fn movies_as_ffmpeg_decodes_them() {
    compare_movies(|k| k == 0 || k == 20);
}

/// All of them: `T3_BIN=... cargo test --release -p psx_media -- --ignored --nocapture`.
#[test]
#[ignore]
fn all_movies_as_ffmpeg_decodes_them() {
    compare_movies(|_| true);
}
