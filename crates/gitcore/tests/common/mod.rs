#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

/// Create a non-bare repo with `commits` commits on branch `main`.
pub fn make_repo(dir: &Path, commits: usize) -> git2::Repository {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    let repo = git2::Repository::init_opts(dir, &opts).unwrap();
    {
        let sig =
            git2::Signature::new("Ada", "ada@example.com", &git2::Time::new(1_700_000_000, 0))
                .unwrap();
        for i in 0..commits {
            let file = format!("file{i}.txt");
            std::fs::write(dir.join(&file), format!("content {i}\n").repeat(50)).unwrap();
            let mut idx = repo.index().unwrap();
            idx.add_path(Path::new(&file)).unwrap();
            idx.write().unwrap();
            let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
            let parents: Vec<git2::Commit<'_>> = repo
                .head()
                .ok()
                .and_then(|h| h.peel_to_commit().ok())
                .into_iter()
                .collect();
            let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
            repo.commit(
                Some("HEAD"),
                &sig,
                &sig,
                &format!("commit {i}"),
                &tree,
                &parent_refs,
            )
            .unwrap();
        }
    }
    repo
}

/// `file://` URL for a local path, valid on Unix and Windows. Forces the "smart" transport
/// so that transfer progress callbacks fire (a plain path uses a local copy instead).
pub fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}
