#![allow(clippy::unwrap_used)]
//! Pull request notifications: watcher thread against a mock GitHub, and the state.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gitcore::{Head, RepoSummary};
use github::{Client, PrEvent, PrEventKind, TokenProvider};
use retrogit::config::Config;
use retrogit::pr_watch::{Poller, PrWatcher, WatchState};
use retrogit::protocol::Event;
use retrogit::state::{AppState, MAX_NOTIFICATIONS, NotificationTarget, Tab};
use serde_json::json;

fn search(state: &str, checks: &str) -> String {
    json!({ "data": { "search": { "nodes": [ {
        "number": 7, "title": "Fix login", "url": "https://github.com/o/r/pull/7",
        "state": state, "repository": { "nameWithOwner": "o/r" },
        "author": { "login": "ada" }, "mergedBy": null,
        "comments": { "totalCount": 0, "nodes": [] },
        "reviews": { "totalCount": 0, "nodes": [] },
        "commits": { "nodes": [ { "commit": { "oid": "abc",
            "statusCheckRollup": { "state": checks, "contexts": { "nodes": [] } } } } ] }
    } ] } } })
    .to_string()
}

#[test]
fn the_watcher_announces_changes_after_a_silent_first_pass() {
    let mut server = mockito::Server::new();
    let first = server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer gho_t")
        .with_body(search("OPEN", "PENDING"))
        .expect(1)
        .create();
    let accounts = github::Accounts::default();
    accounts.upsert(
        github::Account {
            login: "ada".into(),
            token: "gho_t".into(),
        },
        None,
    );
    let got: Arc<Mutex<Vec<PrEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = got.clone();
    let _w = PrWatcher::start(
        Client::with_bases(&server.url(), &server.url()),
        TokenProvider::without_gh(),
        accounts,
        Duration::from_millis(300),
        move |events| sink.lock().unwrap().extend(events),
    );
    std::thread::sleep(Duration::from_millis(150));
    first.assert();
    assert!(got.lock().unwrap().is_empty(), "first pass: reference only");
    server
        .mock("POST", "/graphql")
        .with_body(search("OPEN", "SUCCESS"))
        .create();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while got.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let events = got.lock().unwrap().clone();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].kind, PrEventKind::ChecksPassed);
    assert_eq!(events[0].key, "o/r#7");
}

#[test]
fn every_account_is_watched_and_events_say_whose_they_are() {
    let mut server = mockito::Server::new();
    let accounts = github::Accounts::default();
    for login in ["ada", "bob"] {
        accounts.upsert(
            github::Account {
                login: login.into(),
                token: format!("gho_{login}"),
            },
            None,
        );
    }
    for login in ["ada", "bob"] {
        server
            .mock("POST", "/graphql")
            .match_header("authorization", format!("Bearer gho_{login}").as_str())
            .with_body(search("OPEN", "PENDING"))
            .expect(1)
            .create();
    }
    let got: Arc<Mutex<Vec<PrEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = got.clone();
    let _w = PrWatcher::start(
        Client::with_bases(&server.url(), &server.url()),
        TokenProvider::without_gh(),
        accounts.clone(),
        Duration::from_millis(300),
        move |events| sink.lock().unwrap().extend(events),
    );
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        got.lock().unwrap().is_empty(),
        "first passes: references only"
    );
    // bob is removed; ada's pull request finishes its checks.
    accounts.remove("bob");
    server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer gho_ada")
        .with_body(search("OPEN", "SUCCESS"))
        .create();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while got.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let events = got.lock().unwrap().clone();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].account, "ada");
}

#[test]
fn signing_in_as_someone_else_starts_a_new_reference() {
    let snap = |checks| github::PrSnapshot {
        key: "o/r#1".into(),
        repo: "o/r".into(),
        number: 1,
        title: "t".into(),
        url: "u".into(),
        state: github::PrState::Open,
        merged_by: None,
        closed_by: None,
        head: "h".into(),
        checks,
        failed_checks: 0,
        review_count: 0,
        last_review: None,
        comment_count: 0,
        last_commenter: None,
    };
    let mut w = WatchState::default();
    assert!(
        w.advance("ada", vec![snap(github::ChecksState::Pending)])
            .is_empty()
    );
    assert!(
        w.advance("bob", vec![snap(github::ChecksState::Success)])
            .is_empty()
    );
    assert_eq!(
        w.advance("bob", vec![snap(github::ChecksState::Failure)])
            .len(),
        0,
        "settled to settled: nothing"
    );
    w.reset();
    assert!(
        w.advance("bob", vec![snap(github::ChecksState::Pending)])
            .is_empty()
    );
    assert_eq!(
        w.advance("bob", vec![snap(github::ChecksState::Success)])
            .len(),
        1
    );
}

#[test]
fn the_gh_token_adds_pull_requests_only_for_the_same_account() {
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer gho_app")
        .with_body(search("OPEN", "PENDING"))
        .create();
    let other = json!({ "data": { "search": { "nodes": [ {
        "number": 9, "title": "Restricted org PR", "url": "u", "state": "OPEN",
        "repository": { "nameWithOwner": "corp/x" }, "author": { "login": "ada" },
        "comments": { "totalCount": 0, "nodes": [] }, "reviews": { "totalCount": 0, "nodes": [] },
        "commits": { "nodes": [] } } ] } } })
    .to_string();
    server
        .mock("POST", "/graphql")
        .match_header("authorization", "Bearer gho_cli")
        .with_body(other)
        .create();
    let user = server
        .mock("GET", "/user")
        .match_header("authorization", "Bearer gho_cli")
        .with_body(r#"{"login":"Ada","name":null}"#)
        .expect(1)
        .create();
    let tokens = TokenProvider::new(Arc::new(|_: &str| Some("gho_cli".to_string())));
    let mut p = Poller::new(Client::with_bases(&server.url(), &server.url()), tokens);
    let keys: Vec<String> = p
        .poll("gho_app", "ada", 1_790_856_000)
        .unwrap()
        .into_iter()
        .map(|s| s.key)
        .collect();
    assert_eq!(keys, ["o/r#7", "corp/x#9"]);
    // The account check is cached.
    p.poll("gho_app", "ada", 1_790_856_000).unwrap();
    user.assert();
    // Another account behind gh: its pull requests are not the user's.
    let keys: Vec<String> = p
        .poll("gho_app", "bob", 1_790_856_000)
        .unwrap()
        .into_iter()
        .map(|s| s.key)
        .collect();
    assert_eq!(keys, ["o/r#7"]);
}

fn event(repo: &str, number: u64) -> PrEvent {
    PrEvent {
        account: "ada".into(),
        key: format!("{repo}#{number}"),
        repo: repo.into(),
        number,
        title: "t".into(),
        url: format!("https://github.com/{repo}/pull/{number}"),
        kind: PrEventKind::Merged,
    }
}

fn opened(state: &mut AppState, path: &str, origin: &str) {
    state.apply(Event::RepoOpened(RepoSummary {
        name: Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        path: PathBuf::from(path),
        head: Head::Branch("main".into()),
        origin_url: Some(origin.into()),
        last_commit: None,
    }));
}

#[test]
fn notifications_are_kept_newest_first_with_an_unread_count() {
    let mut st = AppState::new(Config::default());
    st.apply(Event::PrEvents(vec![event("o/r", 1), event("o/r", 2)]));
    assert_eq!(st.notifications.unread, 2);
    assert_eq!(st.notifications.items[0].number, 2);
    st.open_notifications();
    assert_eq!(st.notifications.unread, 0);
    assert!(st.notifications.open);
    st.apply(Event::PrEvents((0..60).map(|n| event("o/r", n)).collect()));
    assert_eq!(st.notifications.items.len(), MAX_NOTIFICATIONS);
    assert_eq!(st.notifications.unread, MAX_NOTIFICATIONS);
}

#[test]
fn clicking_a_notification_finds_the_local_repo() {
    let mut st = AppState::new(Config::default());
    let here = std::env::temp_dir();
    let a = here.join("a").display().to_string();
    let b = here.join("b").display().to_string();
    st.config.add_recent("b", Path::new(&b));
    opened(&mut st, &a, "https://github.com/o/a.git");
    let slug_of = |p: &Path| (p == Path::new(&b)).then(|| ("O".to_string(), "B".to_string()));
    assert_eq!(
        st.notification_target(&event("o/a", 3), slug_of),
        NotificationTarget::Current(3)
    );
    assert_eq!(
        st.notification_target(&event("o/b", 4), slug_of),
        NotificationTarget::Local(PathBuf::from(&b), 4),
        "case-insensitive"
    );
    assert_eq!(
        st.notification_target(&event("x/y", 5), slug_of),
        NotificationTarget::Browser("https://github.com/x/y/pull/5".into())
    );
}

#[test]
fn the_pull_request_opens_once_its_repository_is_open() {
    let mut st = AppState::new(Config::default());
    opened(&mut st, "/tmp/a", "https://github.com/o/a.git");
    st.pulls.open_after_switch = Some((("o".into(), "b".into()), 4));
    opened(&mut st, "/tmp/b", "git@github.com:o/b.git");
    assert_eq!(st.tab, Tab::PullRequests);
    assert_eq!(st.pulls.selected, Some(4));
    assert!(st.pulls.load_selected);
    assert!(st.pulls.open_after_switch.is_none());
    // In the open repository: straight away.
    st.show_pull(9);
    assert_eq!(st.pulls.selected, Some(9));
}

#[test]
fn notification_links_survive_any_repository_name() {
    use retrogit::notify::{PullLink, parse_pull_link, pull_link};
    let l = PullLink {
        repo: "o-1/r.dot_x".into(),
        number: 42,
        account: "me & you".into(),
    };
    let url = pull_link(&l);
    assert!(url.starts_with("retrogit://pull?"), "{url}");
    assert!(!url.contains(' '), "a space is escaped: {url}");
    assert!(!url.contains("&&"), "an `&` in a value is escaped: {url}");
    assert_eq!(parse_pull_link(&url), Some(l.clone()));
    assert_eq!(
        parse_pull_link(&pull_link(&PullLink {
            repo: "a/b".into(),
            number: 1,
            account: String::new()
        }))
        .unwrap()
        .repo,
        "a/b"
    );
    for bad in [
        "https://x",
        "retrogit://pull?repo=o/r",
        "retrogit://pull?repo=o/r&number=x",
        "retrogit://other?number=1",
        "retrogit://pull?repo=a%2Fb%2Fc&number=1",
        "retrogit://pull?repo=%2Fr&number=1",
        "retrogit://pull?repo=o%2F&number=1",
        "retrogit://pull?repo=o%2Fr%3Fx&number=1",
        "retrogit://pull?repo=..%2Fx&number=1",
        "retrogit://pull?repo=o%2F..&number=1",
        "retrogit://pull?repo=o%2F.&number=1",
        "retrogit://pull?repo=o.x%2Fr&number=1",
        "retrogit://pull?repo=o%2Fr%20x&number=1",
        "retrogit://pull?repo=o%2Fr&number=0",
    ] {
        assert_eq!(parse_pull_link(bad), None, "{bad}");
    }
    let long = |n: usize| "x".repeat(n);
    for (repo, ok) in [
        ("my-org/my.repo_1".to_string(), true),
        (format!("{}/r", long(39)), true),
        (format!("{}/r", long(40)), false),
        (format!("o/{}", long(100)), true),
        (format!("o/{}", long(101)), false),
    ] {
        let url = pull_link(&PullLink {
            repo: repo.clone(),
            number: 1,
            account: "a".into(),
        });
        assert_eq!(parse_pull_link(&url).is_some(), ok, "{repo}");
    }
    let e = event("o/r", 7);
    assert_eq!(
        parse_pull_link(&retrogit::notify::event_link(&e)),
        Some(PullLink {
            repo: "o/r".into(),
            number: 7,
            account: e.account.clone()
        })
    );
}

#[test]
fn a_clicked_link_opens_its_pull_request() {
    let mut st = AppState::new(Config::default());
    let here = std::env::temp_dir();
    let a = here.join("a").display().to_string();
    opened(&mut st, &a, "https://github.com/o/a.git");
    let link = retrogit::notify::event_link(&event("o/a", 3));
    st.open_link(&link);
    assert_eq!(st.take_links(), [link.as_str()]);
    assert!(st.take_links().is_empty());
    let target = st.link_target(&link, |_| None);
    assert_eq!(target, Some(("o/a".into(), NotificationTarget::Current(3))));
    assert_eq!(
        st.link_target(&retrogit::notify::event_link(&event("x/y", 5)), |_| None),
        Some((
            "x/y".into(),
            NotificationTarget::Browser("https://github.com/x/y/pull/5".into())
        ))
    );
    assert_eq!(st.link_target("retrogit://nope", |_| None), None);
}

#[test]
fn a_link_on_the_command_line_opens_the_gui() {
    let link = "retrogit://pull?repo=o%2Fr&number=7&account=me";
    let args: Vec<String> = ["retrogit", link].iter().map(|s| s.to_string()).collect();
    assert_eq!(
        retrogit::cli::parse(&args),
        retrogit::cli::Launch::Link {
            link: link.to_string()
        }
    );
}

#[test]
fn a_crafted_link_cannot_smuggle_options() {
    use retrogit::cli::{Launch, parse};
    let args = |a: &[&str]| -> Vec<String> { a.iter().map(|s| s.to_string()).collect() };
    // Windows pastes the URL raw into `"exe" "%1"`: quotes in it split the arguments.
    let smuggled = args(&["retrogit.exe", "retrogit:", "--open", r"\\host\share\repo"]);
    assert_eq!(parse(&smuggled), Launch::Gui { open: None });
    let cli = args(&["retrogit.exe", "retrogit:x", "--cli", "/tmp", "evil"]);
    assert_eq!(parse(&cli), Launch::Gui { open: None });
    let upper = args(&[
        "retrogit.exe",
        "RETROGIT://pull/?repo=o%2Fr&number=7&account=a",
    ]);
    assert!(matches!(parse(&upper), Launch::Link { .. }));
}

#[test]
fn links_normalized_by_windows_still_open_their_pull_request() {
    use retrogit::notify::parse_pull_link;
    for url in [
        "retrogit://pull/?repo=o%2Fr&number=7&account=a",
        "RETROGIT://PULL?repo=o%2Fr&number=7&account=a",
    ] {
        let l = parse_pull_link(url).unwrap_or_else(|| panic!("{url}"));
        assert_eq!((l.repo.as_str(), l.number), ("o/r", 7));
    }
}

#[test]
fn what_arrives_from_another_instance_is_routed_by_kind() {
    use retrogit::cli::{Requested, route};
    assert_eq!(
        route(std::path::PathBuf::from("/w/repo")),
        Requested::Folder(std::path::PathBuf::from("/w/repo"))
    );
    assert_eq!(
        route(std::path::PathBuf::from(
            "Retrogit://pull?repo=o%2Fr&number=1"
        )),
        Requested::Link("Retrogit://pull?repo=o%2Fr&number=1".into())
    );
}

#[cfg(target_os = "macos")]
#[test]
fn outside_retrogit_app_notifications_stay_on_osascript() {
    retrogit::notify::macos::init();
    assert!(
        !retrogit::notify::macos::native(),
        "tests do not run from an app bundle"
    );
}

#[test]
fn a_clicked_notification_restores_then_focuses_the_window() {
    assert_eq!(
        retrogit::app::bring_to_front(),
        [
            egui::ViewportCommand::Minimized(false),
            egui::ViewportCommand::Focus
        ]
    );
}
