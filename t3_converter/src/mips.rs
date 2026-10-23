//! A minimal MIPS R3000 evaluator that reads the game's draw calls from its code: it runs the code from an address
//! (registers zero, sp on an empty stack) over the EXE or an overlay record, loads from its own stores, then that
//! image (memory outside the image reads 0: an overlay's run does not see the EXE's RAM), follows branches and
//! jump tables, steps over the calls it meets (ra untouched) and records their arguments. It stops at the
//! routine's return. No GTE, no traps.

use crate::gamedata_core::Exe;
use anyhow::{Result, bail};
use std::collections::BTreeMap;

const SP: u32 = 0x801F_FF00;
const RETURN: u32 = 0xFFFF_FFF0;
const STEPS: usize = 200_000;

/// A call's arguments: a0-a3, then the stack words sp+16..sp+60 (signed 32-bit values).
pub type Args = [i64; 16];

/// How a run starts: its arguments (a0-a3, then the stack words sp+16...), the RAM the code reads that the run
/// sets (`state`: address, width in bytes, value), and v0 after each call it steps over.
#[derive(Default)]
pub struct Run<'a> {
    pub args: &'a [i64],
    pub state: &'a [(i64, i64, i64)],
    pub ret: i64,
}

/// The arguments of the first `n` calls to `target` that the code from `pc` makes (fewer before it returns: an
/// error).
pub fn calls(e: &Exe, pc: i64, n: usize, target: i64) -> Result<Vec<Args>> {
    let out = trace(e, pc, &Run::default(), n, target)?;
    if out.len() < n {
        bail!("no {n} calls to 0x{target:X} in the code at 0x{pc:X}");
    }
    Ok(out)
}

/// The arguments of the calls to `target` that the code from `pc` makes before it returns, at most `n`.
pub fn trace(e: &Exe, pc: i64, run: &Run, n: usize, target: i64) -> Result<Vec<Args>> {
    Ok(trace_text(e, pc, run, n, target)?.into_iter().map(|(a, _)| a).collect())
}

/// `trace`, each call with the text at a0 as the call sees it (at most 64 bytes; None if it reads memory that is
/// neither stored nor in the image).
pub fn trace_text(e: &Exe, pc: i64, run: &Run, n: usize, target: i64) -> Result<Vec<(Args, Option<String>)>> {
    let (mut r, mut mem, mut out) = ([0u32; 32], BTreeMap::<u32, u8>::new(), Vec::new());
    let (mut hi, mut lo) = (0u32, 0u32);
    (r[29], r[31]) = (SP, RETURN);
    for (k, &v) in run.args.iter().take(16).enumerate() {
        if k < 4 {
            r[4 + k] = v as u32;
        } else {
            for (b, x) in (v as u32).to_le_bytes().into_iter().enumerate() {
                mem.insert(SP + 4 * k as u32 + b as u32, x);
            }
        }
    }
    for &(a, size, v) in run.state {
        for k in 0..size {
            mem.insert((a + k) as u32, (v >> (8 * k)) as u8);
        }
    }
    let (mut pc, mut npc, mut slot) = (pc as u32, pc as u32 + 4, None);
    let byte = |mem: &BTreeMap<u32, u8>, a: u32| mem.get(&a).copied().or_else(|| e.bytes(a as i64, 1).first().copied());
    let load = |mem: &BTreeMap<u32, u8>, a: u32, n: u32| -> u32 {
        (0..n).fold(0, |v, k| v | (byte(mem, a.wrapping_add(k)).unwrap_or(0) as u32) << (8 * k))
    };
    for _ in 0..STEPS {
        // (a return through an ra never saved, from code entered inside its routine, reads 0: see jr)
        if out.len() == n || pc == RETURN || pc == 0 {
            return Ok(out);
        }
        let w = e.u32(pc as i64)? as u32;
        let (op, rs, rt, rd) = (w >> 26, (w >> 21) as usize & 31, (w >> 16) as usize & 31, (w >> 11) as usize & 31);
        let (imm, simm, sa) = (w & 0xFFFF, (w & 0xFFFF) as u16 as i16 as i32 as u32, (w >> 6) & 31);
        let (s, t, addr) = (r[rs], r[rt], r[rs].wrapping_add(simm));
        let (mut next, mut call) = (npc.wrapping_add(4), None);
        let jump = (npc & 0xF000_0000) | (w & 0x3FF_FFFF) << 2;
        let branch = |taken: bool| if taken { npc.wrapping_add(simm << 2) } else { npc.wrapping_add(4) };
        let unknown = || bail!("an instruction the evaluator does not know at 0x{pc:X}");
        match op {
            0 => match w & 63 {
                0 => r[rd] = t << sa,
                2 => r[rd] = t >> sa,
                3 => r[rd] = (t as i32 >> sa) as u32,
                4 => r[rd] = t << (s & 31),
                6 => r[rd] = t >> (s & 31),
                7 => r[rd] = (t as i32 >> (s & 31)) as u32,
                8 if s == 0 && rs != 31 => bail!("a jump to 0 at 0x{pc:X}"),
                8 => next = s,
                9 => call = Some(s),
                0x10 => r[rd] = hi,
                0x11 => hi = s,
                0x12 => r[rd] = lo,
                0x13 => lo = s,
                0x18 => {
                    let p = (s as i32 as i64 * t as i32 as i64) as u64;
                    (hi, lo) = ((p >> 32) as u32, p as u32);
                }
                0x19 => {
                    let p = s as u64 * t as u64;
                    (hi, lo) = ((p >> 32) as u32, p as u32);
                }
                0x1A if t != 0 => {
                    (lo, hi) = ((s as i32).wrapping_div(t as i32) as u32, (s as i32).wrapping_rem(t as i32) as u32)
                }
                0x1B if t != 0 => (lo, hi) = (s / t, s % t),
                0x1A | 0x1B => {}
                0x20 | 0x21 => r[rd] = s.wrapping_add(t),
                0x22 | 0x23 => r[rd] = s.wrapping_sub(t),
                0x24 => r[rd] = s & t,
                0x25 => r[rd] = s | t,
                0x26 => r[rd] = s ^ t,
                0x27 => r[rd] = !(s | t),
                0x2A => r[rd] = ((s as i32) < (t as i32)) as u32,
                0x2B => r[rd] = (s < t) as u32,
                _ => return unknown(),
            },
            1 if rt & 16 == 0 => next = branch(((s as i32) < 0) != (rt & 1 != 0)),
            2 => next = jump,
            3 => call = Some(jump),
            4 => next = branch(s == t),
            5 => next = branch(s != t),
            6 => next = branch(s as i32 <= 0),
            7 => next = branch(s as i32 > 0),
            8 | 9 => r[rt] = s.wrapping_add(simm),
            0x0A => r[rt] = ((s as i32) < (simm as i32)) as u32,
            0x0B => r[rt] = (s < simm) as u32,
            0x0C => r[rt] = s & imm,
            0x0D => r[rt] = s | imm,
            0x0E => r[rt] = s ^ imm,
            0x0F => r[rt] = imm << 16,
            0x20 => r[rt] = load(&mem, addr, 1) as u8 as i8 as i32 as u32,
            0x21 => r[rt] = load(&mem, addr, 2) as u16 as i16 as i32 as u32,
            0x23 => r[rt] = load(&mem, addr, 4),
            0x24 => r[rt] = load(&mem, addr, 1),
            0x25 => r[rt] = load(&mem, addr, 2),
            0x28 | 0x29 | 0x2B => {
                for k in 0..[1, 2, 0, 4][op as usize - 0x28] {
                    mem.insert(addr.wrapping_add(k), (t >> (8 * k)) as u8);
                }
            }
            _ => return unknown(),
        }
        r[0] = 0;
        // after a call's delay slot: its arguments, then on past it with v0 = ret
        if let Some(to) = slot.take() {
            if to as i64 == target & 0xFFFF_FFFF {
                let mut a: Args = [0; 16];
                for (k, v) in a.iter_mut().enumerate() {
                    let w = if k < 4 { r[4 + k] } else { load(&mem, r[29].wrapping_add(4 * k as u32), 4) };
                    *v = w as i32 as i64;
                }
                let text = (0..64)
                    .map(|k| byte(&mem, r[4].wrapping_add(k)))
                    .take_while(|b| *b != Some(0))
                    .map(|b| b.map(char::from))
                    .collect();
                out.push((a, text));
            }
            r[2] = run.ret as u32;
        }
        slot = call;
        (pc, npc) = (npc, next);
    }
    bail!("the code at 0x{pc:X} runs too long")
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 0x8000_0400;
    const JAL_T: u32 = 0x0C00_0100;

    /// `words` at 0x80000000, then `data` from 0x80000100.
    fn image(words: &[u32], data: &[u8]) -> Vec<u8> {
        let mut code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        if !data.is_empty() {
            code.resize(0x100, 0);
            code.extend(data);
        }
        code
    }

    /// a0 of each call to T.
    fn a0(e: &Exe, run: &Run) -> Result<Vec<i64>> {
        Ok(trace(e, 0x8000_0000, run, usize::MAX, T)?.iter().map(|a| a[0]).collect())
    }

    #[test]
    fn loops_branches_and_state() -> Result<()> {
        // s0 = 0; do { call(a0 = s0 + 10); s0 += 4 } while s0 < 12; if byte [0x80000200] != 0 { call(a0 = v0) }
        let words = [
            0x24100000u32,
            0x0C000040,
            0x2604000A,
            0x26100004,
            0x2A03000C,
            0x1460FFFB,
            0,
            0x3C098000,
            0x91280200,
            0x11000003,
            0,
            0x0C000040,
            0x24440000,
            0x03E00008,
            0,
        ];
        let code = image(&words, &[]);
        let e = Exe::with_base(&code, 0x8000_0000);
        let a0 = |run: &Run| -> Result<Vec<i64>> {
            Ok(trace(&e, 0x8000_0000, run, usize::MAX, 0x8000_0100)?.iter().map(|a| a[0]).collect())
        };
        assert_eq!(a0(&Run::default())?, [10, 14, 18]);
        assert_eq!(a0(&Run { state: &[(0x8000_0200, 1, 1)], ret: 7, ..Run::default() })?, [10, 14, 18, 7]);
        assert!(calls(&e, 0x8000_0000, 4, 0x8000_0100).is_err(), "only three calls");
        assert!(trace(&e, 0x8000_0000 + 4 * words.len() as i64, &Run::default(), 1, 0).is_err(), "past the code");
        Ok(())
    }

    #[test]
    fn jump_tables() -> Result<()> {
        // jr table[a0] (0x80000100): case 0 calls with 100, case 1 with 200
        let words = [
            0x00044880, 0x3C088000, 0x01094021, 0x8D080100, 0, 0x01000008, 0, JAL_T, 0x24040064, 0x03E00008, 0, JAL_T,
            0x240400C8, 0x03E00008, 0,
        ];
        let code = image(&words, &[0x1C, 0, 0, 0x80, 0x2C, 0, 0, 0x80]);
        let e = Exe::with_base(&code, 0x8000_0000);
        assert_eq!(a0(&e, &Run { args: &[0], ..Run::default() })?, [100]);
        assert_eq!(a0(&e, &Run { args: &[1], ..Run::default() })?, [200]);
        Ok(())
    }

    #[test]
    fn signs_stores_and_stack_args() -> Result<()> {
        let words = [
            0x3C088000, // lui t0, 0x8000
            0x81040100, // lb a0, 0x100(t0): 0xFF
            JAL_T, 0, 0x85040102, // lh a0, 0x102(t0): 0x8000
            JAL_T, 0, 0x2884FFFF, // slti a0, a0, -1
            JAL_T, 0, 0x3C048000, // lui a0, 0x8000; sra a0, a0, 4
            JAL_T, 0x00042103, 0x240904D2, // sw 1234 at 0x80000200 (the image's end), then lw it back
            0xAD090200, 0x8D040200, JAL_T, 0, 0x8FA40010, // lw a0, 16(sp): the run's fifth argument
            JAL_T, 0, 0x03E00008, 0,
        ];
        let code = image(&words, &[0xFF, 0, 0, 0x80]);
        let e = Exe::with_base(&code, 0x8000_0000);
        let got = trace(&e, 0x8000_0000, &Run { args: &[0, 0, 0, 0, 77, -5], ..Run::default() }, usize::MAX, T)?;
        let a0: Vec<i64> = got.iter().map(|a| a[0]).collect();
        assert_eq!(a0, [-1, -32768, 1, -0x0800_0000, 1234, 77]);
        assert_eq!(got.last().map(|a| [a[4], a[5]]), Some([77, -5]), "stack words sp+16, sp+20");
        Ok(())
    }

    #[test]
    fn returns_jumps_to_zero_and_the_step_limit() -> Result<()> {
        // jr ra with ra read from a stack never written: a return
        let ret = image(&[0x8FBF0020, 0, 0x03E00008, 0], &[]);
        assert!(a0(&Exe::with_base(&ret, 0x8000_0000), &Run::default())?.is_empty());
        let jump_0 = image(&[0x01000008, 0], &[]);
        assert!(a0(&Exe::with_base(&jump_0, 0x8000_0000), &Run::default()).is_err(), "jr t0 = 0");
        let forever = image(&[0x1000FFFF, 0], &[]);
        assert!(a0(&Exe::with_base(&forever, 0x8000_0000), &Run::default()).is_err(), "runs too long");
        Ok(())
    }

    #[test]
    fn texts_as_the_call_reads_them() -> Result<()> {
        // "A" stored at sp+0x40 and printed; then a0 in the scratchpad, never stored
        let words = [0x24090041, 0xA3A90040, 0xA3A00041, JAL_T, 0x27A40040, 0x3C041F80, JAL_T, 0, 0x03E00008, 0];
        let code = image(&words, &[]);
        let got = trace_text(&Exe::with_base(&code, 0x8000_0000), 0x8000_0000, &Run::default(), usize::MAX, T)?;
        let texts: Vec<Option<String>> = got.into_iter().map(|(_, t)| t).collect();
        assert_eq!(texts, [Some("A".to_string()), None]);
        Ok(())
    }
}
