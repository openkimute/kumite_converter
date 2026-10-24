//! export_theater: Tekken 3's Theater mode and arcade ending data (record 301, loaded at 0x800B8D58) for the port.
//!
//! usage: t3_convert export_theater "Tekken_3_(USA)_(Track 1).bin" OUTDIR
//!
//!   OUTDIR/gamedata/theater.json   the Theater list (0x800BA160: movies, thumbnails, unlock masks and rules, titles,
//!                                  the name variants), the per-movie parameters (0x800B91AC: file, frames played,
//!                                  loop, white clear, caption group), the arcade ending table (0x800B95DC:
//!                                  [character][costume] -> movie), the burned-in captions (0x800B96D8) and their
//!                                  schedule (0x800B978C), the music player list (0x800BADAC), the staff roll
//!                                  (0x8010E258) with its fonts' advance tables, and the screen layouts (code
//!                                  immediates, with their routines; docs: theater research).
//!   OUTDIR/pictures/theater/*.png  the Tekken 3 box cover and spine, the CD icon, the two credits fonts, the NAMCO
//!                                  logo and the nine captions (RGBA; a caption's shadow texels are semi-transparent:
//!                                  the game draws them as dst/4 + colour/2).

use crate::disc::Game;
use crate::export_pictures::{render, save_png, valid_tim};
use crate::gamedata_core::Exe;
use crate::gamedata_tables::{imm, rgb};
use crate::pyjson;
use anyhow::{Context, Result, bail};
use conv_io::{Ctx, DiscFile, StdDisc, StdFs, join, print_line};
use image::RgbaImage;
use serde_json::{Value, json};

const BASE: i64 = 0x800B8D58;

/// `Rec.cstr`: the string at a pointer into the record (None for a null or outside pointer).
fn cstr(r: &Exe, a: i64, n: usize) -> Value {
    if a == 0 || !(BASE..BASE + r.d.len() as i64).contains(&a) {
        return Value::Null;
    }
    json!(r.cstr_n(a, n))
}

fn tim(r: &Exe, a: i64) -> Result<RgbaImage> {
    valid_tim(r.bytes(a, usize::MAX)).and_then(|t| render(&t)).with_context(|| format!("no TIM picture at 0x{a:X}"))
}

fn entries(r: &Exe) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for i in 0..26 {
        let e = 0x800BA160 + 28 * i;
        if r.s16(e)? == -2 {
            out.push(Value::Null);
            continue;
        }
        let [pic, mask, kind, l1, l2, l3] = [0, 1, 2, 3, 4, 5].map(|k| r.u32(e + 4 + 4 * k));
        out.push(json!({"movie": r.s16(e)?, "movie_alt": r.s16(e + 2)?, "picture": format!("r301/{:03}.png", pic?),
                        "mask": mask?, "kind": kind?, "line1": cstr(r, l1?, 64), "line2": cstr(r, l2?, 64),
                        "line3": cstr(r, l3?, 64)}));
    }
    Ok(out)
}

fn movies(r: &Exe) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for k in 0..25 {
        let m = 0x800B91AC + 16 * k;
        let flags = r.u16(m + 10)?;
        let group = (flags >> 8) & 15;
        out.push(json!({"file": format!("{:02}", r.u8(m)? - 1), "w": r.s16(m + 2)?, "h": r.s16(m + 4)?,
                        "frames": r.s16(m + 6)?, "volume": r.u8(m + 8)?, "loop": flags & 4 != 0, "white": flags & 8 != 0,
                        "end_mode": flags & 3, "captions": if group != 0 { json!(group - 1) } else { Value::Null }}));
    }
    Ok(out)
}

fn captions(ctx: &Ctx, r: &Exe, pic_dir: &str) -> Result<(Vec<Value>, Vec<Value>)> {
    let base = r.u32(0x8010E1E0)?;
    let pals = |p: i64| match p {
        2 => 0x800B9884,
        3 => 0x800B9904,
        _ => 0x800B9804,
    };
    let mut out = Vec::new();
    for i in 0..9 {
        let c = 0x800B96D8 + 20 * i;
        let (off, x, y, w, rows, pal) =
            (r.u32(c)?, r.s16(c + 4)?, r.s16(c + 6)?, r.s16(c + 10)?, r.s16(c + 12)?, r.s16(c + 16)?);
        let n = w / 2 * rows;
        let raw = r.bytes(base + off, usize::try_from(n).unwrap_or(0));
        if w < 0 || rows < 0 || w % 2 != 0 || raw.len() as i64 != n {
            bail!("caption {i}: a bad picture ({w} x {rows})");
        }
        let p = pals(pal);
        let mut lut = [[0u8; 4]; 16];
        for (k, l) in lut.iter_mut().enumerate() {
            let e = r.bytes(p + 8 * k as i64, 4);
            let [cr, cg, cb, blend] = <[u8; 4]>::try_from(e).context("a caption palette runs short")?;
            // replace (blend 0) or dst/4 + colour/2 (blend 1) ~ alpha 3/4 over the picture with colour * 2/3
            let f = |v: u8| if blend == 1 { (v as f32 * 2.0 / 3.0) as u8 } else { v };
            *l = [
                f(cr),
                f(cg),
                f(cb),
                if k == 0 {
                    0
                } else if blend == 1 {
                    191
                } else {
                    255
                },
            ];
        }
        let px: Vec<u8> = raw.iter().flat_map(|b| [b & 15, b >> 4]).flat_map(|i| lut[i as usize]).collect();
        let img = RgbaImage::from_raw(w as u32, rows as u32, px).context("a caption's size")?;
        let name = format!("theater/caption_{i}.png");
        save_png(ctx.out, &join(pic_dir, &name), &img)?;
        out.push(json!({"picture": name, "x": x, "y": y, "w": w, "h": rows}));
    }
    // groups: from a frame-0 entry to a 0xFFFF one; as many as the movies' caption groups
    let count = movies(r)?.iter().filter_map(|m| m["captions"].as_i64()).max().context("no movie has captions")? + 1;
    let (mut groups, mut a): (Vec<Option<Vec<Value>>>, i64) = (Vec::new(), 0x800B978C);
    while (groups.len() as i64) < count || groups.last().is_some_and(Option::is_some) {
        let (frame, cap) = (r.u16(a)?, r.s16(a + 2)?);
        a += 4;
        if frame == 0 {
            groups.push(Some(Vec::new()));
        } else if frame == 0xFFFF {
            if groups.len() as i64 >= count {
                groups.push(None);
            }
        } else {
            groups.last_mut().and_then(Option::as_mut).context("a caption before its group")?.push(json!([frame, cap]));
        }
    }
    Ok((out, groups.into_iter().flatten().map(Value::from).collect()))
}

fn staff_roll(r: &Exe) -> Result<Vec<Value>> {
    let (mut lines, mut a) = (Vec::new(), 0x8010E258);
    loop {
        let p = r.u32(a)?;
        a += 4;
        if p == 0 {
            break;
        }
        let s = cstr(r, p, 80);
        let s = s.as_str().context("a staff roll line outside the record")?;
        let mut cs = s.chars();
        let t = cs.next().and_then(|c| c.to_digit(10)).context("a staff roll line without its type")?;
        lines.push(json!({"type": t, "text": cs.as_str()}));
    }
    Ok(lines)
}

pub fn main(args: &[String]) -> Result<()> {
    let [src, out, ..] = args else { bail!("usage: export_theater BIN OUTDIR") };
    run(&Ctx { out: &StdFs(out.into()), log: &print_line, jobs: 1 }, &StdDisc(src.into()), "")
}

/// Writes `out`/gamedata/theater.json and `out`/pictures/theater/.
pub fn run(ctx: &Ctx, disc: &dyn DiscFile, out: &str) -> Result<()> {
    let g = Game::load(disc)?;
    let r = Exe::with_base(g.rec.get(301).context("no record 301")?, BASE);
    let r = &r;
    let pic_dir = join(out, "pictures");
    // the disc pictures (0x8010E204: T1 cover, spine, T2 cover, spine, T3 cover, spine, a Japanese sheet, the CD
    // icon), the credits fonts and the NAMCO logo
    let tims = r.u32s(0x8010E204, 8)?;
    let pictures = [
        ("cover", tims[4]),
        ("spine", tims[5]),
        ("cd", tims[7]),
        ("credits_small", 0x8010ED70),
        ("credits_large", 0x8010FF68),
        ("namco", 0x80111D30),
    ];
    for (name, a) in pictures {
        save_png(ctx.out, &join(&pic_dir, &format!("theater/{name}.png")), &tim(r, a)?)?;
    }
    let (caps, schedule) = captions(ctx, r, &pic_dir)?;
    let mut music = Vec::new();
    for k in 0..29 {
        let m = 0x800BADAC + 28 * k;
        music.push(json!({"bgm": r.u32(m)? >> 1, "name": cstr(r, r.u32(m + 16)?, 64)}));
    }
    let strings =
        |a: i64, n: i64| -> Result<Vec<Value>> { (0..n).map(|k| Ok(cstr(r, r.u32(a + 4 * k)?, 64))).collect() };
    let s = |a: i64| cstr(r, a, 64);
    let sp = |a: i64| -> Result<Value> { Ok(cstr(r, r.u32(a)?, 64)) };
    // the screens' layouts, from the code (0x80100FDC list, 0x8010193C music, 0x80100C2C buttons, 0x80102CA8
    // header) and its boxes (0x800BA028: x, y, w, h, colours from +12, the edge colour +28)
    let i = |a: i64| imm(r, a);
    let colour = |a: i64| -> Result<Value> { Ok(rgb(r.u32(a)?)) };
    let (bg, panel, frame) = (0x800BA028, 0x800BA048, 0x800BA068);
    let data = json!({
        "note": "record 301 (mode 19, 0x800FF0C4); coordinates in the 368x480 screen; docs: theater research",
        "entries": entries(r)?,
        // the grid (0x80100FDC: columns, then rows)
        "columns": i(0x80101438)?, "rows": i(0x80101454)?,
        "status": {"routine": "0x801004A8", "kinds": {
            "0": "endings seen & mask -> movie, else locked",
            "1": "alternate (Panda, bit 11) seen -> movie_alt; ending (Kuma) seen -> movie; else locked",
            "2": "alternate (Gun Jack again, bit 16) seen -> movie_alt; ending seen -> movie; else locked",
            "3": "alternate (Tiger, bit 8) seen -> movie; else locked"},
            "alternate_bits": {"1": 11, "2": 16, "3": 8}},
        "names": {"routine": "0x800FFE60", "note": "line1 \"0\"..\"3\": [bit test, names for first / second / both]",
                  "0": {"bits": [["seen", 4], ["seen", 19]], "names": [s(0x800BAB40), s(0x800BB0E4), s(0x800BB710)]},
                  "1": {"bits": [["seen", 11], ["alternate", 11]], "names": [s(0x800BAABC), s(0x800BB728), s(0x800BB730)]},
                  "2": {"bits": [["seen", 16], ["alternate", 16]], "names": [s(0x800BB73C), s(0x800BB0F8), s(0x800BB0F8)]},
                  "3": {"bits": [["seen", 14], ["seen", 20]], "names": [s(0x800BB104), s(0x800BB0D8), s(0x800BB74C)]}},
        "movies": movies(r)?,
        "ending": {"routine": "0x800FEFD4 / flags 0x800B1E88 (record 5)",
                   "table": (0..21).map(|c| (0..4).map(|k| r.u8(0x800B95DC + 4 * c + k)).collect::<Result<Vec<_>>>())
                       .collect::<Result<Vec<_>>>()?,
                   // (0x800FEFD4: this character's ending, the alternate bit (the lui's) set, plays this movie)
                   "second_clear": {"character": i(0x800FF08C)?, "movie": i(0x800FF0B4)?},
                   "alternate": {"note": "which clears set the alternate-ending bit instead of the ending bit",
                                 "rules": [{"character": 8, "costume_at_least": 2, "bit": 8},
                                           {"character": 11, "costume": 1, "bit": 11},
                                           {"character": i(0x800FF08C)?, "second_clear": true,
                                            "bit": (r.u16(0x800FF0A4)? << 16).trailing_zeros()}]},
                   "theater_unlock": 0x3FF},
        "captions": {"routine": "0x800FF964", "pictures": caps, "schedule": schedule,
                     "note": "schedule per group: [movie frame, caption or -1]; a caption shows from its frame to the next"},
        "music": {"routine": "0x80102740 / draw 0x8010193C", "tracks": music,
                  "columns": {"remixed": 0, "arcade": 1}},
        "screen": {
            "routine": "0x80101FE8 (draw), 0x8010217C (input)",
            "background": {"top": colour(bg + 12)?, "bottom": colour(bg + 20)?},
            "panel": {"x": r.s16(panel)?, "y": r.s16(panel + 2)?, "w": r.s16(panel + 4)?, "h": r.s16(panel + 6)?,
                      "top": colour(panel + 12)?, "bottom": colour(panel + 20)?},
            "header": {"y": i(0x80102E64)?, "idle": i(0x80102D1C)?, "title": strings(0x8010E240, 3)?, "push": sp(0x8010E224)?,
                       "title_colours": [i(0x80102E2C)?, i(0x80102E00)?, i(0x80102E2C)?],
                       "push_colours": [i(0x80102E90)?, i(0x80102EAC)?, i(0x80102E90)?, i(0x80102EAC)?]},
            "grid": {"x": i(0x801010BC)?, "y": i(0x801010B0)?, "w": i(0x801012F0)?, "h": i(0x80101334)?,
                     "locked": "r301/000.png", "frame": "r301/061.png"},
            "cursor": {"x": i(0x80101090)?, "y": i(0x8010109C)?, "w": r.s16(frame + 4)?, "h": r.s16(frame + 6)?,
                       "colour": colour(frame + 28)?, "blink": r.u16(0x80101038)?},
            "info": {"picture": [i(0x8010162C)?, i(0x80101634)?],
                     "name": {"font": i(0x801016D4)?, "colour": i(0x801016D8)?, "x": i(0x801016F8)?, "width": i(0x801016EC)?,
                              "y": i(0x801016FC)?},
                     "line2": {"font": i(0x8010172C)?, "colour": i(0x80101730)?, "x": i(0x80101758)?, "width": i(0x80101744)?,
                               "y": [i(0x80101734)?, i(0x80101774)?, i(0x801017B4)?]},
                     "locked": {"text": cstr(r, 0x800BB7AC + 8, 64), "font": i(0x801017F4)?, "colour": i(0x801017F8)?,
                                "x": i(0x80101800)?, "y": i(0x801017E4)?}},
            // (a button's box: its label padded by the margins its callers give, 0x80101490 / 0x80101494, the height of
            // the text, 0x80100CDC, between them)
            "buttons": {"y": i(0x8010149C)?, "font": i(0x80100C7C)?, "colours": [i(0x80101488)?, i(0x8010148C)?],
                        "margin": [i(0x80101490)?, i(0x80101494)?], "h": i(0x80100CDC)?,
                        "fill": rgb(r.u16(0x80100EA4)? << 16), "pulse": rgb(r.u16(0x80100E8C)? << 16 | r.u16(0x80100E90)?),
                        "labels": {"disc": s(0x800BB794), "music": s(0x800BB79C), "exit": s(0x800BB7A4),
                                   "theater": s(0x800BB7C8), "bgm": s(0x800BB7D0)},
                        "theater_x": {"all": [i(0x80101498)?, i(0x801014E0)?, i(0x80101524)?], "exit_only": i(0x80101550)?}},
            "help": {"font": i(0x801018D4)?, "colour": i(0x801018D8)?, "y": i(0x801018DC)?,
                     "theater": {"exit": sp(0x8010E200)?, "music": sp(0x8010E1F8)?, "disc": sp(0x8010E1E4)?},
                     "music": {"theater": sp(0x8010E1F4)?, "bgm": sp(0x8010E1FC)?,
                               "disc": sp(0x8010E1E8)?, "exit": sp(0x8010E200)?}},
            "music_player": {"frame": [i(0x80101960)?, i(0x80101968)?, i(0x80101970)?, i(0x80101978)?], "rows": i(0x80101C6C)?,
                             "row": {"x": i(0x80101BB8)?, "y": i(0x80101B14)?, "dy": i(0x80101C74)?, "font": i(0x80101B90)?,
                                     "colours": [i(0x80101B68)?, i(0x80101B5C)?, i(0x80101B84)?]},
                             "bar": [i(0x80101C24)?, i(0x80101A9C)?, i(0x80101C2C)?, i(0x80101C34)?], "buttons_x": i(0x80101C9C)?,
                             "buttons_y": {"theater": i(0x80101CA0)?, "bgm": i(0x80101CE8)?, "disc": i(0x80101D30)?,
                                           "exit": i(0x80101D78)?},
                             "name": {"font": i(0x80101DE8)?, "colour": i(0x80101DEC)?, "y": i(0x80101DF0)?, "dx": i(0x80101E14)?},
                             "cd": [i(0x80101E20)?, i(0x80101E24)?],
                             "kind": {"font": i(0x80101E4C)?, "colour": i(0x80101E50)?, "x": i(0x80101E58)?, "y": i(0x80101E3C)?,
                                      "remixed": s(0x800BB7E4), "arcade": s(0x800BB7F4)},
                             "cover": [i(0x80101F7C)?, i(0x80101F8C)?], "spine": [i(0x80101F9C)?, i(0x80101FA4)?]},
            "sfx": {"cursor": i(0x80102574)?, "confirm": i(0x80102670)?},
        },
        "staff_roll": {
            "routine": "0x80102EEC", "lines": staff_roll(r)?, "title": strings(0x8010E24C, 2)?,
            // (the title's two lines: from y 0x801031F8 (not traced: its base), the port's places)
            "title_y": [220, 238],
            "types": {"0": "heading", "1": "name", "2": "blank", "3": "heading +16, Tekken Ball unlocked",
                      "4": "heading, Tekken Ball not unlocked", "5": "name, Anna costume unlocked"},
            // the fonts' cells, glyphs per row and space (0x80104FA0: small, then large)
            "fonts": {"small": {"picture": "theater/credits_small.png", "cell": [i(0x80104FE8)?, i(0x80105014)?],
                                "per_row": i(0x80104FE0)?, "space": i(0x80104FEC)?, "advance": r.bytes(0x800BB918, 62)},
                      "large": {"picture": "theater/credits_large.png", "cell": [i(0x80105004)?, i(0x80105014)?],
                                "per_row": i(0x80104FFC)?, "space": i(0x80105008)?, "advance": r.bytes(0x800BB958, 62)},
                      "glyph": "c - 0x21 below 0x31, else c - 0x22; \"@\" is the copyright sign"},
            // (the title shows until frame 239 of its step; the scroll steps 1, or 2 on alternate frames)
            "timing": {"sfx_at": i(0x80103094)?, "sfx": r.u16(0x801030A4)?, "music_at": i(0x801030CC)?, "music": i(0x801030D8)?,
                       "gap": i(0x8010312C)?, "title_frames": i(0x801033BC)? + 1, "pitch": i(0x8010400C)?,
                       "speed": [-i(0x80104064)?, -i(0x80104050)?], "end": i(0x80104078)?, "logo_frames": i(0x801040A0)?},
            "logo": {"picture": "theater/namco.png", "x": i(0x80104D98)?, "y": i(0x80104D54)?, "in": i(0x801040C8)?,
                     "in_frames": i(0x801040A8)?, "hold": i(0x801040F8)? - 1, "out": -i(0x80104110)?},
            "flare": [i(0x80102FA0)?, i(0x80102FAC)?, i(0x80102FB8)?], // (a line at 152 - width / 2 (0x801031D8), plus the roll's x 0x80102F6C)
            "centre": i(0x801031D8)? + i(0x80102F6C)?,
        },
    });
    ctx.out.write(&join(out, "gamedata/theater.json"), pyjson::indent1(&data).as_bytes())?;
    let lines = data["staff_roll"]["lines"].as_array().map_or(0, Vec::len);
    ctx.log(&format!("theater.json: {} entries, {lines} credit lines", data["entries"].as_array().map_or(0, Vec::len)));
    Ok(())
}
