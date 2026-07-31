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
   - *2026-07 rebase*: upstream added `input/paste_guard.rs` and a `paste_guard::note_paste()` call at the
     top of `handle_paste` (issue #544: a stray Enter after a bracketed paste must not submit). The call
     now runs **before** our empty-text early return — an image-only paste is still a paste, so the
     stray-Enter suppression window must arm for it too. Upstream's
     `bare_enter_immediately_after_paste_does_not_submit` passes unchanged.
   - Result: Cmd+V (and Ctrl+V) attach screenshots in the VS Code terminal on macOS-Italian. Good
     upstream-PR candidate.

2. **Env-configurable server idle timeout** — `crates/jcode-app-core/src/server.rs`,
   `crates/jcode-app-core/src/server/util.rs`.
   - The shared server's idle shutdown was hardcoded to 5 minutes. Added `server_idle_timeout_secs()`
     reading `JCODE_SERVER_IDLE_TIMEOUT_SECS` (default 300), mirroring the existing
     `JCODE_EMBEDDING_IDLE_UNLOAD_SECS`. The launcher sets both to 1800 (30 min) so the provider/MCP pool,
     loaded embedder and resumable sessions survive gaps between windows.

3. **Local Ollama HTTP/1.1 transport** — `crates/jcode-provider-core/src/lib.rs`,
   `crates/jcode-base/src/provider/mod.rs`,
   `crates/jcode-provider-openrouter-runtime/src/openrouter_sse_stream.rs`.
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
     recent messages within a token budget, and leave the imported session's `provider_key`/`model`
     **unset** so resuming adopts the runtime provider/model (the local lane) instead of routing back to
     Claude. `provider_session_id` still records the Claude Code origin.
   - *2026-07 rebase*, three adaptations:
     - Upstream split the entrypoint into
       `import_session_from_file_with_target(path, session_id, jcode_session_id, require_source_identity)`.
       `jcode_session_id` is now a **parameter**, so our local `let jcode_session_id = …` is gone.
       `require_source_identity` is an unrelated concern (it asserts the transcript belongs to the named
       live session) and does not interact with this patch.
     - Our "don't clobber a jcode-side continuation" guard was **dropped**: upstream's new
       `finalize_imported_session` performs exactly that check (`session_exists` and
       `existing.messages.len() > session.messages.len()`) for *every* importer, comparing the fully built
       session so the recap message is counted the same way our local `imported_len` did. Keeping ours
       would have duplicated it. Still covered by upstream's
       `test_reimporting_claude_session_preserves_jcode_continuation`.
     - Our `truncate_import_text` helper was **dropped** in favor of upstream's same-named one, which is
       char-boundary safe (`jcode_core::util::truncate_str`) and reports the omitted byte count. Our two
       call sites now pass `String` and byte budgets (`MAX_IMPORT_TOOL_RESULT_BYTES`,
       `MAX_IMPORT_RECAP_PROMPT_BYTES`).
   - Upstream also added `crates/jcode-base/src/claude_live.rs` + `take_over_live_claude_session`, an
     explicit "hand a *running* Claude session to jcode" flow routed through the same importer. It stops the
     Claude process and returns a normal `ResumeTarget::JcodeSession`, so the unset provider/model is the
     right behavior there too — the conversation continues on the runtime provider. Upstream's takeover
     tests assert only `provider_session_id` and message content, so they are unaffected (they are
     `#[cfg(target_os = "linux")]`, hence exercised in CI rather than on macOS).

6. **Self-heal a downed local Ollama mid-session** —
   `crates/jcode-provider-openrouter-runtime/src/openrouter_sse_stream.rs`.
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
   - *2026-07 rebase — the guard was moved and widened.* It originally lived inside a **single arm** of
     `model_switch_request_for_session_route` (`OpenAiCompatible { profile_id: Some(_) }`). Upstream then
     reworked `model_switch_request_for_session_model` to be profile/credential aware, adding a bare-provider
     `AuthRoute::parse` branch that emits `format!("{prefix}:{model}")`; combined with the pre-existing
     `split_once(':')` branch (which returns an already-prefixed model *verbatim*) and the trailing
     named-profile branch, three separate paths could re-emit or pass through a `<provider>:` prefix without
     ever consulting the guard — reproducing the original `400 invalid model name` bug. The guard is now a
     helper (`bare_model_for_local_loopback`) called as an **early return at the top of
     `model_switch_request_for_session_model`**, before any prefix-emitting branch, and it handles both
     shapes: a stored model that already carries a loopback `<profile>:` prefix (stripped) and a bare model
     whose session `provider_key` is a loopback profile (kept bare). The in-arm guard in `..._route` stays,
     and every other `..._route` branch either hardcodes a remote provider or falls through to the hardened
     `..._model`. Explicit credential pins (`claude-oauth:` …) still win, since upstream's
     `explicit_model_provider_prefix` check runs first. Regression test:
     `session_restore_never_leaks_loopback_prefix_through_model_switch_request`. Cross-checked against
     upstream's `provider/tests/issue_534_profile_preservation.rs`, which exercises a **remote** gateway and
     is therefore untouched by the guard.

8. **RETIRED (2026-07 upstream rebase) — runtime reasoning toggle for local loopback endpoints.**
   Superseded by upstream's per-provider `supports_reasoning_effort` config field, which
   `supports_deepseek_reasoning_effort` / `initial_reasoning_effort` now honor directly. Setting
   `supports_reasoning_effort = true` on the `[providers.*]` block does exactly what the patch did, via
   config instead of a code patch, so the patch was dropped rather than re-adapted. The number is kept as a
   tombstone so patches 9+ do not shift.
   > **Machine-local config note:** the key upstream accepts is `supports_reasoning_effort` (aliases:
   > `supports-reasoning-effort`, `reasoning_effort`). `reasoning_effort_support` is **not** a recognized
   > key, and `NamedProviderConfig` is `#[serde(default)]` without `deny_unknown_fields`, so a misspelled
   > key is silently ignored and the local lane loses the effort toggle without any error.

9. **Auto reasoning-escalation by prompt complexity (local lane)** —
   `crates/jcode-provider-openrouter-runtime/src/{lib.rs,openrouter_provider_impl.rs}`,
   `crates/jcode-config-types/src/lib.rs`.
   - Survives patch 8's retirement: its loopback predicate is
     `jcode_base::provider_catalog::api_base_uses_localhost`, which is **upstream-native** (patch 8 only
     called it, never defined it), so nothing had to be re-added. It still needs
     `supports_reasoning_effort = true` in the provider config for the escalated effort to reach the wire.
   - Complements the manual effort keybind: when `[provider] auto_reasoning_effort = true`, a request whose
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

10. **`user_prompt` capturing hook for pre-turn context injection** — `crates/jcode-base/src/hooks.rs`,
    `crates/jcode-app-core/src/agent/turn_execution.rs`,
    `crates/jcode-base/src/config{.rs,/env_overrides.rs}`, `crates/jcode-config-types/src/lib.rs`.
    - A hook run before each user turn, receiving the user message on stdin. On exit 0 its stdout is
      injected into the conversation as extra context ahead of the user message (post-cutoff knowledge
      grounding); empty stdout or a non-zero exit injects nothing. Unlike `pre_tool` it never blocks the
      turn, and it is bounded by `hooks.user_prompt_timeout_ms` (default 5000).
    - Config: `[hooks] user_prompt`, env overrides `JCODE_HOOK_USER_PROMPT` /
      `JCODE_HOOK_USER_PROMPT_TIMEOUT_MS`.

11. **CI: skip the SSH agent step when no `DEPLOY_KEY` is configured** — `.github/workflows/ci.yml`.
    - The fork has no deploy key, so the upstream SSH-agent step failed the run on every push. Guarded on
      the secret being present.

12. **Self-heal the macOS sleep assertion** — `crates/jcode-base/src/{platform.rs,platform_tests.rs,
    power_inhibit.rs,session.rs,session_tests/cases.rs}`, `crates/jcode-app-core/src/agent{.rs,/streaming.rs,
    /turn_loops.rs,/turn_streaming_mpsc.rs}`, `crates/jcode-base/src/config/default_file.rs`.
    - `prevent_sleep_while_streaming` held an open-ended power assertion, so a wedged turn (hung stream,
      blocked send) kept the machine awake until the process was killed — draining the battery.
    - The hold now carries a bounded TTL (150s) and is renewed only while the turn makes **observable
      progress**: provider stream events arriving, or a tool still running. A wedged process stops renewing,
      the hold expires within the TTL, and the machine may sleep again. A long build still keeps it awake.

13. **Wall-clock timeout for one-shot `jcode run`** — `src/cli/{commands.rs,commands_tests.rs}`,
    `crates/jcode-base/src/{config.rs,config_tests.rs,config/default_file.rs,config/env_overrides.rs}`,
    `crates/jcode-config-types/src/lib.rs`.
    - A headless `jcode run` could hang forever, leaving zombie one-shot processes. `[provider]
      run_timeout_secs` (default 1800, `0` disables, env `JCODE_RUN_TIMEOUT_SECS`) makes it exit with an
      error instead. Interactive TUI sessions are unaffected.

14. **Ignore runtime session artifacts** — `.gitignore`.
    - `.claude/worktrees/` (per-session agent worktrees) alongside the `logs/*.json` entry from patch 4.

## Runtime configuration (NOT in this repo — machine-local, templated in `pocket-llm/jcode/`)

- **Reasoning starts OFF, escalates on demand** for gemma4: Ollama `/v1` honors the top-level
  `reasoning_effort` field. Since the 2026-07 rebase the loopback endpoint is made effort-capable by
  **config**, not by patch 8 (retired): set `supports_reasoning_effort = true` on the
  `[providers.ollama-local]` block. Then, instead of pinning `reasoning_effort` in `extra_body` (which would
  lock it), set the cold-start default with `[provider] openai_reasoning_effort = "none"` and let the
  effort-increase keybind raise it per-task. `extra_body` keeps only `temperature` / `top_p`; the launcher's
  `JCODE_OPENAI_EXTRA_BODY` mirrors this.
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

Notes from the 2026-07 rebase (merge-base `cef9b42ca`, 1003 upstream commits):

- Enable `git config rerere.enabled true` first — the same conflicts recur if the rebase is retried.
- Take **upstream's `Cargo.lock` wholesale**. None of the carried patches add a dependency, and letting the
  fork's lockfile win drags stale transitive versions forward (it was holding `tract` at 0.21.10, keeping
  RUSTSEC-2026-0217 alive; upstream's 0.23.4 clears it).
- Fork history must be **merged or rebase-merged, never squashed**. Squashing collapses the fork's ancestry
  with upstream's and makes the next `--onto` rebase re-conflict on everything.
