#![allow(clippy::unwrap_used, unsafe_code)]
//! A `GIT_SEQUENCE_EDITOR` in the environment must not replace the prepared list.
mod common;

use common::remote::{configure, git};
use gitcore::{OpOutcome, Repo, TodoAction};

#[test]
fn the_prepared_list_wins_over_git_sequence_editor() {
    if !gitcore::git_available() {
        return;
    }
    // Only test in this binary: the variable cannot leak into other tests.
    unsafe { std::env::set_var("GIT_SEQUENCE_EDITOR", ":") };
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    configure(dir);
    for name in ["base", "one", "two"] {
        std::fs::write(dir.join(format!("{name}.txt")), name).unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", name]);
    }
    let r = Repo::open(dir).unwrap();
    let base = git(dir, &["rev-parse", "HEAD~2"]).trim().to_string();
    let mut items = r.rebase_list(&base).unwrap();
    items[1].action = TodoAction::Drop;
    assert_eq!(
        r.interactive_rebase(&base, &items).unwrap(),
        OpOutcome::Done
    );
    assert_eq!(git(dir, &["log", "--format=%s"]), "one\nbase\n");
}
