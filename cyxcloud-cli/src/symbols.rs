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

/// Looking glass / search symbol
#[cfg(windows)]
pub const LOOKING_GLASS: &str = "[?]";
#[cfg(not(windows))]
pub const LOOKING_GLASS: &str = "\u{1F50D}"; // 🔍

/// Lock symbol
#[cfg(windows)]
pub const LOCK: &str = "[L]";
#[cfg(not(windows))]
pub const LOCK: &str = "\u{1F512}"; // 🔒

/// Pen / pencil symbol
#[cfg(windows)]
pub const PEN: &str = "[P]";
#[cfg(not(windows))]
pub const PEN: &str = "\u{270F}"; // ✏

/// Shield symbol
#[cfg(windows)]
pub const SHIELD: &str = "[S]";
#[cfg(not(windows))]
pub const SHIELD: &str = "\u{1F6E1}"; // 🛡

/// Warning symbol (alias for WARN)
pub const WARNING: &str = WARN;

/// Package symbol
#[cfg(windows)]
pub const PACKAGE: &str = "[P]";
#[cfg(not(windows))]
pub const PACKAGE: &str = "\u{1F4E6}"; // 📦

/// Sparkles symbol
#[cfg(windows)]
pub const SPARKLES: &str = "[*]";
#[cfg(not(windows))]
pub const SPARKLES: &str = "\u{2728}"; // ✨

/// Link symbol
#[cfg(windows)]
pub const LINK: &str = "[>]";
#[cfg(not(windows))]
pub const LINK: &str = "\u{1F517}"; // 🔗
