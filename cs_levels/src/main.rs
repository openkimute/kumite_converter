//! `cs_levels install LEVEL.glb ASSETS [STAGE_ID]`, `cs_levels force_stage ASSETS STAGE_ID BASE_STAGE [NAME]`:
//! an arena of the CS LEVELS menu, an extra Tekken Force stage; `cs_levels from_game GAME_DIR ASSETS`: all the
//! game's levels and the two Force stages. Each ends by refreshing ASSETS/manifest.json's fingerprint.
//! `cs_levels bsp2glb MAP.bsp OUT.glb [GAME_DIR]`: one level as a glb (to look at; the WADs from GAME_DIR, by default
//! the Half-Life folder the map is in).

use cs_levels::{force_stage, game, install};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("install") => install::main(&args[1..]),
        Some("force_stage") => force_stage::main(&args[1..]),
        Some("from_game") => game::main(&args[1..]),
        Some("bsp2glb") => game::bsp2glb_main(&args[1..]),
        _ => Err(anyhow::anyhow!(
            "usage: cs_levels install LEVEL.glb ASSETS [STAGE_ID]\n       cs_levels force_stage ASSETS STAGE_ID BASE_STAGE [NAME]\n       cs_levels from_game GAME_DIR ASSETS\n       cs_levels bsp2glb MAP.bsp OUT.glb [GAME_DIR]"
        )),
    };
    if let Err(e) = r {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
