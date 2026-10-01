//! "Open in IDE": find the installed editors and open a repository in one of them.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::config::Config;

/// An installed editor/IDE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ide {
    /// Stable id stored in the config, e.g. `rider`.
    pub id: String,
    pub name: String,
    /// macOS: the `.app` bundle; Windows: an `.exe` or a `.cmd` launcher.
    pub program: PathBuf,
}

/// `(id, display name, macOS bundle name)`, in display order.
const KNOWN: &[(&str, &str, &str)] = &[
    ("vscode", "VS Code", "Visual Studio Code"),
    ("cursor", "Cursor", "Cursor"),
    ("visualstudio", "Visual Studio", "Visual Studio"),
    ("rider", "Rider", "Rider"),
    ("idea", "IntelliJ IDEA", "IntelliJ IDEA"),
    ("webstorm", "WebStorm", "WebStorm"),
    ("pycharm", "PyCharm", "PyCharm"),
    ("goland", "GoLand", "GoLand"),
    ("rustrover", "RustRover", "RustRover"),
    ("zed", "Zed", "Zed"),
    ("sublime", "Sublime Text", "Sublime Text"),
    ("xcode", "Xcode", "Xcode"),
];

fn ide(id: &str, program: PathBuf) -> Option<Ide> {
    let (id, name, _) = KNOWN.iter().find(|k| k.0 == id)?;
    Some(Ide {
        id: id.to_string(),
        name: name.to_string(),
        program,
    })
}

/// macOS: match `.app` bundles ("Rider.app", "IntelliJ IDEA CE.app", "Zed Preview.app").
pub fn detect_mac(apps: &[PathBuf]) -> Vec<Ide> {
    let mut out = Vec::new();
    for (id, _, bundle) in KNOWN {
        let found = apps.iter().find(|p| {
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else { return false };
            p.extension().is_some_and(|e| e == "app")
                && (stem == *bundle || stem.strip_prefix(bundle).is_some_and(|rest| rest.starts_with(' ')))
                // "Visual Studio" must not match "Visual Studio Code".
                && !(*id == "visualstudio" && stem.starts_with("Visual Studio Code"))
        });
        if let Some(p) = found.and_then(|p| ide(id, p.clone())) {
            out.push(p);
        }
    }
    out
}

/// Windows: standard install locations (user and machine), JetBrains Toolbox launchers and
/// versioned JetBrains installs, plus Visual Studio found by `vswhere` (`devenv`).
pub fn detect_windows(
    local_app_data: &Path,
    program_files: &Path,
    exists: &dyn Fn(&Path) -> bool,
    list: &dyn Fn(&Path) -> Vec<String>,
    devenv: Option<PathBuf>,
) -> Vec<Ide> {
    let jetbrains = |tool: &str, dir_prefix: &str, exe: &str| -> Vec<PathBuf> {
        let mut v = vec![local_app_data.join(format!(r"JetBrains\Toolbox\scripts\{tool}.cmd"))];
        let root = program_files.join("JetBrains");
        let mut dirs: Vec<String> = list(&root)
            .into_iter()
            .filter(|d| d.starts_with(dir_prefix))
            .collect();
        dirs.sort();
        v.extend(
            dirs.into_iter()
                .rev()
                .map(|d| root.join(d).join("bin").join(exe)),
        );
        v
    };
    let candidates: Vec<(&str, Vec<PathBuf>)> = vec![
        (
            "vscode",
            vec![
                local_app_data.join(r"Programs\Microsoft VS Code\Code.exe"),
                program_files.join(r"Microsoft VS Code\Code.exe"),
            ],
        ),
        (
            "cursor",
            vec![local_app_data.join(r"Programs\cursor\Cursor.exe")],
        ),
        ("visualstudio", devenv.into_iter().collect()),
        (
            "rider",
            jetbrains("rider", "JetBrains Rider", "rider64.exe"),
        ),
        ("idea", jetbrains("idea", "IntelliJ IDEA", "idea64.exe")),
        (
            "webstorm",
            jetbrains("webstorm", "WebStorm", "webstorm64.exe"),
        ),
        ("pycharm", jetbrains("pycharm", "PyCharm", "pycharm64.exe")),
        ("goland", jetbrains("goland", "GoLand", "goland64.exe")),
        (
            "rustrover",
            jetbrains("rustrover", "RustRover", "rustrover64.exe"),
        ),
        ("zed", vec![local_app_data.join(r"Programs\Zed\Zed.exe")]),
        (
            "sublime",
            vec![program_files.join(r"Sublime Text\sublime_text.exe")],
        ),
    ];
    candidates
        .into_iter()
        .filter_map(|(id, paths)| {
            let found = if id == "visualstudio" {
                paths.into_iter().next()
            } else {
                paths.into_iter().find(|p| exists(p))
            };
            found.and_then(|p| ide(id, p))
        })
        .collect()
}

/// Installed IDEs on this machine (file checks only, plus `vswhere` on Windows).
pub fn detect() -> Vec<Ide> {
    #[cfg(target_os = "macos")]
    {
        let mut apps = Vec::new();
        let home = dirs::home_dir().unwrap_or_default();
        for dir in [
            PathBuf::from("/Applications"),
            home.join("Applications"),
            home.join("Applications/JetBrains Toolbox"),
        ] {
            if let Ok(entries) = std::fs::read_dir(dir) {
                apps.extend(entries.flatten().map(|e| e.path()));
            }
        }
        detect_mac(&apps)
    }
    #[cfg(windows)]
    {
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_default();
        let pf = std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        let list = |d: &Path| {
            std::fs::read_dir(d)
                .map(|it| {
                    it.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default()
        };
        detect_windows(&local, &pf, &|p| p.exists(), &list, vswhere_devenv())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Vec::new()
    }
}

#[cfg(windows)]
fn vswhere_devenv() -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    let pf86 = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from)?;
    let vswhere = pf86.join(r"Microsoft Visual Studio\Installer\vswhere.exe");
    let out = std::process::Command::new(vswhere)
        .args(["-latest", "-property", "productPath"])
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Program and arguments that open `repo` in `ide` (`mac`: use `open -a`).
pub fn launch_args(ide: &Ide, repo: &Path, mac: bool) -> (PathBuf, Vec<String>) {
    let repo = repo.display().to_string();
    if mac {
        return (
            PathBuf::from("open"),
            vec!["-a".into(), ide.program.display().to_string(), repo],
        );
    }
    let is_cmd = ide
        .program
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    if is_cmd {
        (
            PathBuf::from("cmd"),
            vec!["/C".into(), ide.program.display().to_string(), repo],
        )
    } else {
        (ide.program.clone(), vec![repo])
    }
}

/// Start the IDE on `repo` without waiting for it.
pub fn open(ide: &Ide, repo: &Path) -> std::io::Result<()> {
    let (program, args) = launch_args(ide, repo, cfg!(target_os = "macos"));
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW (for .cmd launchers)
    }
    cmd.spawn().map(|_| ())
}

/// IDE for `repo`: its own choice, else the last choice, else the first detected one.
pub fn ide_for<'a>(config: &Config, ides: &'a [Ide], repo: &Path) -> Option<&'a Ide> {
    let wanted = config.ide_by_repo.get(repo).or(config.default_ide.as_ref());
    wanted
        .and_then(|id| ides.iter().find(|i| &i.id == id))
        .or_else(|| ides.first())
}

/// Remember `id` for `repo` (and as the default for repositories without a choice).
pub fn remember_ide(config: &mut Config, repo: &Path, id: &str) {
    config
        .ide_by_repo
        .insert(repo.to_path_buf(), id.to_string());
    config.default_ide = Some(id.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn mac_apps_are_found_by_bundle_name() {
        let apps = vec![
            PathBuf::from("/Applications/Visual Studio Code.app"),
            PathBuf::from("/Applications/Safari.app"),
            PathBuf::from("/Users/me/Applications/Rider.app"),
            PathBuf::from("/Applications/IntelliJ IDEA CE.app"),
            PathBuf::from("/Applications/Zed Preview.app"),
        ];
        let ides = detect_mac(&apps);
        let names: Vec<&str> = ides.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["VS Code", "Rider", "IntelliJ IDEA", "Zed"]);
        assert_eq!(
            ides[1].program,
            PathBuf::from("/Users/me/Applications/Rider.app")
        );
    }

    #[test]
    fn windows_ides_are_found_in_standard_locations() {
        let local = Path::new(r"C:\Users\me\AppData\Local");
        let pf = Path::new(r"C:\Program Files");
        let existing = [
            local.join(r"Programs\Microsoft VS Code\Code.exe"),
            local.join(r"JetBrains\Toolbox\scripts\rider.cmd"),
            pf.join(r"JetBrains\JetBrains Rider 2025.2\bin\rider64.exe"),
        ];
        let exists = |p: &Path| existing.iter().any(|e| e == p);
        let list = |dir: &Path| {
            if dir == pf.join("JetBrains") {
                vec!["JetBrains Rider 2025.2".to_string()]
            } else {
                vec![]
            }
        };
        let ides = detect_windows(local, pf, &exists, &list, None);
        let found: Vec<(&str, &Path)> = ides
            .iter()
            .map(|i| (i.id.as_str(), i.program.as_path()))
            .collect();
        assert_eq!(found[0], ("vscode", existing[0].as_path()));
        // The Toolbox launcher wins over a versioned install; one entry per IDE.
        assert_eq!(found[1], ("rider", existing[1].as_path()));
        assert_eq!(found.len(), 2);
        let vs = detect_windows(
            local,
            pf,
            &|_| false,
            &|_| vec![],
            Some(PathBuf::from(r"C:\VS\devenv.exe")),
        );
        assert_eq!(vs[0].id, "visualstudio");
    }

    #[test]
    fn launch_commands() {
        let app = Ide {
            id: "rider".into(),
            name: "Rider".into(),
            program: PathBuf::from("/Applications/Rider.app"),
        };
        let (prog, args) = launch_args(&app, Path::new("/w/repo"), true);
        assert_eq!(prog, PathBuf::from("open"));
        assert_eq!(args, ["-a", "/Applications/Rider.app", "/w/repo"]);
        let cmd = Ide {
            id: "rider".into(),
            name: "Rider".into(),
            program: PathBuf::from(r"C:\T\rider.cmd"),
        };
        let (prog, args) = launch_args(&cmd, Path::new(r"C:\w\repo"), false);
        assert_eq!(prog, PathBuf::from("cmd"));
        assert_eq!(args, ["/C", r"C:\T\rider.cmd", r"C:\w\repo"]);
        let exe = Ide {
            id: "vscode".into(),
            name: "VS Code".into(),
            program: PathBuf::from(r"C:\Code.exe"),
        };
        assert_eq!(
            launch_args(&exe, Path::new(r"C:\r"), false),
            (PathBuf::from(r"C:\Code.exe"), vec![r"C:\r".to_string()])
        );
    }

    #[test]
    fn the_ide_for_a_repo_is_remembered_with_a_default_for_new_repos() {
        let ides = vec![
            Ide {
                id: "vscode".into(),
                name: "VS Code".into(),
                program: "a".into(),
            },
            Ide {
                id: "rider".into(),
                name: "Rider".into(),
                program: "b".into(),
            },
        ];
        let mut cfg = crate::config::Config::default();
        assert_eq!(
            ide_for(&cfg, &ides, Path::new("/r1")).map(|i| i.id.as_str()),
            Some("vscode")
        );
        remember_ide(&mut cfg, Path::new("/r1"), "rider");
        assert_eq!(
            ide_for(&cfg, &ides, Path::new("/r1")).map(|i| i.id.as_str()),
            Some("rider")
        );
        assert_eq!(
            ide_for(&cfg, &ides, Path::new("/r2")).map(|i| i.id.as_str()),
            Some("rider"),
            "last choice is the default"
        );
        assert_eq!(ide_for(&cfg, &[], Path::new("/r1")), None);
    }
}
