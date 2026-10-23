//! A small MIPS R3000 (PS1 CPU) interpreter for running pure-integer game routines (decoders) from an EXE image.
//!
//! No GTE / COP0 / interrupts: only the integer ISA with branch delay slots, enough for data decoders such as
//! Tekken 2's motion key decoder. Routines that call GTE helpers run with hooks: the addresses in `hooks` call the
//! caller's function (returning v0) in place of the helper. Memory is the PS1's: 2 MB of RAM (KUSEG / KSEG0 / KSEG1
//! mirrors) and the 1 KB scratchpad at 0x1F800000. `call` runs a routine until it returns.

use anyhow::{Result, anyhow, bail};

pub const RET_ADDR: u32 = 0x801FFFF8;
pub const RAM_SIZE: usize = 0x200000;
pub const SCRATCH: u32 = 0x1F800000;

pub fn s32(v: u32) -> i64 {
    v as i32 as i64
}

pub struct Cpu {
    pub ram: Vec<u8>,
    pub scratch: Vec<u8>,
    pub r: [u32; 32],
    pub hi: u32,
    pub lo: u32,
    pub steps: u64,
    pub hooks: Vec<u32>,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

/// No hooks.
pub fn no_hook(_: &mut Cpu, _: u32) -> Result<u32> {
    Ok(0)
}

impl Cpu {
    pub fn new() -> Cpu {
        Cpu { ram: vec![0; RAM_SIZE], scratch: vec![0; 0x400], r: [0; 32], hi: 0, lo: 0, steps: 0, hooks: Vec::new() }
    }

    fn at(&mut self, a: u32) -> (&mut Vec<u8>, usize) {
        if a & 0xFFFFFC00 == SCRATCH {
            (&mut self.scratch, (a & 0x3FF) as usize)
        } else {
            (&mut self.ram, (a & 0x1FFFFF) as usize)
        }
    }

    /// Copy `data` to RAM (or the scratchpad) at `base` (past the end it grows, as Python's bytearray).
    pub fn map(&mut self, base: u32, data: &[u8]) {
        let (buf, o) = self.at(base);
        if buf.len() < o + data.len() {
            buf.resize(o + data.len(), 0);
        }
        buf[o..o + data.len()].copy_from_slice(data);
    }

    pub fn read(&mut self, a: u32, n: usize) -> u32 {
        let (buf, o) = self.at(a);
        let mut v = 0u32;
        for (i, b) in buf.iter().skip(o).take(n).enumerate() {
            v |= (*b as u32) << (8 * i);
        }
        v
    }

    pub fn write(&mut self, a: u32, n: usize, v: u32) -> Result<()> {
        let (buf, o) = self.at(a);
        let d = buf.get_mut(o..o + n).ok_or_else(|| anyhow!("a write at {a:#x} runs past the memory"))?;
        d.copy_from_slice(&v.to_le_bytes()[..n]);
        Ok(())
    }

    /// Run the routine at `pc` with `args` (a0..a3, then the stack) until it returns; v0. `hook`: called for the
    /// addresses in `hooks` instead of running them (its result is v0).
    pub fn call(&mut self, pc: u32, args: &[u32], hook: &mut dyn FnMut(&mut Cpu, u32) -> Result<u32>) -> Result<u32> {
        const SP: u32 = 0x801FFF00;
        const LIMIT: u64 = 50_000_000;
        for (i, &a) in args.iter().take(4).enumerate() {
            self.r[4 + i] = a;
        }
        for (i, &a) in args.iter().skip(4).enumerate() {
            self.write(SP + 0x10 + 4 * i as u32, 4, a)?;
        }
        self.r[29] = SP;
        self.r[31] = RET_ADDR;
        let mut pc = pc;
        let mut delay: Option<u32> = None;
        let mut n = 0u64;
        while pc != RET_ADDR {
            if delay.is_none() && self.hooks.contains(&pc) {
                self.r[2] = hook(self, pc)?;
                pc = self.r[31];
                continue;
            }
            n += 1;
            if n > LIMIT {
                bail!("step limit");
            }
            let nxt = self.step(pc)?;
            match delay.take() {
                Some(d) => pc = d,
                None => pc = pc.wrapping_add(4),
            }
            if nxt.is_some() {
                delay = nxt;
            }
        }
        self.steps += n;
        Ok(self.r[2])
    }

    fn load(&mut self, op: u32, addr: u32) -> Result<u32> {
        let (buf, o) = self.at(addr);
        let b = |k: usize| {
            buf.get(o + k).map(|v| *v as u32).ok_or_else(|| anyhow!("a load at {addr:#x} runs past the memory"))
        };
        Ok(match op {
            0x24 => b(0)?,
            0x25 => b(0)? | b(1)? << 8,
            0x23 => buf.iter().skip(o).take(4).enumerate().fold(0, |v, (i, x)| v | (*x as u32) << (8 * i)),
            0x20 => b(0)? as u8 as i8 as i32 as u32,
            _ => (b(0)? | b(1)? << 8) as u16 as i16 as i32 as u32,
        })
    }

    fn step(&mut self, pc: u32) -> Result<Option<u32>> {
        let o = (pc & 0x1FFFFF) as usize;
        let w = self.ram.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let (op, rs, rt, rd, sh) =
            (w >> 26, ((w >> 21) & 31) as usize, ((w >> 16) & 31) as usize, ((w >> 11) & 31) as usize, (w >> 6) & 31);
        let imm = w & 0xFFFF;
        let simm = imm as u16 as i16 as i32 as u32;
        let fnc = w & 63;
        let r = &mut self.r;
        let branch = |c: bool| {
            if c { Some(pc.wrapping_add(4).wrapping_add(simm << 2)) } else { None }
        };
        if op == 0 {
            let (a, b) = (r[rs], r[rt]);
            let v = match fnc {
                0x00 => b << sh,
                0x02 => b >> sh,
                0x03 => ((b as i32) >> sh) as u32,
                0x04 => b << (a & 31),
                0x06 => b >> (a & 31),
                0x07 => ((b as i32) >> (a & 31)) as u32,
                0x08 => return Ok(Some(a)),
                0x09 => {
                    if rd != 0 {
                        r[rd] = pc.wrapping_add(8);
                    }
                    return Ok(Some(a));
                }
                0x10 => self.hi,
                0x12 => self.lo,
                0x11 => {
                    self.hi = a;
                    return Ok(None);
                }
                0x13 => {
                    self.lo = a;
                    return Ok(None);
                }
                0x18 => {
                    let p = s32(a) * s32(b);
                    (self.lo, self.hi) = (p as u32, (p >> 32) as u32);
                    return Ok(None);
                }
                0x19 => {
                    let p = a as u64 * b as u64;
                    (self.lo, self.hi) = (p as u32, (p >> 32) as u32);
                    return Ok(None);
                }
                0x1A => {
                    let (x, y) = (s32(a), s32(b));
                    if y != 0 {
                        let q = x / y; // truncates, as abs(x) // abs(y) with the sign
                        (self.lo, self.hi) = (q as u32, (x - q * y) as u32);
                    }
                    return Ok(None);
                }
                0x1B => {
                    if let (Some(q), Some(r)) = (a.checked_div(b), a.checked_rem(b)) {
                        (self.lo, self.hi) = (q, r);
                    }
                    return Ok(None);
                }
                0x20 | 0x21 => a.wrapping_add(b),
                0x22 | 0x23 => a.wrapping_sub(b),
                0x24 => a & b,
                0x25 => a | b,
                0x26 => a ^ b,
                0x27 => !(a | b),
                0x2A => ((a as i32) < (b as i32)) as u32,
                0x2B => (a < b) as u32,
                _ => bail!("special {fnc:#x} at {pc:#x}"),
            };
            if rd != 0 {
                r[rd] = v;
            }
            return Ok(None);
        }
        if op == 9 {
            if rt != 0 {
                r[rt] = r[rs].wrapping_add(simm);
            }
            return Ok(None);
        }
        if matches!(op, 0x20 | 0x21 | 0x23 | 0x24 | 0x25) {
            let addr = r[rs].wrapping_add(simm);
            let v = self.load(op, addr)?;
            if rt != 0 {
                self.r[rt] = v;
            }
            return Ok(None);
        }
        if matches!(op, 0x28 | 0x29 | 0x2B) {
            let addr = r[rs].wrapping_add(simm);
            let v = r[rt];
            self.write(addr, [1, 2, 0, 4][(op - 0x28) as usize], v)?;
            return Ok(None);
        }
        Ok(match op {
            1 => {
                let a = r[rs] as i32;
                let t = pc.wrapping_add(4).wrapping_add(simm << 2);
                if rt == 0x10 || rt == 0x11 {
                    r[31] = pc.wrapping_add(8);
                }
                if rt & 1 != 0 { (a >= 0).then_some(t) } else { (a < 0).then_some(t) }
            }
            2 => Some((pc & 0xF0000000) | ((w & 0x3FFFFFF) << 2)),
            3 => {
                r[31] = pc.wrapping_add(8);
                Some((pc & 0xF0000000) | ((w & 0x3FFFFFF) << 2))
            }
            4 => branch(r[rs] == r[rt]),
            5 => branch(r[rs] != r[rt]),
            6 => branch(r[rs] as i32 <= 0),
            7 => branch(r[rs] as i32 > 0),
            _ => {
                let a = r[rs];
                let v = match op {
                    8 => a.wrapping_add(simm),
                    10 => ((a as i32) < simm as i32) as u32,
                    11 => (a < simm) as u32,
                    12 => a & imm,
                    13 => a | imm,
                    14 => a ^ imm,
                    15 => imm << 16,
                    _ => bail!("op {op:#x} at {pc:#x}"),
                };
                if rt != 0 {
                    r[rt] = v;
                }
                None
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_a_loop() -> Result<()> {
        // v0 = 0; loop: v0 += a0; a0 -= 1; bne a0, 0, loop (delay: nop); jr ra; nop
        let code: [u32; 6] = [0x00001021, 0x00441021, 0x2484FFFF, 0x1480FFFD, 0x00000000, 0x03E00008];
        let mut cpu = Cpu::new();
        let b: Vec<u8> = code.iter().chain(&[0]).flat_map(|w| w.to_le_bytes()).collect();
        cpu.map(0x80010000, &b);
        assert_eq!(cpu.call(0x80010000, &[10], &mut no_hook)?, 55);
        Ok(())
    }
}
