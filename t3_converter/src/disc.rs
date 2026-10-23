//! The disc image: ISO 9660 on raw 2352-byte sectors (extract_models.py's `iso_files` / `read_file`), and Tekken 3's
//! EXE and BNS archive with its record table (disc.py).

use anyhow::{Context, Result, bail};
use conv_io::{DiscFile, ReadSeek};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const SECTOR: u64 = 2352;
pub const HDR: u64 = 24;
/// The BNS record table in SLUS_004.02 (file offset) and its length.
pub const BNS_TABLE_OFF: usize = 0x14C00;
pub const BNS_COUNT: usize = 303;

pub struct Iso {
    f: Box<dyn ReadSeek>,
    /// Every file, in the directory walk's order (Python's dict order): path, first sector, size.
    pub files: Vec<(String, u32, u32)>,
}

impl Iso {
    pub fn open(disc: &dyn DiscFile) -> Result<Iso> {
        let mut iso = Iso { f: disc.open()?, files: Vec::new() };
        let pvd = iso.sector(16)?;
        let (Some(b"CD001"), Some(root)) = (pvd.get(1..6), pvd.get(156..190)) else {
            bail!("{} is not a raw (2352-byte sector) CD image with ISO 9660 data", disc.name());
        };
        let root = root.to_vec();
        iso.walk(u32le(&root, 2), u32le(&root, 10), String::new(), 0)?;
        Ok(iso)
    }

    /// One sector's 2048 bytes of user data (short at the end of the image).
    pub fn sector(&mut self, n: u64) -> Result<Vec<u8>> {
        self.f.seek(SeekFrom::Start(n * SECTOR + HDR))?;
        let mut buf = Vec::with_capacity(2048);
        (&mut self.f).take(2048).read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Whole raw sectors (2352 bytes each), for XA audio and movies.
    pub fn raw(&mut self, lba: u64, count: u64) -> Result<Vec<u8>> {
        self.f.seek(SeekFrom::Start(lba * SECTOR))?;
        let mut buf = Vec::new();
        (&mut self.f).take(count * SECTOR).read_to_end(&mut buf)?;
        Ok(buf)
    }

    fn walk(&mut self, lba: u32, size: u32, path: String, depth: u32) -> Result<()> {
        if depth > 16 {
            bail!("the disc's directories nest too deep");
        }
        let mut data = Vec::new();
        for i in 0..size.div_ceil(2048) {
            data.extend(self.sector(lba as u64 + i as u64)?);
        }
        let mut o = 0;
        while o < data.len() {
            let l = data[o] as usize;
            if l == 0 {
                o = (o / 2048 + 1) * 2048;
                continue;
            }
            let r = data.get(o..o + l).context("a directory record runs past its directory")?;
            o += l;
            if r.len() < 34 {
                bail!("a directory record is too short");
            }
            let (ext, sz) = (u32le(r, 2), u32le(r, 10));
            let name = r.get(33..33 + r[32] as usize).context("a directory record's name runs past it")?;
            if name == b"\0" || name == b"\x01" {
                continue;
            }
            let n = String::from_utf8_lossy(name);
            let n = n.split(';').next().unwrap_or("").to_string();
            if r[25] & 2 != 0 {
                self.walk(ext, sz, format!("{path}{n}/"), depth + 1)?;
            } else {
                self.files.push((format!("{path}{n}"), ext, sz));
            }
        }
        Ok(())
    }

    pub fn find(&self, name: &str) -> Result<(u32, u32)> {
        self.files
            .iter()
            .find(|f| f.0 == name)
            .map(|f| (f.1, f.2))
            .with_context(|| format!("{name} is not on the disc (is it Tekken 3 USA, SLUS-00402?)"))
    }

    pub fn read_file(&mut self, lba: u32, size: u32) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(size as usize + 2048);
        for i in 0..size.div_ceil(2048) {
            buf.extend(self.sector(lba as u64 + i as u64)?);
        }
        buf.truncate(size as usize);
        Ok(buf)
    }

    pub fn read(&mut self, name: &str) -> Result<Vec<u8>> {
        let (lba, size) = self.find(name)?;
        self.read_file(lba, size)
    }
}

/// Tekken 3's EXE, BNS archive and its records (`rec[i]` of the export scripts).
pub struct Game {
    pub exe: Vec<u8>,
    pub bns: Vec<u8>,
    pub rec: Vec<Vec<u8>>,
}

impl Game {
    pub fn load(disc: &dyn DiscFile) -> Result<Game> {
        let iso = &mut Iso::open(disc)?;
        let exe = iso.read("TEKKEN3/SLUS_004.02")?;
        let bns = iso.read("TEKKEN3/TEKKEN3.BNS")?;
        let mut rec = Vec::with_capacity(BNS_COUNT);
        for i in 0..BNS_COUNT {
            let at = BNS_TABLE_OFF + 8 * i;
            if at + 8 > exe.len() {
                bail!("the EXE is too short for its BNS table");
            }
            let (s, z) = (u32le(&exe, at) as usize, u32le(&exe, at + 4) as usize);
            rec.push(py_slice(&bns, s * 2048, s * 2048 + z).to_vec());
        }
        Ok(Game { exe, bns, rec })
    }

    /// The EXE's bytes at a RAM address (file offset = address - 0x80010000 + 0x800).
    pub fn at(&self, addr: u32) -> &[u8] {
        py_slice(&self.exe, (addr.wrapping_sub(0x8000F800)) as usize, usize::MAX)
    }
}

/// Python's `b[a:b]`: clamped to the data, empty when reversed.
pub fn py_slice(d: &[u8], a: usize, b: usize) -> &[u8] {
    let a = a.min(d.len());
    let b = b.min(d.len());
    if a >= b { &[] } else { &d[a..b] }
}

/// disc.py's BIN: the T3_BIN environment variable, or "Tekken_3_(USA)_(Track 1).bin" at the repository's root.
pub fn bin() -> std::path::PathBuf {
    match std::env::var("T3_BIN") {
        Ok(p) if !p.is_empty() => p.into(),
        _ => Path::new(env!("CARGO_MANIFEST_DIR")).join("../Tekken_3_(USA)_(Track 1).bin"),
    }
}

pub fn u32le(d: &[u8], o: usize) -> u32 {
    d.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
