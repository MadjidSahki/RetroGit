//! Messages between the UI thread and the worker thread.

use std::path::PathBuf;

use gitcore::{
    Branch, CloneProgress, CommitDetail, CommitOutcome, FileDiff, FileStatus, GitError, LogEntry,
    NetProgress, Operation, PullMode, PullOutcome, PushMode, RepoSummary, Selection, Side,
    SignatureStatus, SigningConfig,
};
use github::{
    DeviceFlowFailure, GithubError, Merge, NewPull, PrDetail, PrFile, PrFilter, PrSummary,
    RepoInfo, RepoMeta, Review, TokenStoreError, User,
};

use crate::strings as s;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    ValidateToken,
    StartDeviceFlow,
    SavePat(String),
    RemoveAccount(String),
    /// Use `login` for `slug` from now on (`None`: choose automatically again).
    SetRepoAccount {
        slug: Slug,
        login: Option<String>,
    },
    ListRepos,
    /// `account`: the account to clone with (from the repository list); `None`: the
    /// account of the URL's github.com repository, else the user's git credentials.
    Clone {
        url: String,
        dest: PathBuf,
        account: Option<String>,
    },
    OpenRepo(PathBuf),
    // --- Sub-project 2: all apply to the repository opened last. ---
    RefreshStatus,
    LoadDiff {
        path: String,
        side: Side,
    },
    /// `shown` is the diff the selection was made on (stale-selection check).
    Stage {
        path: String,
        selection: Selection,
        shown: Option<FileDiff>,
    },
    Unstage {
        path: String,
        selection: Selection,
        shown: Option<FileDiff>,
    },
    /// Revert unstaged working-tree changes (needs a confirmation in the UI).
    Discard {
        path: String,
        selection: Selection,
        shown: Option<FileDiff>,
    },
    /// Whole files back to their index version; untracked files go to the trash.
    DiscardFiles(Vec<String>),
    /// Whole files, one index operation and one refresh (Stage all / Unstage all).
    StageFiles(Vec<String>),
    UnstageFiles(Vec<String>),
    Commit {
        message: String,
        amend: bool,
    },
    AddToGitignore(String),
    /// Reply: `Event::AmendInfo`.
    LoadAmendInfo,
    // --- Sub-project 3 ---
    /// Next page of history (`skip` = number of entries already loaded).
    LoadLog {
        skip: usize,
    },
    LoadCommit(String),
    LoadCommitFileDiff {
        id: String,
        path: String,
    },
    LoadBranches,
    CreateBranch {
        name: String,
        switch: bool,
    },
    /// Local branch, or `origin/x` (creates the tracking branch). `stash`: put local changes
    /// aside, switch, then re-apply them.
    SwitchBranch {
        name: String,
        stash: bool,
    },
    RenameBranch {
        old: String,
        new: String,
    },
    DeleteBranch {
        name: String,
        force: bool,
    },
    /// `background`: automatic fetch at open (errors are only logged).
    Fetch {
        background: bool,
    },
    Pull(PullMode),
    Push(PushMode),
    /// Force-push after amending a pushed commit, leased on the remote commit recorded then.
    ForcePush,
    AbortOperation,
    ContinueRebase,
    // --- Sub-project 6c ---
    CherryPick(String),
    /// `mainline`: parent to keep when reverting a merge (1-based).
    Revert {
        id: String,
        mainline: Option<u32>,
    },
    Reset {
        id: String,
        mode: gitcore::ResetMode,
    },
    /// Whether resetting to `id` drops pushed commits (for the Reset dialog).
    LoadResetInfo(String),
    /// Commits after `base` (`None`: the branch's upstream).
    LoadRebaseList(Option<String>),
    InteractiveRebase {
        base: String,
        items: Vec<gitcore::TodoItem>,
    },
    /// Continue / skip the cherry-pick, revert or rebase in progress.
    ContinueOperation,
    SkipOperation,
    LoadStashes,
    StashSave {
        message: String,
        untracked: bool,
    },
    /// Stash actions name the stash by index and id: refused if the list changed.
    StashApply {
        index: usize,
        id: String,
    },
    StashPop {
        index: usize,
        id: String,
    },
    StashDrop {
        index: usize,
        id: String,
    },
    LoadStashFiles(usize),
    LoadStashFileDiff {
        index: usize,
        path: String,
    },
    LoadTags,
    CreateTag {
        name: String,
        id: String,
        message: Option<String>,
    },
    /// `remote`: delete it on origin too.
    DeleteTag {
        name: String,
        remote: bool,
    },
    /// One tag, or all (`None`).
    PushTags(Option<String>),
    /// Stash the local changes (untracked included), then run the blocked command again.
    StashAndRetry(Box<Command>),
    // --- Sub-project 6a: conflicts of the open repository. ---
    LoadConflict(String),
    /// Write `content` as the resolution of `path` and mark it resolved.
    ResolveConflict {
        path: String,
        content: String,
    },
    /// Keep one side's whole version of `path`.
    ResolveConflictWith {
        path: String,
        pick: gitcore::Pick,
    },
    /// Resolve by deleting `path`.
    ResolveDelete(String),
    // --- Sub-project 4: pull requests of the github.com repository `slug`. ---
    LoadPulls {
        slug: Slug,
        filter: PrFilter,
    },
    /// Detail, then files.
    LoadPull {
        slug: Slug,
        number: u64,
    },
    /// The detail only (checks running): the files and the user's selection stay.
    RefreshPull {
        slug: Slug,
        number: u64,
    },
    /// Default branch and labels, for the "New pull request" dialog.
    LoadRepoMeta(Slug),
    /// `publish`: push the head branch (`push -u origin`) first.
    CreatePull {
        slug: Slug,
        pull: NewPull,
        publish: bool,
    },
    SubmitReview {
        slug: Slug,
        number: u64,
        review: Review,
    },
    ReplyToThread {
        slug: Slug,
        number: u64,
        comment_id: u64,
        body: String,
    },
    AddPullComment {
        slug: Slug,
        number: u64,
        body: String,
    },
    ResolveThread {
        slug: Slug,
        number: u64,
        thread_id: String,
        resolve: bool,
    },
    UpdatePull {
        slug: Slug,
        number: u64,
        title: String,
        body: String,
    },
    SetPeople {
        slug: Slug,
        number: u64,
        kind: crate::state::PeopleKind,
        add: Vec<String>,
        remove: Vec<String>,
    },
    /// `pull_id`: the GraphQL id of the pull request.
    SetDraft {
        slug: Slug,
        number: u64,
        pull_id: String,
        draft: bool,
    },
    LoadAssignable {
        slug: Slug,
        query: String,
    },
    /// Apply a suggestion on lines `start..=end` (new side) of `path` as a commit, if the
    /// open repository is on the pull request's branch at `head_sha`.
    ApplySuggestion {
        number: u64,
        head_branch: String,
        head_sha: String,
        path: String,
        start: u32,
        end: u32,
        expected: Vec<String>,
        replacement: String,
        author: String,
    },
    /// A line comment posted at once, outside a review.
    AddLineComment {
        slug: Slug,
        number: u64,
        commit_id: String,
        comment: github::LineComment,
    },
    /// `delete_branch`: head branch to delete afterwards (same repository only).
    MergePull {
        slug: Slug,
        number: u64,
        merge: Merge,
        delete_branch: Option<String>,
    },
    SetLabels {
        slug: Slug,
        number: u64,
        labels: Vec<String>,
    },
    /// `head`: the head branch when it lives in this repository (checked out as a normal
    /// tracking branch); `None` for forks (fetched into `pr/N`).
    CheckoutPull {
        number: u64,
        head: Option<String>,
    },
}

/// `(owner, repo)` of a github.com repository.
pub type Slug = (String, String);

/// Network operation shown in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOp {
    Fetch,
    Pull,
    Push,
}

/// Which operation an error belongs to, so the state can reset the right thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Auth,
    Repos,
    Clone,
    Open(PathBuf),
    /// Status, diff, staging, .gitignore.
    Changes,
    Commit,
    /// History, branches.
    History,
    Sync,
    /// Pull request reads.
    Pulls,
    /// Pull request changes (create, review, merge...).
    PullAction,
    Internal,
}

#[derive(Debug, Clone)]
pub enum Event {
    /// An account was added, or at least one account is usable at startup.
    SignedIn(User),
    /// Every account, in order (valid or to sign in again).
    AccountsChanged(Vec<github::AccountStatus>),
    /// Account the open repository uses (`None`: none can see it).
    RepoAccount {
        slug: Slug,
        login: Option<String>,
    },
    /// Remember (or forget, `None`) the account of repository `key` (`owner/repo`,
    /// lowercase) in the config.
    RepoAccountLearned {
        key: String,
        account: Option<github::RepoAccount>,
    },
    SignedOut,
    /// A token is stored but GitHub could not be reached; it is kept for later calls.
    Offline,
    DeviceCode {
        user_code: String,
        verification_uri: String,
    },
    DeviceFlowCancelled,
    ReposLoaded(Vec<RepoInfo>),
    CloneProgress(CloneProgress),
    CloneDone(RepoSummary),
    CloneCancelled,
    RepoOpened(RepoSummary),
    StatusLoaded(Vec<FileStatus>),
    DiffLoaded(FileDiff),
    Committed(CommitOutcome),
    /// Last commit message and whether HEAD is already on its upstream.
    AmendInfo {
        message: Option<String>,
        pushed: bool,
    },
    /// A page of history; `skip` tells where it goes.
    LogLoaded {
        skip: usize,
        entries: Vec<LogEntry>,
    },
    CommitLoaded(CommitDetail),
    SignatureLoaded {
        id: String,
        status: SignatureStatus,
    },
    CommitFileDiffLoaded {
        id: String,
        diff: FileDiff,
    },
    BranchesLoaded(Vec<Branch>),
    /// Merge/rebase in progress in the open repository.
    OperationChanged(Option<Operation>),
    SigningLoaded(Option<SigningConfig>),
    SyncStarted {
        op: SyncOp,
        background: bool,
    },
    SyncProgress(NetProgress),
    /// Network operation finished (successfully, or with an `Error` sent just before).
    SyncFinished {
        op: SyncOp,
        ok: bool,
    },
    Pulled(PullOutcome),
    /// Pull needs a decision: Merge or Rebase.
    Diverged {
        ahead: usize,
        behind: usize,
    },
    /// `can_force`: HEAD rewrites a commit this branch had pushed (lease recorded).
    PushRejected {
        can_force: bool,
    },
    /// Switching is blocked by local changes to these files.
    WouldOverwrite {
        branch: String,
        files: Vec<String>,
    },
    NotMerged(String),
    /// Syntax colors computed in the background for `diff`.
    /// `dark`: computed with the dark syntax theme (dropped if the scheme changed since).
    ColorsLoaded {
        target: crate::highlight::Target,
        diff: FileDiff,
        colors: Option<crate::highlight::DiffColors>,
        dark: bool,
    },
    // --- Sub-project 4 ---
    PullsLoaded {
        slug: Slug,
        filter: PrFilter,
        list: Vec<PrSummary>,
    },
    PullLoaded {
        slug: Slug,
        detail: Box<PrDetail>,
    },
    PullFilesLoaded {
        slug: Slug,
        number: u64,
        files: Vec<PrFile>,
    },
    RepoMetaLoaded {
        slug: Slug,
        meta: RepoMeta,
    },
    /// A pull request was opened (or already existed): show it.
    PullCreated {
        slug: Slug,
        number: u64,
    },
    /// A change went through; the pull request is reloaded. `note` goes to the status bar.
    PullActionDone {
        number: u64,
        note: String,
    },
    ConflictLoaded(Box<gitcore::ConflictFile>),
    /// `path` is no longer in conflict (sent after the refreshed status).
    ConflictResolved(String),
    /// A history operation finished: done, stopped on conflicts, or empty.
    OpFinished {
        outcome: gitcore::OpOutcome,
        note: String,
    },
    /// Local changes prevent `retry` from starting (nothing was changed).
    /// Answer of the explore service for the repository at `repo`.
    ExploreLoaded {
        repo: std::path::PathBuf,
        result: ExploreResult,
    },
    /// Result of a tag action, shown in the Tags window.
    TagsStatus(String),
    OpBlocked {
        retry: Box<Command>,
        files: Vec<String>,
    },
    ResetInfo {
        id: String,
        drops_pushed: bool,
        /// Untracked files a hard reset would replace.
        overwrites: Vec<String>,
    },
    /// `pushed`: how many of `items` are already on the upstream.
    RebaseListLoaded {
        base: String,
        items: Vec<gitcore::TodoItem>,
        pushed: usize,
    },
    StashesLoaded(Vec<gitcore::StashEntry>),
    StashFilesLoaded {
        index: usize,
        files: Vec<gitcore::ChangedFile>,
    },
    StashFileDiffLoaded {
        index: usize,
        diff: gitcore::FileDiff,
    },
    TagsLoaded(Vec<gitcore::Tag>),
    AssignableLoaded {
        slug: Slug,
        users: Vec<String>,
    },
    /// Changes on the user's pull requests (from the watcher thread).
    PrEvents(Vec<github::PrEvent>),
    Error {
        during: Op,
        error: AppError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// What the message box shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    pub severity: Severity,
    pub message: String,
    /// Technical detail (already redacted), shown in small text.
    pub detail: Option<String>,
    /// Clickable link (e.g. SSO authorization page).
    pub link: Option<String>,
}

impl AppError {
    pub fn new(severity: Severity, message: &str) -> AppError {
        AppError {
            severity,
            message: message.to_string(),
            detail: None,
            link: None,
        }
    }

    fn with_detail(mut self, detail: impl ToString) -> AppError {
        self.detail = Some(detail.to_string());
        self
    }

    pub fn from_github(e: &GithubError) -> AppError {
        match e {
            GithubError::Unauthorized => AppError::new(Severity::Warning, s::ERR_UNAUTHORIZED),
            GithubError::SsoRequired { url } => {
                let mut a = AppError::new(Severity::Warning, s::ERR_SSO);
                a.link = (!url.is_empty()).then(|| url.clone());
                a
            }
            GithubError::RateLimited => AppError::new(Severity::Warning, s::ERR_RATE_LIMIT),
            GithubError::Network(d) => {
                AppError::new(Severity::Warning, s::ERR_NO_NETWORK).with_detail(d)
            }
            GithubError::OAuthRestricted { org } => AppError::new(
                Severity::Warning,
                &format!(
                    "The organization {} restricts third-party applications and has not approved RetroGit. {}",
                    org.as_deref().unwrap_or("of this repository"),
                    s::ERR_OAUTH_RESTRICTED_HELP
                ),
            ),
            GithubError::Rejected { message, .. } => AppError::new(Severity::Warning, message),
            // How GitHub hides a repository from a restricted app; other misses (a deleted
            // pull request or comment) keep GitHub's own words.
            GithubError::NotFound(m) if github::repository_missing(e) => {
                AppError::new(Severity::Warning, s::ERR_PULLS_NOT_FOUND).with_detail(m)
            }
            GithubError::NotFound(m) => AppError::new(Severity::Warning, m),
            other => AppError::new(Severity::Error, &other.to_string()),
        }
    }

    pub fn from_git(e: &GitError) -> AppError {
        match e {
            GitError::DestinationNotEmpty(p) => {
                AppError::new(Severity::Error, s::ERR_DEST_NOT_EMPTY).with_detail(p.display())
            }
            GitError::NotARepository(p) => {
                AppError::new(Severity::Error, s::ERR_NOT_A_REPO).with_detail(p.display())
            }
            GitError::Cancelled => AppError::new(Severity::Info, s::INFO_CLONE_CANCELLED),
            GitError::Auth(d) => AppError::new(Severity::Error, s::ERR_GIT_AUTH).with_detail(d),
            GitError::Network(d) => {
                AppError::new(Severity::Warning, s::ERR_NO_NETWORK).with_detail(d)
            }
            GitError::StaleSelection => AppError::new(Severity::Info, s::INFO_STALE_SELECTION),
            GitError::Unsupported(d) => AppError::new(Severity::Warning, d),
            GitError::CommitRejected { output } => {
                AppError::new(Severity::Error, s::ERR_COMMIT_REJECTED).with_detail(output)
            }
            GitError::MissingIdentity => AppError::new(Severity::Error, s::ERR_MISSING_IDENTITY),
            GitError::WouldOverwrite { files } => {
                AppError::new(Severity::Warning, s::ERR_WOULD_OVERWRITE)
                    .with_detail(files.join("\n"))
            }
            GitError::NotMerged(b) => AppError::new(
                Severity::Warning,
                &format!("Branch '{b}' is not fully merged."),
            ),
            GitError::Diverged { ahead, behind } => AppError::new(
                Severity::Info,
                &format!(
                    "Your branch and its upstream have diverged ({ahead} local and {behind} remote commits)."
                ),
            ),
            GitError::PushRejected => AppError::new(Severity::Warning, s::ERR_PUSH_REJECTED),
            GitError::StashConflict => AppError::new(Severity::Warning, s::ERR_STASH_CONFLICT),
            GitError::SigningRequiresGit => {
                AppError::new(Severity::Error, s::ERR_SIGNING_REQUIRES_GIT)
            }
            GitError::GitMissing => AppError::new(Severity::Warning, s::ERR_GIT_MISSING),
            GitError::MessageRefused { output } => {
                AppError::new(Severity::Warning, s::ERR_REBASE_MESSAGE_REFUSED).with_detail(output)
            }
            GitError::SuggestionOutdated => {
                AppError::new(Severity::Warning, s::ERR_SUGGESTION_OUTDATED)
            }
            GitError::AccessDenied(d) => {
                AppError::new(Severity::Warning, s::ERR_ACCESS_DENIED).with_detail(d)
            }
            GitError::Other(d) => AppError::new(Severity::Error, d),
        }
    }

    pub fn from_device_flow(f: &DeviceFlowFailure) -> AppError {
        match f {
            DeviceFlowFailure::Expired => AppError::new(Severity::Warning, s::ERR_DEVICE_EXPIRED),
            DeviceFlowFailure::Denied => AppError::new(Severity::Warning, s::ERR_DEVICE_DENIED),
            DeviceFlowFailure::Other(code) => {
                AppError::new(Severity::Error, &format!("GitHub sign-in failed: {code}"))
            }
        }
    }

    pub fn from_store(e: &TokenStoreError) -> AppError {
        AppError::new(Severity::Error, s::ERR_KEYCHAIN).with_detail(e)
    }
}

/// Requests to the explore service (its own threads: the worker stays free).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExploreRequest {
    Refs,
    Tree {
        rev: String,
    },
    File {
        rev: String,
        path: String,
    },
    Blame {
        rev: String,
        path: String,
    },
    FileHistory {
        rev: String,
        path: String,
    },
    FileDiff {
        commit: String,
        path: String,
    },
    Grep {
        rev: String,
        text: String,
        match_case: bool,
        paths: String,
    },
    LogSearch {
        kind: gitcore::LogSearch,
        query: String,
    },
    CancelSearch,
    CancelLogSearch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExploreResult {
    Refs(Vec<gitcore::ExploreRef>),
    Tree {
        rev: String,
        commit: String,
        entries: Vec<gitcore::TreeEntry>,
    },
    File {
        rev: String,
        path: String,
        content: gitcore::FileContent,
    },
    Blame {
        rev: String,
        path: String,
        blocks: Vec<gitcore::BlameBlock>,
    },
    FileHistory {
        rev: String,
        path: String,
        commits: Vec<gitcore::FileCommit>,
    },
    FileDiff {
        commit: String,
        path: String,
        diff: FileDiff,
    },
    Grep {
        rev: String,
        text: String,
        result: gitcore::GrepResult,
    },
    LogSearch {
        kind: gitcore::LogSearch,
        query: String,
        entries: Vec<LogEntry>,
        truncated: bool,
    },
    Failed {
        request: ExploreRequest,
        message: String,
    },
}
