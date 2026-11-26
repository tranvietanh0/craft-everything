# Discord porter bot

`craft-anything-discord` lets members of a Discord server port an Android app with one slash
command. `/port` takes an `.apk`, `.xapk`, `.apks` or `.aab` file (attached, or a download link)
and runs `craft-anything-cli rebuild` on it. Progress shows in a channel made for that port.

## What `/port` does

1. **Role check.** Only members with the porter role (`PORTER_ROLE_ID`) may use it. Anyone else
   gets a private refusal.
2. **Rights confirmation.** A private message with the button "I have the rights to port this
   app". Nothing starts until the person presses it (5 minutes, then it times out).
3. **Intake.** The file is downloaded into `PORTS_ROOT/<slug>/input/`. Files over `MAX_UPLOAD_MB`
   and other file types are refused. The app's label and package are read with
   `craft-anything-cli android_package_show` (and `android_resources_read` when the label is a
   resource). If that fails (an `.aab` or `.xapk`, a damaged APK), the name comes from the file
   name and the profile says "(name from file)".
4. **Channel.** `port-<slug>` is created under the ports category (`-2`, `-3`… when the name is
   taken). Its topic is the package and the target. The app profile and the progress board are
   posted and pinned, and a thread named "Log" is opened.
5. **Run.** One port runs at a time. Anyone else is told their place in line, and the next port
   starts when the current one ends. The line is saved (see [Restarts](#restarts)). The bot runs:

   ```sh
   $CRAFT_CLI rebuild <input file> "<request>" --deep --user-skills --unattended \
     --backend "Claude Code (herdr)" \
     --events ndjson --project-folder $PORTS_ROOT/<slug>/project [--build-with unity]
   ```

   The board is edited at most every 3 seconds. It shows the tokens and cost of the run and,
   once the port has had more than one run, the port's total (see [Cost](#cost)). Log lines go
   to the thread in batches. Only
   Unity is a stack the CLI knows, so `target: cocos` and `target: app` name the target in the
   request and let the builder choose the stack.

   The builder is `CRAFT_BACKEND`, by default **Claude Code (herdr)**: your own Claude Code on
   Windows, in a herdr pane you can watch (see [Claude Code in herdr](#claude-code-in-herdr)).
6. **End.** The board shows the final state. A run that finished OK mentions the tester role
   (`TESTER_ROLE_ID`). A failed run says why. If the program crashed or stopped without
   finishing, the end of its error output is shown (up to 1500 characters).

What the bot keeps about a port (its channel, its tester cards, the Plane and GitHub ids, its runs
with their status and cost, what happened when for the daily summary, and its last board) is in
`PORTS_ROOT/<slug>/bot.json`, so cards still work and the board is drawn again after the bot
restarts.

## Control commands

In a port's channel, members with the porter role (`PORTER_ROLE_ID`) can control its runs. Anyone
else, and any use outside a port's channel, gets a private refusal. Answers are private; an action
also gets one line in the channel.

| Command | What it does |
|---|---|
| `/status` | The port's stage, its features by state, its place in the queue (running, waiting at position N, or not queued), how long its last run ran, and its cost. |
| `/stop` | Stops the running build cleanly: the bot asks `craft-anything-cli` to stop (SIGTERM, which it treats as Ctrl+C: it stops the builder, which under herdr presses Escape in the pane, and prints `finished`). After 30 seconds the whole process group is ended. A port still waiting is taken out of the queue. The run is recorded as stopped. |
| `/resume` | Carries on a stopped, paused, failed or interrupted run where it stopped. The CLI supports this: the bot runs `rebuild - "<request>" … --continue-in $PORTS_ROOT/<slug>/project --resume`, which continues the project's saved session and record and ignores the request. It needs the project folder to hold a project; otherwise use `/retry`. |
| `/retry` | Queues the last run's request again from the start, keeping the project folder. The folder already holds a project, so the CLI takes the request as a follow-up on it (`--project-folder`, without `--build-with`: the project keeps its stack). |
| `/note <text>` | Leaves the builder a note (up to 1000 characters), the same way a tester's Fail does (the project's `inbox/notes/`). A running build reads it at its next step; otherwise `/resume` starts with it. The note is also posted in the Log thread. |

`/resume` and `/retry` wait in the same queue as `/port`.

## Cost

Every `turn_done` event adds its tokens and cost to the run's total, saved in `bot.json`; a
port's total is the sum of its runs. Claude Code in herdr prices its turns at list prices, an
estimate (`cost_is_estimate`), so a total with any estimated turn is shown as `~$`. The board's
Usage field and `/status` show it.

## Daily summary

Once a day at `DAILY_SUMMARY_TIME` (default 09:00), the bot posts a summary of the last 24 hours to
`DAILY_CHANNEL_ID`: each port worked on, the features that reached ready to test, passed or
broken, the testers' Pass and Fail counts, and the cost. The time is in
`DAILY_SUMMARY_UTC_OFFSET` (such as `+07:00`) when it is set, otherwise in the system's local time
zone (on Linux, `TZ` in the bot's environment or `/etc/localtime`). A fixed offset does not follow
daylight saving time.

The date of the last summary is saved in `PORTS_ROOT/.bot/daily.json` before it is posted, so a
restart never posts one day twice. A bot that was off at the time posts that day's summary when it
starts again, the same day. If posting fails before anything went out, the date is given back
and the bot tries again a minute later.

## Restarts

The queue is saved in `PORTS_ROOT/.bot/queue.json` whenever it changes (written to a temporary
file, then renamed over the old one). When the bot starts:

- ports that were waiting are queued again, in the same order, and their channel is told;
- the port that was running is **not** started again: its run is recorded as interrupted, its
  board says so, and its channel is told to use `/resume` (or `/retry`);
- every port's pinned board is drawn again from its `bot.json`, and later runs keep editing that
  same message.

The saved queue is read before the bot signs in, so a new `/port` can't overwrite it first. A
`queue.json` that can't be read is moved to `queue.json.unreadable` (kept for you to look at), the
error is logged, and the queue starts empty.

## Tester cards

When the builder reports a feature `ready_to_test` with its guide (a file in the project, such as
`tests/map.md`), the bot posts a card in the port's channel:

- the title "🧪 Ready to test: <feature>", the guide as the card text (cut to Discord's 4096
  characters; the whole `.md` file is attached), the similarity when the builder gave one, and a
  mention of the tester role;
- two buttons, **Pass** and **Fail**. Only members with the tester or porter role may press them;
  anyone else is told so privately.

There is one card per testing round. Reporting the feature ready again does not post a second card.
After the feature is reported `broken` (by the builder, or by a tester's Fail), the next
`ready_to_test` posts a new card, and a card whose buttons are still live is closed ("Replaced by
a newer card"). A guide outside the project folder, or a link out of it, is not read.

**Pass** records the feature as `passed`. **Fail** opens a form: "What went wrong?" (required, up
to 1000 characters) and "Steps to reproduce" (optional). Sending it records the feature as
`broken`, with the tester's words as its note. Either way:

- the verdict is recorded through the engine, with the same tool the builder uses:

  ```sh
  $CRAFT_CLI build_feature_report --building $PORTS_ROOT/<slug>/project \
    --json '{"name":"<feature>","state":"broken","note":"Failed by <tester>: ..."}'
  ```

  `--building <project folder>` points the build tools at a rebuild's project from outside its run;
- the card shows "✅ Passed by <tester>" or "❌ Failed by <tester>" (and what went wrong), and its
  buttons are disabled, so each card is reviewed once;
- the board shows the feature's new state;
- a failure is posted in the Log thread, and left for the builder as a note in the project's
  `inbox/notes/`. A run still working reads it at its next step; otherwise the next run on the
  folder that brings no request of its own (`rebuild … --continue-in <project> --resume`) starts
  with it. The broken record keeps the note too (`build_features_list` shows it);
- Plane is told, when it is set up.

If the verdict can't be recorded, the card opens again and the tester is told why.

## Projects made in the desktop app

`/port` builds are not the only ones the bot can show. With `MIRROR_APP_PROJECTS=1`, a project made
in the craft desktop app (or with `craft-anything-cli`) gets its own channel the moment it is
created, and the bot follows it from then on: the board, feature progress and tester cards with
Pass and Fail, as for a `/port` build. Projects made before this was set up are not picked up.

**How a project is registered.** Craft writes a small file when it creates a project, if
`CRAFT_DISCORD_REGISTER` (in craft's own environment) names a folder: `<folder>/<created_ms>-<name>.json`,
written atomically, with the project folder (as craft sees it: a WSL path when craft runs in WSL), its
name, request, kind (`website`, `local` or `none`), target and stack (often still empty then; the
bot reads the stack from `craft.json` later), and `created_ms`. Point it at the bot's register
folder, `PORTS_ROOT/.bot/register` unless `MIRROR_REGISTER_DIR` says otherwise. The bot looks there
every `MIRROR_POLL_SECS` (default 5). For each new file it:

1. makes `port-<slug>` under the ports category, its topic and intro marked "(from app)", with the
   intro and the board pinned and a "Log" thread;
2. saves the project in `PORTS_ROOT/.bot/mirrors.json` (written atomically), then moves the file to
   `register/done/`. A registration seen again (after a crash between the two) finds the project in
   `mirrors.json` and makes no second channel. A folder inside `PORTS_ROOT` (one of the bot's own
   ports) or without `craft.json` is not mirrored. A `mirrors.json` that can't be read stops
   mirroring, with an error in the log, instead of making every channel again.

**What it follows.** Every poll, the bot looks at the size and modification time of four files in
the project folder and reads a file again only when one of them changed (polling, because file
events don't work reliably on `/mnt/c`):

| File | Becomes |
|---|---|
| `craft.json` | `status` → the stage (`inspecting` Study, `building` Build, `checking` Check, `ready` Done and a finished run, `stopped` paused, `needs_you` "Waiting for you in the desktop app"); `note` → the activity line; each new entry in `checks` → a check result. |
| `inbox/features.json` | each feature whose record changed → a `feature` event, with its guide and similarity: the board, a tester card for `ready_to_test`, Plane. |
| `usage.json` | what was added since the last look → one turn's tokens and cost on the board and in the daily summary (`~$` when the cost is a coding agent's estimate). |
| `capture/README.md` | the study's notes: once they exist, the first look is posted. |

These are the same events a `/port` run prints ([rebuild events](rebuild-events.md)), so the
board, cards, Log thread and Plane treat both alike. What the bot has already seen of each
project is kept in `mirrors.json`, so a restart reports nothing twice. A file that can't be read is
said in the Log thread and read again when it changes. When the project starts again after it was
ready or paused (a new request in the app), the board starts a new run.

**The first look.** The channel's first report is a light look at what is being rebuilt, made from
what craft's study already wrote (no extra AI call): the target and its kind, the stack, the
builder and the request; the pictures of the original in `capture/` and `reference/`, by name, as
the screens and pages it saw; the sections of the study's notes (`capture/*.md`); and the features
and flows the builder reported so far. Later progress shows on the board, in the Log thread and as
tester cards.

**In a mirrored channel.** `/status` works (its queue line says the build is in the desktop app),
and so does `/note` (the note goes to the project's `inbox/notes/`, as for a port). `/stop`,
`/resume` and `/retry` answer that the build belongs to the desktop app and must be controlled
there. Pass and Fail record into the project folder itself
(`build_feature_report --building <project folder>`). Plane works as for `/port`. No GitHub repo is
made and nothing is committed or pushed: the folder is the person's own.

**Set it up.** In the bot's env file:

```sh
MIRROR_APP_PROJECTS=1
# optional: MIRROR_REGISTER_DIR=/home/<you>/ports/.bot/register   MIRROR_POLL_SECS=5
```

and in the environment craft starts in (the desktop app or the CLI, in WSL), the same folder:

```sh
export CRAFT_DISCORD_REGISTER=/home/<you>/ports/.bot/register
```

## Discord application setup

1. In the Discord Developer Portal, create an application. Its **Application ID** is
   `DISCORD_APP_ID`.
2. Under **Bot**, reset the token and copy it into the env file as `DISCORD_TOKEN`. The bot needs
   no privileged intents: it uses only the `GUILDS` intent.
3. Under **OAuth2 → URL Generator**, pick the scopes `bot` and `applications.commands`, and these
   bot permissions: View Channels, Manage Channels, Send Messages, Embed Links, Attach Files (the
   tester guide), Read Message History, Manage Messages (to pin), Create Public Threads and Send
   Messages in Threads. Open the URL to add the bot to the server.
4. In the server, turn on Developer Mode (User Settings → Advanced) to copy IDs. Create (or pick)
   a category for ports, a porter role and a tester role. The tester mention needs the role to be
   mentionable, or the bot needs the "Mention @everyone, @here and All Roles" permission.

`/port` and the control commands are registered as guild commands when the bot signs in, so they
appear at once in that server only.

## Env file

The bot reads `KEY=VALUE` lines from `$CRAFT_DISCORD_ENV`, or from
`~/.config/craft-discord/.env` when that is not set. `#` comments, `export ` and quotes are
allowed. A missing or wrong key stops the bot with a message that names the key. The token is
never printed or logged. Keep the file private (`chmod 600`) and out of git.

| Key | Required | Meaning |
|---|---|---|
| `DISCORD_TOKEN` | yes | The bot token. |
| `DISCORD_APP_ID` | yes | The application ID. |
| `DISCORD_GUILD_ID` | yes | The server where `/port` is registered. |
| `PORTS_CATEGORY_ID` | yes | The category that `port-<slug>` channels go in. |
| `PORTER_ROLE_ID` | yes | Members with this role may use `/port`. |
| `TESTER_ROLE_ID` | yes | Mentioned when a port finishes OK. |
| `DAILY_CHANNEL_ID` | yes | Where the [daily summary](#daily-summary) is posted. |
| `PORTS_ROOT` | yes | The folder that holds each port's `input/` and `project/`. |
| `CRAFT_CLI` | yes | The path to `craft-anything-cli`. |
| `MAX_UPLOAD_MB` | no | The download limit in MB (default 200). |
| `PLANE_URL` | no | The Plane instance's address, such as `http://localhost:8080` (no `/api`). |
| `PLANE_API_KEY` | no | A Plane API key (see below). Never printed or logged. |
| `PLANE_WORKSPACE` | no | The workspace slug, as in the Plane address `…/<workspace>/projects/`. |
| `GITHUB_OWNER` | no | The GitHub user or organisation that gets a private `port-<slug>` repo per port. |
| `CRAFT_BACKEND` | no | The craft backend that builds (`--backend`). Default `Claude Code (herdr)`; set it empty to pass no `--backend` and let craft choose. |
| `DAILY_SUMMARY_TIME` | no | The daily summary's local time, 24-hour `HH:MM` (default `09:00`). |
| `DAILY_SUMMARY_UTC_OFFSET` | no | The summary's time zone as a fixed UTC offset, such as `+07:00` or `-05:30`. Default: the system's local time zone. |
| `PRETRUST_PORTS` | no | `1` to trust each port's project folder in Claude Code's config before a herdr build (see [Trusting port folders for you](#trusting-port-folders-for-you)). Default off. |
| `CLAUDE_CONFIG_JSON` | no | Claude Code's `.claude.json` for `PRETRUST_PORTS`, as WSL (`/mnt/c/…`) or Windows (`C:\…`) sees it. Default: `.claude.json` in the Windows profile herdr runs under. |
| `MIRROR_APP_PROJECTS` | no | `1` to mirror projects the craft desktop app registers when it creates them (see [Projects made in the desktop app](#projects-made-in-the-desktop-app)). Default off. |
| `MIRROR_REGISTER_DIR` | no | Where craft writes the registrations (`CRAFT_DISCORD_REGISTER`). Default `PORTS_ROOT/.bot/register`. |
| `MIRROR_POLL_SECS` | no | How often, in seconds (1 to 3600), the register folder and the mirrored projects are looked at. Default 5. |

The three `PLANE_*` keys go together: with none of them Plane is off, and with only some the bot
stops and names the missing one. Without `GITHUB_OWNER` no repo is made.

Outside the file, `CRAFT_DISCORD_LOG` sets the log level (`error`, `warn`, `info` (default),
`debug`, `off`). Logs go to stderr.

## Plane

With the `PLANE_*` keys set, each port gets a Plane project (named after the app, with an
identifier of up to five letters from its slug, `RETRO`, then `RETR2`… when taken), and each
feature a work item whose state follows the builder's:

| Feature | Plane state |
|---|---|
| `planned` | Backlog |
| `in_progress` | In Progress |
| `ready_to_test` | Ready to test (made in the project when missing) |
| `passed` | Done |
| `broken` | Todo, and a new work item "Bug: <feature>" with the `bug` label and the note |

When the feature passes, its open bug work items move to Done. The bot uses Plane's REST API v1
(`<PLANE_URL>/api/v1/workspaces/<workspace>/…`) with the `X-API-Key` header; an older instance
without `work-items/` is asked at `issues/`.

To make the API key: sign in to Plane, open **Profile settings → Personal access tokens**, press
**Add personal access token**, give it a title (and an expiry if you want one), and copy the key
into the env file as `PLANE_API_KEY`. The key acts as you, so you need to be a member of the
workspace who may create projects.

A Plane error never stops a port: it goes to the bot's log, and the first error of an outage
gets one line in the Log thread.

## GitHub repo per port

With `GITHUB_OWNER` set, each port gets a private repo when it starts:

```sh
gh repo create <owner>/port-<slug> --private
```

(an existing repo of that name is used). Its link is added to the pinned app profile. The project
folder is made a git repository (`git init -b main`, remote
`git@github.com:<owner>/port-<slug>.git`) and is committed and pushed after each check and when
the port ends. Files over 50 MB are left out (GitHub refuses files over 100 MB), and the Log
thread says which.

The original app is never pushed: it is kept in `PORTS_ROOT/<slug>/input/`, outside the project
folder, and the bot's `.gitignore` block also keeps out `.apk`, `.xapk`, `.apks`, `.aab` (and
`.obb`, `.dex`, `.so`) files anywhere, Craft's own records (captures and pictures of the
original, notes, transcripts), the originals an extractor took out (`app/Original/`), build
output and large archives and videos. Lines of the project's own `.gitignore` are kept.

A failed `gh` or `git` command never stops a port: it goes to the bot's log, and the first
failure of an outage gets one line in the Log thread.

## Running in WSL

The bot and craft run inside WSL (Ubuntu). By default Claude Code itself runs on Windows through
herdr (see [Claude Code in herdr](#claude-code-in-herdr)), with your Windows sign-in and skills.
Only with `CRAFT_BACKEND=Claude Code` does it run in WSL, and then, in WSL:

- install Claude Code and sign in (`claude`, then follow the sign-in);
- install the skills and MCP servers you want the builder to have, as that same user;
- install the OpenSSL headers the bot's TLS links against
  (`sudo apt install pkg-config libssl-dev`): the Discord library's rustls backend pins an old
  rustls with open security advisories, so the bot uses the system's TLS instead;
- build the CLI and the bot from the repository:

  ```sh
  cargo build --release -p craft-anything-cli -p craft-anything-discord
  ```

  Set `CRAFT_CLI` to the absolute path of `target/release/craft-anything-cli`.

For the GitHub repos (only with `GITHUB_OWNER`), also in WSL, as the user that runs the bot:

- install the GitHub CLI (`sudo apt install gh`) and sign in with `gh auth login`: pick
  GitHub.com, SSH as the protocol, and let it upload your SSH public key (or make one first with
  `ssh-keygen -t ed25519`). The account must be able to create repos under `GITHUB_OWNER`;
- check the key works: `ssh -T git@github.com` says "You've successfully authenticated";
- set the name and email the commits carry: `git config --global user.name "…"` and
  `git config --global user.email "…"`.

The bot runs `gh` and `git` from `PATH`, with no terminal to ask for a password, so an SSH key with
a passphrase needs `ssh-agent` running for the bot's session.

Start it in tmux, so it keeps running after you close the terminal:

```sh
tmux new -s porter
./target/release/craft-anything-discord
# detach: Ctrl+B then D; stop: Ctrl+C in the session
```

Or use a systemd user service (WSL needs `systemd=true` under `[boot]` in `/etc/wsl.conf`).
Save this as `~/.config/systemd/user/craft-discord.service`, with your own paths:

```ini
[Unit]
Description=Craft Anything Discord porter bot
After=network-online.target

[Service]
ExecStart=%h/craft-everything/target/release/craft-anything-discord
Environment=CRAFT_DISCORD_ENV=%h/.config/craft-discord/.env
WorkingDirectory=%h/craft-everything
Restart=on-failure
RestartSec=10
# Claude Code and your skills must be found on PATH
Environment=PATH=%h/.local/bin:/usr/local/bin:/usr/bin:/bin

[Install]
WantedBy=default.target
```

```sh
systemctl --user daemon-reload
systemctl --user enable --now craft-discord
journalctl --user -u craft-discord -f
loginctl enable-linger "$USER"   # keep it running when you are logged out
```

On Ctrl+C or SIGTERM the bot signs out, and a running rebuild is stopped. When the bot starts
again, the ports that were waiting are queued again, and the one that was running is marked
interrupted and offered `/resume` (see [Restarts](#restarts)).

## Claude Code in herdr

By default every rebuild runs through herdr on Windows: Claude Code starts in a pane of the
project's herdr workspace (`craft project`; a pane per run, beside the workspace's first one),
signed in with your Windows Claude Code and with your full Claude Code setup there (skills, hooks,
rules). How it works and what it changes:
[docs/claude-code.md](claude-code.md#claude-code-in-herdr).

- herdr must be running on Windows (`herdr status`). craft in WSL finds it in
  `%USERPROFILE%\.herdr\packages\standalone\releases\<newest>\herdr.exe`, or wherever
  `CRAFT_HERDR` (in the bot's environment) points.
- Approvals happen in the pane. When Claude Code waits for you, the board shows
  "⏸ Waiting for approval in herdr: …": answer in herdr and the run carries on.
- **Trust the folder once.** Claude Code asks "Do you trust this folder?" the first time it
  starts in a folder, and craft never answers that for you: the run stops with "Claude Code is
  asking to trust <folder> in herdr". Open a terminal in that folder (as Windows sees it: `C:\…`,
  or `\\wsl.localhost\<distro>\…` for a folder inside WSL), run `claude`, choose "Yes, I trust this
  folder", type `/exit`, then `/port` again. Trusting `PORTS_ROOT` itself once covers the ports
  under it, except a port that is a git repository of its own (with `GITHUB_OWNER` set): Claude
  Code stops looking for a trusted folder at the repository's root.
  Or let the bot do it for the ports it makes: [Trusting port folders for you](#trusting-port-folders-for-you).
- Builds need Deep mode, which the bot always uses: Clean room can't build through herdr.
- When a run ends, Claude Code exits and its pane stays for you to look at; the next run in that
  folder closes it.

To build headless again (Claude Code in WSL, `claude -p`), set `CRAFT_BACKEND=Claude Code`.

### Trusting port folders for you

With `PRETRUST_PORTS=1` in the env file (off by default), the bot answers the trust question ahead
of time for the folders it made, and only those: right before a herdr build it records the port's
project folder as trusted in Claude Code's config, the same record Claude Code writes when you
choose "Yes, I trust this folder". Claude Code in herdr itself still never answers the dialog.

- **What is trusted.** Only `PORTS_ROOT/<slug>/project` of a port that has the bot's `bot.json`,
  with `PORTS_ROOT` and the folder resolved first (links followed): the port folder must be
  directly inside `PORTS_ROOT`, its `project` folder inside the port folder, and a path with `..`
  is refused. Nothing outside `PORTS_ROOT` and not `PORTS_ROOT` itself. Only with the herdr
  backend.
- **Where.** `%USERPROFILE%\.claude.json` (from WSL, `/mnt/c/Users/<you>/.claude.json`, the
  profile asked of `cmd.exe`), or `CLAUDE_CONFIG_JSON`. Claude Code 2.1.332 keeps the answer as
  `projects["<folder>"].hasTrustDialogAccepted = true`, the folder as Windows sees it with forward
  slashes (`C:/Projects/ports/app/project`; a folder inside WSL is
  `//wsl.localhost/<distro>/…`). Only that one key of that one folder is set.
- **How the file is treated.** It must exist and be valid JSON, or nothing is written and the Log
  thread says why: the bot never creates or replaces it blindly. The new file is written next to it
  and renamed over it. Every other value is kept, but the keys come out sorted and spaced the way
  serde_json writes them (the workspace doesn't enable serde_json's `preserve_order`); a decimal
  number may come out in a different but equal-valued spelling. Claude Code rewrites the file in
  its own order at its next save.
- **What you see.** The Log thread gets "🔓 Trusted <folder> for Claude Code (PRETRUST_PORTS)" when
  the folder is added, nothing when it was already trusted, and "⚠️ Couldn't trust the project
  folder…" with the reason when it fails. A failure doesn't stop the build: Claude Code then asks
  in the pane and the run stops as it does without the flag.
- **The security trade-off.** Trusting a folder lets Claude Code use that folder's settings, hooks
  and MCP servers (`.claude/`, `.mcp.json`) without asking. A port's project folder holds what the
  builder wrote from a partner's app, so with the flag on you accept that what the builder put in
  it runs with your Windows account the next time Claude Code starts there. Leave the flag off and
  trust folders by hand if that is not acceptable.
- **The race that remains.** Claude Code writes the same file. The bot reads it again right before
  writing and starts over when it changed (three tries), so only the moment between that read and
  the rename is open. A Claude Code that is running at that moment may also save its own copy later
  and drop the record; the build then stops at the trust question as it would without the flag,
  and the next run adds it again.

## Limits

- One rebuild at a time. A run cut off by a restart is not restarted on its own: `/resume` it.
- The `link` option downloads any `http(s)` address the porter gives, up to the size limit.
  Only trusted members should have the porter role.
- `feature` events (per-feature progress on the board) come only from `--user-skills` runs,
  which is what the bot uses.
- Activity for the daily summary is kept for 8 days, and usage is summed per hour, so the
  summary's cost is counted by the hour a turn ended in.
