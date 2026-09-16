//! Providers: each parses one subprocess's stdout once into `Vec<Row>`.
//!
//! This is the rice-specific half of flex: every provider reads this
//! machine's tools and `$HOME` conventions, and every provider's action is
//! executed by the matching `exec` module, never here.

#[path = "providers/theme_.rs"]
pub mod theme_;

pub mod bt;
pub mod center;
pub mod clip;
pub mod launch;
pub mod net;
pub mod power;
pub mod proc;
pub mod shot;
pub mod wallpaper;
pub mod wifi;

use flex_core::{Menu, Row, RowId, Tab, TickHook};

/// No-op row id (the executor exits 0, no effect).
///
/// Defined once here and re-exported by [`center`] (which used to own the
/// constant), because every provider's empty state ends up as a `noop` row
/// (B-026).
pub const NOOP_ID: &str = "noop";

/// Placeholder row for a provider whose scan found nothing.
///
/// Selecting it yields the `noop` id, which every executor treats as a
/// no-op: the menu is never blank, and `Enter` on the placeholder
/// cannot act on a row that does not exist (B-026).
#[must_use]
pub fn empty_row(label: &str) -> Row {
    Row::new(RowId::new(NOOP_ID), label)
}

/// Per-tick refresh for the providers whose rows go stale while the menu is
/// open: `center` re-reads volume/brightness into its gauge in place, `wifi`
/// swaps in a background scan once it finishes, and `proc` re-sweeps `/proc`
/// keeping the cursor on the same pid.
///
/// Installed by [`menu`]; filter, focus and scroll survive all refreshes.
pub fn tick_hook(menu: &mut Menu) {
    if menu.provider == center::PROVIDER {
        center::refresh_gauges(menu);
    }
    if menu.provider == wifi::PROVIDER {
        wifi::refresh_scan(menu);
    }
    if menu.provider == proc::PROVIDER {
        proc::refresh(menu);
    }
    if menu.provider == net::PROVIDER {
        net::refresh(menu);
    }
    if menu.provider == bt::PROVIDER {
        bt::refresh(menu);
    }
}

/// [`Menu::new`] with [`tick_hook`] installed.
///
/// Use this instead of `Menu::new` anywhere in this crate: the engine has no
/// knowledge of which providers need refreshing, so a menu built without the
/// hook would silently stop updating gauges and Wi-Fi scans.
#[must_use]
pub fn menu(provider: impl Into<String>, tabs: Vec<Tab>) -> Menu {
    let hook: TickHook = tick_hook;
    Menu::new(provider, tabs).on_tick(hook)
}
