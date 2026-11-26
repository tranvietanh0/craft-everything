//! The Discord porter bot's library: no Discord types, so every part is unit-testable. Most of it
//! is pure; [`intake`], [`identify`], [`run`], [`review`], [`plane`], [`repo`] and [`pretrust`] also
//! hold the blocking I/O (the download, the CLI calls, the rebuild child, Plane's API, `gh` and
//! `git`, Claude Code's config).
//! The Discord side (serenity) is in the binary's `discord` module.
//!
//! - [`config`]: the env file and its required keys (`CRAFT_DISCORD_ENV`, default `~/.config/craft-discord/.env`).
//! - [`cost`]: tokens and cost summed from `turn_done` events (`~$` for an estimate).
//! - [`events`]: one `rebuild --events ndjson` stdout line → a typed [`events::Event`].
//! - [`board`]: the progress board state, folded from events, rendered to plain view data.
//! - [`log`]: one event → one line for the batched "Log" thread.
//! - [`coalesce`]: rate-limit gate for edits, and packing log lines into 2000-character messages.
//! - [`queue`]: one rebuild at a time, FIFO.
//! - [`saved_queue`]: the queue on disk (`PORTS_ROOT/.bot/queue.json`) and what a restart does with it.
//! - [`jobs`]: a port's runs (`/port`, `/resume`, `/retry`), their status, times and usage.
//! - [`activity`]: what happened on a port and when, for the daily summary.
//! - [`daily`]: the daily summary: when it is due, what it says, the last date posted.
//! - [`control`]: the porter's control commands' rules (`/status`, `/stop`, `/resume`, `/retry`, `/note`).
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
//! - [`pretrust`]: the optional trust of a port's project folder in Claude Code's config before a herdr build (`PRETRUST_PORTS`).
//! - [`mirror`]: projects made in the craft desktop app, registered on creation and mirrored by polling their files into the same events.
//! - [`first_look`]: a mirrored project's first report, from what the study wrote.
//! - [`text`]: clipping text to Discord's limits.

pub mod activity;
pub mod board;
pub mod coalesce;
pub mod config;
pub mod control;
pub mod cost;
pub mod daily;
pub mod events;
pub mod first_look;
pub mod guide;
pub mod identify;
pub mod intake;
pub mod jobs;
pub mod log;
pub mod mirror;
pub mod plane;
pub mod pretrust;
pub mod queue;
pub mod repo;
pub mod review;
pub mod run;
pub mod saved_queue;
pub mod slug;
pub mod state;
pub mod tester;
pub mod text;
