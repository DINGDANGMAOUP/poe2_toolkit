//! Application services shared by desktop and CLI.
pub mod market;
pub mod runtime;
pub mod store;
pub mod transaction;
pub use market::MarketClient;
pub use runtime::{AppState, Command, ServiceHandle};
pub use store::{Store, default_data_dir};
pub mod app_updates;
pub mod discovery;
mod game_guard;
mod maintenance;
pub mod schedule;
pub mod update_activity;
pub mod updates;

mod application_monitor;
