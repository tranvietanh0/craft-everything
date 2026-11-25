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
     --events ndjson --project-folder $PORTS_ROOT/<slug>/project [--build-with unity]
   ```

   The board is edited at most every 3 seconds. Log lines go to the thread in batches. Only
   Unity is a stack the CLI knows, so `target: cocos` and `target: app` name the target in the
   request and let the builder choose the stack.
6. **End.** The board shows the final state. A run that finished OK mentions the tester role
   (`TESTER_ROLE_ID`). A failed run says why. If the program crashed or stopped without
   finishing, the end of its error output is shown (up to 1500 characters).

## Discord application setup

1. In the Discord Developer Portal, create an application. Its **Application ID** is
   `DISCORD_APP_ID`.
2. Under **Bot**, reset the token and copy it into the env file as `DISCORD_TOKEN`. The bot needs
   no privileged intents: it uses only the `GUILDS` intent.
3. Under **OAuth2 → URL Generator**, pick the scopes `bot` and `applications.commands`, and these
   bot permissions: View Channels, Manage Channels, Send Messages, Embed Links, Read Message
   History, Manage Messages (to pin), Create Public Threads and Send Messages in Threads. Open the
   URL to add the bot to the server.
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

Outside the file, `CRAFT_DISCORD_LOG` sets the log level (`error`, `warn`, `info` (default),
`debug`, `off`). Logs go to stderr.

## Running in WSL

The bot and the rebuilds run inside WSL (Ubuntu). Rebuilds use Claude Code with your own skills
(`--user-skills`), so in WSL:

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

## Limits

- One rebuild at a time. The queue is kept in memory, so ports still waiting are lost if the
  bot restarts.
- The `link` option downloads any `http(s)` address the porter gives, up to the size limit.
  Only trusted members should have the porter role.
- `feature` events (per-feature progress on the board) come only from `--user-skills` runs,
  which is what the bot uses.
