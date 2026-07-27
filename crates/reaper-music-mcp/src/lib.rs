//! `reaper-music-mcp` — the QLabs REAPER Music Intelligence MCP server.
//!
//! A hand-written, synchronous, zero-dependency MCP server speaking JSON-RPC
//! 2.0 over stdio. It turns the workspace's music-theory engine into an MCP
//! surface: fourteen tools, thirteen resources and nine prompts, plus a CLI
//! that runs the whole engine with no REAPER present.
//!
//! # The three invariants
//!
//! **stdout is protocol-only.** [`rpc::Writer`] is the single sink for stdout
//! bytes and only ever writes one compact JSON value per line. Every
//! diagnostic, including panics, goes to stderr through [`log`]. The
//! `stdout_contains_no_non_mcp_text` integration test drives the real binary
//! and asserts every stdout line parses as a JSON-RPC message.
//!
//! **Schemas are enforced in both directions.** Every tool declares an
//! `inputSchema` and an `outputSchema` in JSON Schema 2020-12
//! ([`schema_gen`]); both are compiled with [`qjson::schema::Schema`] and
//! checked at call time. Arguments that fail never reach a tool, and a result
//! that fails its own declared schema is reported as a bug rather than served.
//!
//! **Staging cannot overwrite changed material.** Every `EditPlan` the server
//! builds carries all seven preconditions — project uuid, state-change count,
//! item guid, take guid, MIDI hash, tempo-map hash and item bounds — is
//! converted with [`reaper_ipc::plan::plan_to_wire`], and echoes the selection
//! scope it was generated against so the bridge re-derives the same snapshot.
//! See [`staging`].
//!
//! # Module map
//!
//! | Module | Purpose |
//! |---|---|
//! | [`cli`] | argument parsing and the non-`serve` subcommands |
//! | [`config`] | resolving the IPC directory and installation token |
//! | [`log`] | stderr diagnostics, redaction and the panic hook |
//! | [`rpc`] | JSON-RPC framing and the single stdout sink |
//! | [`server`] | the dispatch loop, `initialize`, cancellation |
//! | [`tools`] | the fourteen tools |
//! | [`resources`] | the thirteen resources and their URI parser |
//! | [`prompts`] | the nine prompt templates |
//! | [`store`] | the bounded, TTL'd session store |
//! | [`schema_gen`] | tool schema declaration and compilation |
//! | [`staging`] | building an edit plan from a candidate |
//! | [`bridge`] | the REAPER bridge, and what happens when it is absent |
//! | [`convert`] | snapshot to domain-model translation |
//! | [`doctor`] | the installation self-check |
//! | [`fixtures`] | the REAPER-free fixture commands |
//! | [`error`] | the structured tool-error vocabulary |
//!
//! # Example
//!
//! ```
//! use reaper_music_mcp::schema_gen::ToolRegistry;
//!
//! let tools = ToolRegistry::compile().expect("schemas compile");
//! assert_eq!(tools.all().len(), 14);
//! assert!(tools.get("execute_lua").is_none());
//! ```

#![warn(missing_docs)]

pub mod bridge;
pub mod cli;
pub mod config;
pub mod convert;
pub mod doctor;
pub mod error;
pub mod fixtures;
pub mod log;
pub mod prompts;
pub mod resources;
pub mod rpc;
pub mod schema_gen;
pub mod server;
pub mod staging;
pub mod store;
pub mod tools;

/// The MCP protocol version this server implements.
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

/// The server name reported at `initialize`.
pub const SERVER_NAME: &str = "qlabs-reaper-music-mcp";

/// The server version, taken from the crate manifest.
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The binary name, for help text and MCP host configuration.
pub const BINARY_NAME: &str = "qlabs-reaper-music-mcp";
