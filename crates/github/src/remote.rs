//! The engine's `Remote` on the client (v2-Class). Files cross as blobs: only paths whose
//! blob sha changed are downloaded (kept in memory by sha), and a push is blobs, one tree on
//! the parent's tree, one commit, then a non-forced ref move.
use crate::{Client, Error};
use claude_sync_engine::plan::FileSet;
use claude_sync_engine::traits::{Head, IoError, IoResult, Remote};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

pub struct GithubRemote {
    client: Client,
    owner: String,
    repo: String,
    branch: String,
    /// Blob sha to bytes, for the paths of the last tree read or pushed.
    blobs: Mutex<HashMap<String, Vec<u8>>>,
}

impl GithubRemote {
    pub fn new(client: Client, owner: &str, repo: &str, branch: &str) -> Self {
        GithubRemote {
            client,
            owner: owner.into(),
            repo: repo.into(),
            branch: branch.into(),
            blobs: Mutex::default(),
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<u8>>> {
        self.blobs.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn io(e: Error) -> IoError {
    IoError(e.to_string())
}

impl Remote for GithubRemote {
    fn head(&self, etag: Option<&str>) -> IoResult<Option<Head>> {
        let (o, r) = (&self.owner, &self.repo);
        self.client.head(o, r, &self.branch, etag).map_err(io)
    }

    fn files(&self, sha: &str, _known: &BTreeMap<String, String>) -> IoResult<FileSet> {
        let (o, r) = (&self.owner, &self.repo);
        let tree = self.client.tree(o, r, sha, true).map_err(io)?;
        if tree.truncated {
            return Err(IoError("the repo is too large to read in one piece".into()));
        }
        let mut out = FileSet::new();
        let mut kept = HashMap::new();
        for e in tree.tree.iter().filter(|e| e.kind == "blob") {
            let Some(bsha) = &e.sha else { continue };
            let have = self.cache().get(bsha).cloned();
            let bytes = match have {
                Some(b) => b,
                None => self.client.blob(o, r, bsha).map_err(io)?,
            };
            kept.insert(bsha.clone(), bytes.clone());
            out.insert(e.path.clone(), bytes);
        }
        *self.cache() = kept;
        Ok(out)
    }

    fn push(
        &self,
        parent: &str,
        pushes: &BTreeMap<String, Option<Vec<u8>>>,
        message: &str,
    ) -> IoResult<String> {
        let (o, r) = (&self.owner, &self.repo);
        if parent.is_empty() {
            return Err(IoError("the repo has no commits yet".into()));
        }
        let base = self.client.tree(o, r, parent, false).map_err(io)?.sha;
        let mut entries = Vec::new();
        for (path, bytes) in pushes {
            let sha = match bytes {
                Some(b) => {
                    let s = self.client.create_blob(o, r, b).map_err(io)?;
                    self.cache().insert(s.clone(), b.clone());
                    json!(s)
                }
                None => json!(null),
            };
            entries.push(json!({"path": path, "mode": "100644", "type": "blob", "sha": sha}));
        }
        let tree = self.client.create_tree(o, r, &base, &entries).map_err(io)?;
        let commit = self
            .client
            .create_commit(o, r, message, &tree, parent)
            .map_err(io)?;
        self.client
            .update_ref(o, r, &self.branch, &commit)
            .map_err(io)?;
        Ok(commit)
    }
}
