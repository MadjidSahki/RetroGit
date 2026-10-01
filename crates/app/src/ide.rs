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

/// The `.exe` a JetBrains Toolbox `.cmd` launcher starts (`start "" "C:\...\rider64.exe" %*`).
pub fn toolbox_exe(script: &str) -> Option<PathBuf> {
    script
        .split('"')
        .map(str::trim)
        .find(|part| part.to_ascii_lowercase().ends_with(".exe"))
        .map(PathBuf::from)
}

/// Windows: standard install locations (user and machine), JetBrains IDEs installed by Toolbox
/// (`%LOCALAPPDATA%\Programs\<IDE>`) or by hand (`Program Files\JetBrains`), Toolbox `.cmd`
/// launchers resolved to their `.exe`, and Visual Studio found by `vswhere`. Only `.exe` files
/// are returned, so launching never goes through `cmd` (paths with `&` or `%` stay literal).
pub fn detect_windows(
    local_app_data: &Path,
    program_files: &Path,
    exists: &dyn Fn(&Path) -> bool,
    list: &dyn Fn(&Path) -> Vec<String>,
    read: &dyn Fn(&Path) -> Option<String>,
    devenv: Option<PathBuf>,
) -> Vec<Ide> {
    let programs = local_app_data.join("Programs");
    let jetbrains = |tool: &str, dir_prefix: &str, exe: &str| -> Vec<PathBuf> {
        let mut v = Vec::new();
        for root in [programs.clone(), program_files.join("JetBrains")] {
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
        }
        let script = local_app_data.join(format!(r"JetBrains\Toolbox\scripts\{tool}.cmd"));
        v.extend(read(&script).as_deref().and_then(toolbox_exe));
        v
    };
    let candidates: Vec<(&str, Vec<PathBuf>)> = vec![
        (
            "vscode",
            vec![
                programs.join(r"Microsoft VS Code\Code.exe"),
                program_files.join(r"Microsoft VS Code\Code.exe"),
            ],
        ),
        ("cursor", vec![programs.join(r"cursor\Cursor.exe")]),
        ("visualstudio", devenv.into_iter().collect()),
        (
            "rider",
            jetbrains("rider", "JetBrains Rider", "rider64.exe")
                .into_iter()
                .chain(jetbrains("rider", "Rider", "rider64.exe"))
                .collect(),
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
        ("zed", vec![programs.join(r"Zed\Zed.exe")]),
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

/// Apps known to macOS (LaunchServices) from `lsregister -dump` output: top-level `.app`
/// bundles that still exist (`exists`), outside the Trash and Gatekeeper's temporary
/// translocation copies. Finds apps run from ~/Downloads or elsewhere.
pub fn registered_apps(dump: &str, exists: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for line in dump.lines() {
        let Some(rest) = line.strip_prefix("path:") else {
            continue;
        };
        let rest = rest.trim();
        // "path: /Applications/Rider.app (0x154c)"
        let path = match rest.rfind(" (0x") {
            Some(i) => &rest[..i],
            None => rest,
        };
        let nested = path.trim_end_matches(".app").contains(".app/");
        let skipped = path.contains("/.Trash/") || path.contains("/AppTranslocation/");
        if !path.ends_with(".app") || nested || skipped {
            continue;
        }
        let p = PathBuf::from(path);
        if !out.contains(&p) && exists(&p) {
            out.push(p);
        }
    }
    out
}

/// Installed IDEs on this machine. Slow (a few seconds on macOS): call it off the UI thread.
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
        // Apps anywhere else (e.g. VS Code run from ~/Downloads): ask LaunchServices.
        const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
        if let Ok(out) = std::process::Command::new(LSREGISTER)
            .arg("-dump")
            .stderr(Stdio::null())
            .output()
        {
            apps.extend(registered_apps(
                &String::from_utf8_lossy(&out.stdout),
                &|p| p.exists(),
            ));
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
        let read = |p: &Path| std::fs::read_to_string(p).ok();
        detect_windows(&local, &pf, &|p| p.exists(), &list, &read, vswhere_devenv())
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
        cmd.creation_flags(0x0000_0008); // DETACHED_PROCESS: the IDE outlives RetroGit
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
        // Built the way detection builds them (so the test also runs on macOS).
        let rider_exe = pf
            .join("JetBrains")
            .join("JetBrains Rider 2025.2")
            .join("bin")
            .join("rider64.exe");
        let idea_exe = local
            .join("Programs")
            .join("IntelliJ IDEA Ultimate")
            .join("bin")
            .join("idea64.exe");
        let existing = [
            local.join("Programs").join(r"Microsoft VS Code\Code.exe"),
            rider_exe.clone(),
            idea_exe.clone(),
        ];
        let exists = |p: &Path| existing.iter().any(|e| e == p);
        let list = |dir: &Path| {
            if dir == pf.join("JetBrains") {
                vec!["JetBrains Rider 2025.2".to_string()]
            } else if dir == local.join("Programs") {
                vec![
                    "IntelliJ IDEA Ultimate".to_string(),
                    "Microsoft VS Code".to_string(),
                ]
            } else {
                vec![]
            }
        };
        let no_scripts = |_: &Path| None;
        let ides = detect_windows(local, pf, &exists, &list, &no_scripts, None);
        let found: Vec<(&str, &Path)> = ides
            .iter()
            .map(|i| (i.id.as_str(), i.program.as_path()))
            .collect();
        assert_eq!(
            found,
            [
                ("vscode", existing[0].as_path()),
                ("rider", rider_exe.as_path()),
                ("idea", idea_exe.as_path()),
            ]
        );
        let vs = detect_windows(
            local,
            pf,
            &|_| false,
            &|_| vec![],
            &no_scripts,
            Some(PathBuf::from(r"C:\VS\devenv.exe")),
        );
        assert_eq!(vs[0].id, "visualstudio");
    }

    #[test]
    fn toolbox_scripts_are_resolved_to_their_exe_or_ignored() {
        let local = Path::new(r"C:\L");
        let pf = Path::new(r"C:\P");
        let script = local.join(r"JetBrains\Toolbox\scripts\rider.cmd");
        let target = PathBuf::from(r"C:\L\Programs\Rider\bin\rider64.exe");
        let body = format!("@echo off\r\nstart \"\" \"{}\" %*\r\n", target.display());
        let exists = |p: &Path| p == script || p == target;
        let read = |p: &Path| (p == script).then(|| body.clone());
        let ides = detect_windows(local, pf, &exists, &|_| vec![], &read, None);
        assert_eq!(ides[0].program, target, "the script's exe, never cmd");
        // A stale script whose exe is gone is ignored.
        let gone = |p: &Path| p == script;
        assert!(detect_windows(local, pf, &gone, &|_| vec![], &read, None).is_empty());
        assert_eq!(
            toolbox_exe("start \"\" \"C:\\x y\\idea64.exe\" %*"),
            Some(PathBuf::from(r"C:\x y\idea64.exe"))
        );
        assert_eq!(toolbox_exe("echo hi"), None);
    }

    #[test]
    fn apps_registered_with_macos_are_found_wherever_they_are() {
        // VS Code run from ~/Downloads is not in /Applications, but macOS knows it.
        let dump = "\
path:                       /Applications/Rider.app (0x154c)
path:                       /Applications/Rider.app/Contents/jbr/Frameworks/cef_server.app (0x1b44)
path:                       /Users/me/.Trash/Rider.app (0x1bbc)
path:                       /Users/me/Downloads/Visual Studio Code.app (0x1788)
path:                       /private/var/folders/m6/T/AppTranslocation/D3/d/Visual Studio Code.app (0x1d78)
name:                       Something else
";
        let apps = registered_apps(dump, &|_| true);
        assert_eq!(
            apps,
            [
                PathBuf::from("/Applications/Rider.app"),
                PathBuf::from("/Users/me/Downloads/Visual Studio Code.app")
            ]
        );
        let ids: Vec<String> = detect_mac(&apps).into_iter().map(|i| i.id).collect();
        assert_eq!(ids, ["vscode", "rider"]);
        assert!(
            registered_apps(dump, &|p| !p.starts_with("/Applications"))
                .iter()
                .all(|p| !p.starts_with("/Applications")),
            "deleted apps are skipped"
        );
    }

    #[test]
    fn an_app_found_twice_is_listed_once() {
        let apps = vec![
            PathBuf::from("/Users/me/Downloads/Visual Studio Code.app"),
            PathBuf::from("/Users/me/Downloads/Visual Studio Code.app"),
            PathBuf::from("/Applications/Rider.app"),
        ];
        assert_eq!(detect_mac(&apps).len(), 2);
    }

    #[test]
    fn launch_commands() {
        let app = Ide {
            id: "rider".into(),
            name: "Rider".into(),
            program: PathBuf::from("/Applications/Rider.app"),
        };
        let (prog, args) = launch_args(&app, Path::new("/w/my repo"), true);
        assert_eq!(prog, PathBuf::from("open"));
        assert_eq!(args, ["-a", "/Applications/Rider.app", "/w/my repo"]);
        // Windows always starts the exe directly (no cmd: "R&D" or "%x%" in a path stay literal).
        let r = Ide {
            id: "rider".into(),
            name: "Rider".into(),
            program: PathBuf::from(r"C:\R\rider64.exe"),
        };
        assert_eq!(
            launch_args(&r, Path::new(r"C:\w\R&D"), false),
            (
                PathBuf::from(r"C:\R\rider64.exe"),
                vec![r"C:\w\R&D".to_string()]
            )
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
