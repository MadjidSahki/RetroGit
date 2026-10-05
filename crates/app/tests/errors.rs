#![allow(clippy::unwrap_used)]
//! Every refusal from gitcore is worded by the app (strings.rs), not by gitcore.

use gitcore::{GitError, Refusal, TodoError, WholeAction, WholeKind};
use github::GithubError;
use retrogit::protocol::{AppError, Severity};
use retrogit::strings as s;

fn shown(r: Refusal) -> AppError {
    AppError::from_git(&GitError::Refused(r))
}

#[test]
fn each_refusal_is_a_warning_worded_by_strings() {
    let table: Vec<(Refusal, String)> = vec![
        (
            Refusal::LocalChangesFirst,
            s::ERR_LOCAL_CHANGES_FIRST.into(),
        ),
        (
            Refusal::ResolveConflictsFirst,
            s::ERR_RESOLVE_CONFLICTS_FIRST.into(),
        ),
        (
            Refusal::OperationInProgress,
            s::ERR_OPERATION_IN_PROGRESS.into(),
        ),
        (Refusal::FinishMergeByCommit, s::ERR_FINISH_MERGE.into()),
        (Refusal::NothingToAmend, s::ERR_NOTHING_TO_AMEND.into()),
        (Refusal::DetachedHead, s::ERR_DETACHED_HEAD.into()),
        (
            Refusal::NoUpstream { publish: true },
            s::ERR_NO_UPSTREAM_PUBLISH.into(),
        ),
        (
            Refusal::NoUpstream { publish: false },
            s::ERR_NO_UPSTREAM.into(),
        ),
        (
            Refusal::InvalidTagName("a b".into()),
            s::invalid_tag_name("a b"),
        ),
        (
            Refusal::InvalidIgnorePattern,
            s::ERR_INVALID_IGNORE_PATTERN.into(),
        ),
        (Refusal::MergesInRange, s::ERR_MERGES_IN_RANGE.into()),
        (Refusal::BareRepository, s::ERR_BARE_REPOSITORY.into()),
        (Refusal::UnknownRev("v9".into()), s::unknown_rev("v9")),
        (
            Refusal::NotInConflict("f.txt".into()),
            s::not_in_conflict("f.txt"),
        ),
        (Refusal::PullNotFetched(7), s::pull_not_fetched(7)),
        (Refusal::CommitNotFound, s::ERR_COMMIT_NOT_FOUND.into()),
        (
            Refusal::Todo(TodoError::KeepOne),
            s::todo_error(TodoError::KeepOne).into(),
        ),
    ];
    for (r, text) in table {
        let e = shown(r.clone());
        assert_eq!(e.message, text, "{r:?}");
        assert_eq!(e.severity, Severity::Warning, "{r:?}");
        assert!(e.message.ends_with('.'), "{r:?}: {}", e.message);
        assert!(
            e.message.chars().next().unwrap().is_uppercase() || e.message.starts_with('\''),
            "{r:?}: {}",
            e.message
        );
    }
}

#[test]
fn texts_moved_from_gitcore_read_as_before() {
    assert_eq!(
        s::ERR_LOCAL_CHANGES_FIRST,
        "Commit or stash your local changes first."
    );
    assert_eq!(
        s::ERR_NO_UPSTREAM_PUBLISH,
        "This branch has no upstream: publish it first."
    );
    assert_eq!(s::invalid_tag_name("a b"), "'a b' is not a valid tag name.");
    assert_eq!(s::unknown_rev("v9"), "Unknown version v9.");
    assert_eq!(s::pull_not_fetched(7), "Pull request #7 was not fetched.");
    assert_eq!(
        s::todo_error(TodoError::NoKeptAbove),
        "A squash or fixup needs a kept commit above it."
    );
    assert_eq!(
        s::todo_error(TodoError::KeepOne),
        "Keep at least one commit."
    );
}

#[test]
fn whole_file_refusals_name_the_kind_and_the_action() {
    let cases = [
        (
            WholeKind::Deleted,
            WholeAction::Stage,
            "Deleted files can only be staged as a whole.",
        ),
        (
            WholeKind::Deleted,
            WholeAction::Restore,
            "Deleted files can only be restored as a whole.",
        ),
        (
            WholeKind::Symlink,
            WholeAction::Discard,
            "Symbolic links can only be discarded as a whole.",
        ),
        (
            WholeKind::Untracked,
            WholeAction::Discard,
            "Untracked files can only be discarded as a whole.",
        ),
        (
            WholeKind::Filter,
            WholeAction::Stage,
            "Files with a Git filter (e.g. LFS) can only be staged as a whole.",
        ),
        (
            WholeKind::Binary,
            WholeAction::Discard,
            "Binary files can only be discarded as a whole.",
        ),
        (
            WholeKind::Renamed,
            WholeAction::Unstage,
            "Renamed files can only be unstaged as a whole.",
        ),
    ];
    for (kind, action, text) in cases {
        let e = shown(Refusal::WholeFileOnly { kind, action });
        assert_eq!(e.message, text);
        assert_eq!(e.severity, Severity::Warning);
    }
}

#[test]
fn a_technical_git_failure_shows_git_failed_with_the_output_as_detail() {
    let e = AppError::from_git(&GitError::Other("fatal: bad object".into()));
    assert_eq!(e.message, s::ERR_GIT_FAILED);
    assert_eq!(e.detail.as_deref(), Some("fatal: bad object"));
    assert_eq!(e.severity, Severity::Error);
}

#[test]
fn the_oauth_restriction_names_the_organization_or_says_this_repository() {
    let e = AppError::from_github(&GithubError::OAuthRestricted {
        org: Some("acme".into()),
    });
    assert_eq!(e.message, s::ERR_OAUTH_RESTRICTED.replace("{org}", "acme"));
    assert!(e.message.starts_with("The organization acme restricts"));
    assert!(e.message.ends_with(s::ERR_OAUTH_RESTRICTED_HELP));
    let e = AppError::from_github(&GithubError::OAuthRestricted { org: None });
    assert_eq!(
        e.message,
        s::ERR_OAUTH_RESTRICTED.replace("{org}", s::ORG_OF_THIS_REPO)
    );
    let old = e.for_old_gh();
    assert!(old.message.ends_with(s::ERR_OAUTH_RESTRICTED_GH_OLD));
    assert!(old.message.contains(s::ORG_OF_THIS_REPO));
}

#[test]
fn shared_sentences_keep_their_values() {
    assert_eq!(
        s::GH_TOO_OLD,
        "Update the GitHub CLI (2.40 or later), then run 'gh auth login'."
    );
    assert_eq!(
        s::ERR_OAUTH_RESTRICTED_HELP,
        "Install the GitHub CLI and run 'gh auth login': RetroGit then uses it for this organization. Or ask an owner to approve RetroGit (link below)."
    );
    assert_eq!(
        s::ERR_PULLS_NOT_FOUND,
        "GitHub does not show this repository to RetroGit. If its organization restricts third-party applications, install the GitHub CLI and run 'gh auth login' with the same account: RetroGit then uses it. Or ask an owner to approve RetroGit (link below)."
    );
    assert_eq!(
        s::ERR_OAUTH_RESTRICTED_GH_OLD,
        "Update the GitHub CLI (2.40 or later), then run 'gh auth login'. RetroGit then uses it for this organization. Or ask an owner to approve RetroGit (link below)."
    );
    assert_eq!(
        s::ERR_PULLS_NOT_FOUND_GH_OLD,
        "GitHub does not show this repository to RetroGit. Its organization may restrict third-party applications. Update the GitHub CLI (2.40 or later), then run 'gh auth login'. RetroGit then uses it with the same account. Or ask an owner to approve RetroGit (link below)."
    );
    assert_eq!(
        s::ERR_ACCESS_DENIED,
        "GitHub refused access to this repository with the RetroGit sign-in and with your own git credentials. If the organization restricts third-party OAuth apps, ask an owner to approve RetroGit (link below) or sign in with a personal access token (Advanced tab)."
    );
    assert_eq!(
        s::ERR_OAUTH_RESTRICTED,
        "The organization {org} restricts third-party applications and has not approved RetroGit. Install the GitHub CLI and run 'gh auth login': RetroGit then uses it for this organization. Or ask an owner to approve RetroGit (link below)."
    );
}

#[test]
fn a_conflicted_file_is_marked_with_the_conflict_mark() {
    assert_eq!(
        retrogit::ui::changes::describe("f.txt", &gitcore::Change::Conflicted),
        format!("[{}] f.txt", s::CONFLICT_MARK)
    );
    assert_eq!(s::CONFLICT_MARK, "!");
}
