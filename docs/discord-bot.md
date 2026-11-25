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
   starts when the current one ends. The bot runs:

   ```sh
   $CRAFT_CLI rebuild <input file> "<request>" --deep --user-skills --unattended \
     --backend "Claude Code (herdr)" \
     --events ndjson --project-folder $PORTS_ROOT/<slug>/project [--build-with unity]
   ```

   The board is edited at most every 3 seconds. Log lines go to the thread in batches. Only
   Unity is a stack the CLI knows, so `target: cocos` and `target: app` name the target in the
   request and let the builder choose the stack.

   The builder is `CRAFT_BACKEND`, by default **Claude Code (herdr)**: your own Claude Code on
   Windows, in a herdr pane you can watch (see [Claude Code in herdr](#claude-code-in-herdr)).
6. **End.** The board shows the final state. A run that finished OK mentions the tester role
   (`TESTER_ROLE_ID`). A failed run says why. If the program crashed or stopped without
   finishing, the end of its error output is shown (up to 1500 characters).

What the bot keeps about a port (its channel, its tester cards, the Plane and GitHub ids) is in
`PORTS_ROOT/<slug>/bot.json`, so cards still work after the bot restarts.

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
- the board shows the feature's new state (until the bot restarts; it doesn't redraw an old
  board);
- a failure is posted in the Log thread, and left for the builder as a note in the project's
  `inbox/notes/`. A run still working reads it at its next step; otherwise the next run on the
  folder that brings no request of its own (`rebuild … --continue-in <project> --resume`) starts
  with it. The broken record keeps the note too (`build_features_list` shows it);
- Plane is told, when it is set up.

If the verdict can't be recorded, the card opens again and the tester is told why.

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

`/port` is registered as a guild command when the bot signs in, so it appears at once in that
server only.

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
| `DAILY_CHANNEL_ID` | yes | Saved for the daily summary. Nothing is posted there yet. |
| `PORTS_ROOT` | yes | The folder that holds each port's `input/` and `project/`. |
| `CRAFT_CLI` | yes | The path to `craft-anything-cli`. |
| `MAX_UPLOAD_MB` | no | The download limit in MB (default 200). |
| `PLANE_URL` | no | The Plane instance's address, such as `http://localhost:8080` (no `/api`). |
| `PLANE_API_KEY` | no | A Plane API key (see below). Never printed or logged. |
| `PLANE_WORKSPACE` | no | The workspace slug, as in the Plane address `…/<workspace>/projects/`. |
| `GITHUB_OWNER` | no | The GitHub user or organisation that gets a private `port-<slug>` repo per port. |
| `CRAFT_BACKEND` | no | The craft backend that builds (`--backend`). Default `Claude Code (herdr)`; set it empty to pass no `--backend` and let craft choose. |

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

On Ctrl+C or SIGTERM the bot signs out, and a running rebuild is stopped. A port that was
stopped is not resumed when the bot starts again. Start it again with `/port`.

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
- Builds need Deep mode, which the bot always uses: Clean room can't build through herdr.
- When a run ends, Claude Code exits and its pane stays for you to look at; the next run in that
  folder closes it.

To build headless again (Claude Code in WSL, `claude -p`), set `CRAFT_BACKEND=Claude Code`.

## Limits

- One rebuild at a time. The queue is kept in memory, so ports still waiting are lost if the
  bot restarts.
- The `link` option downloads any `http(s)` address the porter gives, up to the size limit.
  Only trusted members should have the porter role.
- `feature` events (per-feature progress on the board) come only from `--user-skills` runs,
  which is what the bot uses.
- After a restart, Pass and Fail still work on old cards (from `bot.json`), but the board of a
  finished port is not redrawn.
