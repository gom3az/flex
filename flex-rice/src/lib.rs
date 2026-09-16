//! `flex-rice` — the rice-specific half of flex.
//!
//! The eight providers here adapt this machine's tools to the generic
//! [`flex_core`] menu engine: hyprpaper (wallpaper), cliphist (clip),
//! nmcli (wifi), wpctl/brightnessctl (center), the theme directories
//! (theme), `.desktop` entries (launch), `grim`/`slurp` (shot) and
//! systemctl (power). The `flex` binary (`src/main.rs`) parses the provider
//! names, renders exactly one menu and prints a single `ACTION:` line; the
//! shell wrappers in `wrappers/` resolve that line into side effects.
//!
//! Nothing in [`flex_core`] depends on this crate — the dependency runs one
//! way, so the engine stays publishable and this crate stays local.

pub mod providers;

pub use providers::{menu, tick_hook};
