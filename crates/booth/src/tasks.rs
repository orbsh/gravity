//! Which code rows close which tasks (k10r ADR-0021) — BEHAVIOUR, not prompt text.
//!
//! What a turn is SHOWN about its work is the store's: k10r renders it into `session.owed`, because
//! which tasks a prompt carries and in what shape is knowledge policy, and a caller that decided it
//! would be deciding it again in every caller. What stays here is the table the call site closes
//! work with — a code row finishing closes the tasks bound to it.

use serde_json::Value;

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
    use serde_json::json;

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

}
