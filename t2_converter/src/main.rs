//! `t2_convert BIN [OUTDIR] [characters,stages,media]` (convert_all), or `t2_convert SCRIPT ARGS...` for one of
//! the scripts: export_characters, export_stages, export_media, transplant.

use t2_converter::*;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    let r = match args.first().map(String::as_str) {
        Some("export_characters") => export_characters::main(rest),
        Some("export_stages") => export_stages::main(rest),
        Some("export_media") => export_media::main(rest),
        Some("transplant") => transplant::main(rest),
        _ => convert_all::main(&args),
    };
    if let Err(e) = r {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
