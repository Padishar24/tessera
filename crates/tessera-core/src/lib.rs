//! Platform-independent logic of Tessera. Deliberately contains no Win32 code,
//! so that everything here can be tested via `cargo test`.

pub mod action;
pub mod config;
pub mod geometry;
pub mod ipc;
pub mod keys;
pub mod layout;
pub mod layout_edit;
pub mod nav;
pub mod rules;
pub mod state;
pub mod tree;
pub mod workspace;
