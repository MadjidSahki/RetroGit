#![allow(clippy::unwrap_used)]
//! History and commit details (needs `git`).
mod common;

use common::remote::{Env, git};
use gitcore::{RefKind, SignatureStatus};

#[test]
fn log_lists_all_branches_with_ref_labels() {
    let Some(env) = Env::new() else { return };
    git(&env.work, &["switch", "-q", "-c", "feature"]);
    env.local_commit("f.txt", "f\n");
    git(&env.work, &["switch", "-q", "main"]);
    let log = env.repo().log(0, 100).unwrap();
    assert_eq!(log.len(), 3);
    assert_eq!(log[0].summary, "local f.txt");
    assert!(
        log[0]
            .refs
            .iter()
            .any(|r| r.name == "feature" && r.kind == RefKind::LocalBranch)
    );
    let main = log
        .iter()
        .find(|e| e.refs.iter().any(|r| r.name == "main"))
        .unwrap();
    assert!(main.refs.iter().any(|r| r.kind == RefKind::Head));
    assert!(
        main.refs
            .iter()
            .any(|r| r.name == "origin/main" && r.kind == RefKind::RemoteBranch)
    );
    assert_eq!(env.repo().log(1, 100).unwrap().len(), 2);
    assert_eq!(env.repo().log(0, 1).unwrap().len(), 1);
}

#[test]
fn commit_detail_files_diff_and_signature() {
    let Some(env) = Env::new() else { return };
    env.local_commit("new.txt", "hello\n");
    let r = env.repo();
    let head = r.log(0, 1).unwrap().remove(0);
    let d = r.commit_detail(&head.id).unwrap();
    assert_eq!(d.message, "local new.txt");
    assert_eq!(d.files.len(), 1);
    assert_eq!(d.files[0].path, "new.txt");
    assert_eq!(d.files[0].change, gitcore::Change::Added);
    let diff = r.commit_file_diff(&head.id, "new.txt").unwrap();
    assert_eq!(diff.hunks[0].lines[0].text, "hello\n");
    assert_eq!(
        r.signature_status(&head.id).unwrap(),
        SignatureStatus::Unsigned
    );
    // Root commit: everything added.
    let root = r.log(0, 100).unwrap().pop().unwrap();
    assert!(
        r.commit_detail(&root.id)
            .unwrap()
            .files
            .iter()
            .all(|f| f.change == gitcore::Change::Added)
    );
}
