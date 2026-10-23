//! Tekken 2's hanging chains (Roger's / Alex's tail, Kunimitsu's ribbons, belts, braids), swung by the game's own
//! routine run in the MIPS interpreter.
//!
//! The game keeps a 360-byte record per player at 0x800C4638 + 0x168 * player: model arc part 2 copied there when
//! the fighter loads (0x80080FDC), its chains' state written into it as they move. 0x800810D4 lays the links out
//! (hanging from the fighters' places), 0x800811C4 (fighter, 1) steps them once a frame (0x8005FFC0): fixed-point
//! Verlet with gravity and drag, kept above the floor and off planes on the body, the links hanging from their bone's
//! world matrix (fighter +0x600 + 0x50 * bone, a PSX MATRIX) and their world matrices handed to the drawing
//! 0x8006FC00 (player, count, MATRIX[count], 0x20 apart). Its GTE helpers run here: SetRotMatrix / SetTransMatrix
//! (0x80071CF8), RotTrans (0x8001CF4C), SquareRoot0 (0x80086FDC); the drawing call is where the link matrices are
//! read.
//!
//! Tekken 2 world space: y down, game units (~1 mm); rotations 4096 = 1.

use crate::mips::{Cpu, s32};
use crate::ps1::{M3, V3};
use anyhow::Result;

const EXE_LOAD: u32 = 0x80010000;
const RECORDS: u32 = 0x800C4638; // per player 0x168 bytes
const RECORD: usize = 0x168;
const LAYOUT: u32 = 0x800810D4;
const STEP: u32 = 0x800811C4;
const SET_MATRIX: u32 = 0x80071CF8;
const ROT_TRANS: u32 = 0x8001CF4C;
const SQRT: u32 = 0x80086FDC;
const DRAW: u32 = 0x8006FC00;
const GAME_STATE: u32 = 0x800A4150; // 0x800627CC: states 6 / 7 hold the chains still
const FIGHTER: u32 = 0x80100000;
const OTHER: u32 = 0x80102000;
const BONE_MATRICES: u32 = 0x600; // fighter + 0x600 + 0x50 * bone: the bone's world MATRIX

fn s16(v: u32) -> f64 {
    v as u16 as i16 as f64
}

/// The GTE state and what the drawing call was handed.
struct Gte {
    m: M3,
    t: V3,
    out: Option<Vec<(M3, V3)>>,
}

fn matrix(c: &mut Cpu, a: u32) -> (M3, V3) {
    let mut m = [[0.0; 3]; 3];
    for i in 0..9 {
        m[i / 3][i % 3] = s16(c.read(a + 2 * i as u32, 2));
    }
    let t = [0, 1, 2].map(|i| s32(c.read(a + 0x14 + 4 * i, 4)) as f64);
    (m, t)
}

impl Gte {
    fn hook(&mut self, c: &mut Cpu, pc: u32) -> Result<u32> {
        match pc {
            SET_MATRIX => (self.m, self.t) = matrix(c, c.r[4]),
            ROT_TRANS => {
                let v = [0, 1, 2].map(|i| s16(c.read(c.r[4] + 2 * i, 2)));
                for i in 0..3 {
                    let mv = self.m[i][0] * v[0] + self.m[i][1] * v[1] + self.m[i][2] * v[2];
                    let o = (mv / 4096.0).floor() + self.t[i];
                    c.write(c.r[5] + 4 * i as u32, 4, o as i64 as u32)?;
                }
                c.write(c.r[6], 4, 0)?;
            }
            SQRT => {
                let a = s32(c.r[4]);
                return Ok(if a > 0 { (a as u64).isqrt() as u32 } else { 0 });
            }
            _ => {
                let (n, base) = (s32(c.r[5]), c.r[6]);
                self.out = Some((0..n.max(0) as u32).map(|k| matrix(c, base + 0x20 * k)).collect());
            }
        }
        Ok(0)
    }
}

/// One fighter's chains: `part2` its model's arc part 2, `character` its Tekken 2 id (fighter +0x1A).
pub struct ChainSim {
    cpu: Cpu,
    part2: Vec<u8>,
    character: u32,
    gte: Gte,
}

/// {Tekken 2 bone: (3x3 rotation, translation)} in Tekken 2 world space (game units), in bone order.
pub type Bones = Vec<(usize, M3, V3)>;

impl ChainSim {
    pub fn new(exe: &[u8], part2: &[u8], character: u32) -> ChainSim {
        let mut cpu = Cpu::new();
        cpu.map(EXE_LOAD, exe.get(0x800..).unwrap_or(&[]));
        let mut p = part2[..part2.len().min(RECORD)].to_vec();
        p.resize(RECORD, 0);
        cpu.hooks = vec![SET_MATRIX, ROT_TRANS, SQRT, DRAW];
        ChainSim {
            cpu,
            part2: p,
            character,
            gte: Gte { m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3], out: None },
        }
    }

    fn place(&mut self, bones: &Bones) -> Result<()> {
        let c = &mut self.cpu;
        for (k, r, t) in bones {
            let a = FIGHTER + BONE_MATRICES + 0x50 * *k as u32;
            for i in 0..9 {
                let m = (r[i / 3][i % 3] * 4096.0).round_ties_even().clamp(-32768.0, 32767.0) as i64;
                c.write(a + 2 * i as u32, 2, m as u32)?;
            }
            for (i, v) in t.iter().enumerate() {
                c.write(a + 0x14 + 4 * i as u32, 4, v.round_ties_even() as i64 as u32)?;
            }
        }
        Ok(())
    }

    fn run(&mut self, pc: u32, args: &[u32]) -> Result<()> {
        let gte = &mut self.gte;
        self.cpu.call(pc, args, &mut |c, pc| gte.hook(c, pc))?;
        Ok(())
    }

    /// A fresh fighter at `place` (x, z: fighter +0x2E8 / +0x2F0) with its chains laid out.
    pub fn start(&mut self, bones: &Bones, place: (f64, f64)) -> Result<()> {
        let c = &mut self.cpu;
        c.write(GAME_STATE, 4, 0)?;
        for (base, player) in [(FIGHTER, 0), (OTHER, 1)] {
            c.map(base, &[0; 0x1000]);
            c.write(base + 0x14, 2, player)?;
            c.write(base + 0x1A, 2, if player == 0 { self.character } else { 0 })?;
            c.write(base + 0x2E8, 4, place.0 as i64 as u32)?;
            c.write(base + 0x2F0, 4, place.1 as i64 as u32)?;
        }
        c.map(RECORDS, &self.part2);
        c.map(RECORDS + RECORD as u32, &[0; RECORD]); // the other player: no chains
        self.place(bones)?;
        self.run(LAYOUT, &[FIGHTER, OTHER])
    }

    /// One frame with the bones at `bones`: the links' world matrices [(R 3x3, T)] (Tekken 2 space; R 4096 = 1).
    pub fn step(&mut self, bones: &Bones) -> Result<Vec<(M3, V3)>> {
        self.place(bones)?;
        self.gte.out = None;
        self.run(STEP, &[FIGHTER, 1])?;
        Ok(self.gte.out.take().unwrap_or_default())
    }
}
