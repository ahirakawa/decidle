# decidle

[日本語](README.ja.md)

A Stop hook for Claude Code. It makes the agent write its report as JSON, checks the report against the rules and the session transcript, and turns a passing report into an HTML page. You read that page instead of the terminal reply.

The rules are in `docs/report-format.en.md`. The Maestri roles are in `docs/role-orchestrator.en.md` and `docs/role-reviewer.en.md`.

## Language

decidle shows its text in English or Japanese. Set `language` in `~/.config/decidle/config.json`:

```json
{ "language": "ja" }
```

The value is `en` (default) or `ja`. The server, the hooks, the report pages, the browser Home, and the mod all follow this one setting. Restart `decidle serve` to change the Home screen. Hooks read the setting each time, so they change from the next hook call. Report pages already written keep their language.

## Install on a new machine

1. Build in this folder and install to `~/.local/bin/decidle`. You need Rust.

   ```
   cargo install --path . --root ~/.local --locked
   ```

   Run the same command after you change the source. Hooks keep calling the old binary until you reinstall.

2. Add the entries in `docs/settings-snippet.json` to `hooks` in `~/.claude/settings.json`. Every hook calls `~/.local/bin/decidle` directly. Stop runs `decidle stop` to check the report. PostToolUse for every tool runs `decidle hint`, which hands the report format to the agent once a turn needs a report, so that it writes the report before the Stop hook sends it back. SessionStart, UserPromptSubmit, Notification, SessionEnd, and PreToolUse and PostToolUse for AskUserQuestion run `decidle event` to record sessions for the Home page. The SessionStart hook also tells the agent to end its reply with a one-line note for Home.

3. Only if you use Maestri, register the roles from this folder in a Maestri terminal. It installs the role files for the `language` setting.

   ```
   ./install-roles.sh
   ```

On a machine without `~/.local/bin/decidle`, every hook exits without doing anything. Settings do not use `hooks/stop_decidle.sh`. Run `hooks/stop_decidle.sh render <report.json>` by hand to rewrite a report page.

## Use with Codex CLI

Codex CLI (tested with 0.169) can use the same binary for report checks and Home records.

1. Append `docs/codex-hooks-snippet.toml` to `~/.codex/config.toml`.
2. In codex, open `/hooks` and trust the hooks you added. They do not run until you trust them.

In Codex, waiting for permission is recorded from PermissionRequest, and denied approvals and interrupts from Interrupt. A session disappears from the table after an interrupt. Limits (not tested: the flow after you allow an approval, compaction, resume, stopping without waiting for a subagent, and more) are in the "Codex CLI" section of `docs/spec-home.md` (Japanese).

## Use with Herdr

decidle finds the terminal of a session from the environment of the hook: `HERDR_PANE_ID` (Herdr TUI), `HC_TERMINAL_ID` (Herdr app), or `CMUX_SURFACE_ID` (cmux), in this order. With a recorded terminal, Home can bring the terminal to the front and send an answer to it. Nothing needs to be set for this.

### Show Home in the Herdr app

The mod in `herdr-mod/decidle/` shows the Home list on the right of the Herdr app's terminals. Copy it into the app's mods folder:

```sh
mkdir -p ~/.config/herdr-app/mods/decidle
cp herdr-mod/decidle/main.js herdr-mod/decidle/mod.json ~/.config/herdr-app/mods/decidle/
herdr-app check ~/.config/herdr-app/mods/decidle
```

The app loads it after the copy. To remove it, delete `~/.config/herdr-app/mods/decidle/`. Copy the files again after you update this repository. Details are in `herdr-mod/decidle/README.md` (Japanese).

### Start `decidle serve`

The mod and the browser Home both read `decidle serve`, so keep it running. Start it in a terminal of the Herdr app:

```sh
~/.local/bin/decidle serve
```

After "go to terminal", the server brings the app it was started from to the front. Started in a Herdr app terminal, that is the Herdr app. The mod reads the port from `home_port` and the token from `~/.local/share/decidle/home.token`, which the server writes at every start.

### Settings

Nothing needs to change when you move from Maestri to Herdr. The Maestri settings do nothing outside Maestri:

- `portal_command` is empty by default, so no portal is opened.
- `trigger_commands` (`maestri ask`) asks for a report only when a turn runs `maestri ask`.
- `role_dir_marker`, `review_command` and `review_marker` apply only to sessions started with a Maestri role. The review step of the orchestrator role does not exist outside Maestri.

You do not need `./install-roles.sh` either.

## Usage

- To turn decidle off for a terminal, start it with `DECIDLE_MODE=off claude`.
- Check results are logged to `~/.local/share/decidle/gate.jsonl`. Use the SQL in `docs/queries.sql` to aggregate them.
- A report is required only for turns that edit 3 or more files, make 20 or more tool calls, or run `maestri ask`. Change the numbers with `edited_files_threshold` and `tool_call_threshold`.
- To change limits such as character counts or which extensions count as code, put a file shaped like `config.default.json` at `~/.config/decidle/config.json`. Only the keys you write are overridden.
- Report JSON and pages go to `.decidle/` in the working folder. Add it to your own gitignore if you do not want it in git.
- Open a report page from the title of its row on the Home page. Start Home with `decidle serve` and open `http://127.0.0.1:7878/` (the port is `home_port`). The spec is `docs/spec-home.md` (Japanese).
- Report JSON and pages are deleted 1 day after the session is resolved or ended. `decidle serve` deletes them at startup and once a day. Run `decidle clean` to delete them by hand, or `decidle clean --dry-run` to only list them. Change the days with `clean_after_days`; 0 keeps them. Reports whose decisions were never answered (including sessions closed with the end button) are deleted 7 days after the end. Change the days with `clean_unanswered_after_days`; 0 keeps them. The two settings work independently. decidle does not delete reports of sessions still running, reports of sessions shown on Home, files not in its records, or files outside `.decidle/`. Details are in "報告の自動削除" in `docs/spec-home.md`.
- Opening a Maestri portal for each report is off by default. To turn it back on, set `portal_command` to `"maestri"`. Then, in a Maestri terminal, decidle opens a portal named after the task on the canvas after it writes the page. It does not open one in worker terminals started by an orchestrator (terminals connected to other agents, not the Maestro).

## Count revision requests

```
~/.local/bin/decidle extract > extract.jsonl
```

Have an LLM classify the output with the instructions in `docs/classify-prompt.md`, then aggregate by week with the last SQL in `docs/queries.sql`. The output contains the prompts as they are, so do not commit it.

## Known limits

- Between checking the report folder and writing the page, another process with the same permissions could replace the folder with a link, and the page would be written somewhere else. decidle checks again just before writing, but the gap cannot be removed.
- The whole transcript is read every time. Transcripts over 256MB are not read, and that turn is not checked.
- Whether a check command ran is judged by whether the command string contains certain words. The result of the command is not checked.
- Files changed through Bash, and files edited by other terminals or subagents, are not checked.
- The check log `gate.jsonl` keeps growing. Delete it by hand when you no longer need it.
