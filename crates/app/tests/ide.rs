//! Open in IDE: the program and arguments started for a repository and a file.

use std::path::{Path, PathBuf};

use retrogit::ide::{Ide, file_arg, launch_args};

fn ide(program: &str) -> Ide {
    Ide {
        id: "vscode".into(),
        name: "VS Code".into(),
        program: PathBuf::from(program),
    }
}

#[test]
fn a_file_is_opened_after_its_repository_by_every_launcher() {
    let repo = Path::new("/w/my repo");
    let file = Some(Path::new("src/a b.rs"));
    let full = file_arg(repo, "src/a b.rs").display().to_string();
    let app = ide("/Applications/Visual Studio Code.app");
    assert_eq!(
        launch_args(&app, repo, file, true),
        (
            PathBuf::from("open"),
            vec![
                "-a".to_string(),
                "/Applications/Visual Studio Code.app".to_string(),
                "/w/my repo".to_string(),
                full.clone(),
            ]
        )
    );
    let exe = ide("/opt/code/bin/code");
    assert_eq!(
        launch_args(&exe, repo, file, false),
        (
            PathBuf::from("/opt/code/bin/code"),
            vec!["/w/my repo".to_string(), full]
        )
    );
}

#[test]
fn without_a_file_only_the_repository_is_opened() {
    assert_eq!(
        launch_args(&ide("/opt/code"), Path::new("/w/r"), None, false),
        (PathBuf::from("/opt/code"), vec!["/w/r".to_string()])
    );
}

#[test]
fn the_file_path_is_built_one_segment_at_a_time() {
    // Git gives `/`-separated paths: each segment is joined, so Windows gets only `\\`.
    let repo = Path::new("/w/my repo");
    let path = file_arg(repo, "src/a b.rs");
    let parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let mut want: Vec<String> = repo
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    want.extend(["src".to_string(), "a b.rs".to_string()]);
    assert_eq!(parts, want);
    assert_eq!(path, repo.join("src").join("a b.rs"));
    let sep = std::path::MAIN_SEPARATOR;
    assert_eq!(
        path.display().to_string(),
        format!("{}{sep}src{sep}a b.rs", repo.display())
    );
    assert_eq!(file_arg(repo, "top.rs"), repo.join("top.rs"));
    assert_eq!(
        file_arg(repo, "a//b/"),
        repo.join("a").join("b"),
        "empty segments skipped"
    );
}
