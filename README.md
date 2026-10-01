# RetroGit

**A fast, lightweight Git client for macOS and Windows — with the look and feel of Windows 95.**

RetroGit is a native desktop Git client written in Rust. It is built for daily use:
browse and clone your GitHub repositories, stage exactly the lines you want, commit with
your hooks and signature, explore history on a commit graph, sync with GitHub, and
review and merge pull requests — all in grey bevelled windows, a blue gradient title bar
and a pixel font.

> Screenshot: _coming soon_

- Single native binary (~16 MB), no webview, no runtime
- Idle CPU ≈ 0 % (redraws only when something changes)
- macOS (Apple Silicon) and Windows (x86-64)

## Features

**GitHub accounts**
- Sign in with GitHub in your browser (OAuth Device Flow), or paste a personal access token
- Several accounts at once (e.g. personal and work): **File > Accounts...** to add or remove them
- Each repository uses the right account automatically (one that can see it: its owner, a
  member of its organization, or the first that has access), shown in the status bar and
  changeable with **Repository > Account...**; pull requests, fetch, pull and push all use it
- Tokens are stored in the macOS Keychain / Windows Credential Manager (one entry per
  account), never on disk, never in logs
- List the repositories of all your accounts in one list (with the accounts that see each),
  filter, and clone with progress and cancel — or clone from an HTTPS URL (github.com SSH
  URLs are cloned over HTTPS with the right account)
- SSO-aware: tells you when an organization needs you to authorize the app, with the link
- Organizations that restrict third-party OAuth apps: fetch, pull and push retry with your own git
  credentials; pull requests use the [GitHub CLI](https://cli.github.com/) token of the same account

**Local work**
- Status with staged / unstaged / untracked files, refreshed automatically when files change
- Unified diff with syntax highlighting (200+ languages) and per-line check boxes: stage or unstage a whole file, a hunk or single lines
- Discard changes (file, hunk or lines), with confirmation; untracked files go to the trash
- Commit through your installed `git`: hooks (`pre-commit`, husky, …) and GPG/SSH signing just work
- Amend the last commit, add files or extensions to `.gitignore`
- Shows whether your next commit will be signed; never creates an unsigned commit when signing is required

**History and sync**
- Commit graph of all branches with colored lanes and ref labels, paged for large histories
- Commit details: full message, signature status, changed files next to the colored diff of the selected file
- Branches: create, switch (with stash-and-reapply when local changes are in the way), rename, delete, check out remote branches, publish new ones
- Fetch (also automatically when a repository is opened), pull (fast-forward, or Merge / Rebase when branches diverged) and push, with progress and cancel
- Conflicts are shown with Abort / Continue; force push is only offered after amending a pushed commit, and is protected by a lease
- Resolve conflicts in RetroGit (merge, pull, rebase, stash and switch): conflicted files are listed apart in the Changes tab and open in a three-pane editor (Mine | Result | Theirs) with syntax colors
  - per block: Use mine / theirs / both, Previous / Next, conflicts left; or keep one side for the whole file
  - the result stays freely editable; Mark resolved stages it and opens the next conflicted file, then tells you to commit the merge or continue the rebase
  - the sides are named after what they are: upstream / my commit during a rebase, current / my stash after a stash
  - binary files and files deleted on one side offer the simple choices
  - your edits are never lost silently: changes on disk are offered with Reload, and leaving the file, switching repository or aborting asks first

**Pull requests** (github.com repositories)
- List open, mine, review-requested or closed pull requests, with checks, review status, labels, author, target branch and age
- Read the description (Markdown) and the conversation, the commits, the changed files with colored diffs and their line comments, and the checks with links to their runs
- Create a pull request from the current branch: title, description, target branch, draft, labels, and publish the branch first if needed (an existing pull request for the branch is opened instead)
- Comment on diff lines (right-click a line): post the comment at once, or add it to a review
- Review: comment, approve or request changes (approving your own pull request is disabled, as on GitHub); reply to threads; resolve or unresolve conversations
- Comment in the conversation; add and remove labels
- Merge, squash or rebase (as the repository allows), with editable commit title and message and optional branch deletion; the button is disabled with the reason when GitHub would refuse (draft, conflicts, required reviews or checks, out of date, no permission)
- Check out a pull request: as its branch when it lives in the repository (brought up to date), or as `pr/<number>` for forks

**Notifications**
- While RetroGit is open, the pull requests of all your accounts are checked every 2 minutes: checks passed or failed, review received, new comment, merged or closed
- Shown as system notifications and in the **Notifications** list (toolbar), which opens the pull request in its local clone, or on github.com

**Everyday comfort**
- **Open in IDE**: VS Code, Cursor, Visual Studio, Rider, IntelliJ IDEA, WebStorm, PyCharm, GoLand, RustRover, Zed, Sublime Text, Xcode — detected automatically, choice remembered per repository
- **`retrogit` command**: open the repository of the current folder from a terminal, in the window already open
- Recent repositories list; window size and position remembered

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

## Open from a terminal and in your IDE

**`retrogit` command.** Use **File → Install command line tool…** once (macOS asks for your
administrator password to create `/usr/local/bin/retrogit`; Windows adds the command to your
user `PATH`). Then, from any folder of a repository:

```bash
retrogit          # opens the repository of the current folder
retrogit ../other # or another folder
```

If RetroGit is already open, the repository opens in that window (and is added to the
repository list); otherwise RetroGit starts. The terminal is not blocked.

**Open in IDE.** The toolbar button opens the repository in your editor. RetroGit detects
installed IDEs (VS Code, Cursor, Visual Studio, Rider, IntelliJ IDEA, WebStorm, PyCharm,
GoLand, RustRover, Zed, Sublime Text, Xcode) and remembers your choice for each repository.

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
like `git` in a terminal. For pull requests, RetroGit uses the token of the
[GitHub CLI](https://cli.github.com/) when it is installed and signed in (`gh auth login`),
since organizations usually approve it; the token is read from `gh` when needed and never
stored. Otherwise, ask an organization owner to approve RetroGit, or sign in with a personal
access token.

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
| `crates/github` | GitHub client: REST (writes) and GraphQL (pull request reads), OAuth Device Flow, token storage, pull request watch |
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

`main` is protected: changes land through pull requests only. Every pull request is built
and tested on macOS and Windows; each merge into `main` is built again and publishes a new
release `v<major>.<minor>.<run number>` with both binaries (only if both platforms pass).

## Credits

Syntax highlighting uses [syntect](https://github.com/trishume/syntect) with the syntax
definitions and theme of [bat](https://github.com/sharkdp/bat) (through
[two-face](https://codeberg.org/CosmicHarper/two-face)). The pixel font is
[W95FA](https://fontsarena.com/w95fa-by-alina-sava/) by Alina Sava (SIL OFL 1.1).
Licenses of embedded third-party data: [THIRD_PARTY.md](THIRD_PARTY.md).

## Known limitations

**Accounts and hosting**
- The account of a repository is used for GitHub and Git network operations only: the
  author of your commits is still your Git configuration (`user.name`, `user.email`).
- github.com only: no GitHub Enterprise Server, GitLab or Bitbucket for sign-in, repository
  lists and pull requests (plain Git operations work with any remote).
- Without an account, the Pull Requests tab is unavailable (the GitHub CLI is only used for
  your signed-in accounts). The GitHub CLI fallback needs `gh` 2.40 or newer, signed in
  with the same account (`gh auth login`).

**Pull requests**
- Not supported: editing the title or description, reviewers, assignees, milestones, marking a
  draft as ready, reactions, multi-line comments, suggested changes.
- Lists are limited to the 50 most recently updated pull requests, and a pull request to its
  first 100 comments, reviews, review threads, commits and checks (repository labels: 100).
- Markdown is simplified: no HTML, tables or inline images (images are shown as links).
- Line comments waiting for a review are dropped when you select another pull request.
- Commits of a fork's pull request open in History only after it has been checked out.
- Checking out a pull request shows no progress and cannot be cancelled.

**Notifications**
- Only while RetroGit is open (no catch-up of what happened while it was closed).
- macOS: they appear under "Script Editor", and clicking them does not open RetroGit (use the
  Notifications list). Windows: they appear under "Windows PowerShell".

**General**
- Cloning works over HTTPS only (github.com SSH URLs are converted); fetch, pull and push
  use your remotes as they are, SSH included.
- Binaries are not code-signed or notarized. No Linux build.
- The conflict editor has no word-level merge (blocks are Git's), no syntax colors above 5,000 lines, and inserts LF line endings when you type in a CRLF file.
- Syntax highlighting is skipped for very large diffs (over 5,000 lines or 256 KB) and diffs
  over 20,000 lines are only shown on request.
- History pages are recomputed on each reload; very large repositories (100k+ commits) may feel slow.
