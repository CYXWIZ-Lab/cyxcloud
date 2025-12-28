//! Cross-platform console symbols
//!
//! Provides ASCII-safe symbols for Windows compatibility.

/// Checkmark symbol
#[cfg(windows)]
pub const CHECK: &str = "[OK]";
#[cfg(not(windows))]
pub const CHECK: &str = "\u{2713}"; // ✓

/// Cross/error symbol
#[cfg(windows)]
pub const CROSS: &str = "[X]";
#[cfg(not(windows))]
pub const CROSS: &str = "\u{2717}"; // ✗

/// Box drawing - top line
#[cfg(windows)]
pub const BOX_TOP: &str = "+----------------------------------------------------------+";
#[cfg(not(windows))]
pub const BOX_TOP: &str = "\u{2554}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2557}";

/// Box drawing - bottom line
#[cfg(windows)]
pub const BOX_BOTTOM: &str = "+----------------------------------------------------------+";
#[cfg(not(windows))]
pub const BOX_BOTTOM: &str = "\u{255A}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255D}";

/// Box drawing - vertical line with space
#[cfg(windows)]
pub const BOX_SIDE: &str = "|";
#[cfg(not(windows))]
pub const BOX_SIDE: &str = "\u{2551}"; // ║
