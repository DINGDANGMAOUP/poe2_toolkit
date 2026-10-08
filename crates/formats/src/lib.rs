//! Bounded game resource parsing and capability detection.
pub mod bundle;
mod bundle_store;
pub mod client;
mod csd;
pub mod dat;
mod ggpk;
pub mod installation;
pub mod resource;
pub mod schema;
pub mod stream;
pub use installation::*;
pub use resource::*;
