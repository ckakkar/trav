//! Quantum — the terminal front-end for Trav.

mod app;
/// Human-friendly sizes, rates, ETAs and progress bars (shared with `trav get`).
pub mod fmt;
mod ui;

pub use app::TuiApp;
