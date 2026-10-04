//! The version shown is the release's (set by the CI), else Cargo's.

#[test]
fn the_version_comes_from_the_release_build() {
    let v = retrogit::version::version();
    match option_env!("RETROGIT_VERSION") {
        Some(release) => assert_eq!(v, release),
        None => assert_eq!(v, env!("CARGO_PKG_VERSION")),
    }
    assert!(
        v.split('.').count() == 3 && v.split('.').all(|p| p.parse::<u32>().is_ok()),
        "{v}"
    );
}

#[test]
fn version_numbers_compare_as_numbers() {
    use retrogit::version::parse_version;
    assert!(parse_version("0.1.10") > parse_version("0.1.9"));
    assert_eq!(parse_version("v0.1.42"), Some((0, 1, 42)));
    assert_eq!(parse_version("0.1"), None);
    assert_eq!(parse_version("x.y.z"), None);
}

#[test]
fn the_windows_numeric_version_carries_the_release_number() {
    assert_eq!(
        retrogit::version::numeric_version("0.1.42"),
        Some(0x0000_0001_002A_0000)
    );
    assert_eq!(retrogit::version::numeric_version("1.2"), None);
}
