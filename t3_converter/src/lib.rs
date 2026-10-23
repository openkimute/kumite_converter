//! The Tekken 3 converter: ported from Python tools (since removed), writing their exact output.
//! One module per former Python file, under the same name.

// (the screens' tables are large json! literals)
#![recursion_limit = "256"]

pub mod anim;
pub mod assemble;
pub mod atlas;
pub mod camera_motion;
pub mod convert_all;
pub mod disc;
pub mod embu;
pub mod export_effects;
pub mod export_gamedata;
pub mod export_gltf;
pub mod export_media;
pub mod export_pictures;
pub mod export_shadows;
pub mod export_stages;
pub mod export_theater;
pub mod extract_models;
pub mod gamedata_core;
pub mod gamedata_moves;
pub mod gamedata_tables;
pub mod glb;
pub mod mips;
pub mod np;
pub mod pyjson;
pub mod qa_glb;
pub mod reference;
pub mod skel;
pub mod skin;
pub mod t3m;
pub mod texrender;
pub mod tim;

pub type Main = fn(&[String]) -> anyhow::Result<()>;
