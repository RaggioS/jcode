//! Listwise LLM reranking of memory retrieval candidates (recall-5, Mode-2).
//!
//! Hybrid retrieval (`MemoryManager::find_similar_hybrid`) reliably pulls the
//! relevant memories into a top-N candidate pool, but ranks them poorly: on the
//! recall benchmark the pool holds ~99% of relevant memories yet only ~53% reach
//! the top 5. A reranker that reorders the existing pool closes most of that gap.
//!
//! A local cross-encoder (MS-MARCO) was tried and *hurt* recall (out-of-domain
//! for memory statements, and it chokes on noisy multi-message context). A
//! listwise LLM reranker, fed the *focused* query (latest user intent, with
//! system-reminder/tool noise stripped) and all candidates in one call, lifts
//! benchmark recall@5 0.53 -> 0.75 and precision@5 0.23 -> 0.35.
//!
//! This module is the single source of truth for that reranking, shared by the
//! offline benchmark (`memory_recall_bench`) and the live memory agent so the
//! shipped behavior matches what was measured. It is pure with respect to the
//! memory agent (depends only on `Sidecar` + `MemoryEntry`).

use std::collections::HashSet;

use crate::memory_types::MemoryEntry;
use crate::sidecar::Sidecar;

/// System prompt instructing the model to rank candidates by usefulness.
pub const LLM_RERANK_SYSTEM: &str = "You re-rank stored MEMORIES by how useful each would be to surface to an AI coding agent for the CURRENT request. \
Order them best-first: a memory ranks high if a competent engineer would say knowing it specifically helps respond here (a relevant fact, preference, correction, or procedure). \
Off-topic, generic, or keyword-only matches rank low. \
Reply with ONLY a JSON array of candidate numbers, best first, e.g. [3,1,7]. Include only clearly useful candidates; omit ones that are not relevant. No prose.";

/// Cap the query length fed to the reranker. The query should already be the
/// focused (noise-stripped) view; this is a defensive bound. We keep the TAIL,
/// which carries the most recent intent.
const MAX_QUERY_CHARS: usize = 4000;

/// Per-candidate content cap so a single huge memory cannot dominate the prompt.
const MAX_CANDIDATE_CHARS: usize = 600;

fn truncate_tail(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    s.chars().skip(count - max).collect()
}

fn truncate_head(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

/// Build the listwise rerank prompt from a focused query and `(id, content)`
/// candidate pairs. Candidates are presented as a 1-based numbered list.
pub fn build_rerank_prompt(focused_query: &str, candidates: &[(String, String)]) -> String {
    let q = truncate_tail(focused_query, MAX_QUERY_CHARS);
    let mut p = String::with_capacity(256 + candidates.len() * 64);
    p.push_str("CURRENT REQUEST:\n");
    p.push_str(&q);
    p.push_str("\n\nCANDIDATE MEMORIES:\n");
    for (i, (_id, content)) in candidates.iter().enumerate() {
        let one_line = truncate_head(content, MAX_CANDIDATE_CHARS).replace('\n', " ");
        p.push_str(&format!("{}. {}\n", i + 1, one_line));
    }
    p.push_str("\nReturn candidate numbers ranked best-first as a JSON array.");
    p
}

/// Parse a ranked JSON array of 1-based candidate numbers into 0-based indices,
/// preserving order and dropping out-of-range / duplicate entries. Tolerates
/// surrounding prose by extracting the first `[`..`]` span.
pub fn parse_rerank_response(resp: &str, n: usize) -> Vec<usize> {
    let (Some(s), Some(e)) = (resp.find('['), resp.rfind(']')) else {
        return Vec::new();
    };
    if e < s {
        return Vec::new();
    }
    let nums: Vec<i64> = serde_json::from_str(&resp[s..=e]).unwrap_or_default();
    let mut seen = HashSet::new();
    nums.into_iter()
        .filter_map(|x| {
            let idx = usize::try_from(x).ok()?;
            if idx >= 1 && idx <= n && seen.insert(idx) {
                Some(idx - 1)
            } else {
                None
            }
        })
        .collect()
}

/// Rerank `candidates` with a listwise LLM call.
///
/// Returns ALL candidates reordered best-first (callers truncate to their own
/// top-k). Candidates the model ranks are placed first in model order; any
/// candidate the model omits is appended afterwards in the original hybrid order
/// (so omitted-but-retrieved memories are never lost, just deprioritized).
///
/// On any LLM/parse failure this falls back to the original input order, so the
/// reranker can never regress below the hybrid baseline.
pub async fn rerank_candidates(
    sidecar: &Sidecar,
    focused_query: &str,
    candidates: Vec<(MemoryEntry, f32)>,
) -> Vec<MemoryEntry> {
    if candidates.len() <= 1 {
        return candidates.into_iter().map(|(e, _)| e).collect();
    }

    let pairs: Vec<(String, String)> = candidates
        .iter()
        .map(|(e, _)| (e.id.clone(), e.content.clone()))
        .collect();
    let prompt = build_rerank_prompt(focused_query, &pairs);
    let n = candidates.len();

    let order = match sidecar.complete(LLM_RERANK_SYSTEM, &prompt).await {
        Ok(resp) => parse_rerank_response(&resp, n),
        Err(e) => {
            crate::logging::info(&format!(
                "Memory rerank failed ({e}); falling back to hybrid order"
            ));
            Vec::new()
        }
    };

    if order.is_empty() {
        // No usable ranking: preserve hybrid order.
        return candidates.into_iter().map(|(e, _)| e).collect();
    }

    let ranked_set: HashSet<usize> = order.iter().copied().collect();
    let mut entries: Vec<Option<MemoryEntry>> =
        candidates.into_iter().map(|(e, _)| Some(e)).collect();

    let mut out: Vec<MemoryEntry> = Vec::with_capacity(n);
    // Model-ranked candidates first, in model order.
    for idx in order {
        if let Some(entry) = entries[idx].take() {
            out.push(entry);
        }
    }
    // Then any candidates the model omitted, in original hybrid order.
    for (idx, slot) in entries.iter_mut().enumerate() {
        if !ranked_set.contains(&idx)
            && let Some(entry) = slot.take()
        {
            out.push(entry);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rerank_response_basic() {
        assert_eq!(parse_rerank_response("[3,1,2]", 3), vec![2, 0, 1]);
    }

    #[test]
    fn parse_rerank_response_dedups_and_bounds() {
        // 9 is out of range (n=3), duplicate 1 dropped, 0 invalid (1-based).
        assert_eq!(parse_rerank_response("[1, 9, 1, 2, 0]", 3), vec![0, 1]);
    }

    #[test]
    fn parse_rerank_response_tolerates_prose() {
        assert_eq!(
            parse_rerank_response("Here is the ranking: [2,1] (best first)", 2),
            vec![1, 0]
        );
    }

    #[test]
    fn parse_rerank_response_empty_on_garbage() {
        assert!(parse_rerank_response("no array here", 5).is_empty());
        assert!(parse_rerank_response("][", 5).is_empty());
    }

    #[test]
    fn build_prompt_numbers_candidates_one_based() {
        let cands = vec![
            ("a".to_string(), "first memory".to_string()),
            ("b".to_string(), "second memory".to_string()),
        ];
        let p = build_rerank_prompt("fix the scroll bug", &cands);
        assert!(p.contains("CURRENT REQUEST:\nfix the scroll bug"));
        assert!(p.contains("1. first memory"));
        assert!(p.contains("2. second memory"));
    }
}
