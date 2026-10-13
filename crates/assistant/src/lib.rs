//! CADCraft assistant.
//!
//! A conversational agent that carries out drafting requests ("draw a 20×10 rectangle at the
//! origin and fillet its corners") with CADCraft's own commands. The model is any
//! OpenAI-compatible chat-completions endpoint ([`Provider`]); its tools are the MCP tools of
//! `cadcraft-mcp`, run against any [`cadcraft_mcp::Backend`] (a headless session or the app).
//! Model output is treated as hostile input: bounded steps, tool calls and result sizes, and
//! tools that replace the drawing or write files run only when the user confirms.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod agent;
#[cfg(feature = "http")]
mod openai;
mod provider;

pub use agent::{Agent, Config, Error, Event, MAX_STEPS, Observer, Turn};
#[cfg(feature = "http")]
pub use openai::OpenAiCompatible;
pub use provider::{Provider, Scripted, text_reply, tool_reply};

#[cfg(test)]
mod tests;
