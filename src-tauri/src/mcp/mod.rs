//! Outbound MCP clients only. See README.md for the explicit stdio trust boundary.

pub mod bridge;
pub mod client;
pub mod config;
mod glossmod_download;
mod redaction;
mod runtime;

pub use bridge::McpAgentTool;
pub use client::{ConnectionConsent, McpClient, McpTool};
pub(crate) use config::playwright_receipt_dir;
pub use config::{McpServers, McpTransport, ServerConfig};
pub use runtime::McpRuntime;

#[cfg(test)]
pub(crate) mod tests;
