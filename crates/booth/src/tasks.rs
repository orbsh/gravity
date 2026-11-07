//! What a turn must know about the work it owes (k10r ADR-0021).
//!
//! The store owns the tasks; this module owns one decision the store cannot make
//! for it: which of them a prompt should carry, and in what shape. It is a separate
//! module because it is pure — a list of rows in, a block of text out — and because
//! the first version of it, written inline next to the fetch, was wrong in a way
//! only a test would have caught: it asked the store for `state=open`, so a task the
//! agent had just moved to `doing` fell out of the list, and the one thing it was
//! actually doing became the one thing the next turn could not see.

use serde_json::Value;

/// Everything not `done`, with the state spelled out — one line per task, id
/// first because the model needs it to call `move_task`.
///
/// The state is on every line because `blocked` and `open` mean different things
/// to a reader: one is work that will not move without an answer, the other is
/// work that only needs doing. `None` when nothing is owed, so the caller can
/// leave the prompt alone rather than send an empty heading.
pub fn render_owed(tasks: &[Value]) -> Option<String> {
    let owed: Vec<&Value> = tasks
        .iter()
        .filter(|task| task.get("state").and_then(Value::as_str) != Some("done"))
        .collect();
    if owed.is_empty() {
        return None;
    }
    let mut block = String::from("Tasks this memory still holds (id - state - name - next action):");
    for task in owed {
        let id = task
            .get("node_id")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let state = task.get("state").and_then(Value::as_str).unwrap_or_default();
        let name = task.get("name").and_then(Value::as_str).unwrap_or_default();
        let next = task.get("next").and_then(Value::as_str).unwrap_or_default();
        block.push_str(&format!("\n- {id} - {state} - {name}"));
        if !next.is_empty() {
            block.push_str(&format!(" - {next}"));
        }
        // A bound task closes ITSELF when its code row runs (ADR-0003 §3). Saying
        // so here is what keeps the model from moving work the booth is about
        // to close, and from waiting on a `move_task` nobody needs to make.
        let bound: Vec<String> = task
            .get("finishes_on")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|code| code.as_u64())
            .map(|code| code.to_string())
            .collect();
        if !bound.is_empty() {
            block.push_str(&format!(" [closes when code {} runs]", bound.join(", ")));
        }
    }
    block.push_str(
        "\nMove them with `move_task`, add work with `open_task`, close with `state: done`. \
         A task marked `[closes when code N runs]` closes by itself when that step \
         succeeds — do not move it afterwards.",
    );
    Some(block)
}

/// Which tasks each CODE row finishes, built from the listing the turn already
/// fetched (`finishes_on` edges, k10r ADR-0021). One map, read once per turn:
/// the call site looks up a code id here instead of asking the store anything —
/// ADR-0003 §4's "every in-turn check is a memory read", literally.
///
/// A task already `done` is left out: closing it again would only bump its
/// clock, and the map exists to close work, not to restate it.
pub fn bindings(tasks: &[Value]) -> std::collections::BTreeMap<u64, Vec<u64>> {
    let mut out: std::collections::BTreeMap<u64, Vec<u64>> = std::collections::BTreeMap::new();
    for task in tasks {
        if task.get("state").and_then(Value::as_str) == Some("done") {
            continue;
        }
        let Some(id) = task.get("node_id").and_then(Value::as_u64) else {
            continue;
        };
        let bound = task
            .get("finishes_on")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for code in bound {
            if let Some(code) = code.as_u64() {
                out.entry(code).or_default().push(id);
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::render_owed;
    use serde_json::json;

    fn task(id: u64, state: &str, name: &str, next: &str) -> serde_json::Value {
        json!({ "node_id": id, "state": state, "name": name, "next": next })
    }

    #[test]
    fn a_done_task_needs_no_closing() {
        // The map is what the call site closes with; a task already `done` must
        // not ride it, or every later run of that code bumps its clock.
        let bound = json!({ "node_id": 12, "state": "doing", "name": "a",
                            "finishes_on": [7] });
        let closed = json!({ "node_id": 13, "state": "done", "name": "b",
                             "finishes_on": [7] });
        let map = super::bindings(&[bound, closed]);
        assert_eq!(map.get(&7), Some(&vec![12]), "only the open one: {map:?}");
    }

    #[test]
    fn one_code_row_may_finish_several_tasks() {
        let first = json!({ "node_id": 12, "state": "open", "name": "a", "finishes_on": [7] });
        let second = json!({ "node_id": 14, "state": "blocked", "name": "b", "finishes_on": [7] });
        let map = super::bindings(&[first, second]);
        assert_eq!(map.get(&7), Some(&vec![12, 14]));
    }

    #[test]
    fn the_work_in_progress_is_not_filtered_out() {
        // The bug this module exists to prevent: `doing` is the MOST relevant
        // state, not a state to ask the store to exclude.
        let block = render_owed(&[
            task(3, "open", "write-the-diff", ""),
            task(9, "doing", "ship-the-face-change", "read the diff"),
            task(12, "done", "old-thing", ""),
        ])
        .expect("two tasks are owed");
        assert!(block.contains("- 9 - doing - ship-the-face-change - read the diff"));
        assert!(block.contains("- 3 - open - write-the-diff"));
        assert!(!block.contains("old-thing"), "done is not owed");
    }

    #[test]
    fn a_bound_task_says_which_step_closes_it() {
        // The model must see that this one needs no `move_task`: the call site
        // closes it. Unbound tasks stay unmarked.
        let bound = json!({ "node_id": 12, "state": "doing", "name": "a", "next": "",
                            "finishes_on": [7, 9] });
        let free = task(13, "open", "b", "");
        let block = render_owed(&[bound, free]).expect("owed");
        assert!(
            block.contains("- 12 - doing - a [closes when code 7, 9 runs]"),
            "{block}"
        );
        assert!(block.contains("- 13 - open - b\n"), "unmarked: {block}");
        assert!(block.contains("closes by itself"), "{block}");
    }

    #[test]
    fn a_blocked_task_says_so() {
        let block = render_owed(&[task(4, "blocked", "wait-on-review", "")]).expect("owed");
        assert!(block.contains("blocked"));
    }

    #[test]
    fn nothing_owed_is_no_block_at_all() {
        assert_eq!(render_owed(&[]), None);
        assert_eq!(render_owed(&[task(12, "done", "old-thing", "")]), None);
    }

    #[test]
    fn an_empty_next_action_leaves_no_trailing_dash() {
        let block = render_owed(&[task(3, "open", "write-the-diff", "")]).expect("owed");
        assert!(
            block.contains("- 3 - open - write-the-diff\n"),
            "no next action, no dash: {block}"
        );
    }
}