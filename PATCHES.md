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

5. **Cross-harness resume `--resume-external <path>`** — `crates/jcode-app-core/src/external_resume.rs`
   (new), `crates/jcode-app-core/src/lib.rs`, `src/cli/args.rs`, `src/cli/dispatch.rs`.
   - Continue a conversation started in **Claude Code** offline with the local model. The flag imports a
     Claude Code transcript (`~/.claude/projects/<proj>/<id>.jsonl`) into a fresh local session, then enters
     the normal `--resume <id>` path (so no resume/TUI code changes).
   - **Mechanical, no model calls** (at switch time Claude is out of tokens, so an LLM recap is impossible):
     parses the JSONL, keeps only `user`/`assistant` lines, drops sidechain/meta/visible-only and `thinking`
     blocks, maps `text`/`tool_use`/`tool_result` (truncating huge tool outputs).
   - **Recap**: reuses the latest `isCompactSummary` block Claude already wrote (captured *before* the
     visible-only skip filter — Claude marks the summary `isVisibleInTranscriptOnly`); falls back to the
     user-prompt thread when no compaction summary exists.
   - **Budget**: prepends the recap, then keeps the most recent messages within ~80k tokens (constants at the
     top of the module), noting how many older messages were omitted.
   - The new session is tagged `provider_key = "ollama-local"`, `model = "gemma4:12b"`, `working_dir` from the
     transcript `cwd`, title `↩ <aiTitle>`. Unit-tested (parse/skip, thinking-drop, tool-result truncation,
     recap precedence, budget). Launcher integration ("Continue from Claude Code" button) is a follow-up.

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
