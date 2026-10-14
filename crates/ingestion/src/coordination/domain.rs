//! The [`Coordinator`] port, and the rule that picks an owner for a source among live instances.

use std::collections::{BTreeSet, HashSet};

use async_trait::async_trait;

use crate::datasource::DataSourceId;

/// Identity of one running `pipa-ingestion` process among its peers.
pub type InstanceId = String;

#[derive(Debug, thiserror::Error)]
pub enum CoordinationError {
    /// The coordination service couldn't be reached or answered in time. Transient: callers
    /// keep what they own and ask again at the next round.
    #[error("coordination service unavailable: {0}")]
    Unavailable(String),
}

/// Decides which of the registered sources this instance captures.
#[async_trait]
pub trait Coordinator: Send + Sync {
    /// One coordination round: shows this instance is alive, and returns the subset of
    /// `registered` it should be capturing now. Called every `Coordination::interval`.
    ///
    /// A source the instance stops owning drops out of the result; it must stop capturing it
    /// and then [`release`](Coordinator::release) it. On an error the caller keeps the
    /// previous result — a coordination outage never stops a running capture.
    async fn assign(
        &self,
        registered: &[DataSourceId],
    ) -> Result<HashSet<DataSourceId>, CoordinationError>;

    /// Gives up `sources` this instance has stopped capturing, so their new owner can take
    /// over at once instead of waiting for the claim to lapse. Best effort.
    async fn release(&self, sources: &[DataSourceId]);

    /// Shutdown: releases `held` and leaves the group, so peers re-assign right away.
    /// Best effort.
    async fn leave(&self, held: &[DataSourceId]);
}

/// The member that owns `source`, by rendezvous (highest-random-weight) hashing: each member
/// scores the source and the highest score wins. Unlike `id % member_count`, a member joining
/// or leaving moves only the sources it wins or held (about `1 / members` of them), and every
/// instance reaches the same answer from the same member set, with no coordination.
///
/// The hash is spelled out here rather than taken from `std`'s hashers, whose output isn't
/// guaranteed across Rust versions — instances of different builds must agree during a
/// rolling upgrade.
pub fn owner_of(members: &BTreeSet<InstanceId>, source: DataSourceId) -> Option<&InstanceId> {
    members
        .iter()
        .max_by_key(|member| (score(member, source), *member))
}

/// FNV-1a over the member id and the source's bytes, then a `splitmix64` finalizer (FNV alone
/// leaves the high bits too correlated between similar ids for the comparison to be fair).
fn score(member: &str, source: DataSourceId) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in member.bytes().chain(source.0.as_bytes().iter().copied()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^= hash >> 27;
    hash = hash.wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^ (hash >> 31)
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn members(ids: &[&str]) -> BTreeSet<InstanceId> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    fn sources(count: u128) -> Vec<DataSourceId> {
        (0..count)
            .map(|n| {
                DataSourceId(Uuid::from_u128(
                    n.wrapping_mul(0x9e37_79b9_7f4a_7c15_f39c_c060_5ced_c835),
                ))
            })
            .collect()
    }

    #[test]
    fn no_members_own_nothing() {
        assert_eq!(owner_of(&BTreeSet::new(), sources(1)[0]), None);
    }

    #[test]
    fn a_sole_member_owns_everything() {
        let members = members(&["a"]);
        assert!(
            sources(50)
                .into_iter()
                .all(|id| owner_of(&members, id).is_some_and(|m| m == "a"))
        );
    }

    #[test]
    fn the_owner_does_not_depend_on_who_asks_or_in_what_order() {
        let forward = members(&["a", "b", "c"]);
        let reversed: BTreeSet<_> = ["c", "b", "a"].iter().map(|id| (*id).to_owned()).collect();
        for id in sources(100) {
            assert_eq!(owner_of(&forward, id), owner_of(&reversed, id));
        }
    }

    #[test]
    fn sources_spread_across_members() {
        let members = members(&["a", "b", "c", "d"]);
        let mut counts = std::collections::HashMap::new();
        for id in sources(4000) {
            *counts
                .entry(owner_of(&members, id).unwrap().clone())
                .or_insert(0u32) += 1;
        }
        assert_eq!(counts.len(), 4);
        // 1000 each on average; leave room for the spread of a hash.
        assert!(
            counts.values().all(|n| (800..1200).contains(n)),
            "{counts:?}"
        );
    }

    #[test]
    fn a_member_leaving_moves_only_its_own_sources() {
        let before = members(&["a", "b", "c"]);
        let after = members(&["a", "c"]);
        for id in sources(500) {
            let was = owner_of(&before, id).unwrap();
            let now = owner_of(&after, id).unwrap();
            if was != "b" {
                assert_eq!(was, now, "a source of a member that stayed moved");
            }
        }
    }

    #[test]
    fn a_member_joining_only_takes_sources() {
        let before = members(&["a", "b"]);
        let after = members(&["a", "b", "c"]);
        for id in sources(500) {
            let was = owner_of(&before, id).unwrap();
            let now = owner_of(&after, id).unwrap();
            assert!(
                now == "c" || now == was,
                "a source moved between old members"
            );
        }
    }
}
