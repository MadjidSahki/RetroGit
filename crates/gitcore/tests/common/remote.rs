#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
//! A bare "origin" remote and clones of it, driven with the git command line.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use gitcore::{CommitBackend, Repo, Selection, git_available};

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn configure(dir: &Path) {
    for (k, v) in [
        ("user.name", "Ada"),
        ("user.email", "ada@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".git/hooks"),
        ("core.autocrlf", "false"),
        ("pull.rebase", "false"),
    ] {
        git(dir, &["config", k, v]);
    }
}

/// `origin.git` (bare) with 2 commits on main, and a clone `work` tracking it.
pub struct Env {
    pub _tmp: tempfile::TempDir,
    pub root: PathBuf,
    pub work: PathBuf,
}

impl Env {
    pub fn new() -> Option<Env> {
        if !git_available() {
            return None;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let seed = root.join("seed");
        super::make_repo(&seed, 2);
        git(
            &root,
            &[
                "clone",
                "-q",
                "--bare",
                seed.to_str().unwrap(),
                "origin.git",
            ],
        );
        let work = root.join("work");
        git(&root, &["clone", "-q", "origin.git", "work"]);
        configure(&work);
        Some(Env {
            _tmp: tmp,
            root,
            work,
        })
    }

    pub fn repo(&self) -> Repo {
        Repo::open(&self.work).unwrap()
    }

    /// Another clone that pushes `file` to origin/main.
    pub fn remote_commit(&self, file: &str, content: &str) {
        let other = self.root.join(format!("other-{file}"));
        git(
            &self.root,
            &["clone", "-q", "origin.git", other.to_str().unwrap()],
        );
        configure(&other);
        std::fs::write(other.join(file), content).unwrap();
        git(&other, &["add", file]);
        git(&other, &["commit", "-q", "-m", &format!("remote {file}")]);
        git(&other, &["push", "-q"]);
    }

    pub fn local_commit(&self, file: &str, content: &str) {
        std::fs::write(self.work.join(file), content).unwrap();
        let r = self.repo();
        r.stage(file, &Selection::All, None).unwrap();
        r.commit(&format!("local {file}"), false, CommitBackend::PreferCli)
            .unwrap();
    }
}

pub fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}
