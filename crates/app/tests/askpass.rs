//! git runs RetroGit as GIT_ASKPASS with the prompt as the only argument.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn git_credential_fill_gets_the_token_from_the_retrogit_binary() {
    if !gitcore::git_available() {
        return;
    }
    let mut child = Command::new("git")
        .args(["-c", "credential.helper=", "credential", "fill"])
        .env("GIT_ASKPASS", env!("CARGO_BIN_EXE_retrogit"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(gitcore::ASKPASS_TOKEN_VAR, "gho_test_token")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("{e}"));
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"protocol=https\nhost=github.com\n\n");
    }
    // A GUI started by mistake would never exit: fail instead of hanging.
    let end = Instant::now() + Duration::from_secs(20);
    while child.try_wait().ok().flatten().is_none() {
        if Instant::now() > end {
            let _ = child.kill();
            panic!("git credential fill did not finish: askpass did not answer");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap_or_else(|e| panic!("{e}"));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("username=x-access-token"), "{text}");
    assert!(text.contains("password=gho_test_token"), "{text}");
}
