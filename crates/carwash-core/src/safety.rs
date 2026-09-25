//! Deletion safety policy.
//!
//! Evidence, strongest first:
//! 1. Git-tracked content is never deleted.
//! 2. A generic name (`build`, `dist`, `vendor`...) needs either confirming content or git
//!    reporting the directory as ignored; otherwise the user must review it.
//! 3. An ambiguous directory that git sees as untracked (not ignored) may be work in progress.

use crate::model::{Artifact, GitState, ProtectReason, ReviewReason, Safety};

pub fn assess(artifact: &Artifact) -> Safety {
    if let GitState::Tracked(files) = artifact.git
        && files > 0
    {
        return Safety::Protected(ProtectReason::TrackedFiles);
    }
    if !artifact.ambiguous {
        return Safety::Safe;
    }
    match artifact.git {
        GitState::Ignored => Safety::Safe,
        GitState::Untracked => Safety::Review(ReviewReason::UntrackedInGit),
        _ if artifact.detection.is_certain() => Safety::Safe,
        _ => Safety::Review(ReviewReason::GenericName),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecosystem::RuleId;
    use crate::model::{ArtifactId, ArtifactKind, Detection};
    use std::path::PathBuf;

    fn artifact(ambiguous: bool, confirmed: bool, git: GitState) -> Artifact {
        Artifact {
            id: ArtifactId(0),
            path: PathBuf::from("/p/build"),
            project: None,
            ecosystem: None,
            kind: ArtifactKind::Build,
            detection: Detection::Rule {
                rule: RuleId(0),
                confirmed,
            },
            ambiguous,
            modified: None,
            git,
            repo: None,
            size: None,
            outside_root: false,
        }
    }

    #[test]
    fn tracked_content_is_always_protected() {
        for (ambiguous, confirmed) in [(false, false), (true, true), (true, false)] {
            let a = artifact(ambiguous, confirmed, GitState::Tracked(3));
            assert_eq!(assess(&a), Safety::Protected(ProtectReason::TrackedFiles));
        }
    }

    #[test]
    fn unique_names_are_safe_without_git() {
        assert_eq!(
            assess(&artifact(false, false, GitState::NotInRepo)),
            Safety::Safe
        );
    }

    #[test]
    fn generic_names_need_confirmation_or_git() {
        assert_eq!(
            assess(&artifact(true, false, GitState::NotInRepo)),
            Safety::Review(ReviewReason::GenericName)
        );
        assert_eq!(
            assess(&artifact(true, true, GitState::NotInRepo)),
            Safety::Safe
        );
        assert_eq!(
            assess(&artifact(true, false, GitState::Ignored)),
            Safety::Safe
        );
        assert_eq!(
            assess(&artifact(true, true, GitState::Untracked)),
            Safety::Review(ReviewReason::UntrackedInGit)
        );
    }
}
