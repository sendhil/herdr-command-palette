pub mod app;
pub mod client;
pub mod command;
pub mod context;
pub mod error;
pub mod executor;
pub mod input;
pub mod model;
pub mod registry;
pub mod state;
pub mod terminal;
pub mod view;

pub use app::{run_open, run_palette, AppError};
