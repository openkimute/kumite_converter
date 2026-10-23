//! `flac_pcm IN.s16le RATE CHANNELS OUT.flac`: encodes raw PCM with psx_media::flac (checks its output is the same on
//! every platform: run natively and as wasm32-wasip1, compare).
#![allow(clippy::unwrap_used, clippy::expect_used)] // a check: a failure stops it

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let raw = std::fs::read(&a[0]).unwrap();
    let samples = raw.as_chunks::<2>().0.iter().map(|&c| i16::from_le_bytes(c)).collect();
    let pcm = psx_media::Pcm { rate: a[1].parse().unwrap(), channels: a[2].parse().unwrap(), samples };
    std::fs::write(&a[3], psx_media::flac(&pcm).unwrap()).unwrap();
}
