#![allow(clippy::unwrap_used)]
//! The Repositories side panel as a tree: repositories, then the open one's branches in
//! virtual folders.

use std::path::{Path, PathBuf};

use gitcore::Branch;
use retrogit::config::RecentRepo;
use retrogit::state::repo_tree::{RepoTree, Row, RowKind, sidebar_rows};

fn recent(name: &str) -> RecentRepo {
    RecentRepo {
        name: name.into(),
        path: PathBuf::from(format!("/r/{name}")),
    }
}

fn branch(name: &str, remote: bool, head: bool) -> Branch {
    Branch {
        name: name.into(),
        remote,
        is_head: head,
        upstream: None,
        ahead: 0,
        behind: 0,
    }
}

/// `depth label` per row, folders marked `+`/`-` (closed/open), the current branch `*`.
fn outline(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|r| {
            let mark = match &r.kind {
                RowKind::Repo { expanded, .. } => {
                    if *expanded {
                        "v "
                    } else {
                        "> "
                    }
                }
                RowKind::Folder { open, .. } => {
                    if *open {
                        "- "
                    } else {
                        "+ "
                    }
                }
                RowKind::Branch { current: true, .. } => "* ",
                RowKind::Branch { .. } => "",
            };
            format!("{}{mark}{}", "  ".repeat(r.depth), r.label)
        })
        .collect()
}

fn branches() -> Vec<Branch> {
    vec![
        branch("main", false, true),
        branch("feat/login", false, false),
        branch("Feat/api", false, false),
        branch("fix/deep/one", false, false),
        branch("origin/main", true, false),
        branch("origin/feat/remote-only", true, false),
        branch("origin/feat/login", true, false),
    ]
}

#[test]
fn a_closed_tree_lists_only_the_repositories() {
    let recents = [recent("alpha"), recent("beta")];
    let tree = RepoTree::default();
    let rows = sidebar_rows(&recents, Some(Path::new("/r/alpha")), &tree, &branches());
    assert_eq!(outline(&rows), ["> alpha", "> beta"]);
}

#[test]
fn the_open_repository_shows_its_branches_in_folders_first() {
    let recents = [recent("alpha"), recent("beta")];
    let tree = RepoTree {
        expanded: true,
        ..Default::default()
    };
    let rows = sidebar_rows(&recents, Some(Path::new("/r/alpha")), &tree, &branches());
    assert_eq!(
        outline(&rows),
        [
            "v alpha",
            "  + Feat",
            "  + feat",
            "  + fix",
            "  * main",
            "  + origin",
            "> beta",
        ]
    );
}

#[test]
fn open_folders_show_their_branches_and_remotes_skip_local_twins() {
    let recents = [recent("alpha")];
    let mut tree = RepoTree {
        expanded: true,
        ..Default::default()
    };
    for key in [
        "feat",
        "fix",
        "fix/deep",
        "remote:origin",
        "remote:origin/feat",
    ] {
        tree.open.insert(key.into());
    }
    let rows = sidebar_rows(&recents, Some(Path::new("/r/alpha")), &tree, &branches());
    assert_eq!(
        outline(&rows),
        [
            "v alpha",
            "  + Feat",
            "  - feat",
            "    login",
            "  - fix",
            "    - deep",
            "      one",
            "  * main",
            "  - origin",
            "    - feat",
            "      remote-only",
        ]
    );
    let login = rows.iter().find(|r| r.label == "login").unwrap();
    assert_eq!(
        login.kind,
        RowKind::Branch {
            name: "feat/login".into(),
            current: false,
            remote: false,
        }
    );
    let remote = rows.iter().find(|r| r.label == "remote-only").unwrap();
    assert!(
        matches!(&remote.kind, RowKind::Branch { name, remote: true, .. } if name == "origin/feat/remote-only")
    );
}

#[test]
fn only_the_open_repository_can_be_expanded() {
    let recents = [recent("alpha"), recent("beta")];
    let tree = RepoTree {
        expanded: true,
        ..Default::default()
    };
    let rows = sidebar_rows(&recents, None, &tree, &branches());
    assert_eq!(outline(&rows), ["> alpha", "> beta"]);
}

#[test]
fn revealing_the_current_branch_opens_its_folders() {
    let mut tree = RepoTree::default();
    let b = vec![
        branch("feat/deep/mine", false, true),
        branch("main", false, false),
    ];
    tree.reveal_current(&b);
    assert!(tree.open.contains("feat") && tree.open.contains("feat/deep"));
    assert_eq!(tree.open.len(), 2);
}

mod state {
    use super::*;
    use gitcore::{Head, RepoSummary};
    use retrogit::config::Config;
    use retrogit::protocol::Event;
    use retrogit::state::AppState;

    fn opened(st: &mut AppState, name: &str) {
        st.apply(Event::RepoOpened(RepoSummary {
            name: name.into(),
            path: PathBuf::from(format!("/r/{name}")),
            head: Head::Branch("feat/mine".into()),
            origin_url: None,
            last_commit: None,
        }));
    }

    fn mine() -> Vec<Branch> {
        vec![
            branch("feat/mine", false, true),
            branch("main", false, false),
        ]
    }

    #[test]
    fn expanding_the_open_repository_reveals_the_current_branch() {
        let mut st = AppState::new(Config::default());
        opened(&mut st, "alpha");
        st.apply(Event::BranchesLoaded(mine()));
        assert!(st.toggle_repo_branches(Path::new("/r/alpha")));
        assert!(st.repo_tree.expanded);
        assert!(st.repo_tree.open.contains("feat"));
        assert!(st.toggle_repo_branches(Path::new("/r/alpha")));
        assert!(!st.repo_tree.expanded);
    }

    #[test]
    fn the_arrow_of_another_repository_opens_it_then_expands_it() {
        let mut st = AppState::new(Config::default());
        opened(&mut st, "alpha");
        st.repo_tree.expanded = true;
        st.repo_tree.open.insert("feat".into());
        // Not open yet: the caller must open it.
        assert!(!st.toggle_repo_branches(Path::new("/r/beta")));
        opened(&mut st, "beta");
        assert!(st.repo_tree.expanded, "expanded once open");
        assert!(
            st.repo_tree.open.is_empty(),
            "alpha's folders are forgotten"
        );
        st.apply(Event::BranchesLoaded(mine()));
        assert!(
            st.repo_tree.open.contains("feat"),
            "revealed when the branches arrive"
        );
    }

    #[test]
    fn opening_another_repository_by_its_name_folds_the_tree() {
        let mut st = AppState::new(Config::default());
        opened(&mut st, "alpha");
        st.repo_tree.expanded = true;
        opened(&mut st, "beta");
        assert!(!st.repo_tree.expanded);
    }

    #[test]
    fn a_new_current_branch_in_a_closed_folder_is_revealed() {
        let mut st = AppState::new(Config::default());
        opened(&mut st, "alpha");
        st.apply(Event::BranchesLoaded(mine()));
        st.toggle_repo_branches(Path::new("/r/alpha"));
        st.repo_tree.open.clear();
        st.apply(Event::BranchesLoaded(mine()));
        assert!(
            st.repo_tree.open.is_empty(),
            "same current branch: the user's folders stay"
        );
        st.apply(Event::BranchesLoaded(vec![
            branch("feat/mine", false, false),
            branch("fix/deep/new", false, true),
        ]));
        assert!(st.repo_tree.open.contains("fix") && st.repo_tree.open.contains("fix/deep"));
    }

    #[test]
    fn a_cancelled_or_failed_open_forgets_to_unfold() {
        use retrogit::protocol::{AppError, Op};
        let mut st = AppState::new(Config::default());
        opened(&mut st, "alpha");
        st.toggle_repo_branches(Path::new("/r/beta"));
        st.cancel_repo_switch();
        assert_eq!(st.repo_tree.expand_after_open, None);

        st.toggle_repo_branches(Path::new("/r/beta"));
        st.apply(Event::Error {
            during: Op::Open(PathBuf::from("/r/beta")),
            error: AppError::from_git(&gitcore::GitError::Other("gone".into())),
        });
        assert_eq!(st.repo_tree.expand_after_open, None);
    }
}
