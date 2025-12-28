//! CLI Commands

pub mod auth;
pub mod delete;
pub mod download;
pub mod list;
pub mod status;
pub mod upload;

pub use delete::run as delete;
pub use download::run as download;
pub use list::run as list;
pub use status::run as status;
pub use upload::run as upload;
