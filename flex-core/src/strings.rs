//! Centralized UI state strings.
//!
//! Provides a single source of truth for string constants used across various
//! providers, ensuring consistency and making localization or modifications easier.

/// Placeholder label used when a provider is entirely offline.
pub const OFFLINE_LABEL: &str = "— offline";

/// Parenthetical shown when a Wi-Fi scan returns no available networks.
pub const NO_NETWORKS_LABEL: &str = "(No Wi-Fi networks)";

/// Parenthetical shown when a Bluetooth scan returns no paired devices.
pub const NO_DEVICES_LABEL: &str = "(No paired devices)";

/// Fallback text when a search/filter yields no results.
pub const EMPTY_SEARCH_LABEL: &str = "— no matches —";
