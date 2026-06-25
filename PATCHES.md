# Fork divergence — RaggioS/jcode

This is a personal fork of [1jehuang/jcode](https://github.com/1jehuang/jcode) used as the sole
local-model coding harness driving Ollama `gemma4:12b` on macOS. It tracks upstream `master` and carries
a small set of changes on the `feat/local-gemma4-macos` branch.

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

## Runtime configuration (NOT in this repo — machine-local, templated in `pocket-llm/jcode/`)

- **Thinking off** for gemma4: Ollama `/v1` honors top-level `reasoning_effort`. Set it via a named provider
  in `~/.jcode/config.toml` (`[providers.ollama-local] extra_body = { reasoning_effort = "none" }`) or the
  `JCODE_OPENAI_EXTRA_BODY='{"reasoning_effort":"none"}'` env in the launcher.
- **Telemetry off**: `DO_NOT_TRACK=1`.
- **Italian persona**: jcode loads `~/AGENTS.md` (global) + project `AGENTS.md` into the system prompt
  (`crates/jcode-base/src/prompt.rs`). The lean Italian persona lives there.

## Keeping in sync with upstream

```bash
git fetch upstream
git rebase upstream/master            # on feat/local-gemma4-macos
cargo build --release --bin jcode
scripts/install_release.sh
```
