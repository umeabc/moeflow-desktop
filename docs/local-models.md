# Local backend models and agy ACP evidence

Verified on Windows, 2026-11-16 through 2026-11-17 (Asia/Shanghai). Backend choices are `codex`, `dsh`, and `agy`. The removed backend's module, IPC registration, permissions, settings/path/model fields, overlay references, local-settings discovery branch, and dedicated documentation have been removed. Model IDs naming an upstream vendor remain legitimate catalog data.

## agy process and path resolution

Each translation launches exactly one native `agy acp` process. Resolution order: `AGY_BIN`, PATH (`agy.exe`, then `agy` per directory), `%USERPROFILE%\Documents\deepseek-harness\default-workspacegy-bridge\dist-tunnelgy.exe` (resolved at runtime, so the binary carries no machine-specific path), configured Settings path. Reject `.cmd`, `.bat`, and `.ps1`; never invoke a shell. An explicit AGY_BIN remains authoritative and reports an error if invalid. Configured paths cannot override higher-priority candidates. Status, candidates, version and error reuse the Codex response shape; status uses a real ACP initialize/session exchange, not a guessed version flag.

The existing try-lock mutex style rejects concurrent agy inference/discovery. Requests use newline-delimited JSON over piped stdin, never prompt argv. Raw input is capped at 8 MiB; serialized frames and total stdout are capped at 16 MiB, including malformed frames. The entire inference exchange has a maximum 180-second deadline (a smaller request timeout is honored). Success, protocol errors, parsing errors and timeout all kill and reap the child. stderr is discarded to avoid an undrained pipe. No retries or persistent process are used. Agent permission requests are cancelled; unsupported client operations receive JSON-RPC errors. Status/discovery handshakes have 20-second deadlines and always terminate their children.

Protocol reference read before implementation: `C:/Users/Administrator/Documents/deepseek-harness/default-workspace/agy-bridge/_test/persona_test.py`.

## Exact exchange and evidence

JSON-RPC 2.0, one JSON object per line. `SID` below denotes the server-generated session ID; PNG_DATA denotes the literal base64 contents of `src-tauri/src/testdata/agy-green.png`. No image URI or path is substituted for image bytes.

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}
{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"D:/reasonix/moeflow-desktop","mcpServers":[]}}
{"jsonrpc":"2.0","id":4,"method":"session/set_model","params":{"sessionId":"SID","modelId":"gemini-3.8-flash-high"}}
{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"SID","prompt":[{"type":"text","text":"PROMPT\nDo not use tools, execute commands, or modify files. Return ONLY a JSON object matching this schema, no prose or fences:\nSCHEMA"},{"type":"image","mimeType":"image/png","data":"PNG_DATA"}]}}
```

Text requests omit the image block. Model selection is omitted when no explicit/saved model is set. Initialization returned protocolVersion 1, `agentInfo={"name":"Antigravity ACP Server","version":"1.5.9"}`, and `promptCapabilities.image=true`. Session creation returned currentModelId `gemini-4-argon-medium` and 18 available model IDs (listed below). The valid `session/set_model` reply was `{"jsonrpc":"2.0","id":4,"result":{}}`; a separate deliberately invalid ID returned code -32602, `Unknown model ID: moeflow-capability-probe`. No model flags were needed or advertised.

The Rust text-test PROMPT is exactly `Return JSON with answer equal to ok. Do not use tools.`; its SCHEMA is `{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}`. The image-test PROMPT is exactly `Do not use tools or inspect files. Look directly at the attached image. What is its dominant color? Return JSON with answer equal to the color.` and uses the same schema. The image fixture contains only pixels, with no filename/color hint in the content block.

Successful assistant notification content was `{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"{\"answer\":\"green\"}"}}`. The matching prompt reply had `id=3`, `result.stopReason="end_turn"`; usage fields were returned but are not needed to recognize completion.

Only `session/update` notifications with `update.sessionUpdate=agent_message_chunk` contribute `update.content.text`. Thought/user/tool chunks are excluded. After the matching id=3 prompt result, parse a balanced JSON object from the accumulated assistant text, then kill/reap the process. Observed prompt completion was `result.stopReason=end_turn`.

The ignored Rust test uses this exact application code path and prints parsed answers. With model `gemini-3.8-flash-high`, text prompt `Return JSON with answer equal to ok. Do not use tools.` returned `{"answer":"ok"}`. A real 160x100 RGB PNG, pixel value (23,197,81), sent as a base64 image block with a prompt asking its dominant color returned `{"answer":"green"}`. The expected color was not in the prompt. Latest run: text 7876 ms, image 8334 ms, test total 31.54 seconds. A preliminary probe requested a tool permission and was not counted as success; the successful direct-image probe and Rust test used no tools.

Vision is advertised only when the ACP handshake declares `promptCapabilities.image` **and** the selected model belongs to a family that was actually exercised here: `gemini-4-`, `gemini-3.8-`, `gemini-pro-orchestrator`, `claude-opus-5-5`, `claude-sonnet-5-5` (probed 2026-11-17 with a solid-green 160x100 PNG; `gpt-oss-120b-medium` returned nothing and stays text-only). The list is code, not inference from the model name: capability flags prove the transport accepts an image block, not that a model reads it. Server version is deliberately *not* pinned any more — a bridge update used to turn vision off silently. All other agy model IDs, including the implicit default, stay unknown and translate existing labels without an image; with no labels they fail before downloading anything. Extend the list only after re-running the image probe against the new model. Catalog capability flags alone never establish agy vision support. This is evidence for image delivery/color recognition, not an OCR/layout quality benchmark.

## Model discovery and selection

Settings refresh is explicit. Codex/dsh discovery reads only local JSON/TOML/YAML/SQLite and never runs a CLI or sends a network request. agy refresh starts an ACP handshake and reads `session/new.models.availableModels`; it does not prompt the model. Boot, focus and translation never trigger discovery. Snapshots are persisted for the overlay.

`codex_model` and `agy_model` default to empty, preserving backend defaults. A request model overrides the saved choice. Codex passes `-m ID`. agy validates the requested ID against the fresh session catalog, then requires a successful `session/set_model` before prompting. Saving a nonempty agy choice requires it to exist in the refreshed catalog. The model-setting method was verified with the selected Flash model; the remaining available IDs have not each been prompted.

There is no persisted `dsh_model` field or boot field. dsh selector/custom/save controls are disabled, and the mutation endpoint rejects dsh model selection. Runtime also rejects any nonempty per-request dsh model. Set dsh defaults in its external profile. Its model directory is informational even when upstream metadata declares vision.

`ccs-<route>/<model>` aliases are never selectable IDs. Discovery, Settings rendering and startup snapshots exclude them; save/runtime guards reject them. Plain IDs retain matching routes only in `source` metadata. Obsolete backend catalog keys and unknown backend settings are discarded at AppState initialization.

Sources:

- `.codex/cc-switch-model-catalog.json`, `.codex/models_cache.json`: model slug and explicit modality metadata.
- `.cc-switch/codex-official-models.json`: account-keyed cached official model catalogs; account keys are not exposed.
- `.codex/config.toml`: configured root model, capability unknown.
- `.cc-switch/cc-switch.db`: read-only SQLite, 100 ms busy timeout, provider settings/meta catalogs and TOML configs. Both `settings_config` and `meta` are `Option<String>` so a NULL column does not discard the other column's catalog. Tests exercise NULL in either column.
- `.cc-switch/live-state.json`: route-to-provider metadata only, retained on plain IDs.
- `.dsh/profiles/*/cordis.patch.yml`: provider models, parsed by `serde_yaml_ng` 0.10.0. The fixture includes `script: !!js 'throw 42'` alongside model data; successful catalog assertions prove tagged patches parse without executing JavaScript. No evaluator or subprocess exists in this parser.
- Price catalogs are excluded. Missing/invalid/conflicting modality declarations stay unknown; names/prices do not establish vision.

## Complete observed discovery list

- codex (19): `claude-fable-5-1`, `claude-opus-5-5`, `claude-sonnet-5-5`, `codex-auto-review`, `deepseek-v4.1-flash`, `gemini-3.8-flash-high`, `gemini-4-argon`, `glm-5.3-flash`, `gpt-5.6-luna`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-6-astra`, `gpt-6-luna`, `gpt-6-sol`, `gpt-6.1-sol`, `gpt-6.1-terra`, `gpt-reserve`, `kimi-k3.1-pro`, `minimax-m3.1-flash`
- dsh (22): `claude-opus-5-5`, `deepseek-v4-flash`, `deepseek-v4.1-flash`, `deepseek-v4.1-flash-1017`, `deepseek-v4.1-flash-economy`, `doubao-seed-2.0-mini`, `gemini-3.1-pro`, `gemini-3.1-pro-preview`, `gemini-3.8-flash-high`, `gemini-4-argon`, `glm-5.3`, `glm-5.3-flash`, `gpt-5.6-luna`, `gpt-5.6-sol`, `gpt-6-astra`, `gpt-6-luna`, `gpt-6.1-sol`, `grok-4.7`, `hy3`, `hy4-preview-f`, `kimi-k3.1-pro`, `minimax-m3.1-flash`
- agy ACP live (18): `gemini-4-argon-medium`, `gemini-4-argon-high`, `gemini-4-argon-low`, `gemini-4-hydrogen-high`, `gemini-4-hydrogen-xhigh`, `gemini-4-hydrogen-medium`, `gemini-4-hydrogen-low`, `gemini-pro-orchestrator-g1-pro`, `gemini-3.8-flash-high`, `gemini-3.8-flash-medium`, `gemini-3.8-flash-low`, `claude-opus-5-5-low`, `claude-opus-5-5-medium`, `claude-opus-5-5-high`, `claude-sonnet-5-5-low`, `claude-sonnet-5-5-medium`, `claude-sonnet-5-5-high`, `gpt-oss-120b-medium`

The local-only `real_local_discovery` test prints `agy: []` because agy has no guessed local-file catalog; its separate real ACP test prints the live 18-model list. No backend named for the removed implementation appears. No ID starts with `ccs-`; route strings remain source metadata. Local catalog contents do not prove provider reachability.

## Verification

All final commands exited 0:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
cargo test --manifest-path src-tauri/Cargo.toml --lib --no-run
MT='C:/Program Files (x86)/Windows Kits/10/bin/10.0.22621.0/x64/mt.exe'
"$MT" -manifest src-tauri/target/common-controls.manifest '-outputresource:src-tauri/target/debug/deps/moeflow_desktop_lib-e8fc10f8fb4e5ade.exe;#1'
```

Then directly ran the embedded library harness: **130 passed, 0 failed, 5 ignored** (135 tests). Explicit `agy::tests::real_agy --exact --ignored --nocapture`: **1 passed, 0 failed**, text and image parsed answers above. Explicit `models::tests::real_local_discovery --exact --ignored --nocapture`: **1 passed, 0 failed**, 19 Codex / 22 dsh local entries. Manifest embedding is into the harness resource #1, not a sidecar and not application packaging.

Copied `overlay/src/desktop/.` to ignored `frontend/src/desktop/`, then from frontend ran its `node_modules/.bin/tsc` with `--noEmit --strict --jsx react --moduleResolution bundler --module esnext --target es2020 --lib es2020,dom --skipLibCheck --esModuleInterop src/desktop/*.tsx src/desktop/*.ts`: passed. Settings inline JavaScript syntax and `git diff --check`: passed. Windows `.cmd` executable and expanded file arrays were used for tsc under PowerShell with the same flags.

An initial dsh-bundled Bash failed to create a signal pipe. PowerShell with equivalent Cargo PATH completed the Rust checks, and the requested literal export/check command subsequently passed under `D:/Program Files/Git/bin/bash.exe` (its login profile printed an unrelated missing env-file warning; the final check and no-run compilation also passed with `--noprofile --norc`). Missing rg/Pillow were handled using native file reads and a deterministic stdlib-generated PNG; neither is an application dependency. Failed preliminary probes/commands were corrected and did not count as passes.

Ignored local evidence: `src-tauri/target/agy-harness-output.txt`, `agy-real-test-output.txt`, `model-discovery-output.txt`, `agy-image-evidence-2.txt`. Pre-edit source snapshots are in `src-tauri/target/pre-agy-changes/`; existing uncommitted work was retained and no commits were made.

Unverified: GUI settings interaction, full page OCR/box quality, every other model's inference/image support, remote-provider reachability, and installer behavior. No GUI, application build/installer packaging, or commits were run. agy itself creates its normal ACP session artifacts in its own home directory.

## Files changed

- Added `src-tauri/src/agy.rs` and `src-tauri/src/testdata/agy-green.png`; deleted the previous backend module and `docs/claude-local.md`.
- `src-tauri/src/models.rs`: discovery review fixes, nullable SQLite columns, safe tagged YAML, ID guards and fixture/ignored tests.
- `src-tauri/src/config.rs`, `config/tests.rs`, `commands.rs`, `profiles.rs`: agy settings/boot data, atomic selection persistence, obsolete snapshot cleanup and unsupported-selection regression coverage.
- `src-tauri/src/codex.rs`, `dsh.rs`: saved Codex selection and image validation; explicit dsh unsupported-selection guard.
- `src-tauri/src/lib.rs`, `main.rs`, `permissions/moeflow/default.toml`: backend/model registration and ACL replacement.
- `src-tauri/Cargo.toml`, `Cargo.lock`: retained current catalog dependencies and replaced serde_yaml 0.9 with serde_yaml_ng 0.10.
- `src-tauri/ui/settings.html`: Chinese agy controls, ACP model refresh/selection, accurate vision limitations, informational dsh controls, `.buttons` styling.
- `overlay/src/desktop/codex.ts`, mirrored frontend desktop files: backend replacement, selected models, verified image attachment and translate-only fallback.
- `docs/local-models.md`: current protocol, evidence, limitations and verification.
