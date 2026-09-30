//! THIRD_PARTY.md must list the licenses of the embedded syntax definitions and themes.
//! Regenerate with: RETROGIT_WRITE_NOTICES=1 cargo test -p retrogit --test third_party

const HEADER: &str = "# Third-party notices\n\n\
RetroGit embeds syntax definitions and a color theme from the \
[bat](https://github.com/sharkdp/bat) project (via the \
[two-face](https://codeberg.org/CosmicHarper/two-face) and \
[syntect](https://github.com/trishume/syntect) crates), and the W95FA font \
(SIL Open Font License 1.1, see `crates/win95/assets/OFL.txt`). \
Their licenses follow.\n\n";

#[test]
fn third_party_notices_are_up_to_date() {
    // LF only: git would convert CRLF (some license texts have it) and break the comparison.
    let expected =
        format!("{HEADER}{}", two_face::acknowledgement::listing().to_md()).replace("\r\n", "\n");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../THIRD_PARTY.md");
    if std::env::var_os("RETROGIT_WRITE_NOTICES").is_some() {
        std::fs::write(&path, &expected).unwrap_or_else(|e| panic!("{e}"));
    }
    let actual = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        actual == expected,
        "THIRD_PARTY.md is missing or outdated: run with RETROGIT_WRITE_NOTICES=1"
    );
}
