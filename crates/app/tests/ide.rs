//! Open in IDE: the program and arguments started for a repository and a file.

use std::path::{Path, PathBuf};

use retrogit::ide::{Ide, launch_args};

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
    let full = repo.join("src/a b.rs").display().to_string();
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
            vec!["/w/my repo".to_string(), full.clone()]
        )
    );
    let cmd = ide("/opt/code/bin/code.cmd");
    assert_eq!(
        launch_args(&cmd, repo, file, false),
        (
            PathBuf::from("cmd"),
            vec![
                "/C".to_string(),
                "/opt/code/bin/code.cmd".to_string(),
                "/w/my repo".to_string(),
                full,
            ]
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
