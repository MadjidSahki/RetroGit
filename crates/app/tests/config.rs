#![allow(clippy::unwrap_used)]
//! The settings file: one wrong entry only resets itself, unreadable files are kept aside.

use std::path::PathBuf;

use retrogit::config::{AppearanceConfig, Config, UpdatesConfig};

fn load(text: &str) -> (Config, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("config.json");
    std::fs::write(&p, text).unwrap();
    (Config::load_from(&p), d)
}

#[test]
fn a_wrong_zoom_keeps_the_rest_of_the_file() {
    let (c, _d) = load(
        r#"{"recent":[{"name":"a","path":"/a"}],"accounts":["octo"],
            "appearance":{"zoom":"x","scheme":"Dark"}}"#,
    );
    assert_eq!(c.recent.len(), 1);
    assert_eq!(c.recent[0].path, PathBuf::from("/a"));
    assert_eq!(c.accounts, vec!["octo".to_string()]);
    assert_eq!(c.appearance.scheme, "Dark");
    assert_eq!(c.appearance.zoom, 1.0);
    assert_eq!(c.appearance.font, AppearanceConfig::default().font);
}

#[test]
fn a_wrong_section_only_resets_that_section() {
    let (c, _d) = load(r#"{"updates":5,"recent":[{"name":"a","path":"/a"}]}"#);
    assert_eq!(c.recent.len(), 1);
    assert_eq!(c.updates, UpdatesConfig::default());
    assert!(c.updates.check);
    let (c, _d) = load(
        r#"{"updates":{"check":"yes","skipped":"1.2.3"},"recent":5,"accounts":["octo"],
            "window":"big","last_clone_dir":7}"#,
    );
    assert!(c.updates.check, "a wrong check is the default (on)");
    assert_eq!(c.updates.skipped.as_deref(), Some("1.2.3"));
    assert!(c.recent.is_empty());
    assert_eq!(c.accounts, vec!["octo".to_string()]);
    assert_eq!(c.window, None);
    assert_eq!(c.last_clone_dir, None);
}

#[test]
fn an_unreadable_file_gives_defaults_and_is_kept_aside() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("config.json");
    std::fs::write(&p, b"{").unwrap();
    assert_eq!(Config::load_from(&p), Config::default());
    assert_eq!(
        std::fs::read(d.path().join("config.json.bad")).unwrap(),
        b"{"
    );
}

#[test]
fn a_missing_file_leaves_no_bad_copy() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("config.json");
    assert_eq!(Config::load_from(&p), Config::default());
    assert!(!d.path().join("config.json.bad").exists());
}
