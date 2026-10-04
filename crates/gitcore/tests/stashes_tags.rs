#![allow(clippy::unwrap_used)]
//! Stashes and tags (needs `git`).
mod common;

use common::remote::{Env, configure, git, no_cancel};
use gitcore::{NetAuth, Repo};

fn repo(dir: &std::path::Path) -> Repo {
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    configure(dir);
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "base"]);
    Repo::open(dir).unwrap()
}

#[test]
fn stashes_are_saved_listed_shown_applied_popped_and_dropped() {
    if !gitcore::git_available() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let r = repo(d.path());
    std::fs::write(d.path().join("a.txt"), "changed\n").unwrap();
    std::fs::write(d.path().join("new.txt"), "new\n").unwrap();
    assert!(r.stash_save("first", false).unwrap());
    assert!(
        d.path().join("new.txt").exists(),
        "untracked kept without the option"
    );
    std::fs::write(d.path().join("a.txt"), "again\n").unwrap();
    assert!(r.stash_save("second", true).unwrap());
    assert!(!d.path().join("new.txt").exists(), "untracked stashed too");
    let list = r.stashes().unwrap();
    let names: Vec<(usize, &str, &str)> = list
        .iter()
        .map(|s| (s.index, s.message.as_str(), s.branch.as_str()))
        .collect();
    assert_eq!(names, [(0, "second", "main"), (1, "first", "main")]);
    let files = r.stash_files(1).unwrap();
    assert_eq!(
        files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        ["a.txt"]
    );
    let diff = r.stash_file_diff(1, "a.txt").unwrap();
    assert!(diff.hunks[0].lines.iter().any(|l| l.text == "changed\n"));
    r.stash_apply(1).unwrap();
    assert_eq!(
        std::fs::read_to_string(d.path().join("a.txt")).unwrap(),
        "changed\n"
    );
    assert_eq!(r.stashes().unwrap().len(), 2, "apply keeps it");
    git(d.path(), &["checkout", "--", "a.txt"]);
    r.stash_pop_at(0).unwrap();
    assert!(d.path().join("new.txt").exists());
    assert_eq!(r.stashes().unwrap().len(), 1, "pop removes it");
    let hash = r.stash_drop(0).unwrap();
    assert_eq!(
        hash.len(),
        40,
        "the dropped stash can still be found by its id"
    );
    assert!(r.stashes().unwrap().is_empty());
    git(d.path(), &["checkout", "--", "."]);
    git(d.path(), &["clean", "-fdq"]);
    assert!(!r.stash_save("nothing", true).unwrap(), "nothing to stash");
}

#[test]
fn tags_are_created_listed_deleted_and_pushed() {
    let Some(env) = Env::new() else { return };
    let r = env.repo();
    let head = git(&env.work, &["rev-parse", "HEAD"]).trim().to_string();
    let first = git(&env.work, &["rev-parse", "HEAD~1"]).trim().to_string();
    r.create_tag("v1.0", &head, Some("Release 1.0")).unwrap();
    r.create_tag("light", &first, None).unwrap();
    assert!(r.create_tag("v1.0", &head, None).is_err(), "exists");
    assert!(r.create_tag("bad name", &head, None).is_err(), "invalid");
    let tags = r.tags().unwrap();
    let rows: Vec<(&str, &str, bool, &str)> = tags
        .iter()
        .map(|t| {
            (
                t.name.as_str(),
                t.commit.as_str(),
                t.annotated,
                t.message.as_str(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("light", first.as_str(), false, ""),
            ("v1.0", head.as_str(), true, "Release 1.0")
        ]
    );
    let auth = NetAuth::default();
    r.push_tag(&auth, "v1.0", |_| {}, &no_cancel()).unwrap();
    let remote = git(&env.root.join("origin.git"), &["tag"]);
    assert_eq!(remote.trim(), "v1.0");
    r.push_tags(&auth, |_| {}, &no_cancel()).unwrap();
    assert_eq!(
        git(&env.root.join("origin.git"), &["tag"]).lines().count(),
        2
    );
    r.delete_remote_tag(&auth, "light", |_| {}, &no_cancel())
        .unwrap();
    r.delete_tag("light").unwrap();
    assert_eq!(git(&env.root.join("origin.git"), &["tag"]).trim(), "v1.0");
    assert_eq!(r.tags().unwrap().len(), 1);
}

/// A server that refuses to delete a ref it does not have, with git's own message.
fn refuse_deleting_missing_refs(bare: &std::path::Path) {
    let hooks = bare.join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-receive");
    std::fs::write(
        &hook,
        "#!/bin/sh\nwhile read old new ref; do\n  case \"$old$new\" in\n    *[!0]*) ;;\n    *) echo \"error: unable to delete '$ref': remote ref does not exist\" >&2; exit 1 ;;\n  esac\ndone\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    git(bare, &["config", "core.hooksPath", hooks.to_str().unwrap()]);
}

#[test]
fn deleting_on_origin_a_tag_never_pushed_succeeds() {
    let Some(env) = Env::new() else { return };
    let r = env.repo();
    let head = git(&env.work, &["rev-parse", "HEAD"]).trim().to_string();
    r.create_tag("local-only", &head, None).unwrap();
    let auth = NetAuth::default();
    r.delete_remote_tag(&auth, "local-only", |_| {}, &no_cancel())
        .unwrap();
    refuse_deleting_missing_refs(&env.root.join("origin.git"));
    let result = r.delete_remote_tag(&auth, "local-only", |_| {}, &no_cancel());
    assert!(result.is_ok(), "{result:?}");
    git(
        &env.work,
        &["remote", "set-url", "origin", "/nowhere/origin.git"],
    );
    assert!(
        r.delete_remote_tag(&auth, "local-only", |_| {}, &no_cancel())
            .is_err(),
        "a real failure is still reported"
    );
}
