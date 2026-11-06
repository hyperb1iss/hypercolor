//! The Windows programs outside Hypercolor's release units.
//!
//! `Hypercolor.exe` and the two recovery bootstrap slots are the installer's
//! own, so an update never replaces the code that decides what runs while
//! that update is in doubt. What they decide lives in
//! `hypercolor_install::windows`, where it runs on any platform against
//! fakes. This crate holds the two entry points and the few Win32 calls
//! that crate's safe surface does not cover.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod failure_copy;

#[cfg(windows)]
pub mod bootstrap;
#[cfg(windows)]
mod dialog;
#[cfg(windows)]
mod job;
#[cfg(windows)]
pub mod launcher;
#[cfg(windows)]
mod service_security;
