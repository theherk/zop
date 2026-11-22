//! zop: every project, ranked by use.
//!
//! See `docs/introduction.md` (an mdBook) for the design.

#![deny(unsafe_code)]

pub mod cli;
pub mod config;
pub mod discover;
pub mod error;
pub mod expand;
pub mod ids;
pub mod index;
pub mod rank;
mod sys;
pub mod tmp;
pub mod zoxide;

pub use cli::run;
