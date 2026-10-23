//! camera_golden.py: write golden traces of the reference fight camera (camera.rs) for the Rust port's tests.
//!
//! Usage: t3_convert camera_golden OUT.json (the tests read ASSETS/tests/camera_golden.json)
//! Each scenario lists, per frame, the two samples [x, y, z, side], frames_since_start, move_cam and the
//! resulting RVIEW [eye x, eye y, eye z, pitch, yaw, H]. Needs the disc image (camera.rs reads its tables).

use super::camera::{FightCamera, STAND_Y as S};
use crate::pyjson;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

type P = [i64; 3];

#[allow(clippy::too_many_arguments)]
fn scenario(
    name: &str,
    frames: i64,
    traj: impl Fn(i64) -> (P, P),
    intro: bool,
    variant: i64,
    first_round: bool,
    fss0: i64,
    mc: impl Fn(i64) -> [i64; 2],
) -> Result<Value> {
    let mut cam = FightCamera::new(intro, variant, first_round)?;
    let mut rows = Vec::new();
    for f in 0..frames {
        let (p1, p2) = traj(f);
        let r = cam.step(p1, p2, fss0 + f, mc(f)).context("the camera mixed no slot")?;
        rows.push(json!({"p": [p1, p2], "fss": fss0 + f, "mc": mc(f),
                         "view": [r.eye[0], r.eye[1], r.eye[2], r.pitch, r.yaw, r.h]}));
    }
    Ok(json!({"name": name, "intro": intro, "variant": variant, "first_round": first_round, "frames": rows}))
}

/// The common case: no intro, frames_since_start from 1000, no move_cam.
fn plain(name: &str, frames: i64, traj: impl Fn(i64) -> (P, P)) -> Result<Value> {
    scenario(name, frames, traj, false, 0, true, 1000, |_| [0, 0])
}

fn jump(f: i64) -> (P, P) {
    let h = if (60..100).contains(&f) {
        (4800.0 * ((f - 60) as f64 / 40.0) * (1.0 - (f - 60) as f64 / 40.0)) as i64
    } else {
        0
    };
    ([-1500, S, 0], [1500, S - h, 0])
}

fn side(f: i64, speed: f64) -> (P, P) {
    let a = (f.min(60) as f64 * speed).to_radians();
    ([0, S, 0], [(2000.0 * a.cos()) as i64, S, (2000.0 * a.sin()) as i64])
}

/// both drift and P2 crosses over to P1's side (side switch freeze)
fn walk(f: i64) -> (P, P) {
    ([-1000 + 10 * f, S, 300 - 3 * f], [1000 - 15 * f, S, -200 + 2 * f])
}

pub fn main(args: &[String]) -> Result<()> {
    let Some(out_path) = args.first() else { bail!("usage: t3_convert camera_golden OUT.json") };
    let mut out = Vec::new();
    for s in [1500i64, 3000, 6000] {
        out.push(plain(&format!("sep{s}"), 240, |_| ([(-s).div_euclid(2), S, 0], [s / 2, S, 0]))?);
    }
    out.push(plain("jump", 140, jump)?);
    out.push(plain("grow", 200, |f| {
        let s = 1500 + f.min(90) * 50;
        ([(-s).div_euclid(2), S, 0], [s / 2, S, 0])
    })?);
    out.push(plain("sidestep", 200, |f| side(f, 0.75))?);
    out.push(scenario(
        "sidestep_capped",
        200,
        |f| side(f, 1.5),
        false,
        0,
        true,
        1000,
        |f| {
            if f < 120 { [0x28, 0] } else { [0, 0x2A] }
        },
    )?);
    out.push(scenario("round_start_freeze", 260, |f| side(f, 1.0), false, 0, true, 0, |_| [0, 0])?);
    out.push(plain("cross", 220, walk)?);
    out.push(scenario("intro_v0", 130, |_| ([-1000, S, 0], [1000, S, 0]), true, 0, true, 1000, |_| [0, 0])?);
    out.push(scenario(
        "intro_v8_round2",
        80,
        |_| ([-1250, S, 100], [1250, S, -100]),
        true,
        8,
        false,
        1000,
        |_| [0, 0],
    )?);
    let frames: usize = out.iter().map(|s| s["frames"].as_array().map_or(0, |a| a.len())).sum();
    let doc = json!({"source": "tools/t3_converter/reference/camera.py", "scenarios": out});
    if let Some(dir) = std::path::Path::new(out_path).parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot make {}", dir.display()))?;
    }
    std::fs::write(out_path, pyjson::compact(&doc)).with_context(|| format!("cannot write {out_path}"))?;
    println!("{frames} frames");
    Ok(())
}
