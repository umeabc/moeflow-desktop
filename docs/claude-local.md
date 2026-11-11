# Local Claude Code backend

Claude is currently **translate-only**. The no-label recognition path and image requests are rejected before invocation because a successful vision run could not be verified on this machine. This is not evidence that Claude lacks vision.

## Invocation and output contract

The implementation starts the real executable directly, with cwd set to a unique temporary directory:

```text
claude.exe -p --output-format json --tools Read --allowedTools Read --permission-mode dontAsk --no-session-persistence --add-dir <scratch> [--model <requested-model>]
```

`prompt.txt` in that directory is opened as stdin. It contains the user prompt, the JSON schema and an instruction to return only the matching JSON object. No user text is passed through a shell. Stdout and stderr go to files to avoid pipe deadlocks. The process is killed and reaped on timeout (default and maximum 180 seconds); one mutex prevents concurrent runs. Missing inner JSON is retried once within the same deadline. Scratch files are removed on return.

`--help` on 2.1.310 explicitly describes `-p` as print-and-exit, `--output-format json` as a single result, `--model` as a model alias/full name, `--add-dir` as additional tool directories, and `--tools` as the available built-in tool list. `--allowedTools Read` alone is not a tool restriction, so `--tools Read` also limits available built-ins. `dontAsk` avoids interactive permission prompts. `--no-session-persistence` prevents saving the conversation. No permission bypass is used.

`--json-schema` exists, but this integration embeds the schema in the prompt and consumes `result`, rather than depending on the separate structured-output contract. Parse the outer JSON first, reject `is_error`, require `type: "result"` and a string `result`, then scan balanced objects in that assistant text, respecting quoted/escaped braces. An error envelope must never become a successful translation.

## Verified locally on 2026-11-12

- Native executable: `C:/Users/Administrator/Downloads/claude-offline-packages-windows/node_modules/@anthropic-ai/claude-code/bin/claude.exe`.
- `--version`: `2.1.310 (Claude Code)`.
- Read the installed CLI's `--help`; no external documentation was needed.
- Generated a 700×160 PNG containing `BLUE FOX 7391` and actually ran:

```text
claude.exe -p "Read D:/reasonix/moeflow-desktop/claude-vision-probe.png with Read. Transcribe and translate its words to Chinese. Return only JSON with text and translated fields." --output-format json --tools Read --allowedTools Read --permission-mode dontAsk --add-dir D:/reasonix/moeflow-desktop --no-session-persistence
```

The CLI exited 1. Stdout was a parseable single JSON object including:

```json
{"type":"result","is_error":true,"result":"API Error: Invalid URL","terminal_reason":"api_error"}
```

Stderr reported `unrecognized_model` with `YOUR_MODEL_HERE[1m]`. Local settings contain placeholder default-model values. No credentials or connection settings were changed. The result string envelope is observed for errors; a successful assistant result and actual image transcription/translation remain unverified. The temporary probe PNG was removed after the experiment.

The ignored `claude::tests::runs_claude_and_returns_json` exercises the production file-stdin invocation and asserts `{"answer":"ok"}`. It was run with `--ignored --nocapture`, found the same version, and failed because of that connection configuration. After configuring Claude locally, rerun this test; optionally set `MOEFLOW_CLAUDE_TEST_MODEL` to select a model for the test. It intentionally does not accept an API error as success.

## Resolution and settings

Settings path → `CLAUDE_BIN` → known locations → PATH. Automatic candidates include the APPDATA npm shim's known native package targets, the offline-package executable above, and LOCALAPPDATA/Programs/claude/claude.exe. Existing candidates are deduplicated. `.cmd`, `.bat` and `.ps1` are never spawned; a standard npm `.cmd` may resolve to its sibling native package executable, otherwise it is rejected. The settings card detects/locks or saves/probes the executable, which checks installation rather than model-service connectivity.

Stored backend values remain `codex`, `dsh`, `claude`; old profiles default to Codex. Claude's path defaults to empty for old profiles. The backend's five IPC commands each have an explicit permission and membership in the default permission set.

## Validation

- `export PATH="$HOME/.cargo/bin:$PATH"; cargo check --manifest-path src-tauri/Cargo.toml --all-targets`: passed.
- `cargo test --manifest-path src-tauri/Cargo.toml --lib --no-run`: passed; attached Common Controls v6 using Windows SDK 10.0.22621.0 `mt.exe`, resource `#1`, before executing the harness.
- Library suite: 127 passed, 0 failed, 4 ignored. Claude's ignored real-CLI test: 0 passed, 1 failed (connection above).
- Copied the changed overlay source to the ignored `frontend/src/desktop` working tree, then ran from `frontend`: `node_modules/.bin/tsc --noEmit --strict --jsx react --moduleResolution bundler --module esnext --target es2020 --lib es2020,dom --skipLibCheck --esModuleInterop src/desktop/*.tsx src/desktop/*.ts`: passed. Running against `overlay` directly cannot resolve frontend's React/Ant Design dependencies.
- No GUI, frontend bundle build, installer packaging, or commit was performed. Settings interaction and the owner's final exe remain untested.
