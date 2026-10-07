//! Tags: list, create, delete, push.

use std::sync::atomic::AtomicBool;

use crate::NetProgress;
use crate::net::NetAuth;
use crate::remote::retry_without_token;
use crate::{GitError, Refusal, Repo};

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
            return Err(GitError::Refused(Refusal::InvalidTagName(name.to_string())));
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
        mut progress: impl FnMut(NetProgress),
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        let args = ["push", "--progress", "origin", refspec];
        retry_without_token(auth, |a| self.run_net(a, &args, &mut progress, cancel)).map(|_| ())
    }

    pub fn push_tag(
        &self,
        auth: &NetAuth,
        name: &str,
        progress: impl FnMut(NetProgress),
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        self.push_refspec(auth, &format!("refs/tags/{name}"), progress, cancel)
    }

    pub fn push_tags(
        &self,
        auth: &NetAuth,
        mut progress: impl FnMut(NetProgress),
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        let args = ["push", "--progress", "--tags", "origin"];
        retry_without_token(auth, |a| self.run_net(a, &args, &mut progress, cancel)).map(|_| ())
    }

    /// Every tag of origin, also those on commits no branch holds (a plain fetch only
    /// follows tags of the commits it brings). A local tag is never overwritten nor
    /// deleted: tags that differ on origin give `TagsDiffer`, the others still arrive.
    pub fn fetch_tags(
        &self,
        auth: &NetAuth,
        mut progress: impl FnMut(NetProgress),
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        // `--no-prune`: a user's `fetch.pruneTags` would delete the tags never pushed.
        let args = ["fetch", "--tags", "--no-prune", "--progress", "origin"];
        retry_without_token(auth, |a| {
            self.run_net(a, &args, &mut progress, cancel)
                .map_err(|e| match e {
                    GitError::Other(out) => match clobbered_tags(&out) {
                        tags if tags.is_empty() => GitError::Other(out),
                        tags => GitError::TagsDiffer(tags),
                    },
                    other => other,
                })
        })
        .map(|_| ())
    }

    pub fn delete_remote_tag(
        &self,
        auth: &NetAuth,
        name: &str,
        progress: impl FnMut(NetProgress),
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        // A tag never pushed is already gone from origin: nothing to do.
        match self.push_refspec(auth, &format!(":refs/tags/{name}"), progress, cancel) {
            Err(GitError::Other(out)) if out.contains("remote ref does not exist") => Ok(()),
            other => other,
        }
    }
}

/// Tags git refused to update: ` ! [rejected]  v1  -> v1  (would clobber existing tag)`.
fn clobbered_tags(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|l| l.contains("would clobber existing tag"))
        .filter_map(|l| {
            let after = l.split_once(']')?.1;
            // " -> " with spaces: a tag name may hold "->" but never a space.
            let name = after.split_once(" -> ")?.0.trim();
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::clobbered_tags;

    #[test]
    fn rejected_tags_are_read_whole_even_with_an_arrow_in_their_name() {
        let out = "From /origin\n * [new tag]  v2 -> v2\n ! [rejected]        x->y       -> x->y  (would clobber existing tag)\n ! [rejected]  v1 -> v1  (would clobber existing tag)\n";
        assert_eq!(clobbered_tags(out), ["x->y", "v1"]);
    }
}
