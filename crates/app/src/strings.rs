//! Every user-visible string, in one place.

pub const APP_NAME: &str = "RetroGit";

pub const MENU_FILE: &str = "File";
pub const MENU_REPOSITORY: &str = "Repository";
pub const MENU_VIEW: &str = "View";
pub const MENU_HELP: &str = "Help";
pub const SIGN_IN_MENU: &str = "Sign in...";
pub const SIGN_OUT: &str = "Sign out";
pub const CLONE_MENU: &str = "Clone...";
pub const OPEN_MENU: &str = "Open...";
pub const EXIT: &str = "Exit";
pub const REFRESH_REPO: &str = "Refresh";
pub const ABOUT_MENU: &str = "About RetroGit";

pub const CLONE: &str = "Clone";
pub const OPEN: &str = "Open";
pub const FETCH: &str = "Fetch";
pub const PULL: &str = "Pull";
pub const PUSH: &str = "Push";

pub const REPOSITORIES: &str = "Repositories";
pub const REMOVE_FROM_LIST: &str = "Remove from list";
pub const NO_REPO: &str = "No repository open. Use File > Clone... or File > Open...";
pub const FOLDER_MISSING: &str = "This folder no longer exists.";
pub const BRANCH: &str = "Branch:";
pub const DETACHED: &str = "(detached)";
pub const NO_COMMITS: &str = "(no commits yet)";
pub const REMOTE: &str = "Remote:";
pub const NO_REMOTE: &str = "(none)";
pub const LAST_COMMIT: &str = "Last commit:";
pub const READY: &str = "Ready";
pub const NOT_SIGNED_IN: &str = "Not signed in";
pub const CHECKING: &str = "Checking GitHub session...";
pub const OFFLINE: &str = "Offline - GitHub unreachable";

pub const SIGN_IN_TITLE: &str = "Sign in to GitHub";
pub const TAB_STANDARD: &str = "Standard";
pub const TAB_ADVANCED: &str = "Advanced";
pub const SIGN_IN_INTRO: &str =
    "RetroGit will show a code. Enter it on github.com to authorize this computer.";
pub const SIGN_IN_BUTTON: &str = "Sign in";
pub const DEVICE_GO_TO: &str = "Go to:";
pub const DEVICE_ENTER_CODE: &str = "and enter this code:";
pub const COPY_CODE: &str = "Copy code";
pub const OPEN_BROWSER: &str = "Open browser";
pub const WAITING_AUTH: &str = "Waiting for authorization...";
pub const PAT_LABEL: &str = "Personal access token:";
pub const PAT_HELP: &str = "Use a token with 'repo' and 'read:org' scopes, authorized for SSO.";
pub const OK: &str = "OK";
pub const CANCEL: &str = "Cancel";

pub const CLONE_TITLE: &str = "Clone a repository";
pub const FILTER: &str = "Filter:";
pub const REFRESH: &str = "Refresh";
pub const LOADING: &str = "Loading repositories...";
pub const COL_NAME: &str = "Name";
pub const COL_OWNER: &str = "Owner";
pub const COL_PRIVATE: &str = "Private";
pub const COL_UPDATED: &str = "Updated";
pub const YES: &str = "Yes";
pub const DEST_FOLDER: &str = "Destination folder:";
pub const BROWSE: &str = "Browse...";
pub const WILL_CLONE_INTO: &str = "Will be cloned into:";
pub const CLONING_TITLE: &str = "Cloning";
pub const RECEIVING: &str = "Receiving objects:";

pub const ABOUT_TITLE: &str = "About RetroGit";
pub const ABOUT_TAGLINE: &str = "A Git client with a Windows 95 look.";
pub const ABOUT_FONT: &str = "Font: W95FA by Alina Sava (SIL Open Font License 1.1)";

pub const ERR_TITLE: &str = "RetroGit";
pub const ERR_NO_NETWORK: &str = "Could not reach GitHub. Check your network connection.";
pub const ERR_UNAUTHORIZED: &str = "GitHub rejected your session. Please sign in again.";
pub const ERR_PAT_REJECTED: &str = "GitHub rejected this token.";
pub const ERR_SSO: &str = "Your organization requires SSO authorization for this token. Open the link below, authorize, then try again.";
pub const ERR_SSO_PARTIAL: &str = "Some organization repositories are hidden because RetroGit is not authorized for their SSO. Open the link below, grant access (or authorize your token for SSO), then click Refresh.";
pub const ERR_RATE_LIMIT: &str = "GitHub API rate limit reached. Try again in a few minutes.";
pub const ERR_DEVICE_EXPIRED: &str = "The sign-in code expired. Click Sign in to get a new one.";
pub const ERR_DEVICE_DENIED: &str = "Authorization was denied on github.com.";
pub const ERR_NO_CLIENT_ID: &str = "This build has no GitHub OAuth App client ID. Use the Advanced tab to paste a personal access token.";
pub const ERR_DEST_NOT_EMPTY: &str = "The destination folder already exists and is not empty.";
pub const ERR_NOT_A_REPO: &str = "This folder is not a Git repository.";
pub const ERR_GIT_AUTH: &str = "GitHub refused access to this repository. Check that your token is authorized for the organization (SSO).";
pub const ERR_KEYCHAIN: &str = "Could not access the system credential store.";
pub const ERR_INTERNAL: &str = "An internal error occurred. Details were written to the log file.";
pub const INFO_CLONE_CANCELLED: &str = "Clone cancelled.";
