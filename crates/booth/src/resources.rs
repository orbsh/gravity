//! The resource table: which durable data a turn may touch, and what material to mint for one
//! transfer of it (ADR-0003 §7).
//!
//! **Prism is a pipe.** It carries the Effector's `MaterialRequest` and `Synced` frames between a
//! node and this booth and knows nothing about what a resource is — its database holds user logins
//! and no business data. The knowledge of where data lives, who may read it and what can open it
//! belongs to the component that owns the turn, which is this one.
//!
//! # Two layers, one key space
//!
//! A resource is a composite KEY, not a row with a name:
//!
//! ```text
//! [owner][US][type][US][id: u64 BE]                      the resource itself
//! [owner][US][type][US][id: u64 BE][version: uuid v7]    one of its versions
//! ```
//!
//! - `owner` first: "everything I own" is the query that has to be possible, and it is the only
//!   field that can carry a permission boundary. A task never declares its own owner — it runs
//!   under whoever submitted the turn, and the plane fills it in. Otherwise one task could borrow
//!   another's data by naming it.
//! - `type` is part of the identity, not a transport detail: the same dataset delivered as `git`
//!   and as a `snapshot` are two resources, because their versions, writability and undo differ.
//! - `id` is a fixed-width `u64`, big-endian, so byte order is numeric order and a range scan after
//!   `[owner][type]` is a plain contiguous walk. It is a sequence, not a label: `desc` carries the
//!   text, so nobody has to encode meaning into an identifier.
//! - `version` is a UUID v7 in its BINARY form: 16 bytes, timestamp in the leading 48 bits, so
//!   byte order is time order and the segment is FIXED width. Fixed width is what makes the key a
//!   clean prefix: `[owner][type][id]` is a boundary the store can compute exactly, with no
//!   separator to scan for and no variable tail. It is also 16 bytes instead of a ~60-character
//!   stamp. The human reference (`<branch>@<sha>`) is NOT in the key — the key says WHEN, the value
//!   says WHAT — so a version's meaning can change shape (a snapshot id, an object version) without
//!   touching the key layout.
//!
//! That is what makes "the current version" a POSITION in the key space rather than the result of a
//! comparison: publishing is an append, undo steps back one key, and nothing is rewritten in place.
//!
//! # What lives where
//!
//! The prefix record holds what every type has: `desc`, `mode`, `created_at`, and a dynamic
//! section for the type's own fields (`git`: `source`, `ref`). The version record holds what the
//! node reported when it published. Credentials are in NEITHER: they belong to the owner's secret
//! material and are fetched when material is minted, so a dump of this table is not a set of keys.

use std::collections::BTreeMap;

/// Field separator: a control character, so it cannot occur in a name that was not meant to
/// contain it. Refused in `owner` and `type` for exactly that reason.
const US: u8 = 0x1f;

/// Whether a task may publish to a resource. Reading is the ordinary case, so `ro` is the default
/// wherever a choice is made by omission — a resource that becomes writable by omission is how
/// data gets lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    ReadOnly,
    ReadWrite,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::ReadOnly => "ro",
            Mode::ReadWrite => "rw",
        }
    }
}

/// The identity of a resource. `owner` is filled by the plane from the turn, never by the task.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Id {
    pub owner: String,
    pub kind: String,
    pub seq: u64,
}

impl Id {
    /// The prefix key: everything under it is this resource's history.
    pub fn key(&self) -> Vec<u8> {
        let mut key = Vec::with_capacity(self.owner.len() + self.kind.len() + 10);
        key.extend_from_slice(self.owner.as_bytes());
        key.push(US);
        key.extend_from_slice(self.kind.as_bytes());
        key.push(US);
        key.extend_from_slice(&self.seq.to_be_bytes());
        key
    }

    /// A key must be usable as a prefix: no separator inside a part, and nothing empty.
    fn validate(&self) -> anyhow::Result<()> {
        for (label, part) in [("owner", &self.owner), ("type", &self.kind)] {
            if part.is_empty() {
                anyhow::bail!("a resource {label} cannot be empty");
            }
            if part.bytes().any(|b| b == US || b.is_ascii_control()) {
                anyhow::bail!("a resource {label} cannot contain control characters");
            }
            // Not a path, but `/` is refused anyway: the node names its directory
            // `<type>/<id>`, so a slash in either part would move the data somewhere else.
            if part.contains('/') || part.contains('\\') {
                anyhow::bail!("a resource {label} is a name, not a path");
            }
        }
        Ok(())
    }
}

/// The prefix record: what is true of the resource regardless of version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    /// Text, for humans. The identifier is `Id`; this is where meaning goes.
    pub desc: String,
    pub mode: Mode,
    /// Milliseconds since the epoch, as the caller's clock reported it (the caller owns the clock).
    pub created_at: u64,
    /// The type's own fields, e.g. `git` → `source`, `ref`. Adding a type adds fields here and
    /// changes no key and no schema.
    pub spec: BTreeMap<String, String>,
}

/// A version record: exactly what the node said when it published, under a v7 key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// The version's identity — a UUID v7. Its bytes ARE the last key segment, so this value and
    /// the key can never disagree.
    pub id: uuid::Uuid,
    /// What the node reported: `<branch>@<sha>` for `git`. Free-form on purpose: another transport
    /// reports its own kind of reference, and the key does not care.
    pub reference: String,
    /// The call that produced it, so a bad write is traceable to the turn that made it.
    pub call_id: String,
    /// When, as the caller's clock reported it. Equal to the v7 timestamp by construction.
    pub created_at: u64,
}

/// What a node receives: where to reach the store for THIS transfer, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Material {
    pub url: String,
    pub credential: Option<String>,
}

impl Resource {
    /// Material for one transfer of this resource. `write` asks for the publishing half: a
    /// read-only resource has none, and asking is an ERROR rather than a quiet read handle —
    /// a task that publishes to a read-only resource has a bug, and it should surface here.
    pub fn material(&self, write: bool, credential: Option<&str>) -> anyhow::Result<Material> {
        if write && self.mode != Mode::ReadWrite {
            anyhow::bail!("resource is read-only");
        }
        let url = self
            .spec
            .get("source")
            .ok_or_else(|| anyhow::anyhow!("resource has no `source` field"))?
            .clone();
        Ok(Material {
            url,
            credential: credential.map(str::to_string),
        })
    }

    /// The ref to check out when the task did not name one.
    pub fn default_ref(&self) -> Option<&str> {
        self.spec.get("ref").map(String::as_str)
    }
}

/// The table. In-memory here because a booth holds no connection state (its store URL arrives per
/// call, ADR-0031); the same bytes go into whatever key-value store the venue uses, because the key
/// layout is the contract and this type keeps nothing else.
#[derive(Debug)]
pub struct Table {
    prefix: BTreeMap<Vec<u8>, Resource>,
    versions: BTreeMap<Vec<u8>, Version>,
    /// The v7 generator's context. It carries the monotonic counter, so two versions published in
    /// the same millisecond still sort in publication order — without it, the same-millisecond
    /// order would be the 74 random bits' business, which is not an order at all.
    clock: uuid::timestamp::context::ContextV7,
}

impl Default for Table {
    fn default() -> Self {
        Self {
            prefix: BTreeMap::new(),
            versions: BTreeMap::new(),
            clock: uuid::timestamp::context::ContextV7::new(),
        }
    }
}

impl Table {
    pub fn declare(&mut self, id: &Id, resource: Resource) -> anyhow::Result<()> {
        id.validate()?;
        if !resource.spec.contains_key("source") {
            anyhow::bail!("a resource must say where its data lives (`spec.source`)");
        }
        self.prefix.insert(id.key(), resource);
        Ok(())
    }

    pub fn resource(&self, id: &Id) -> Option<&Resource> {
        self.prefix.get(&id.key())
    }

    /// Record a published version. The caller supplies the clock, as everywhere else in the plane.
    pub fn publish(
        &mut self,
        id: &Id,
        call_id: &str,
        reference: &str,
        now_ms: u64,
    ) -> Option<&Version> {
        self.resource(id)?;
        let timestamp = uuid::Timestamp::from_unix(
            &self.clock,
            now_ms / 1000,
            ((now_ms % 1000) * 1_000_000) as u32,
        );
        let version = uuid::Uuid::new_v7(timestamp);
        let mut key = id.key();
        // No separator: the uuid is 16 bytes, so the prefix is still an exact boundary.
        key.extend_from_slice(version.as_bytes());
        self.versions.insert(
            key,
            Version {
                id: version,
                reference: reference.to_string(),
                call_id: call_id.to_string(),
                created_at: now_ms,
            },
        );
        self.current(id)
    }

    /// The current version: the LAST key under this resource's prefix. One ordered walk of this
    /// resource's own history — it stops at the first key that is not ours — and it reads no value
    /// to decide, which is what makes the answer a position rather than a comparison.
    pub fn current(&self, id: &Id) -> Option<&Version> {
        let prefix = id.key();
        self.versions
            .range(prefix.clone()..)
            .take_while(|(key, _)| key.starts_with(&prefix) && key.len() > prefix.len())
            .last()
            .map(|(_, version)| version)
    }

    /// Undo: the version before the current one, or `None` when there is nothing to go back to.
    /// Undoing is stepping back a key, never rewriting anything.
    pub fn previous(&self, id: &Id) -> Option<&Version> {
        let prefix = id.key();
        let mut history: Vec<&Version> = self
            .versions
            .range(prefix.clone()..)
            .take_while(|(key, _)| key.starts_with(&prefix) && key.len() > prefix.len())
            .map(|(_, version)| version)
            .collect();
        history.pop()?;
        history.pop()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn git_id(seq: u64) -> Id {
        Id {
            owner: "alice".into(),
            kind: "git".into(),
            seq,
        }
    }

    fn resource(mode: Mode) -> Resource {
        let mut spec = BTreeMap::new();
        spec.insert(
            "source".to_string(),
            "git@example.invalid:notes.git".to_string(),
        );
        spec.insert("ref".to_string(), "main".to_string());
        Resource {
            desc: "my notes".into(),
            mode,
            created_at: 1,
            spec,
        }
    }

    #[test]
    fn the_key_orders_by_owner_then_type_then_numeric_id() {
        // Big-endian u64: 10 sorts after 2, even though its text would not.
        let mut ids = [git_id(10), git_id(2), git_id(1)];
        ids.sort_by_key(|id| id.key());
        assert_eq!(
            ids.iter().map(|id| id.seq).collect::<Vec<_>>(),
            vec![1, 2, 10]
        );
        assert!(git_id(1).key().starts_with(b"alice\x1fgit\x1f"));
        // A different owner never shares a prefix with this one.
        let other = Id {
            owner: "bob".into(),
            ..git_id(1)
        };
        assert!(!other.key().starts_with(b"alice"));
    }

    #[test]
    fn material_is_minted_per_transfer_and_never_invented_for_a_read_only_resource() {
        let mut table = Table::default();
        table
            .declare(&git_id(1), resource(Mode::ReadOnly))
            .unwrap();
        let r = table.resource(&git_id(1)).unwrap();
        assert_eq!(
            r.material(false, Some("token")).unwrap(),
            Material {
                url: "git@example.invalid:notes.git".into(),
                credential: Some("token".into()),
            }
        );
        assert!(r.material(true, None).is_err(), "ro has no write half");
        assert_eq!(r.default_ref(), Some("main"));
        // Nothing is invented for a resource that was never declared.
        assert!(table.resource(&git_id(2)).is_none());
    }

    #[test]
    fn publishing_appends_and_the_current_version_is_the_last_key() {
        let mut table = Table::default();
        table
            .declare(&git_id(7), resource(Mode::ReadWrite))
            .unwrap();

        let first = table
            .publish(&git_id(7), "call-1", "main@aaa", 1_700_000_000_000)
            .unwrap()
            .clone();
        assert_eq!(first.reference, "main@aaa");
        assert_eq!(first.created_at, 1_700_000_000_000);
        assert_eq!(first.id.get_version_num(), 7, "the key is a v7 uuid");
        let second = table
            .publish(&git_id(7), "call-2", "main@bbb", 1_700_000_005_000)
            .unwrap()
            .clone();
        assert_eq!(second.reference, "main@bbb");
        assert!(second.id > first.id, "time order is byte order");

        // "Current" is a position, not a scan result: the last key under the prefix.
        assert_eq!(table.current(&git_id(7)).unwrap(), &second);
        // Undo steps back one key. Nothing was rewritten to make that true.
        assert_eq!(table.previous(&git_id(7)).unwrap(), &first);
        assert_eq!(
            table.previous(&git_id(9)),
            None,
            "nothing declared, nothing to undo"
        );
    }

    #[test]
    fn a_publish_to_an_undeclared_resource_records_nothing() {
        let mut table = Table::default();
        assert!(table.publish(&git_id(3), "call", "main@x", 1).is_none());
        assert!(table.current(&git_id(3)).is_none());
    }

    #[test]
    fn a_key_that_could_not_be_a_prefix_is_refused() {
        let mut table = Table::default();
        for (owner, kind) in [
            ("", "git"),
            ("alice", ""),
            ("al\x1fice", "git"),
            ("a/b", "git"),
        ] {
            let id = Id {
                owner: owner.into(),
                kind: kind.into(),
                seq: 1,
            };
            assert!(
                table.declare(&id, resource(Mode::ReadOnly)).is_err(),
                "{owner}/{kind}"
            );
        }
        // A resource with no source can be neither materialized nor checked out.
        let mut bare = resource(Mode::ReadOnly);
        bare.spec.remove("source");
        assert!(table.declare(&git_id(1), bare).is_err());
    }

    #[test]
    fn two_versions_in_the_same_millisecond_still_sort_in_publication_order() {
        let mut table = Table::default();
        table
            .declare(&git_id(4), resource(Mode::ReadWrite))
            .unwrap();
        let same_ms = 1_700_000_000_000;
        let a = table.publish(&git_id(4), "c-a", "main@a", same_ms).unwrap().clone();
        let b = table.publish(&git_id(4), "c-b", "main@b", same_ms).unwrap().clone();
        assert!(b.id > a.id, "the generator's counter orders within a millisecond");
        assert_eq!(table.current(&git_id(4)).unwrap(), &b);
        assert_eq!(table.previous(&git_id(4)).unwrap(), &a);
    }
}