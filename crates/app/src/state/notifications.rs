//! Pull request notifications received while RetroGit is open.

use std::path::{Path, PathBuf};

use github::PrEvent;

use super::AppState;
use crate::protocol::Slug;

/// Older notifications are dropped beyond this.
pub const MAX_NOTIFICATIONS: usize = 50;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NotificationsView {
    /// Newest first.
    pub items: Vec<PrEvent>,
    pub unread: usize,
    /// The list dialog is shown.
    pub open: bool,
}

/// Where clicking a notification leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationTarget {
    /// The pull request belongs to the open repository.
    Current(u64),
    /// Open this local clone, then the pull request.
    Local(PathBuf, u64),
    /// Not cloned here: github.com.
    Browser(String),
}

/// `"owner/repo"` => `(owner, repo)`.
pub fn split_repo(full: &str) -> Option<Slug> {
    let (o, r) = full.split_once('/')?;
    Some((o.to_string(), r.to_string()))
}

impl AppState {
    pub fn add_notifications(&mut self, events: Vec<PrEvent>) {
        let n = &mut self.notifications;
        n.unread += events.len();
        for e in events {
            n.items.insert(0, e);
        }
        n.items.truncate(MAX_NOTIFICATIONS);
        n.unread = n.unread.min(n.items.len());
    }

    pub fn open_notifications(&mut self) {
        self.notifications.open = true;
        self.notifications.unread = 0;
    }

    /// Where `e` leads; `slug_of` reads the github.com repository of a local clone.
    pub fn notification_target(
        &self,
        e: &PrEvent,
        slug_of: impl Fn(&Path) -> Option<Slug>,
    ) -> NotificationTarget {
        let Some(slug) = split_repo(&e.repo) else {
            return NotificationTarget::Browser(e.url.clone());
        };
        let same =
            |s: &Slug| s.0.eq_ignore_ascii_case(&slug.0) && s.1.eq_ignore_ascii_case(&slug.1);
        if self.github_slug().as_ref().is_some_and(same) {
            return NotificationTarget::Current(e.number);
        }
        for r in self.recents_sorted() {
            if !self.missing.contains(&r.path) && slug_of(&r.path).as_ref().is_some_and(same) {
                return NotificationTarget::Local(r.path, e.number);
            }
        }
        NotificationTarget::Browser(e.url.clone())
    }

    /// Show pull request `number` of the open repository (loads it).
    pub fn show_pull(&mut self, number: u64) {
        self.tab = super::Tab::PullRequests;
        self.pulls.select(number);
        self.pulls.load_selected = true;
    }
}
