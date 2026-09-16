//! Executors: one module per provider, porting the retired wrapper logic.
//!
//! The binaries stay thin (argv → shared runner → select → `exec::<provider>`)
//! while each `exec` module owns the side effects the matching shell wrapper
//! used to own. Integration tests reach the library
//! (`src/bin/*.rs` is not importable), so executors must live here for the
//! dispatch coverage to become Rust tests. Each module documents its
//! deliberate departures from the wrapper it ported.

pub mod bt;
pub mod center;
pub mod clip;
pub mod launch;
pub mod mixer;
pub mod net;
pub mod power;
pub mod proc;
pub mod record;
pub mod shot;
pub mod speedtest;
pub mod theme;
pub mod wallpaper;
pub mod wifi;
