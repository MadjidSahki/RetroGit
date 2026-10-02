//! Tags: list, create, delete, push.

use std::sync::atomic::AtomicBool;

use crate::net::NetAuth;
use crate::remote::retry_without_token;
use crate::{GitError, Repo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    /// Commit the tag points to.
    pub commit: String,
    pub annotated: bool,
    /// First line of the annotation.
    pub message: String,
}

impl Repo {
    /// Tags by name.
    pub fn tags(&self) -> Result<Vec<Tag>, GitError> {
        let out = self.git_ok(&[
            "for-each-ref",
            "--sort=refname",
            "--format=%(refname:short)%00%(objecttype)%00%(*objectname)%00%(objectname)%00%(contents:subject)",
            "refs/tags",
        ])?;
        Ok(out
            .stdout
            .lines()
            .filter_map(|l| {
                let p: Vec<&str> = l.split('\0').collect();
                let annotated = *p.get(1)? == "tag";
                let commit = if annotated { p.get(2)? } else { p.get(3)? };
                Some(Tag {
                    name: p.first()?.to_string(),
                    commit: commit.to_string(),
                    annotated,
                    message: if annotated {
                        p.get(4).unwrap_or(&"").to_string()
                    } else {
                        String::new()
                    },
                })
            })
            .collect())
    }

    /// Tag commit `id` as `name`; annotated when `message` is given.
    pub fn create_tag(&self, name: &str, id: &str, message: Option<&str>) -> Result<(), GitError> {
        let check = self.run_git(&["check-ref-format", &format!("refs/tags/{name}")])?;
        if !check.success || name.trim() != name || name.is_empty() {
            return Err(GitError::Unsupported(format!(
                "'{name}' is not a valid tag name"
            )));
        }
        let mut args = vec!["tag"];
        if let Some(m) = message {
            args.extend(["-a", "-m", m]);
        }
        args.extend(["--", name, id]);
        self.git_ok(&args).map(|_| ())
    }

    pub fn delete_tag(&self, name: &str) -> Result<(), GitError> {
        self.git_ok(&["tag", "-d", name]).map(|_| ())
    }

    fn push_refspec(
        &self,
        auth: &NetAuth,
        refspec: &str,
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        let args = ["push", "--progress", "origin", refspec];
        retry_without_token(auth, |a| self.run_net(a, &args, |_| {}, cancel)).map(|_| ())
    }

    pub fn push_tag(
        &self,
        auth: &NetAuth,
        name: &str,
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        self.push_refspec(auth, &format!("refs/tags/{name}"), cancel)
    }

    pub fn push_tags(&self, auth: &NetAuth, cancel: &AtomicBool) -> Result<(), GitError> {
        let args = ["push", "--progress", "--tags", "origin"];
        retry_without_token(auth, |a| self.run_net(a, &args, |_| {}, cancel)).map(|_| ())
    }

    pub fn delete_remote_tag(
        &self,
        auth: &NetAuth,
        name: &str,
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        self.push_refspec(auth, &format!(":refs/tags/{name}"), cancel)
    }
}
