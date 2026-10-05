#![allow(clippy::unwrap_used)]
//! macOS tokens through `/usr/bin/security` (no Keychain prompt after an update).

use github::{
    SECURITY_MARKER, parse_security_comment, parse_security_password, security_add_command,
};

#[test]
fn the_token_never_appears_as_an_argument() {
    let cmd = security_add_command("RetroGit", "github.com:ada", "gho_Secret\"1").unwrap();
    assert!(!cmd.contains("gho_Secret"), "{cmd}");
    assert_eq!(
        cmd,
        format!(
            "add-generic-password -U -s \"RetroGit\" -a \"github.com:ada\" -j \"{SECURITY_MARKER}\" -X \"{}\"\n",
            "67686f5f5365637265742231"
        )
    );
    // Quotes in names cannot end the argument.
    let odd = security_add_command("Retro\"Git", "a\\b", "t").unwrap();
    assert!(
        odd.contains("-s \"Retro\\\"Git\"") && odd.contains("-a \"a\\\\b\""),
        "{odd}"
    );
}

#[test]
fn the_marker_is_read_from_the_item_attributes() {
    let out = r#"keychain: "/Users/x/Library/Keychains/login.keychain-db"
attributes:
    "acct"<blob>="github.com:ada"
    "icmt"<blob>="retrogit-security"
    "svce"<blob>="RetroGit"
"#;
    assert_eq!(
        parse_security_comment(out).as_deref(),
        Some(SECURITY_MARKER)
    );
    let old = "attributes:\n    \"icmt\"<blob>=<NULL>\n";
    assert_eq!(parse_security_comment(old), None);
}

#[test]
fn a_line_break_or_nul_cannot_start_another_command() {
    for bad in ["a\nb", "a\rb", "a\0b"] {
        assert!(security_add_command(bad, "acct", "t").is_err(), "{bad:?}");
        assert!(
            security_add_command("RetroGit", bad, "t").is_err(),
            "{bad:?}"
        );
        assert!(
            security_add_command("RetroGit", "acct", bad).is_err(),
            "{bad:?}"
        );
    }
    assert!(security_add_command("RetroGit", "acct", "gho_é ").is_ok());
}

#[test]
fn the_password_is_read_in_both_forms_security_prints() {
    // Printable ASCII: quoted as is (security switches to hex for `"` and `\` too).
    let plain = "password: \"gho_abc\"\nkeychain: \"/k\"\n";
    assert_eq!(parse_security_password(plain).as_deref(), Some("gho_abc"));
    assert_eq!(
        parse_security_password("password: \"gho_abc \"\n").as_deref(),
        Some("gho_abc "),
        "a trailing space is kept"
    );
    // Anything else: hex bytes, then a lossy quoted rendering.
    let hex = "password: 0x67686F5FC3A920  \"gho_\\303\\251 \"\n";
    assert_eq!(parse_security_password(hex).as_deref(), Some("gho_é "));
    let quote = "keychain: \"/k\"\npassword: 0x615C62226320  \"a\\134b\"c \"\n";
    assert_eq!(parse_security_password(quote).as_deref(), Some("a\\b\"c "));
    let newline = "password: 0x610A62  \"a\\012b\"\n";
    assert_eq!(parse_security_password(newline).as_deref(), Some("a\nb"));
    // No password, or something unreadable.
    assert_eq!(
        parse_security_password("password: \nkeychain: \"/k\"\n"),
        None
    );
    assert_eq!(parse_security_password("keychain: \"/k\"\n"), None);
    assert_eq!(parse_security_password("password: 0x6G  \"?\"\n"), None);
    assert_eq!(parse_security_password("password: 0xFF  \"?\"\n"), None);
}

/// Uses the real login Keychain: run by hand with `cargo test -- --ignored`.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "writes to the login Keychain"]
fn real_keychain_round_trip_and_migration() {
    use github::{KeyringStore, TokenStore};
    let service = format!("RetroGit-Test-{}", std::process::id());
    let store = KeyringStore::new(&service, "github.com:probe");
    assert_eq!(store.load().unwrap(), None);
    store.save("gho_first").unwrap();
    assert_eq!(store.load().unwrap().as_deref(), Some("gho_first"));
    store.save("gho_second").unwrap();
    assert_eq!(store.load().unwrap().as_deref(), Some("gho_second"));
    // Non-ASCII and a trailing space come back exactly.
    store.save("gho_é ").unwrap();
    assert_eq!(store.load().unwrap().as_deref(), Some("gho_é "));
    // An entry from an older RetroGit (no marker) is re-created with it.
    store.clear().unwrap();
    let status = std::process::Command::new("/usr/bin/security")
        .args([
            "add-generic-password",
            "-s",
            &service,
            "-a",
            "github.com:probe",
            "-w",
            "gho_old",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(store.load().unwrap().as_deref(), Some("gho_old"));
    let attrs = std::process::Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-s",
            &service,
            "-a",
            "github.com:probe",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&attrs.stdout).into_owned();
    assert_eq!(
        parse_security_comment(&text).as_deref(),
        Some(SECURITY_MARKER),
        "migrated"
    );
    store.clear().unwrap();
    store.clear().unwrap();
    assert_eq!(store.load().unwrap(), None);
}

#[test]
fn a_token_read_during_migration_is_kept_even_if_re_saving_fails() {
    use github::keep_after_migration;
    let failed = Err(github::TokenStoreError("keychain locked".into()));
    assert_eq!(keep_after_migration("gho_t".into(), failed), "gho_t");
    assert_eq!(keep_after_migration("gho_t".into(), Ok(())), "gho_t");
}
