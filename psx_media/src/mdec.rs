//! STR movies: their video sectors gathered into frames, each frame's bitstream (versions 2 and 3) decoded as the
//! MDEC would (run-lengths, dequantisation, IDCT, YCbCr to RGB). Integer maths only, so every platform agrees.

use anyhow::{Context, Result, bail};

/// One picture, RGB 8-bit, rows top to bottom.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// A video sector's header (+24): 0160h, 8001h; then chunk, chunks, frame number, size, width, height.
const MAGIC: [u8; 4] = [0x60, 0x01, 0x01, 0x80];
/// A video sector's share of its frame: 2048 bytes less the 32-byte header.
const PAYLOAD: usize = 2016;

fn u16_(d: &[u8], o: usize) -> usize {
    u16::from_le_bytes([d[o], d[o + 1]]) as usize
}

/// The pictures of the STR video sectors among raw CD sectors (audio sectors skipped), in order: a frame's sectors
/// are gathered by their chunk number until the frame number changes.
pub fn decode_str(sectors: &[Vec<u8>], mut f: impl FnMut(Frame) -> Result<()>) -> Result<()> {
    let ac = ac_table();
    // (frame number, width, height, data)
    let mut cur: Option<(u32, usize, usize, Vec<u8>)> = None;
    for s in sectors {
        let Some(h) = s.get(24..56) else { continue };
        if s[18] & 4 != 0 || h[..4] != MAGIC {
            continue;
        }
        let (chunk, frame, w, hg) =
            (u16_(h, 4), u32::from_le_bytes([h[8], h[9], h[10], h[11]]), u16_(h, 16), u16_(h, 18));
        if chunk >= 256 || !(1..=1024).contains(&w) || !(1..=1024).contains(&hg) {
            bail!("frame {frame}'s sector has an unlikely chunk {chunk} or size {w}x{hg}");
        }
        let body = s.get(56..56 + PAYLOAD).context("a video sector is cut short")?;
        if let Some((n, w, h, d)) = cur.take_if(|c| c.0 != frame) {
            f(decode_frame(&ac, n, w, h, &d)?)?;
        }
        let d = &mut cur.get_or_insert_with(|| (frame, w, hg, Vec::new())).3;
        let at = chunk * PAYLOAD;
        if d.len() < at + PAYLOAD {
            d.resize(at + PAYLOAD, 0);
        }
        d[at..at + PAYLOAD].copy_from_slice(body);
    }
    if let Some((n, w, h, d)) = cur {
        f(decode_frame(&ac, n, w, h, &d)?)?;
    }
    Ok(())
}

/// The bitstream: little-endian 16-bit words, each read from its top bit; zeros past the end.
struct Bits<'a> {
    d: &'a [u8],
    at: usize,
    buf: u64,
    n: u32,
}

impl Bits<'_> {
    fn peek(&mut self, k: u32) -> u32 {
        while self.n <= 48 {
            let w = self.d.get(self.at..self.at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
            self.buf |= (w as u64) << (48 - self.n);
            (self.at, self.n) = (self.at + 2, self.n + 16);
        }
        (self.buf >> (64 - k)) as u32
    }

    fn skip(&mut self, k: u32) {
        self.buf <<= k;
        self.n -= k;
    }

    fn take(&mut self, k: u32) -> u32 {
        let v = self.peek(k);
        self.skip(k);
        v
    }

    fn overrun(&self) -> bool {
        self.at * 8 - self.n as usize > self.d.len() * 8
    }
}

/// MPEG-1's DCT coefficient codes (table B.14, not the first coefficient), less the sign bit: (code, run, level).
/// "10" ends a block; "000001" escapes to a 6-bit run and a signed 10-bit level.
pub(crate) const AC: [(&str, u8, i16); 111] = [
    ("11", 0, 1),
    ("011", 1, 1),
    ("0100", 0, 2),
    ("0101", 2, 1),
    ("00101", 0, 3),
    ("00110", 4, 1),
    ("00111", 3, 1),
    ("000100", 7, 1),
    ("000101", 6, 1),
    ("000110", 1, 2),
    ("000111", 5, 1),
    ("0000100", 2, 2),
    ("0000101", 9, 1),
    ("0000110", 0, 4),
    ("0000111", 8, 1),
    ("00100000", 13, 1),
    ("00100001", 0, 6),
    ("00100010", 12, 1),
    ("00100011", 11, 1),
    ("00100100", 3, 2),
    ("00100101", 1, 3),
    ("00100110", 0, 5),
    ("00100111", 10, 1),
    ("0000001000", 16, 1),
    ("0000001001", 5, 2),
    ("0000001010", 0, 7),
    ("0000001011", 2, 3),
    ("0000001100", 1, 4),
    ("0000001101", 15, 1),
    ("0000001110", 14, 1),
    ("0000001111", 4, 2),
    ("000000010000", 0, 11),
    ("000000010001", 8, 2),
    ("000000010010", 4, 3),
    ("000000010011", 0, 10),
    ("000000010100", 2, 4),
    ("000000010101", 7, 2),
    ("000000010110", 21, 1),
    ("000000010111", 20, 1),
    ("000000011000", 0, 9),
    ("000000011001", 19, 1),
    ("000000011010", 18, 1),
    ("000000011011", 1, 5),
    ("000000011100", 3, 3),
    ("000000011101", 0, 8),
    ("000000011110", 6, 2),
    ("000000011111", 17, 1),
    ("0000000010000", 10, 2),
    ("0000000010001", 9, 2),
    ("0000000010010", 5, 3),
    ("0000000010011", 3, 4),
    ("0000000010100", 2, 5),
    ("0000000010101", 1, 7),
    ("0000000010110", 1, 6),
    ("0000000010111", 0, 15),
    ("0000000011000", 0, 14),
    ("0000000011001", 0, 13),
    ("0000000011010", 0, 12),
    ("0000000011011", 26, 1),
    ("0000000011100", 25, 1),
    ("0000000011101", 24, 1),
    ("0000000011110", 23, 1),
    ("0000000011111", 22, 1),
    ("00000000010000", 0, 31),
    ("00000000010001", 0, 30),
    ("00000000010010", 0, 29),
    ("00000000010011", 0, 28),
    ("00000000010100", 0, 27),
    ("00000000010101", 0, 26),
    ("00000000010110", 0, 25),
    ("00000000010111", 0, 24),
    ("00000000011000", 0, 23),
    ("00000000011001", 0, 22),
    ("00000000011010", 0, 21),
    ("00000000011011", 0, 20),
    ("00000000011100", 0, 19),
    ("00000000011101", 0, 18),
    ("00000000011110", 0, 17),
    ("00000000011111", 0, 16),
    ("000000000010000", 0, 40),
    ("000000000010001", 0, 39),
    ("000000000010010", 0, 38),
    ("000000000010011", 0, 37),
    ("000000000010100", 0, 36),
    ("000000000010101", 0, 35),
    ("000000000010110", 0, 34),
    ("000000000010111", 0, 33),
    ("000000000011000", 0, 32),
    ("000000000011001", 1, 14),
    ("000000000011010", 1, 13),
    ("000000000011011", 1, 12),
    ("000000000011100", 1, 11),
    ("000000000011101", 1, 10),
    ("000000000011110", 1, 9),
    ("000000000011111", 1, 8),
    ("0000000000010000", 1, 18),
    ("0000000000010001", 1, 17),
    ("0000000000010010", 1, 16),
    ("0000000000010011", 1, 15),
    ("0000000000010100", 6, 3),
    ("0000000000010101", 16, 2),
    ("0000000000010110", 15, 2),
    ("0000000000010111", 14, 2),
    ("0000000000011000", 13, 2),
    ("0000000000011001", 12, 2),
    ("0000000000011010", 11, 2),
    ("0000000000011011", 31, 1),
    ("0000000000011100", 30, 1),
    ("0000000000011101", 29, 1),
    ("0000000000011110", 28, 1),
    ("0000000000011111", 27, 1),
];

/// The codes by their next 17 bits (the longest, with its sign): (length, run, level); length 0 for no code.
pub(crate) fn ac_table() -> Vec<(u32, usize, i32)> {
    let mut t = vec![(0, 0, 0); 1 << 17];
    for (code, run, level) in AC {
        let len = code.len() + 1;
        let bits = code.bytes().fold(0, |a, c| a * 2 + (c == b'1') as usize);
        for sign in 0..2 {
            let at = (bits << 1 | sign) << (17 - len);
            t[at..at + (1 << (17 - len))].fill((
                len as u32,
                run as usize,
                (if sign == 1 { -level } else { level }) as i32,
            ));
        }
    }
    t
}

/// The MDEC's quantisation table (MPEG-1's intra matrix, the DC's 2), in zigzag order.
const QT: [i32; 64] = [
    2, 16, 16, 19, 16, 19, 22, 22, 22, 22, 22, 22, 26, 24, 26, 27, 27, 27, 26, 26, 26, 26, 27, 27, 27, 29, 29, 29, 34,
    34, 34, 29, 29, 29, 27, 27, 29, 29, 32, 32, 34, 34, 37, 38, 37, 35, 35, 34, 35, 38, 38, 40, 40, 40, 48, 48, 46, 46,
    56, 56, 58, 69, 69, 83,
];

/// The zigzag: coefficient k's place in the 8x8 block.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21,
    28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54,
    47, 55, 62, 63,
];

fn sext10(v: u32) -> i32 {
    ((v << 22) as i32) >> 22
}

/// A version 3 DC's size: MPEG-1's luminance or chrominance code.
fn dc_size(b: &mut Bits, luma: bool) -> Result<u32> {
    Ok(match (luma, b.take(2)) {
        (true, 0) => 1,
        (true, 1) => 2,
        (true, 2) => 3 * b.take(1), // 100: 0, 101: 3
        (false, s @ 0..=2) => s,
        _ => {
            let mut s = if luma { 4 } else { 3 };
            while b.take(1) == 1 {
                s += 1;
                if s > 8 {
                    bail!("a DC size code is too long");
                }
            }
            s
        }
    })
}

/// One block's coefficients, dequantised, in place (row-major). `dc` is version 3's running DC of its component.
fn block(b: &mut Bits, ac: &[(u32, usize, i32)], q: i32, v3: bool, dc: &mut i32, luma: bool) -> Result<[i32; 64]> {
    let mut blk = [0; 64];
    let raw = if v3 {
        let size = dc_size(b, luma)?;
        let diff = match size {
            0 => 0,
            _ => match b.take(size) as i32 {
                d if d < 1 << (size - 1) => d - (1 << size) + 1,
                d => d,
            },
        };
        *dc += diff;
        sext10((*dc * 4) as u32 & 0x3ff) // the 10-bit DC field the game hands the MDEC
    } else {
        sext10(b.take(10))
    };
    blk[0] = (raw * QT[0]).clamp(-0x400, 0x3ff);
    let mut k = 0;
    loop {
        let p = b.peek(17) as usize;
        let (run, level) = if p >> 15 == 2 {
            b.skip(2);
            return Ok(blk);
        } else if p >> 11 == 1 {
            b.skip(6);
            let e = b.take(16);
            ((e >> 10) as usize, sext10(e & 0x3ff))
        } else {
            let (len, run, level) = ac[p];
            if len == 0 {
                bail!("a bad coefficient code");
            }
            b.skip(len);
            (run, level)
        };
        k += run + 1;
        if k > 63 {
            bail!("a block's coefficients run past 64");
        }
        blk[ZIGZAG[k]] = ((level * QT[k] * q + 4) >> 3).clamp(-0x400, 0x3ff);
    }
}

/// cos((2x+1)uπ/16) · 32768 at [u][x], u = 0 also times 1/√2.
fn cos_table() -> [[i64; 8]; 8] {
    const C: [i64; 9] = [32768, 32138, 30274, 27246, 23170, 18205, 12540, 6393, 0];
    let mut t = [[0; 8]; 8];
    for (u, row) in t.iter_mut().enumerate() {
        for (x, v) in row.iter_mut().enumerate() {
            *v = match ((2 * x + 1) * u) % 32 {
                _ if u == 0 => C[4],
                m @ 0..=8 => C[m],
                m @ 9..=16 => -C[16 - m],
                m @ 17..=24 => -C[m - 16],
                m => C[32 - m],
            };
        }
    }
    t
}

/// The 8x8 inverse DCT (orthonormal, as JPEG's), exact in 64 bits and rounded once.
fn idct(f: &[i32; 64], t: &[[i64; 8]; 8]) -> [i32; 64] {
    let mut tmp = [[0i64; 8]; 8]; // [u][c]: rows transformed
    for u in 0..8 {
        let row = &f[u * 8..u * 8 + 8];
        if row.iter().all(|&v| v == 0) {
            continue;
        }
        for (c, o) in tmp[u].iter_mut().enumerate() {
            *o = (0..8).map(|v| row[v] as i64 * t[v][c]).sum();
        }
    }
    let mut out = [0; 64];
    for (i, o) in out.iter_mut().enumerate() {
        let (r, c) = (i / 8, i % 8);
        *o = (((0..8).map(|u| tmp[u][c] * t[u][r]).sum::<i64>() + (1 << 31)) >> 32) as i32;
    }
    out
}

/// YCbCr (full range, chroma at half size each way) to RGB: JPEG's matrix in 16-bit fixed point.
pub(crate) fn rgb(y: &[u8], cb: &[u8], cr: &[u8], stride: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 3);
    for r in 0..h {
        for c in 0..w {
            let at = r / 2 * stride.div_ceil(2) + c / 2;
            let (l, b, e) = (y[r * stride + c] as i32, cb[at] as i32 - 128, cr[at] as i32 - 128);
            for v in [
                l + ((91881 * e + 32768) >> 16),
                l - ((22554 * b + 46802 * e - 32768) >> 16),
                l + ((116130 * b + 32768) >> 16),
            ] {
                out.push(v.clamp(0, 255) as u8);
            }
        }
    }
    out
}

/// Frame data (a 4-word header: size, 3800h, quantiser scale, version 1-3 (1 as 2); then the bitstream) to its
/// picture as YCbCr planes (Y `w`x`h` rounded up to 16, Cb and Cr half that each way). Macroblocks run down each
/// column of 16 pixels, then across; each is Cr, Cb, then the four Y blocks.
pub(crate) fn decode_planes(ac: &[(u32, usize, i32)], d: &[u8], w: usize, h: usize) -> Result<[Vec<u8>; 3]> {
    let hd = d.get(..8).context("no header")?;
    let (q, version) = ((u16_(hd, 4) & 63) as i32, u16_(hd, 6)); // the MDEC takes a 6-bit scale
    if hd[2..4] != [0, 0x38] || !(1..=3).contains(&version) {
        bail!("not an MDEC version 1-3 bitstream (header {hd:02x?})");
    }
    let (mw, mh) = (w.div_ceil(16), h.div_ceil(16));
    let stride = mw * 16;
    let (mut y, mut cb, mut cr) = (vec![0u8; stride * mh * 16], vec![0u8; stride * mh * 4], vec![0u8; stride * mh * 4]);
    let (t, mut b, mut dc) = (cos_table(), Bits { d: &d[8..], at: 0, buf: 0, n: 0 }, [0i32; 3]);
    for mx in 0..mw {
        for my in 0..mh {
            for k in 0..6 {
                let px = idct(&block(&mut b, ac, q, version == 3, &mut dc[k.min(2)], k >= 2)?, &t);
                let (plane, s, x0, y0) = match k {
                    0 => (&mut cr, stride / 2, mx * 8, my * 8),
                    1 => (&mut cb, stride / 2, mx * 8, my * 8),
                    _ => (&mut y, stride, mx * 16 + (k - 2) % 2 * 8, my * 16 + (k - 2) / 2 * 8),
                };
                for (i, v) in px.iter().enumerate() {
                    let v = if k < 2 { v.clamp(&-128, &127) + 128 } else { (v + 128).clamp(0, 255) };
                    plane[(y0 + i / 8) * s + x0 + i % 8] = v as u8;
                }
            }
            if b.overrun() {
                bail!("the bitstream ends early");
            }
        }
    }
    Ok([y, cb, cr])
}

fn decode_frame(ac: &[(u32, usize, i32)], n: u32, w: usize, h: usize, d: &[u8]) -> Result<Frame> {
    let [y, cb, cr] = decode_planes(ac, d, w, h).with_context(|| format!("STR frame {n}"))?;
    Ok(Frame { width: w as u32, height: h as u32, rgb: rgb(&y, &cb, &cr, w.div_ceil(16) * 16, w, h) })
}
