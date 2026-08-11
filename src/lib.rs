//! agent-graph-tui: re-exports for integration testing.
//!
//! The binary (`src/main.rs`) drives the TUI; the library is here so
//! `tests/*.rs` can `use agent_graph_tui::parser::*` etc.

pub mod app;
pub mod parser;
pub mod render;
pub mod tree;
