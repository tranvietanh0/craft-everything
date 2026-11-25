//! The Discord porter bot's library: no Discord types, so every part is unit-testable. Most of it
//! is pure; [`intake`], [`identify`], [`run`], [`review`], [`plane`] and [`repo`] also hold the
//! blocking I/O (the download, the CLI calls, the rebuild child, Plane's API, `gh` and `git`).
//! The Discord side (serenity) is in the binary's `discord` module.
//!
//! - [`config`]: the env file and its required keys (`CRAFT_DISCORD_ENV`, default `~/.config/craft-discord/.env`).
//! - [`events`]: one `rebuild --events ndjson` stdout line → a typed [`events::Event`].
//! - [`board`]: the progress board state, folded from events, rendered to plain view data.
//! - [`log`]: one event → one line for the batched "Log" thread.
//! - [`coalesce`]: rate-limit gate for edits, and packing log lines into 2000-character messages.
//! - [`queue`]: one rebuild at a time, FIFO.
//! - [`slug`]: app name → `port-<slug>` channel names.
//! - [`intake`]: accepted file types and sizes, and the capped download into `PORTS_ROOT/<slug>/input/`.
//! - [`identify`]: the app's label and package through the Craft CLI's Android tools.
//! - [`run`]: the `rebuild` command line, the end-of-run verdict and the child process.
//! - [`tester`]: tester-card rules (who may review, one card per round, guide paths, Plane states and identifiers).
//! - [`guide`]: reading a feature's test guide from inside the project folder for its card.
//! - [`state`]: what the bot keeps per port in `PORTS_ROOT/<slug>/bot.json` (cards, Plane and GitHub ids).
//! - [`review`]: a tester's Pass or Fail, recorded through the engine, and the note left for the builder.
//! - [`plane`]: the optional Plane sync (a project per port, a work item per feature, bugs).
//! - [`repo`]: the optional GitHub repo per port (`gh`, git over SSH, the `.gitignore`).
//! - [`text`]: clipping text to Discord's limits.

pub mod board;
pub mod coalesce;
pub mod config;
pub mod events;
pub mod guide;
pub mod identify;
pub mod intake;
pub mod log;
pub mod plane;
pub mod queue;
pub mod repo;
pub mod review;
pub mod run;
pub mod slug;
pub mod state;
pub mod tester;
pub mod text;
