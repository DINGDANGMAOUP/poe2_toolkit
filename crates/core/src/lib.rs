//! Domain contracts. This crate has no UI, network, database or platform I/O.
pub mod game;
pub mod market;
pub mod patch;
pub mod profile;
pub use game::*;

pub use market::*;
pub use patch::*;
pub use profile::*;

pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
mod rules;
pub use rules::*;

pub mod enhancements;
pub use enhancements::*;
