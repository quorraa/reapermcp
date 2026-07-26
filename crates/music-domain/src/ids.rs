//! Identifier helpers built on `qjson::uuid`.
//!
//! Two kinds of id exist in this system and they must not be confused:
//!
//! * **Content-derived** ids ([`derived_id`] and the `*_id_from` helpers) are
//!   pure functions of a namespace and a name. The same input always produces
//!   the same id, forever, which is what makes golden fixtures and knowledge
//!   digests reproducible.
//! * **Runtime** ids ([`IdFactory::process_unique`]) are unique per process and
//!   are used for analyses, candidates and transactions that only live inside
//!   one session. [`IdFactory::from_seed`] gives the same sequence for the same
//!   seed, which is what determinism tests use.

pub use qjson::uuid::{uuid_from_name, UuidGen};

/// Namespace for analysis ids.
pub const NS_ANALYSIS: &str = "qlabs.reapermcp.analysis";
/// Namespace for candidate ids.
pub const NS_CANDIDATE: &str = "qlabs.reapermcp.candidate";
/// Namespace for edit-plan ids.
pub const NS_PLAN: &str = "qlabs.reapermcp.plan";
/// Namespace for transaction ids.
pub const NS_TRANSACTION: &str = "qlabs.reapermcp.transaction";
/// Namespace for project-snapshot ids.
pub const NS_SNAPSHOT: &str = "qlabs.reapermcp.snapshot";
/// Namespace for knowledge-derived ids.
pub const NS_KNOWLEDGE: &str = "qlabs.reapermcp.knowledge";
/// Namespace for fixture ids.
pub const NS_FIXTURE: &str = "qlabs.reapermcp.fixture";

/// Every namespace this crate defines, in declaration order.
pub const NAMESPACES: &[&str] = &[
    NS_ANALYSIS,
    NS_CANDIDATE,
    NS_PLAN,
    NS_TRANSACTION,
    NS_SNAPSHOT,
    NS_KNOWLEDGE,
    NS_FIXTURE,
];

/// A stable, content-derived id: the same `(namespace, name)` always yields the
/// same UUID.
pub fn derived_id(namespace: &str, name: &str) -> String {
    uuid_from_name(namespace, name)
}

/// Content-derived analysis id.
pub fn analysis_id_from(name: &str) -> String {
    derived_id(NS_ANALYSIS, name)
}

/// Content-derived candidate id.
pub fn candidate_id_from(name: &str) -> String {
    derived_id(NS_CANDIDATE, name)
}

/// Content-derived edit-plan id.
pub fn plan_id_from(name: &str) -> String {
    derived_id(NS_PLAN, name)
}

/// Content-derived transaction id.
pub fn transaction_id_from(name: &str) -> String {
    derived_id(NS_TRANSACTION, name)
}

/// Content-derived snapshot id.
pub fn snapshot_id_from(name: &str) -> String {
    derived_id(NS_SNAPSHOT, name)
}

/// Content-derived fixture id.
pub fn fixture_id_from(name: &str) -> String {
    derived_id(NS_FIXTURE, name)
}

/// A source of fresh ids.
///
/// Wraps [`UuidGen`] so callers do not have to decide between the deterministic
/// and the process-unique generator at every call site.
#[derive(Debug)]
pub struct IdFactory {
    generator: UuidGen,
}

impl IdFactory {
    /// A factory whose sequence depends only on `seed`.
    pub fn from_seed(seed: u64) -> IdFactory {
        IdFactory {
            generator: UuidGen::from_seed(seed),
        }
    }

    /// A factory seeded from the clock and process id.
    pub fn process_unique() -> IdFactory {
        IdFactory {
            generator: UuidGen::process_unique(),
        }
    }

    /// The next id in the sequence.
    pub fn next(&self) -> String {
        self.generator.next()
    }

    /// The next id with a readable prefix, e.g. `"cand-<uuid>"`.
    pub fn next_prefixed(&self, prefix: &str) -> String {
        format!("{prefix}-{}", self.generator.next())
    }
}

/// True when `s` looks like a canonical lowercase UUID.
pub fn is_uuid_shaped(s: &str) -> bool {
    let groups = [8usize, 4, 4, 4, 12];
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != groups.len() {
        return false;
    }
    parts.iter().zip(groups).all(|(p, len)| {
        p.len() == len
            && p.chars()
                .all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_ids_are_stable() {
        let a = derived_id(NS_CANDIDATE, "melody-1/functional");
        let b = derived_id(NS_CANDIDATE, "melody-1/functional");
        assert_eq!(a, b);
        assert!(is_uuid_shaped(&a));
    }

    #[test]
    fn namespaces_separate_identical_names() {
        assert_ne!(analysis_id_from("x"), candidate_id_from("x"));
        assert_ne!(plan_id_from("x"), transaction_id_from("x"));
        assert_ne!(snapshot_id_from("x"), fixture_id_from("x"));
    }

    #[test]
    fn namespace_list_is_unique() {
        let mut sorted = NAMESPACES.to_vec();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len());
        assert_eq!(NAMESPACES.len(), 7);
    }

    #[test]
    fn seeded_factories_repeat_their_sequence() {
        let a = IdFactory::from_seed(7);
        let b = IdFactory::from_seed(7);
        let first: Vec<String> = (0..4).map(|_| a.next()).collect();
        let second: Vec<String> = (0..4).map(|_| b.next()).collect();
        assert_eq!(first, second);
        assert!(first.iter().all(|id| is_uuid_shaped(id)));
    }

    #[test]
    fn different_seeds_diverge() {
        let a = IdFactory::from_seed(1);
        let b = IdFactory::from_seed(2);
        assert_ne!(a.next(), b.next());
    }

    #[test]
    fn prefixed_ids_keep_the_prefix() {
        let f = IdFactory::from_seed(3);
        let id = f.next_prefixed("cand");
        assert!(id.starts_with("cand-"));
        assert!(is_uuid_shaped(&id["cand-".len()..]));
    }

    #[test]
    fn process_unique_ids_differ() {
        let f = IdFactory::process_unique();
        assert_ne!(f.next(), f.next());
    }

    #[test]
    fn uuid_shape_check_rejects_garbage() {
        assert!(!is_uuid_shaped(""));
        assert!(!is_uuid_shaped("not-a-uuid"));
        assert!(!is_uuid_shaped("XXXXXXXX-1234-1234-1234-123456789012"));
        assert!(is_uuid_shaped(&uuid_from_name("ns", "name")));
    }
}
