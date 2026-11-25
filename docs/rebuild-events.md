# Rebuild events (`--events ndjson`)

`craft-anything-cli rebuild … --events ndjson` prints a run as machine-readable events. Use it
when a program runs the CLI as a subprocess (a bot reads the progress).

## Contract

- **stdout** carries only events: one JSON object per line, each with a string `type`. Nothing
  else is written to stdout.
- **stderr** carries the human text (the "Using Claude Code…" notice, the replaced-builder notice,
  and the unattended answers). Errors that stop the run before it starts (bad flags, an unknown
  backend, a missing project folder) are written to stderr and exit `1` with no events, since no
  run exists yet.
- The first line is always `project`; the last line is always `finished` (for a run that started).
- The exit code is `0` when the build finished OK, `1` otherwise.
- Without `--events`, the output is the human text, as before.

## Questions and approvals

An `ask` event is followed by an answer read from **stdin**, one line per question or approval,
in the order they are asked:

- question: a choice number (1-based) or free text;
- approval: `y` (allow), `a` (allow always for this project) or `n` (refuse).

`--unattended` answers without reading stdin: the first choice, and approvals are refused.

## Event types

| `type` | Fields | Meaning |
|---|---|---|
| `project` | `dir`, `target` | First line. `dir` is the project folder. `target` is `{"kind":"website","url":…}`, `{"kind":"local","path":…}` or `{"kind":"none"}`. |
| `step` | `step` (`study`, `build`, `check`, `done`), `label`, `text` | A stage started. `text` is a plain-words line. |
| `agent_text` | `text` | One line of the agent's reply. Deltas are joined; a line is emitted when it ends, and any unfinished line is emitted before the next event. |
| `status` | `text` | What the agent is doing now. |
| `output` | `text` | A line a program printed outside the conversation (its stderr). |
| `tool_started` | `call_id`, `tool`, `input` | The agent started a tool call. |
| `tool_finished` | `call_id`, `tool`, `ok`, `summary`, `evidence_id` | The tool call ended. |
| `consent_needed` | `call_id`, `tool`, `title`, `explanation` | The agent wants to run a tool that needs approval. |
| `turn_done` | `backend`, `input_tokens`, `output_tokens`, `tokens_unknown`, `cost_usd`, `cost_is_estimate` | One agent turn finished. `cost_usd` is `null` when the agent reports none; `cost_is_estimate` true means it is an estimate at API prices, not a bill. |
| `error` | `message`, `remediation`, `log` | The agent reported an error. `log` is a path, or `null`. |
| `note` | `shown`, `pictures` | A note the person added while the run worked. |
| `ask` | `id`, `kind` (`question`, `approval`), `title`, `detail`, `choices`, `allow_text`, `action` | A question or approval waiting for an answer on stdin. |
| `checked` | `round`, `check`, `passed`, `similarity` (0-100 or `null`), `note`, `artifacts` | A check finished. |
| `preview` | `url` | The build can be previewed at this address. |
| `finished` | `ok`, `paused`, `message` | Last line. `paused` true means the person paused it: not a failure. |

Agent event types keep the names `--format jsonl` uses for `craft-anything-cli ai chat`.

Consumers should ignore types they do not know: new types may be added, and existing fields are
not renamed or removed.

## Example

```
{"dir":"/home/me/projects/make-a-small-page","target":{"kind":"none"},"type":"project"}
{"label":"Build","step":"build","text":"Building the first version…","type":"step"}
{"text":"Asking Claude Code…","type":"status"}
{"text":"Built it.","type":"agent_text"}
{"backend":"Claude Code","cost_is_estimate":true,"cost_usd":null,"input_tokens":0,"output_tokens":0,"tokens_unknown":false,"type":"turn_done"}
{"artifacts":[],"check":"build","note":"app/ has no files yet…","passed":false,"round":1,"similarity":null,"type":"checked"}
{"message":"The build isn't working yet. …","ok":false,"paused":false,"type":"finished"}
```

## Not in this version

- Per-feature progress (`feature` events) is not emitted yet.
- Token totals are per turn; the bot sums them if it needs a run total.
