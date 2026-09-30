# RetroGit

**A fast, lightweight Git client for macOS and Windows — with the look and feel of Windows 95.**

RetroGit is a native desktop Git client written in Rust. It is built for daily use:
browse and clone your GitHub repositories, stage exactly the lines you want, commit with
your hooks and signature, explore history on a commit graph, and sync with GitHub —
all in grey bevelled windows, a blue gradient title bar and a pixel font.

> Screenshot: _coming soon_

- Single native binary (~13 MB), no webview, no runtime
- Idle CPU ≈ 0 % (redraws only when something changes)
- macOS (Apple Silicon) and Windows (x86-64)

## Features

**GitHub**
- Sign in with GitHub in your browser (OAuth Device Flow), or paste a personal access token
- The token is stored in the macOS Keychain / Windows Credential Manager, never on disk
- List your personal and organization repositories, filter, and clone with progress and cancel
- SSO-aware: tells you when an organization needs you to authorize the app

**Local work**
- Status with staged / unstaged / untracked files, refreshed automatically when files change
- Unified diff with per-line check boxes: stage or unstage a whole file, a hunk or single lines
- Discard changes (file, hunk or lines), with confirmation; untracked files go to the trash
- Commit through your installed `git`: hooks (`pre-commit`, husky, …) and GPG/SSH signing just work
- Amend the last commit, add files or extensions to `.gitignore`
- Shows whether your next commit will be signed; never creates an unsigned commit when signing is required

**History and sync**
- Commit graph of all branches with colored lanes and ref labels, paged for large histories
- Commit details: full message, signature status, changed files and their diffs
- Branches: create, switch (with stash-and-reapply when local changes are in the way), rename, delete, check out remote branches, publish new ones
- Fetch, pull (fast-forward, or Merge / Rebase when branches diverged) and push, with progress and cancel
- Conflicts are shown with Abort / Continue; force push is only offered after amending a pushed commit, and is protected by a lease

## Install

Download the latest build from the [Releases](../../releases) page.

| Platform | File |
|---|---|
| macOS (Apple Silicon) | `RetroGit-macos-arm64.zip` |
| Windows (x86-64) | `RetroGit-windows-x64.zip` |

The binaries are not code-signed yet:

- **macOS**: unzip, then right-click `retrogit` → **Open** the first time, or run
  `xattr -d com.apple.quarantine retrogit`.
- **Windows**: if SmartScreen appears, click **More info** → **Run anyway**.

[Git](https://git-scm.com/) should be installed: RetroGit uses it for commits, branches and
network operations (so your hooks, signing, SSH keys and credential helpers keep working).
Without it, RetroGit can still browse history and stage files.

## Sign in to GitHub

1. Open RetroGit: the **Sign in to GitHub** window appears.
2. **Standard** tab → **Sign in**: a code is shown. Click **Open browser**, enter the code and
   authorize RetroGit (and the organizations you need, if they use SSO).
3. Or use the **Advanced** tab to paste a personal access token (classic, scopes `repo` and
   `read:org`, authorized for SSO if needed).

**Organizations that restrict OAuth apps.** Some organizations block third-party OAuth apps
until an owner approves them. RetroGit then cannot list or clone that organization's
repositories with its sign-in. For fetch, pull and push on repositories you already have,
RetroGit automatically retries with your own git credentials (Keychain, credential manager),
like `git` in a terminal. To use everything, ask an organization owner to approve RetroGit,
or sign in with a personal access token.

## Build from source

Requirements: [Rust](https://rustup.rs/) 1.95 or newer.

```bash
git clone https://github.com/MadjidSahki/RetroGit.git
cd RetroGit
cargo build --release -p retrogit
./target/release/retrogit
```

To use your own GitHub OAuth App (Device Flow enabled, no client secret needed):

```bash
RETROGIT_GITHUB_CLIENT_ID=<your client id> cargo build --release -p retrogit
```

## Architecture

A Cargo workspace with four crates:

| Crate | Role |
|---|---|
| `crates/win95` | Windows 95 widget kit for [egui](https://github.com/emilk/egui): bevels, buttons, title bar, list view, dialogs, tabs, combo box, splitter… |
| `crates/gitcore` | RetroGit's Git API: status, diff, line-level staging, commit, history and graph layout, branches, fetch/pull/push. Reads with libgit2 ([git2](https://github.com/rust-lang/git2-rs)), writes with the `git` command line |
| `crates/github` | GitHub REST client, OAuth Device Flow, token storage |
| `crates/app` | The application: state (a pure reducer), one background worker thread, and the screens |

The UI thread never blocks: it sends commands to a single worker thread and applies the
events it sends back. When RetroGit runs `git` for a github.com remote, it hands the token
to git through `GIT_ASKPASS` (the RetroGit binary itself answers), never on the command line.

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Tests use temporary repositories, a local bare remote and a mock HTTP server: no network
access is needed. Tests that need the `git` command line are skipped when it is not installed.

Every push, on any branch, is built and tested on macOS and Windows. `main` is protected:
changes land through pull requests only, and each merge into `main` publishes a new release
`v<major>.<minor>.<run number>` with both binaries (only if both platforms pass).

## Credits

Syntax highlighting uses [syntect](https://github.com/trishume/syntect) with the syntax
definitions and theme of [bat](https://github.com/sharkdp/bat) (through
[two-face](https://codeberg.org/CosmicHarper/two-face)). The pixel font is
[W95FA](https://fontsarena.com/w95fa-by-alina-sava/) by Alina Sava (SIL OFL 1.1).
Licenses of embedded third-party data: [THIRD_PARTY.md](THIRD_PARTY.md).

## Known limitations

- Pull requests are not supported yet (planned).
- Binaries are not code-signed or notarized.
- Conflicts are resolved in your editor (RetroGit shows them and lets you abort or continue).
- History pages are recomputed on each reload; very large repositories (100k+ commits) may feel slow.
