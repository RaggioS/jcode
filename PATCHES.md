# Fork divergence — RaggioS/jcode

Personal fork of [1jehuang/jcode](https://github.com/1jehuang/jcode), used as the sole local-model
coding harness driving Ollama `gemma4:12b` on macOS (paired with the
[pocket-llm](https://github.com/RaggioS/pocket-llm) VS Code launcher). It tracks upstream `master`;
our changes live on `master` (merged) and are kept rebase-able on top of upstream.

## Carried patches (vs upstream)

1. **macOS image paste on non-English locales** — `crates/jcode-tui/src/tui/app/helpers.rs`,
   `crates/jcode-tui/src/tui/app/input.rs`.
   - `clipboard_image()`: AppleScript-ObjC reader → **JXA** (`osascript -l JavaScript`). The AppleScript
     dialect failed to parse on non-English system locales (Italian raised `-2741 "found plural class name"`),
     so image paste silently returned nothing. JXA syntax is locale-independent.
   - `handle_paste()`: when a bracketed paste delivers **empty text** (how macOS / the VS Code integrated
     terminal report Cmd+V for an image-only clipboard), read the clipboard image and attach it. Safe only
     for empty text — non-empty pastes keep the upstream Wayland multi-MIME guard.
   - Result: Cmd+V (and Ctrl+V) attach screenshots in the VS Code terminal on macOS-Italian. Good
     upstream-PR candidate.

2. **Env-configurable server idle timeout** — `crates/jcode-app-core/src/server.rs`,
   `crates/jcode-app-core/src/server/util.rs`.
   - The shared server's idle shutdown was hardcoded to 5 minutes. Added `server_idle_timeout_secs()`
     reading `JCODE_SERVER_IDLE_TIMEOUT_SECS` (default 300), mirroring the existing
     `JCODE_EMBEDDING_IDLE_UNLOAD_SECS`. The launcher sets both to 1800 (30 min) so the provider/MCP pool,
     loaded embedder and resumable sessions survive gaps between windows.

3. **Local Ollama HTTP/1.1 transport** — `crates/jcode-provider-core/src/lib.rs`,
   `crates/jcode-base/src/provider/{mod.rs,openrouter_sse_stream.rs}`.
   - jcode's OpenAI-compatible transport used an HTTP/2 keep-alive client (tuned for cloud). Ollama speaks
     only cleartext HTTP/1.1, so the first request to a fresh local server stalled ~58s on connect retries
     before falling back. Added `shared_local_http1_client()` (pooled, `.http1_only()`) used when the
     endpoint is localhost / 127.0.0.1 / [::1]. First local connection dropped from ~58s to ~2s.

4. **Stop tracking runtime hook logs** — `.gitignore`, `logs/*.json`.
   - `logs/*.json` are Claude Code hook event logs that churn on every run and showed as permanent
     uncommitted changes. Gitignored and untracked (regenerated locally, kept out of the repo).

5. **Offline-friendly Claude Code import** — `crates/jcode-base/src/import.rs`.
   - jcode already lists Claude Code sessions in the `/resume` picker and imports the selected one
     (`import_session_from_file`). Upstream imports the **full** transcript verbatim and tags the session with
     the original `claude-code` provider + Claude model — fine for continuing with Claude, but on the local
     offline lane a long session blows the context budget and the wrong provider is selected on resume.
   - This patch makes the importer offline-friendly: drop `thinking` blocks, prepend a **recap** from the
     latest `isCompactSummary` Claude already wrote (fallback: the user-prompt thread), keep only the most
     recent messages within a token budget, and tag the imported session with the **configured default
     provider/model** so resuming continues with the local model instead of Claude.

6. **Self-heal a downed local Ollama mid-session** — `crates/jcode-app-core/src/network_retry.rs`,
   `crates/jcode-tui/src/tui/app/turn.rs`.
   - A chat request that fails with `connection refused` against the loopback Ollama port is not a network
     outage — the local server simply died (auto-stop hook, manual kill, launcher race). Upstream's retry
     loop probes internet connectivity, which is up, so it retries against the dead server forever.
   - This patch detects the loopback-port refusal (model-agnostic, keyed on `127.0.0.1:11434` /
     `localhost:11434`, not on any model name) and revives the server in place: spawn `ollama serve`, poll
     the API port up to ~20s, then retry. The spawn inherits the process env so the launcher's `OLLAMA_*`
     tuning carries through. Remote/cloud providers are unaffected (the loopback guard excludes them).

## Runtime configuration (NOT in this repo — machine-local, templated in `pocket-llm/jcode/`)

- **Thinking off** for gemma4: Ollama `/v1` honors top-level `reasoning_effort`. Set via a named provider
  in `~/.jcode/config.toml` (`[providers.ollama-local] extra_body = { reasoning_effort = "none" }`) or the
  `JCODE_OPENAI_EXTRA_BODY='{"reasoning_effort":"none"}'` env in the launcher.
- **Full 256k context**: `context_window = 262144` on the model entry. gemma4's KV cache stays small
  (windowed attention + q8_0), so the full native window loads 100% on GPU on a 16GB Mac.
- **Lean tool profile for fast cold start**: `[tools] profile = "acp"` keeps the full coding tool set but
  drops the agentic extras (swarm, memory-ops, websearch, browser) from the prompt → the cold first turn's
  prefill drops from ~60s to ~20s, with no loss of coding quality. Full tools return with `profile = ""`.
- **Telemetry off**: `DO_NOT_TRACK=1`.
- **Italian persona**: jcode loads `~/AGENTS.md` (global) + project `AGENTS.md` into the system prompt
  (`crates/jcode-base/src/prompt.rs`). The lean Italian persona lives there.

## Keeping in sync with upstream

```bash
git fetch upstream
git rebase upstream/master
cargo build --release --bin jcode
scripts/install_release.sh
```
