//! Shared type definitions for the Hypercolor RGB lighting engine.
//!
//! This crate contains all data types used across crate boundaries.
//! No logic, no I/O, no async — pure data structures with serde derives.
pub mod attachment;
pub mod audio;
pub mod canvas;
pub mod config;
pub mod controls;
pub mod device;
pub mod effect;
pub mod event;
pub mod library;
pub mod palette;
pub mod scene;
pub mod sensor;
pub mod server;
pub mod session;
pub mod spatial;
pub mod viewport;
