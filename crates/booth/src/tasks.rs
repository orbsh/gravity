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
    }
    block.push_str("\nMove them with `move_task`, add work with `open_task`, close with `state: done`.");
    Some(block)
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