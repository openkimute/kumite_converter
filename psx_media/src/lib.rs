//! PlayStation media with no game in it, in place of ffmpeg: XA ADPCM sound and STR (MDEC) movies from raw CD sectors
//! (2352 bytes each, as the converters gather them), FLAC to keep the sound and Motion-JPEG AVI to keep the pictures.
//! Written from the psx-spx documentation ("CDROM XA Audio ADPCM Compression", "CDROM File Video Contents STR",
//! "MDEC").

mod mdec;
pub use mdec::{Frame, decode_str};

use anyhow::{Context, Result, anyhow, bail};

/// 16-bit PCM, its channels interleaved.
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

/// The filters (old and older sample, /64): XA has the SPU's first four.
const XA_K: [(i32, i32); 4] = [(0, 0), (60, 0), (115, -52), (98, -55)];

/// The XA ADPCM sound of raw CD sectors, those whose subheader (+16) flags audio (+18 bit 2); the others (a movie's
/// video) are skipped. Each holds 18 sound groups of 128 bytes at +24, in the format of its coding byte (+19: bits
/// 0-1 stereo, 2-3 18.9 kHz, 4-5 8-bit): the first audio sector's, which the others must share.
pub fn decode_xa(sectors: &[Vec<u8>]) -> Result<Pcm> {
    let (mut coding, mut samples, mut old) = (None, Vec::new(), [(0i32, 0i32); 2]);
    for s in sectors {
        if s.get(18).is_none_or(|m| m & 4 == 0) {
            continue;
        }
        let groups = s.get(24..24 + 18 * 128).context("an XA sector is cut short")?;
        let c = s[19] & 0x3f;
        if *coding.get_or_insert(c) != c {
            bail!("the XA sectors change format (coding {c:#04x} after {:#04x})", coding.unwrap_or(c));
        }
        let (stereo, eight) = (c & 3 == 1, c >> 4 & 3 == 1);
        let units = if eight { 4 } else { 8 };
        for g in groups.as_chunks::<128>().0 {
            let mut unit = [[0i16; 28]; 8];
            for (u, out) in unit.iter_mut().enumerate().take(units) {
                let p = g[4 + u]; // parameters (range, filter) of units 0-7 at +4 (copies around them)
                let range = match p & 15 {
                    r @ 0..=12 => r,
                    _ => 9, // 13-15 act as 9
                };
                let (k0, k1) = XA_K[(p >> 4 & 3) as usize];
                let (s1, s2) = &mut old[if stereo { u & 1 } else { 0 }];
                for (j, o) in out.iter_mut().enumerate() {
                    let t = if eight {
                        ((g[16 + 4 * j + u] as u16) << 8) as i16
                    } else {
                        ((((g[16 + 4 * j + u / 2] >> ((u & 1) * 4)) & 15) as u16) << 12) as i16
                    };
                    let v = ((t as i32 >> range) + ((*s1 * k0 + *s2 * k1 + 32) >> 6)).clamp(-32768, 32767);
                    (*s2, *s1) = (*s1, v);
                    *o = v as i16;
                }
            }
            if stereo {
                for pair in unit[..units].as_chunks::<2>().0 {
                    samples.extend(pair[0].iter().zip(&pair[1]).flat_map(|(l, r)| [*l, *r]));
                }
            } else {
                samples.extend(unit[..units].iter().flatten());
            }
        }
    }
    let c = coding.unwrap_or(1);
    Ok(Pcm { rate: if c >> 2 & 3 == 1 { 18900 } else { 37800 }, channels: if c & 3 == 1 { 2 } else { 1 }, samples })
}

/// PCM as a FLAC file, on this thread. The same bytes on every platform: flacenc's default Tukey window (`cos`) and
/// entropy-estimated fixed-order choice (`log2`) go through the platform's libm, so a rectangular window and the
/// exact bit count are used instead; what remains is plain arithmetic, the same everywhere.
pub fn flac(pcm: &Pcm) -> Result<Vec<u8>> {
    use flacenc::{component::BitRepr, error::Verify};
    let mut config = flacenc::config::Encoder::default();
    config.subframe_coding.qlpc.window = flacenc::config::Window::Rectangle;
    config.subframe_coding.qlpc.mae_optimization_steps = 0;
    config.subframe_coding.fixed.order_sel = flacenc::config::OrderSel::BitCount;
    let config = config.into_verified().map_err(|e| anyhow!("{e:?}"))?;
    let samples: Vec<i32> = pcm.samples.iter().map(|&s| s as i32).collect();
    let src = flacenc::source::MemSource::from_samples(&samples, pcm.channels as usize, 16, pcm.rate as usize);
    let mut stream = flacenc::encode_with_fixed_block_size(&config, src, config.block_size)
        .map_err(|e| anyhow!("cannot encode FLAC: {e:?}"))?;
    // flacenc counts the shorter last frame in the minimum, which makes decoders take the stream as variable-size
    stream.stream_info_mut().set_block_sizes(config.block_size, config.block_size).map_err(|e| anyhow!("{e:?}"))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|e| anyhow!("cannot write FLAC: {e:?}"))?;
    Ok(sink.into_inner())
}

/// Motion-JPEG frames as an AVI: RIFF 'AVI ' with hdrl (avih, one MJPG video stream), movi (a 00dc chunk per frame)
/// and idx1.
pub fn avi(frames: &[Vec<u8>], width: u32, height: u32, fps: u32) -> Vec<u8> {
    fn chunk(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut c = id.to_vec();
        c.extend((body.len() as u32).to_le_bytes());
        c.extend(body);
        if body.len() & 1 == 1 {
            c.push(0);
        }
        c
    }
    let list = |kind: &[u8], items: &[u8]| chunk(b"LIST", &[kind, items].concat());
    let le = |v: &[u32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let (n, big) = (frames.len() as u32, frames.iter().map(Vec::len).max().unwrap_or(0) as u32);
    // avih: µs per frame, max bytes/s, padding, flags (has idx1), frames, initial frames, streams, buffer, size
    let avih = le(&[1_000_000 / fps.max(1), big.saturating_mul(fps), 0, 0x10, n, 0, 1, big, width, height, 0, 0, 0, 0]);
    // strh: flags, priority + language, initial frames, scale, rate, start, length, buffer, quality, sample size,
    // the frame rectangle (four i16)
    let strh = [b"vidsMJPG", &le(&[0, 0, 0, 1, fps, 0, n, big, u32::MAX, 0, 0, width | height << 16])[..]].concat();
    // strf: BITMAPINFOHEADER (24-bit MJPG)
    let strf = le(&[
        40,
        width,
        height,
        1 | 24 << 16,
        u32::from_le_bytes(*b"MJPG"),
        width.saturating_mul(height).saturating_mul(3),
        0,
        0,
        0,
        0,
    ]);
    let strl = list(b"strl", &[chunk(b"strh", &strh), chunk(b"strf", &strf)].concat());
    let hdrl = list(b"hdrl", &[chunk(b"avih", &avih), strl].concat());
    let (mut movi, mut idx1) = (Vec::new(), Vec::new());
    for f in frames {
        // idx1 offsets count from the 'movi' list's type
        idx1.extend([&b"00dc"[..], &le(&[0x10, movi.len() as u32 + 4, f.len() as u32])].concat());
        movi.extend(chunk(b"00dc", f));
    }
    let body = [&b"AVI "[..], &hdrl, &list(b"movi", &movi), &chunk(b"idx1", &idx1)].concat();
    chunk(b"RIFF", &body)
}

/// A movie's pictures (the STR video sectors among `sectors`) as a Motion-JPEG AVI at `fps`, JPEG `quality` 1-100.
pub fn str_to_avi(sectors: &[Vec<u8>], fps: u32, quality: u8) -> Result<Vec<u8>> {
    let (mut jpegs, mut size) = (Vec::new(), (0, 0));
    decode_str(sectors, |f| {
        if jpegs.is_empty() {
            size = (f.width, f.height);
        }
        let mut j = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut j, quality).encode(
            &f.rgb,
            f.width,
            f.height,
            image::ExtendedColorType::Rgb8,
        )?;
        jpegs.push(j);
        Ok(())
    })?;
    if jpegs.is_empty() {
        bail!("the movie has no pictures");
    }
    Ok(avi(&jpegs, size.0, size.1, fps))
}

#[cfg(test)]
mod tests;
