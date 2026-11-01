//! Orphan detection: journal rows whose session no longer exists.
//!
//! A decree, inscription, or verdict is keyed by the `session_at` of the
//! block it describes. If that block has no live session in the store — a
//! peer device never synced it, or the record was pruned — the journal row
//! is an orphan: it speaks about work that is not in the archive. Orphans
//! are silent by design (they do not fail the build), but they inflate a
//! "spoken" count against a "delivered" count in Reckoning forever, so the
//! app walks the record after sync and logs them.
//!
//! Pure: takes slices, not a `Store` or a `Data`, and never touches the
//! clock or the filesystem. Everything here is unit-testable.

use crate::journal::{Decree, Inscription, Verdict};
use crate::state::store::SessionRecord;

/// Journal rows whose `session_at` has no matching live session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Orphans {
    /// `session_at` of decrees with no matching session, ascending.
    pub decrees: Vec<i64>,
    /// `session_at` of inscriptions with no matching session, ascending.
    pub inscriptions: Vec<i64>,
    /// `session_at` of verdicts with no matching session, ascending.
    pub verdicts: Vec<i64>,
}

/// Find every live journal row whose `session_at` names no session in
/// `sessions`. Tombstoned rows (deleted) are skipped: a withdrawn promise is
/// not an orphan, it is a deliberate call-off.
pub fn find_orphans(
    sessions: &[SessionRecord],
    decrees: &[Decree],
    inscriptions: &[Inscription],
    verdicts: &[Verdict],
) -> Orphans {
    let live: std::collections::BTreeSet<i64> = sessions.iter().map(|s| s.at).collect();

    let mut out = Orphans::default();
    for d in decrees.iter().filter(|d| !d.deleted) {
        if !live.contains(&d.session_at) {
            out.decrees.push(d.session_at);
        }
    }
    for i in inscriptions.iter().filter(|i| !i.deleted) {
        if !live.contains(&i.session_at) {
            out.inscriptions.push(i.session_at);
        }
    }
    for v in verdicts.iter().filter(|v| !v.deleted) {
        if !live.contains(&v.session_at) {
            out.verdicts.push(v.session_at);
        }
    }
    out.decrees.sort_unstable();
    out.inscriptions.sort_unstable();
    out.verdicts.sort_unstable();
    out
}

/// True when the record holds at least one orphan.
pub fn has_orphans(orphans: &Orphans) -> bool {
    !orphans.decrees.is_empty() || !orphans.inscriptions.is_empty() || !orphans.verdicts.is_empty()
}

/// Total number of orphaned rows across all three collections.
pub fn total_count(orphans: &Orphans) -> usize {
    orphans.decrees.len() + orphans.inscriptions.len() + orphans.verdicts.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::decree;
    use crate::journal::inscription;
    use crate::journal::verdict;

    const T: i64 = 1_793_431_800; // 2026-10-31 09:00 +0330

    fn sess(at: i64) -> SessionRecord {
        SessionRecord { at, minutes: 25 }
    }

    fn decree_at(session_at: i64) -> Decree {
        decree::create(session_at, session_at - 1500, "a promise").unwrap()
    }

    fn inscription_at(session_at: i64) -> Inscription {
        inscription::create(session_at, session_at, "the outcome").unwrap()
    }

    #[test]
    fn find_orphans_empty_when_all_paired() {
        let s = vec![sess(T)];
        let d = vec![decree_at(T)];
        let i = vec![inscription_at(T)];
        let v = vec![verdict::declare(T, verdict::VerdictKind::Kept, T).unwrap()];
        let o = find_orphans(&s, &d, &i, &v);
        assert_eq!(o, Orphans::default());
        assert!(!has_orphans(&o));
        assert_eq!(total_count(&o), 0);
    }

    #[test]
    fn find_orphans_detects_decree_after_session_missing() {
        let s = vec![sess(T)];
        let d = vec![decree_at(T + 1000)];
        let o = find_orphans(&s, &d, &[], &[]);
        assert_eq!(o.decrees, vec![T + 1000]);
        assert!(has_orphans(&o));
        assert_eq!(total_count(&o), 1);
    }

    #[test]
    fn find_orphans_detects_inscription_after_session_missing() {
        let s = vec![sess(T)];
        let i = vec![inscription_at(T + 1000)];
        let o = find_orphans(&s, &[], &i, &[]);
        assert_eq!(o.inscriptions, vec![T + 1000]);
        assert_eq!(total_count(&o), 1);
    }

    #[test]
    fn find_orphans_detects_verdict_after_session_missing() {
        let s = vec![sess(T)];
        let v = vec![verdict::declare(T + 1000, verdict::VerdictKind::Kept, T).unwrap()];
        let o = find_orphans(&s, &[], &[], &v);
        assert_eq!(o.verdicts, vec![T + 1000]);
        assert_eq!(total_count(&o), 1);
    }

    #[test]
    fn find_orphans_ignores_tombstoned_decree() {
        let mut d = decree_at(T + 1000);
        d.deleted = true;
        let o = find_orphans(&[], &[d], &[], &[]);
        assert!(o.decrees.is_empty(), "a discarded decree is not an orphan");
        assert!(!has_orphans(&o));
    }

    #[test]
    fn find_orphans_sorted_ascending() {
        let o = find_orphans(
            &[],
            &[decree_at(T + 3000), decree_at(T), decree_at(T + 1000)],
            &[],
            &[],
        );
        assert_eq!(o.decrees, vec![T, T + 1000, T + 3000]);
    }

    #[test]
    fn has_orphans_false_when_empty() {
        assert!(!has_orphans(&Orphans::default()));
        assert_eq!(total_count(&Orphans::default()), 0);
    }
}
