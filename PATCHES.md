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

6. **Self-heal a downed local Ollama mid-session** — `crates/jcode-base/src/provider/openrouter_sse_stream.rs`.
   - A chat request that fails with `connection refused` against the loopback Ollama port is not a network
     outage — the local server simply died (auto-stop hook, manual kill, launcher race). Upstream's retry
     loop probes internet connectivity, which is up, so it retries against the dead server forever.
   - The revive lives at the single chokepoint every caller funnels through: the provider stream retry loop
     (`run_stream_with_retries`). On a loopback-Ollama `connection refused` (model-agnostic, keyed on port
     11434, not on any model name) it spawns `ollama serve`, polls the API port (~15s), then the existing
     retry loop reconnects on its next attempt. The spawn inherits the process env so the launcher's
     `OLLAMA_*` tuning carries through. Remote/cloud endpoints are excluded by the loopback guard. This
     covers the interactive turn loop, swarm workers, the deferred client→daemon retry, and headless
     `jcode run`. Verified e2e: kill Ollama, `jcode run` revives it in place and completes (`HEAL_OK`).
   - (An earlier first cut also added the revive at the TUI turn-loop retry sites in `turn.rs` +
     `jcode-app-core::network_retry`; that was removed once the provider-layer chokepoint above made it
     redundant — the TUI layer only sees errors the provider layer already failed to revive.)

7. **Bare model on restore for local loopback profiles** — `crates/jcode-base/src/provider/selection.rs`.
   - A session tagged with a local OpenAI-compatible provider (Ollama / LM Studio) re-emitted the
     `<provider>:` routing prefix on restore (`model_switch_request_for_session_{model,route}`). Upstream's
     strip only runs in `OpenRouterProvider::set_model`; a session launched under the bare built-in `ollama`
     runtime applies the spec without it, so `ollama-local:gemma4:12b` leaked to the loopback endpoint and
     was rejected with `400 invalid model name`.
   - `session_provider_is_local_loopback(provider_key)` resolves the provider (built-in catalog profile or
     user `[providers.*]` entry) and, when its endpoint host is loopback, emits the bare model. Single local
     endpoint → no routing ambiguity. Remote/cloud profiles keep their prefix (cross-provider restore intact).

8. **Runtime reasoning toggle for local loopback endpoints** — `crates/jcode-base/src/provider/openrouter.rs`.
   - Upstream only accepts the DeepSeek-style top-level `reasoning_effort` field for the `deepseek` profile
     id or a DeepSeek-family model (`profile_supports_reasoning_effort`). A local Ollama endpoint serving
     gemma4 matched neither, so `set_reasoning_effort` / the effort-increase keybind were inert and the only
     way to control reasoning was pinning it in `extra_body` — which is merged last and overrides the
     jcode-generated field, permanently locking the value and defeating any runtime change.
   - This patch treats any loopback endpoint as effort-capable (keyed on `api_base_uses_localhost`, not on a
     model name — model-agnostic, consistent with patches 6/7). `supports_deepseek_reasoning_effort` and
     `initial_reasoning_effort` both honor it, so the local lane constructs at the configured
     `[provider] openai_reasoning_effort` (`"none"` = OFF, fast cold start) and the keybind escalates it live
     (none → low → medium → high) only when a task needs it — no prompt-cache cost, since `reasoning_effort`
     is a request-body field, not part of the cached tools/prompt prefix. Remote endpoints keep upstream's
     model-based auto-detection unchanged.

9. **Auto reasoning-escalation by prompt complexity (local lane)** — `crates/jcode-base/src/provider/openrouter.rs`,
   `crates/jcode-base/src/provider/openrouter_provider_impl.rs`, `crates/jcode-config-types/src/lib.rs`.
   - Complements patch 8's manual keybind: when `[provider] auto_reasoning_effort = true`, a request whose
     latest human message looks complex (a bilingual IT/EN signal keyword — refactor, architett/architecture,
     debug, deadlock, ottimizz/optimize, progett/design, migrat… — or a clearly long / multi-question prompt)
     gets `reasoning_effort` raised to `auto_reasoning_effort_level` (default `low`) for that turn. `low` is
     deliberate: on a small local model `medium`/`high` think too long before answering (e2e: a `medium`
     design prompt did not finish in 160s; the same class of prompt at `low` completes in ~40s with a good
     answer), so the auto level stays light.
   - `auto_escalated_reasoning_effort()` runs at request build (where `self.reasoning_effort()` is read), so it
     is **pure per-request**: it never mutates stored effort, fires only on a **loopback** endpoint and only
     when effort is otherwise off (`none`/unset), and a manual effort-increase keybind always wins (a non-`none`
     stored value short-circuits it). Simple lookups/edits stay at `none` (fast). Off by default (the new config
     fields default to `false`/`None`); enabled in the local-lane config template.

## Runtime configuration (NOT in this repo — machine-local, templated in `pocket-llm/jcode/`)

- **Reasoning starts OFF, escalates on demand** for gemma4: Ollama `/v1` honors the top-level
  `reasoning_effort` field, and patch 8 makes the loopback endpoint effort-capable. So instead of pinning
  `reasoning_effort` in `extra_body` (which would lock it), set the cold-start default with
  `[provider] openai_reasoning_effort = "none"` and let the effort-increase keybind raise it per-task.
  `extra_body` keeps only `temperature` / `top_p`; the launcher's `JCODE_OPENAI_EXTRA_BODY` mirrors this.
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
