//! The Tekken 2 converter: ported from Python tools (since removed), writing their exact output. One module per
//! former Python file, under the same name; np.rs and pil.rs are the numpy and Pillow arithmetic they rely on. Nothing here
//! uses the Tekken 3 converter.

#![allow(clippy::type_complexity, clippy::needless_range_loop)] // numpy-like arrays and indices

pub mod chain_sim;
pub mod convert_all;
pub mod export_characters;
pub mod export_media;
pub mod export_stages;
pub mod mips;
pub mod np;
pub mod pil;
pub mod ps1;
pub mod pyjson;
pub mod transplant;
pub mod transplant_moves;
