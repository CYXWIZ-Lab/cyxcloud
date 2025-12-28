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

/// Warning symbol
#[cfg(windows)]
pub const WARN: &str = "[!]";
#[cfg(not(windows))]
pub const WARN: &str = "!";

/// Info symbol
#[cfg(windows)]
pub const INFO: &str = "[*]";
#[cfg(not(windows))]
pub const INFO: &str = "*";

/// Horizontal line (for headers)
#[cfg(windows)]
pub const HLINE: &str = "------------";
#[cfg(not(windows))]
pub const HLINE: &str = "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}"; // ────────────

/// Short horizontal line
#[cfg(windows)]
pub const HLINE_SHORT: &str = "-------";
#[cfg(not(windows))]
pub const HLINE_SHORT: &str = "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}"; // ───────
