//! Mutable user-state storage, kept separate from replaceable dictionary data.

pub mod db;
pub mod jobs;
pub mod mining;
pub mod recent;
pub mod settings;

pub use db::Storage;
