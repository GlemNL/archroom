//! `archroom-catalog`: the SQLite-backed catalog (plan §5). M0 covers
//! create/open, migrations, WAL and online backup; row-level queries and the
//! `Criterion` filter registry land in M1/M2.

mod catalog;
pub mod command;
mod error;
pub mod repo;
mod schema;

pub use catalog::Catalog;
pub use error::{Error, Result};
