//! Executors: one module per provider, porting the wrapper logic.
//!
//! The binaries stay thin (argv → shared runner → select → `exec::<provider>`)
//! while each `exec` module owns the side effects the matching shell wrapper
//! in `wrappers/` used to own. Integration tests reach the library
//! (`src/bin/*.rs` is not importable), so executors must live here for the
//! dispatch coverage to become Rust tests. The wrappers stay live until
//! cutover; each module documents its deliberate departures from its wrapper.

pub mod center;
pub mod clip;
pub mod launch;
pub mod power;
pub mod shot;
pub mod theme;
pub mod wallpaper;
pub mod wifi;
