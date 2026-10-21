// Screenshots and logs go to a private repo, never into the public issue. The relay
// writes them with its own token in one commit (Git Data API) under issues/<number>/,
// after the issue exists, so a refused report never leaves files behind.

import { discard, github, GitHubError, githubJson, repoPath } from "./github.ts";

export interface Attachment {
  path: string;
  bytes: Buffer;
}

// Another report committed between our read of the branch and our update: try again.
const ATTEMPTS = 3;

function refPath(branch: string): string {
  return branch.split("/").map(encodeURIComponent).join("/");
}

/** Commits `files` to the default branch of `repo` in one commit. Throws when it cannot. */
export async function commitFiles(repo: string, token: string, files: Attachment[], message: string): Promise<void> {
  if (files.length === 0) return;
  const base = repoPath(repo);
  const { default_branch } = await githubJson<{ default_branch: string }>("attachments repo", base, { token });
  const branch = refPath(default_branch);

  // Blobs do not depend on the branch head, so they are uploaded once.
  const tree: Array<{ path: string; mode: string; type: string; sha: string }> = [];
  for (const file of files) {
    const blob = await githubJson<{ sha: string }>("attachment blob", `${base}/git/blobs`, {
      token,
      method: "POST",
      body: { content: file.bytes.toString("base64"), encoding: "base64" },
    });
    tree.push({ path: file.path, mode: "100644", type: "blob", sha: blob.sha });
  }

  for (let attempt = 1; ; attempt++) {
    const head = await githubJson<{ object: { sha: string } }>("attachments ref", `${base}/git/ref/heads/${branch}`, { token });
    const parent = await githubJson<{ tree: { sha: string } }>("attachments commit", `${base}/git/commits/${head.object.sha}`, { token });
    const newTree = await githubJson<{ sha: string }>("attachments tree", `${base}/git/trees`, {
      token,
      method: "POST",
      body: { base_tree: parent.tree.sha, tree },
    });
    const commit = await githubJson<{ sha: string }>("attachments commit", `${base}/git/commits`, {
      token,
      method: "POST",
      body: { message, tree: newTree.sha, parents: [head.object.sha] },
    });
    const res = await github(`${base}/git/refs/heads/${branch}`, { token, method: "PATCH", body: { sha: commit.sha } });
    discard(res);
    if (res.ok) return;
    // 422: not a fast-forward, the branch moved.
    if (res.status !== 422 || attempt >= ATTEMPTS) throw new GitHubError("update attachments ref", res.status);
  }
}
