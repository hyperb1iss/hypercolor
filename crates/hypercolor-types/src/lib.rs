//! Shared type definitions for the Hypercolor RGB lighting engine.
//!
//! This crate contains domain types used across crate boundaries together
//! with their validation and canonicalization. It performs no I/O and owns
//! no async runtime.
pub mod api;
pub mod asset;
pub mod attachment;
pub mod audio;
pub mod canvas;
pub mod config;
pub mod config_registry;
pub mod control;
pub mod controls;
pub mod device;
pub mod display;
pub mod effect;
pub mod event;
pub mod identity;
pub mod layer;
pub mod library;
pub mod lighting;
pub mod media;
pub mod motherboard;
pub mod net;
pub mod pairing;
pub mod portable;
pub mod scene;
pub mod sensor;
pub mod server;
pub mod session;
pub mod spatial;
pub mod viewport;
