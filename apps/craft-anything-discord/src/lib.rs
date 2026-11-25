//! The pure core of the Discord porter bot: no Discord types and no network, so every part is
//! unit-testable. The bot's I/O (serenity/poise, downloads, the rebuild child process) builds on it.
//!
//! - [`config`]: the env file and its required keys (`CRAFT_DISCORD_ENV`, default `~/.config/craft-discord/.env`).
//! - [`events`]: one `rebuild --events ndjson` stdout line → a typed [`events::Event`].
//! - [`board`]: the progress board state, folded from events, rendered to plain view data.
//! - [`log`]: one event → one line for the batched "Log" thread.
//! - [`coalesce`]: rate-limit gate for edits, and packing log lines into 2000-character messages.
//! - [`queue`]: one rebuild at a time, FIFO.
//! - [`slug`]: app name → `port-<slug>` channel names.

pub mod board;
pub mod coalesce;
pub mod config;
pub mod events;
pub mod log;
pub mod queue;
pub mod slug;
