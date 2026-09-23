//! Filters and the default selection policy, shared by every frontend.

use crate::ecosystem::EcoId;
use crate::model::{Artifact, ArtifactKind, Safety};
use std::time::{Duration, SystemTime};

/// Narrows which artifacts are considered at all.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Minimum reclaimable bytes; unmeasured artifacts never pass.
    pub min_size: Option<u64>,
    /// Only artifacts whose newest content is at least this old.
    pub older_than: Option<Duration>,
    /// Empty means every kind.
    pub kinds: Vec<ArtifactKind>,
    /// Empty means every ecosystem.
    pub ecosystems: Vec<EcoId>,
}

impl Filter {
    pub fn matches(&self, artifact: &Artifact, now: SystemTime) -> bool {
        if let Some(min) = self.min_size
            && artifact.size.is_none_or(|s| s.reclaimable < min)
        {
            return false;
        }
        if let Some(older_than) = self.older_than
            && age(artifact, now).is_some_and(|age| age < older_than)
        {
            return false;
        }
        if !self.kinds.is_empty() && !self.kinds.contains(&artifact.kind) {
            return false;
        }
        if !self.ecosystems.is_empty()
            && !artifact
                .ecosystem
                .is_some_and(|eco| self.ecosystems.contains(&eco))
        {
            return false;
        }
        true
    }
}

/// Time since the artifact's newest known modification.
pub fn age(artifact: &Artifact, now: SystemTime) -> Option<Duration> {
    artifact
        .last_modified()
        .map(|t| now.duration_since(t).unwrap_or_default())
}

/// Why the default policy leaves an artifact unselected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// Never deletable.
    Protected,
    /// Needs a human look first.
    Review,
    /// Modified recently; probably in active use.
    Recent,
}

impl Hold {
    pub fn describe(self) -> &'static str {
        match self {
            Hold::Protected => "protected",
            Hold::Review => "needs review",
            Hold::Recent => "recently used",
        }
    }
}

/// Which artifacts are selected by default.
#[derive(Debug, Clone)]
pub struct Policy {
    pub include_review: bool,
    /// Artifacts modified within this window are held back; `None` disables the check.
    pub recent: Option<Duration>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            include_review: false,
            recent: Some(Duration::from_secs(7 * 86_400)),
        }
    }
}

impl Policy {
    pub fn hold(&self, artifact: &Artifact, now: SystemTime) -> Option<Hold> {
        match artifact.safety() {
            Safety::Protected(_) => return Some(Hold::Protected),
            Safety::Review(_) if !self.include_review => return Some(Hold::Review),
            _ => {}
        }
        if let Some(recent) = self.recent
            && age(artifact, now).is_some_and(|age| age < recent)
        {
            return Some(Hold::Recent);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecosystem::RuleId;
    use crate::model::{ArtifactId, Detection, GitState, Size};
    use std::path::PathBuf;

    fn artifact(reclaimable: u64, age_days: u64, ambiguous: bool) -> (Artifact, SystemTime) {
        let now = SystemTime::now();
        let modified = now - Duration::from_secs(age_days * 86_400);
        let artifact = Artifact {
            id: ArtifactId(1),
            path: PathBuf::from("/p/target"),
            project: None,
            ecosystem: Some(EcoId(0)),
            kind: ArtifactKind::Build,
            detection: Detection::Rule {
                rule: RuleId(0),
                confirmed: false,
            },
            ambiguous,
            modified: Some(modified),
            git: GitState::NotInRepo,
            repo: None,
            size: Some(Size {
                reclaimable,
                on_disk: reclaimable,
                ..Size::default()
            }),
            outside_root: false,
        };
        (artifact, now)
    }

    #[test]
    fn filter_by_size_age_kind_and_ecosystem() {
        let (a, now) = artifact(500, 40, false);
        assert!(Filter::default().matches(&a, now));
        let big = Filter {
            min_size: Some(1_000),
            ..Filter::default()
        };
        assert!(!big.matches(&a, now));
        let old = Filter {
            older_than: Some(Duration::from_secs(30 * 86_400)),
            ..Filter::default()
        };
        assert!(old.matches(&a, now));
        let very_old = Filter {
            older_than: Some(Duration::from_secs(60 * 86_400)),
            ..Filter::default()
        };
        assert!(!very_old.matches(&a, now));
        let deps = Filter {
            kinds: vec![ArtifactKind::Dependencies],
            ..Filter::default()
        };
        assert!(!deps.matches(&a, now));
        let other_eco = Filter {
            ecosystems: vec![EcoId(9)],
            ..Filter::default()
        };
        assert!(!other_eco.matches(&a, now));
    }

    #[test]
    fn policy_holds_review_and_recent() {
        let policy = Policy::default();
        let (old_safe, now) = artifact(1, 30, false);
        assert_eq!(policy.hold(&old_safe, now), None);
        let (recent, now) = artifact(1, 2, false);
        assert_eq!(policy.hold(&recent, now), Some(Hold::Recent));
        let (generic, now) = artifact(1, 30, true);
        assert_eq!(policy.hold(&generic, now), Some(Hold::Review));
        let lenient = Policy {
            include_review: true,
            recent: None,
        };
        assert_eq!(lenient.hold(&generic, now), None);
        assert_eq!(lenient.hold(&recent, now), None);
    }
}
