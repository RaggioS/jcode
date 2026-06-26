//! Import an external coding-agent transcript (currently Claude Code) into a
//! fresh jcode session so it can be resumed offline with the local model.
//!
//! Motivation: when a Claude Code session runs out of tokens/limits, the user
//! wants to continue the SAME conversation with the local gemma4 model. At that
//! moment Claude is unavailable, so the summary cannot be produced by an LLM —
//! the import is fully MECHANICAL (zero model calls). It reuses the compaction
//! summaries Claude already wrote into the transcript when available.
//!
//! Pipeline: parse `~/.claude/projects/<proj>/<id>.jsonl` -> map to jcode
//! `ContentBlock`s -> prepend a mechanical recap -> keep the most recent
//! messages within a token budget -> persist a normal jcode `Session`. The CLI
//! `--resume-external <path>` flag calls [`import_claude_session`] and then
//! drives the existing `--resume <id>` path, so no resume/TUI code changes.
//!
//! A Claude Code transcript is JSONL, one event per line. Only `user` and
//! `assistant` lines carry conversation; everything else (modes, attachments,
//! titles, hooks, file snapshots, …) is metadata we skip. `isSidechain` marks
//! subagent threads, `isMeta`/`isVisibleInTranscriptOnly` mark non-context
//! lines, and `isCompactSummary` marks a summary Claude generated on compaction.

use crate::message::{ContentBlock, Role};
use crate::session::Session;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::Path;

/// Token budget for the recent verbatim tail (≈ a third of the 262k window,
/// leaving room to keep working). Estimated as chars / `CHARS_PER_TOKEN`.
const RECENT_TOKEN_BUDGET: usize = 80_000;
const CHARS_PER_TOKEN: usize = 4;
/// Cap a single tool result so a giant file dump cannot blow the budget.
const MAX_TOOL_RESULT_CHARS: usize = 3_000;
/// Cap how many user prompts the fallback recap lists.
const MAX_RECAP_PROMPTS: usize = 40;
/// Cap each fallback-recap prompt line.
const MAX_RECAP_PROMPT_CHARS: usize = 200;

struct ParsedMsg {
    role: Role,
    content: Vec<ContentBlock>,
    /// Cheap size estimate (chars) for budget accounting.
    chars: usize,
}

/// Parse a Claude Code transcript and persist it as a new jcode session.
/// Returns the new session id (feed it to the normal `--resume <id>` path).
pub fn import_claude_session(path: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading Claude Code transcript {}", path.display()))?;

    let mut parsed: Vec<ParsedMsg> = Vec::new();
    let mut compact_summary: Option<String> = None;
    let mut user_prompts: Vec<String> = Vec::new();
    let mut title: Option<String> = None;
    let mut cwd: Option<String> = None;

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Tolerant: skip any line that is not valid JSON (format drift safety).
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        // Session metadata (first occurrence wins).
        if title.is_none() {
            if let Some(t) = v.get("aiTitle").and_then(Value::as_str) {
                title = Some(t.to_string());
            } else if let Some(t) = v.get("slug").and_then(Value::as_str) {
                title = Some(t.to_string());
            }
        }
        if cwd.is_none()
            && let Some(c) = v.get("cwd").and_then(Value::as_str)
        {
            cwd = Some(c.to_string());
        }

        let typ = v.get("type").and_then(Value::as_str).unwrap_or("");
        if typ != "user" && typ != "assistant" {
            continue;
        }

        // A compaction summary IS a ready-made recap Claude wrote; keep the last
        // one and do not also include it as a raw message. Check this BEFORE the
        // skip filters: Claude flags the summary `isVisibleInTranscriptOnly`
        // (shown but not re-sent), yet for an offline import it is the single
        // best recap we have, so we must not discard it.
        if flag(&v, "isCompactSummary") {
            if let Some(text) = message_plain_text(&v) {
                if !text.trim().is_empty() {
                    compact_summary = Some(text);
                }
            }
            continue;
        }

        if flag(&v, "isSidechain") || flag(&v, "isMeta") || flag(&v, "isVisibleInTranscriptOnly") {
            continue;
        }

        let role = if typ == "assistant" {
            Role::Assistant
        } else {
            Role::User
        };
        let content = map_content(&v);
        if content.is_empty() {
            continue;
        }

        // Collect genuine user prompts (text-only, not tool results) for the
        // fallback recap.
        if matches!(role, Role::User) {
            let has_tool_result = content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. }));
            if !has_tool_result {
                let text: String = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let text = text.trim();
                if !text.is_empty() && !text.starts_with("<system-reminder>") {
                    user_prompts.push(truncate(text, MAX_RECAP_PROMPT_CHARS));
                }
            }
        }

        let chars = content.iter().map(block_chars).sum();
        parsed.push(ParsedMsg {
            role,
            content,
            chars,
        });
    }

    if parsed.is_empty() {
        bail!(
            "no conversational messages found in {} (not a Claude Code transcript?)",
            path.display()
        );
    }

    let recap = build_recap(compact_summary.as_deref(), &user_prompts);
    let (recent, dropped) = take_recent_within_budget(parsed, RECENT_TOKEN_BUDGET * CHARS_PER_TOKEN);

    let title_str = title.unwrap_or_else(|| "Imported Claude Code session".to_string());
    let mut session = Session::create(None, Some(format!("↩ {title_str}")));
    session.provider_key = Some("ollama-local".to_string());
    session.model = Some("gemma4:12b".to_string());
    if let Some(c) = cwd {
        session.working_dir = Some(c);
    }

    let dropped_note = if dropped > 0 {
        format!(
            "\n\n({dropped} older message(s) omitted to fit the context budget; they are covered by the recap above.)"
        )
    } else {
        String::new()
    };
    let header = format!(
        "[Imported from Claude Code · {title_str}]\n\n\
         You are continuing a conversation that was started in Claude Code. \
         Recap of the work so far:\n\n{recap}{dropped_note}\n\n\
         --- recent conversation context below ---"
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: header,
            cache_control: None,
        }],
    );
    for m in recent {
        session.add_message(m.role, m.content);
    }

    session
        .save()
        .with_context(|| format!("saving imported session {}", session.id))?;
    Ok(session.id.clone())
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Map a Claude Code `message.content` to jcode content blocks. `thinking`/
/// reasoning blocks are dropped (the local model does not need Claude's hidden
/// reasoning, and it would cost tokens). `content` may be a plain string.
fn map_content(v: &Value) -> Vec<ContentBlock> {
    let Some(message) = v.get("message") else {
        return Vec::new();
    };
    let content = match message.get("content") {
        Some(c) => c,
        None => return Vec::new(),
    };

    if let Some(s) = content.as_str() {
        let s = s.trim();
        if s.is_empty() {
            return Vec::new();
        }
        return vec![ContentBlock::Text {
            text: s.to_string(),
            cache_control: None,
        }];
    }

    let Some(arr) = content.as_array() else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for block in arr {
        let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "text" => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    if !text.trim().is_empty() {
                        out.push(ContentBlock::Text {
                            text: text.to_string(),
                            cache_control: None,
                        });
                    }
                }
            }
            "tool_use" => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let input = block.get("input").cloned().unwrap_or(Value::Null);
                out.push(ContentBlock::ToolUse {
                    id,
                    name,
                    input,
                    thought_signature: None,
                });
            }
            "tool_result" => {
                let tool_use_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let content = truncate(&tool_result_text(block), MAX_TOOL_RESULT_CHARS);
                let is_error = block.get("is_error").and_then(Value::as_bool);
                out.push(ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                });
            }
            // thinking / reasoning / image / unknown -> drop
            _ => {}
        }
    }
    out
}

/// A Claude `tool_result` `content` is either a string or an array of
/// `{type:"text", text}` (and occasionally image blocks we ignore).
fn tool_result_text(block: &Value) -> String {
    match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Plain text of a whole message (used for compaction-summary extraction).
fn message_plain_text(v: &Value) -> Option<String> {
    let content = v.get("message")?.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    let arr = content.as_array()?;
    let text = arr
        .iter()
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    Some(text)
}

fn build_recap(summary: Option<&str>, prompts: &[String]) -> String {
    if let Some(s) = summary {
        let s = s.trim();
        if !s.is_empty() {
            return s.to_string();
        }
    }
    if prompts.is_empty() {
        return "(no recoverable summary; continue from the recent context below)".to_string();
    }
    let start = prompts.len().saturating_sub(MAX_RECAP_PROMPTS);
    let mut out = String::from("Conversation thread (user requests, oldest→newest):\n");
    for p in &prompts[start..] {
        out.push_str("- ");
        out.push_str(p);
        out.push('\n');
    }
    out
}

/// Keep the most recent messages whose cumulative size fits the char budget.
/// Returns (chronological recent messages, count dropped from the front).
fn take_recent_within_budget(parsed: Vec<ParsedMsg>, char_budget: usize) -> (Vec<ParsedMsg>, usize) {
    let total = parsed.len();
    let mut acc = 0usize;
    let mut kept_rev: Vec<ParsedMsg> = Vec::new();
    for m in parsed.into_iter().rev() {
        let next = acc.saturating_add(m.chars);
        if !kept_rev.is_empty() && next > char_budget {
            break;
        }
        acc = next;
        kept_rev.push(m);
    }
    let dropped = total - kept_rev.len();
    kept_rev.reverse();
    (kept_rev, dropped)
}

fn block_chars(b: &ContentBlock) -> usize {
    match b {
        ContentBlock::Text { text, .. } => text.len(),
        ContentBlock::ToolResult { content, .. } => content.len(),
        ContentBlock::ToolUse { name, input, .. } => name.len() + input.to_string().len(),
        _ => 0,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str(" …[truncated]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_user(text: &str) -> String {
        serde_json::json!({
            "type": "user",
            "cwd": "/work",
            "aiTitle": "demo",
            "message": { "role": "user", "content": text }
        })
        .to_string()
    }

    fn line_assistant_text(text: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "thinking", "thinking": "ignore me" },
                { "type": "text", "text": text }
            ]}
        })
        .to_string()
    }

    #[test]
    fn parses_and_skips_non_context() {
        let transcript = [
            serde_json::json!({"type":"mode","mode":"x"}).to_string(),
            line_user("first prompt"),
            line_assistant_text("an answer"),
            serde_json::json!({"type":"user","isSidechain":true,"message":{"role":"user","content":"sub"}}).to_string(),
            "{ not json".to_string(),
        ]
        .join("\n");

        // Reparse via the internal pieces (no disk write in unit test).
        let mut prompts = Vec::new();
        let mut msgs = 0;
        for line in transcript.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            let typ = v.get("type").and_then(Value::as_str).unwrap_or("");
            if typ != "user" && typ != "assistant" {
                continue;
            }
            if flag(&v, "isSidechain") {
                continue;
            }
            let blocks = map_content(&v);
            if blocks.is_empty() {
                continue;
            }
            if typ == "user" {
                if let ContentBlock::Text { text, .. } = &blocks[0] {
                    prompts.push(text.clone());
                }
            }
            msgs += 1;
        }
        assert_eq!(msgs, 2, "kept user + assistant, skipped sidechain/mode/badjson");
        assert_eq!(prompts, vec!["first prompt".to_string()]);
    }

    #[test]
    fn thinking_blocks_dropped_tool_result_truncated() {
        let big = "x".repeat(MAX_TOOL_RESULT_CHARS + 500);
        let v: Value = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "t1", "content": big }
            ]}
        });
        let blocks = map_content(&v);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.len() < MAX_TOOL_RESULT_CHARS + 100);
                assert!(content.ends_with("…[truncated]"));
            }
            _ => panic!("expected tool result"),
        }
    }

    #[test]
    fn recap_prefers_compaction_summary() {
        let r = build_recap(Some("THE SUMMARY"), &["p1".into()]);
        assert_eq!(r, "THE SUMMARY");
        let r2 = build_recap(None, &["p1".into(), "p2".into()]);
        assert!(r2.contains("p1") && r2.contains("p2"));
    }

    #[test]
    fn budget_keeps_recent_tail() {
        let parsed: Vec<ParsedMsg> = (0..10)
            .map(|i| ParsedMsg {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: format!("m{i}"),
                    cache_control: None,
                }],
                chars: 100,
            })
            .collect();
        let (recent, dropped) = take_recent_within_budget(parsed, 350);
        assert_eq!(recent.len() + dropped, 10);
        assert!(recent.len() <= 4 && !recent.is_empty());
    }
}
