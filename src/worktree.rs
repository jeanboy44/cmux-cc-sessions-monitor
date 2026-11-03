use crate::branch::BranchTarget;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const GIT_TIMEOUT: Duration = Duration::from_secs(3);
const PROC_TIMEOUT: Duration = Duration::from_secs(2);
const TRANSCRIPT_TAIL_BYTES: u64 = 200_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub is_main: bool,
}

/// Lexically resolves `.` and `..` components.
fn normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Canonicalizes `p`; if it does not exist (deleted dir, file about to be
/// created) normalizes `.`/`..` lexically, canonicalizes the deepest existing
/// ancestor and re-appends the rest.
fn canon(p: &Path) -> PathBuf {
    if let Ok(c) = p.canonicalize() {
        return c;
    }
    let p = normalize(p);
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cur = p.as_path();
    while let Some(parent) = cur.parent() {
        if let Some(name) = cur.file_name() {
            rest.push(name);
        }
        if let Ok(mut c) = parent.canonicalize() {
            for n in rest.iter().rev() {
                c.push(n);
            }
            return c;
        }
        cur = parent;
    }
    p
}

/// Runs a command via the shared bounded runner; stdout on success, None on
/// spawn failure, non-zero exit or timeout.
fn run_text(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let out = crate::exec::run_bounded(program, args, None, timeout).ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn parse_worktree_porcelain(out: &str) -> Vec<Worktree> {
    let mut result: Vec<Worktree> = Vec::new();
    for block in out.split("\n\n") {
        let mut path: Option<PathBuf> = None;
        let mut branch: Option<String> = None;
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if let Some(b) = line.strip_prefix("branch ") {
                branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
            }
        }
        if let Some(path) = path {
            let is_main = result.is_empty();
            result.push(Worktree { path: canon(&path), branch, is_main });
        }
    }
    result
}

/// Lists worktrees of the repo containing `repo_dir`. The first entry is the
/// main checkout. Any failure yields an empty Vec.
pub fn list_worktrees(repo_dir: &Path) -> Vec<Worktree> {
    let dir = repo_dir.to_string_lossy();
    match run_text("git", &["-C", &dir, "worktree", "list", "--porcelain"], GIT_TIMEOUT) {
        Some(out) => parse_worktree_porcelain(&out),
        None => Vec::new(),
    }
}

/// Finds the worktree with the longest path (by components) containing `cwd`.
/// Returns its branch name and whether it is the main checkout. A matching
/// worktree without a branch (detached HEAD) yields None.
pub fn match_branch(cwd: &Path, worktrees: &[Worktree]) -> Option<(String, bool)> {
    let cwd = canon(cwd);
    let best = worktrees
        .iter()
        .filter(|w| cwd.starts_with(canon(&w.path)))
        .max_by_key(|w| w.path.components().count())?;
    best.branch.clone().map(|b| (b, best.is_main))
}

/// First path (newest-first) inside a non-main worktree with a branch.
pub fn guess_worktree(paths: &[PathBuf], worktrees: &[Worktree]) -> Option<String> {
    let linked: Vec<(PathBuf, &str)> = worktrees
        .iter()
        .filter(|w| !w.is_main)
        .filter_map(|w| Some((canon(&w.path), w.branch.as_deref()?)))
        .collect();
    paths.iter().find_map(|p| {
        let p = canon(p);
        linked
            .iter()
            .filter(|(wp, _)| p.starts_with(wp))
            .max_by_key(|(wp, _)| wp.components().count())
            .map(|(_, b)| b.to_string())
    })
}

fn strip_token(tok: &str) -> &str {
    tok.trim_matches(|c| matches!(c, '"' | '\'' | ';' | '&' | '|' | '(' | ')'))
}

fn push_input_paths(input: &serde_json::Value, out: &mut Vec<PathBuf>) {
    for key in ["file_path", "path", "notebook_path"] {
        if let Some(s) = input.get(key).and_then(|v| v.as_str()) {
            if s.starts_with('/') {
                out.push(PathBuf::from(s));
            }
        }
    }
    if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
        for tok in cmd.split_whitespace() {
            let t = strip_token(tok);
            // `--flag=/path` yields the value after the first `=`.
            let t = match t.split_once('=') {
                Some((_, v)) if !t.starts_with('/') => strip_token(v),
                _ => t,
            };
            if t.starts_with('/') {
                out.push(PathBuf::from(t));
            }
        }
    }
}

/// Reads the last `max_bytes` of a transcript and returns absolute paths found
/// in tool_use inputs, newest first, de-duplicated. Broken lines are skipped.
pub fn recent_tool_paths(transcript: &Path, max_bytes: u64) -> Vec<PathBuf> {
    let Ok(mut file) = std::fs::File::open(transcript) else {
        return Vec::new();
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return Vec::new();
    };
    let start = len.saturating_sub(max_bytes);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if file.take(max_bytes).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut body: &str = &text;
    if start > 0 {
        // The first line is probably cut in the middle: drop it.
        body = match body.split_once('\n') {
            Some((_, rest)) => rest,
            None => "",
        };
    }
    let mut result: Vec<PathBuf> = Vec::new();
    for line in body.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(items) = v.pointer("/message/content").and_then(|c| c.as_array()) else {
            continue;
        };
        // Later tool calls are newer; paths within one call keep left-to-right order.
        for item in items.iter().rev() {
            if item.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                continue;
            }
            let mut call_paths = Vec::new();
            if let Some(input) = item.get("input") {
                push_input_paths(input, &mut call_paths);
            }
            for p in call_paths {
                if !result.contains(&p) {
                    result.push(p);
                }
            }
        }
    }
    result
}

fn pgrep_children(pid: u32) -> Vec<u32> {
    run_text("pgrep", &["-P", &pid.to_string()], PROC_TIMEOUT)
        .map(|o| o.lines().filter_map(|l| l.trim().parse().ok()).collect())
        .unwrap_or_default()
}

fn process_cwd(pid: u32) -> Option<PathBuf> {
    let out = run_text("lsof", &["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"], PROC_TIMEOUT)?;
    out.lines()
        .find_map(|l| l.strip_prefix('n'))
        .map(PathBuf::from)
}

/// Working directories of descendant processes (depth <= 3, at most 50 pids,
/// overall time budget 4s).
pub fn child_cwds(pid: u32) -> Vec<PathBuf> {
    const MAX_DEPTH: usize = 3;
    const MAX_PIDS: usize = 50;
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut pids: Vec<u32> = Vec::new();
    let mut frontier = vec![pid];
    'walk: for _ in 0..MAX_DEPTH {
        let mut next = Vec::new();
        for p in frontier {
            if Instant::now() >= deadline {
                break 'walk;
            }
            for c in pgrep_children(p) {
                if pids.len() >= MAX_PIDS {
                    break 'walk;
                }
                if c != pid && !pids.contains(&c) {
                    pids.push(c);
                    next.push(c);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    let mut cwds = Vec::new();
    for p in pids {
        if Instant::now() >= deadline {
            break;
        }
        if let Some(c) = process_cwd(p) {
            if !cwds.contains(&c) {
                cwds.push(c);
            }
        }
    }
    cwds
}

/// The session's transcript: the hook's `transcriptPath` when it exists,
/// else `<projects_dir>/*/<session_id>.jsonl` (bounded scan).
fn find_transcript(projects_dir: &Path, session_id: &str, hint: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = hint.filter(|p| p.is_file()) {
        return Some(p.to_path_buf());
    }
    if session_id.is_empty() || session_id.contains('/') {
        return None;
    }
    let file = format!("{session_id}.jsonl");
    std::fs::read_dir(projects_dir)
        .ok()?
        .take(5000)
        .filter_map(|e| e.ok())
        .map(|e| e.path().join(&file))
        .find(|p| p.is_file())
}

pub struct BranchResolver {
    pub projects_dir: PathBuf,
    pub child_cwds: fn(u32) -> Vec<PathBuf>,
}

impl Default for BranchResolver {
    fn default() -> Self {
        Self {
            projects_dir: dirs::home_dir().unwrap_or_default().join(".claude/projects"),
            child_cwds,
        }
    }
}

impl BranchResolver {
    pub fn resolve(&self, target: &BranchTarget, guess: bool) -> (Option<String>, bool) {
        let worktrees = list_worktrees(&target.cwd);
        let Some((branch, is_main)) = match_branch(&target.cwd, &worktrees) else {
            return (None, false);
        };
        if !is_main || !guess {
            return (Some(branch), false);
        }
        let mut paths = (self.child_cwds)(target.pid);
        if let Some(t) = find_transcript(&self.projects_dir, &target.id, target.transcript.as_deref()) {
            paths.extend(recent_tool_paths(&t, TRANSCRIPT_TAIL_BYTES));
        }
        match guess_worktree(&paths, &worktrees) {
            Some(b) => (Some(b), true),
            None => (Some(branch), false),
        }
    }
}

/// Slow (git, pgrep/lsof, transcript): only called on the branch thread.
pub fn resolve_branch(target: &BranchTarget, guess: bool) -> (Option<String>, bool) {
    BranchResolver::default().resolve(target, guess)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::{Command, Stdio};

    fn git(dir: &Path, args: &[&str]) {
        let st = Command::new("git")
            .arg("-C")
            .arg(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?} failed");
    }

    /// Returns (tempdir, main checkout, linked worktree on `feature`).
    fn repo_with_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("repo");
        fs::create_dir(&main).unwrap();
        git(&main, &["init", "-b", "develop"]);
        git(&main, &["commit", "--allow-empty", "-m", "init"]);
        let wt = tmp.path().join("repo-feature");
        git(&main, &["worktree", "add", "-b", "feature", wt.to_str().unwrap()]);
        (tmp, main, wt)
    }

    fn wt(path: &str, branch: Option<&str>, is_main: bool) -> Worktree {
        Worktree { path: PathBuf::from(path), branch: branch.map(String::from), is_main }
    }

    fn session(cwd: &Path) -> BranchTarget {
        BranchTarget { id: "sess-1".into(), cwd: cwd.to_path_buf(), pid: 0, transcript: None, done: false }
    }

    fn no_children(_: u32) -> Vec<PathBuf> {
        Vec::new()
    }

    #[test]
    fn match_branch_in_linked_worktree() {
        let wts = vec![wt("/a/repo", Some("develop"), true), wt("/a/repo-feature", Some("feature"), false)];
        assert_eq!(match_branch(Path::new("/a/repo-feature"), &wts), Some(("feature".into(), false)));
    }

    #[test]
    fn match_branch_in_subdir_of_worktree() {
        let wts = vec![wt("/a/repo", Some("develop"), true), wt("/a/repo/.claude/worktrees/x", Some("x"), false)];
        assert_eq!(
            match_branch(Path::new("/a/repo/.claude/worktrees/x/src/deep"), &wts),
            Some(("x".into(), false))
        );
    }

    #[test]
    fn match_branch_main_checkout_is_main() {
        let wts = vec![wt("/a/repo", Some("develop"), true), wt("/a/repo-feature", Some("feature"), false)];
        assert_eq!(match_branch(Path::new("/a/repo/src"), &wts), Some(("develop".into(), true)));
    }

    #[test]
    fn match_branch_does_not_match_sibling_prefix() {
        let wts = vec![wt("/a/repo", Some("develop"), true)];
        assert_eq!(match_branch(Path::new("/a/repo-feature"), &wts), None);
    }

    #[test]
    fn guess_prefers_most_recent_non_main_path() {
        let wts = vec![
            wt("/a/repo", Some("develop"), true),
            wt("/a/one", Some("one"), false),
            wt("/a/two", Some("two"), false),
        ];
        let paths = vec![PathBuf::from("/a/repo/x"), PathBuf::from("/a/two/f.rs"), PathBuf::from("/a/one/g.rs")];
        assert_eq!(guess_worktree(&paths, &wts), Some("two".into()));
    }

    #[test]
    fn guess_none_when_all_paths_in_main() {
        let wts = vec![wt("/a/repo", Some("develop"), true), wt("/a/one", Some("one"), false)];
        let paths = vec![PathBuf::from("/a/repo/x"), PathBuf::from("/elsewhere")];
        assert_eq!(guess_worktree(&paths, &wts), None);
    }

    fn tool_line(input: &str, name: &str) -> String {
        format!(
            r#"{{"cwd":"/a/repo","message":{{"content":[{{"type":"tool_use","name":"{name}","input":{input}}}]}}}}"#
        )
    }

    #[test]
    fn recent_tool_paths_skips_broken_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("t.jsonl");
        let body = format!(
            "{}\nnot json {{{{\n{}\n",
            tool_line(r#"{"file_path":"/a/one/f.rs"}"#, "Edit"),
            tool_line(r#"{"file_path":"/a/two/g.rs"}"#, "Read"),
        );
        fs::write(&f, body).unwrap();
        assert_eq!(
            recent_tool_paths(&f, 100_000),
            vec![PathBuf::from("/a/two/g.rs"), PathBuf::from("/a/one/f.rs")]
        );
    }

    #[test]
    fn recent_tool_paths_extracts_cd_and_file_paths_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("t.jsonl");
        let body = format!(
            "{}\n{}\n{}\n{}\n",
            tool_line(r#"{"file_path":"/a/one/f.rs"}"#, "Edit"),
            tool_line(r#"{"command":"cd /a/two && cargo test"}"#, "Bash"),
            tool_line(r#"{"path":"/a/three","pattern":"x"}"#, "Grep"),
            tool_line(r#"{"file_path":"/a/one/f.rs"}"#, "Write"),
        );
        fs::write(&f, body).unwrap();
        assert_eq!(
            recent_tool_paths(&f, 100_000),
            vec![PathBuf::from("/a/one/f.rs"), PathBuf::from("/a/three"), PathBuf::from("/a/two")]
        );
    }

    #[test]
    fn recent_tool_paths_discards_partial_first_line() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("t.jsonl");
        let first = tool_line(r#"{"file_path":"/a/old/f.rs"}"#, "Edit");
        let second = tool_line(r#"{"file_path":"/a/new/f.rs"}"#, "Edit");
        fs::write(&f, format!("{first}\n{second}\n")).unwrap();
        // Window starts inside the first line: it must be dropped.
        let max = (second.len() + 1 + 10) as u64;
        assert_eq!(recent_tool_paths(&f, max), vec![PathBuf::from("/a/new/f.rs")]);
        // Window starting exactly at offset 0 keeps the first line.
        assert_eq!(recent_tool_paths(&f, 100_000).len(), 2);
    }

    fn paths_of(input: &str, name: &str) -> Vec<PathBuf> {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("t.jsonl");
        fs::write(&f, format!("{}\n", tool_line(input, name))).unwrap();
        recent_tool_paths(&f, 100_000)
    }

    #[test]
    fn recent_tool_paths_extracts_git_dash_c_path() {
        assert_eq!(
            paths_of(r#"{"command":"git -C /a/two status"}"#, "Bash"),
            vec![PathBuf::from("/a/two")]
        );
    }

    #[test]
    fn recent_tool_paths_extracts_manifest_path_value_after_equals() {
        assert_eq!(
            paths_of(r#"{"command":"cargo test --manifest-path /a/two/Cargo.toml --dir=/a/three"}"#, "Bash"),
            vec![PathBuf::from("/a/two/Cargo.toml"), PathBuf::from("/a/three")]
        );
    }

    #[test]
    fn recent_tool_paths_extracts_cd_then_other_args() {
        assert_eq!(
            paths_of(r#"{"command":"cd /a/two && grep foo /a/three/x; echo /a/four"}"#, "Bash"),
            vec![PathBuf::from("/a/two"), PathBuf::from("/a/three/x"), PathBuf::from("/a/four")]
        );
    }

    #[test]
    fn recent_tool_paths_orders_calls_newest_first_and_commands_left_to_right() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("t.jsonl");
        let body = format!(
            "{}\n{}\n",
            tool_line(r#"{"command":"cd /a/one && ls /a/two"}"#, "Bash"),
            tool_line(r#"{"command":"cd /a/three && ls /a/four"}"#, "Bash"),
        );
        fs::write(&f, body).unwrap();
        assert_eq!(
            recent_tool_paths(&f, 100_000),
            ["/a/three", "/a/four", "/a/one", "/a/two"].map(PathBuf::from).to_vec()
        );
    }

    #[test]
    fn canon_normalizes_dotdot_in_nonexistent_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let p = base.join("missing/sub/../../other/./x");
        assert_eq!(canon(&p), base.join("other/x"));
    }

    #[test]
    fn child_cwds_includes_spawned_child_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let mut child = Command::new("sleep")
            .arg("5")
            .current_dir(tmp.path())
            .spawn()
            .unwrap();
        let cwds = child_cwds(std::process::id());
        let _ = child.kill();
        let _ = child.wait();
        let want = tmp.path().canonicalize().unwrap();
        assert!(cwds.iter().any(|c| canon(c) == want), "{cwds:?}");
    }

    #[test]
    fn list_worktrees_non_git_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(list_worktrees(tmp.path()).is_empty());
        assert!(list_worktrees(&tmp.path().join("missing")).is_empty());
    }

    #[test]
    fn list_worktrees_detached_head_has_no_branch() {
        let (_tmp, main, wt_path) = repo_with_worktree();
        git(&wt_path, &["checkout", "--detach"]);
        let wts = list_worktrees(&main);
        assert_eq!(wts.len(), 2);
        assert!(wts[0].is_main);
        assert_eq!(wts[0].branch.as_deref(), Some("develop"));
        assert!(!wts[1].is_main);
        assert_eq!(wts[1].branch, None);
    }

    #[test]
    fn resolve_branch_no_guess_when_disabled() {
        let (tmp, main, wt_path) = repo_with_worktree();
        let projects = tmp.path().join("projects");
        let dir = projects.join("-enc");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("sess-1.jsonl"),
            tool_line(&format!(r#"{{"file_path":"{}/f.rs"}}"#, wt_path.display()), "Edit"),
        )
        .unwrap();
        let r = BranchResolver { projects_dir: projects, child_cwds: no_children };
        assert_eq!(r.resolve(&session(&main), false), (Some("develop".into()), false));
        assert_eq!(r.resolve(&session(&wt_path), false), (Some("feature".into()), false));
        assert_eq!(r.resolve(&session(tmp.path()), true), (None, false));
    }

    #[test]
    fn find_transcript_prefers_hook_path_then_scans_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join("projects");
        fs::create_dir_all(projects.join("-enc")).unwrap();
        let scanned = projects.join("-enc").join("sess-1.jsonl");
        fs::write(&scanned, "").unwrap();
        let hinted = tmp.path().join("elsewhere.jsonl");
        fs::write(&hinted, "").unwrap();
        assert_eq!(find_transcript(&projects, "sess-1", Some(&hinted)), Some(hinted.clone()));
        let missing = tmp.path().join("gone.jsonl");
        assert_eq!(find_transcript(&projects, "sess-1", Some(&missing)), Some(scanned.clone()), "stale hint: scan");
        assert_eq!(find_transcript(&projects, "sess-1", None), Some(scanned));
    }

    #[test]
    fn resolve_branch_guesses_from_hook_transcript_path() {
        let (tmp, main, wt_path) = repo_with_worktree();
        // transcript outside the projects dir, known only through the hook
        let t = tmp.path().join("hooked.jsonl");
        fs::write(&t, format!("{}\n", tool_line(&format!(r#"{{"file_path":"{}/f.rs"}}"#, wt_path.display()), "Edit"))).unwrap();
        let r = BranchResolver { projects_dir: tmp.path().join("no-projects"), child_cwds: no_children };
        let mut target = session(&main);
        assert_eq!(r.resolve(&target, true), (Some("develop".into()), false), "no hint, nothing found");
        target.transcript = Some(t);
        assert_eq!(r.resolve(&target, true), (Some("feature".into()), true));
    }

    #[test]
    fn resolve_branch_guesses_from_transcript_when_session_in_main() {
        let (tmp, main, wt_path) = repo_with_worktree();
        let projects = tmp.path().join("projects");
        let dir = projects.join("-enc");
        fs::create_dir_all(&dir).unwrap();
        let t = dir.join("sess-1.jsonl");
        fs::write(
            &t,
            format!(
                "{}\n",
                tool_line(&format!(r#"{{"command":"cd {} && ls"}}"#, wt_path.display()), "Bash")
            ),
        )
        .unwrap();
        let r = BranchResolver { projects_dir: projects.clone(), child_cwds: no_children };
        assert_eq!(r.resolve(&session(&main), true), (Some("feature".into()), true));
        // No transcript for this session id: falls back to the main branch.
        let mut s = session(&main);
        s.id = "other".into();
        assert_eq!(r.resolve(&s, true), (Some("develop".into()), false));
    }
}
