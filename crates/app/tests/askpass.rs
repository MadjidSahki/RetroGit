//! The binary answers git's credential prompts when started with `--askpass`.

use std::process::Command;

fn ask(prompt: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_retrogit"))
        .args(["--askpass", prompt])
        .env(gitcore::ASKPASS_TOKEN_VAR, "gho_test_token")
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn askpass_mode_answers_username_then_token() {
    assert_eq!(ask("Username for 'https://github.com': "), "x-access-token");
    assert_eq!(
        ask("Password for 'https://x-access-token@github.com': "),
        "gho_test_token"
    );
}
