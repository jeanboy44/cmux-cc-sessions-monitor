use crate::exec::run_bounded;
use crate::model::{Pr, PrState};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const GIT_TIMEOUT: Duration = Duration::from_secs(3);
const STOP_SLICE: Duration = Duration::from_millis(50);
const MAX_REASON: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remote {
    AzureDevOps { org: String, project: String, repo: String },
    GitHub { owner: String, repo: String },
}

pub trait PrProvider: Send + Sync {
    fn find(&self, repo_dir: &Path, branch: &str) -> Result<Option<Pr>>;
    /// The CLI this provider needs (`az` or `gh`).
    fn tool(&self) -> &'static str;
}

#[derive(Debug, Clone, PartialEq)]
pub enum PrLookup {
    Unknown,
    None,
    Found(Pr),
    Unavailable(String),
}

/// Chooses the provider for a repo directory (None: unsupported remote).
pub type ProviderFactory = Arc<dyn Fn(&Path) -> Option<Box<dyn PrProvider>> + Send + Sync>;

// ---------------------------------------------------------------- remotes

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Decoded path segment safe to pass as a CLI argument value.
fn segment(s: &str) -> Option<String> {
    let d = percent_decode(s)?;
    let bad = d.is_empty() || d.starts_with('-') || d.chars().any(|c| c.is_control() || c == '/');
    (!bad).then_some(d)
}

fn segments(path: &str) -> Vec<&str> {
    path.trim_matches('/').split('/').collect()
}

fn azure(org: &str, project: &str, repo: &str) -> Option<Remote> {
    let org = segment(org)?;
    if !org.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return None;
    }
    Some(Remote::AzureDevOps { org, project: segment(project)?, repo: segment(repo)? })
}

fn github(owner: &str, repo: &str) -> Option<Remote> {
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    Some(Remote::GitHub { owner: segment(owner)?, repo: segment(repo)? })
}

fn from_host_path(host: &str, path: &str) -> Option<Remote> {
    let host = host.to_ascii_lowercase();
    let segs = segments(path);
    match (host.as_str(), segs.as_slice()) {
        ("dev.azure.com", [org, project, "_git", repo]) => azure(org, project, repo),
        ("ssh.dev.azure.com", ["v3", org, project, repo]) => azure(org, project, repo),
        ("github.com", [owner, repo]) => github(owner, repo),
        _ => None,
    }
}

/// Parses a git remote URL. The URL may carry credentials: nothing from it is
/// logged and the result only holds the parsed organisation/project/repo names.
pub fn parse_remote_url(url: &str) -> Option<Remote> {
    let url = url.trim();
    if let Some((scheme, rest)) = url.split_once("://") {
        if !matches!(scheme, "https" | "http" | "ssh") {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let host = if scheme == "ssh" { host.split(':').next()? } else { host };
        return from_host_path(host, path);
    }
    // scp-like: [user@]host:path
    let (left, path) = url.split_once(':')?;
    let host = left.rsplit_once('@').map_or(left, |(_, h)| h);
    from_host_path(host, path)
}

/// Reads the `origin` remote of `repo_dir` (only `origin`).
pub fn detect_remote(repo_dir: &Path) -> Option<Remote> {
    let dir = repo_dir.to_string_lossy();
    let out = run_bounded("git", &["-C", &dir, "remote", "get-url", "origin"], None, GIT_TIMEOUT).ok()?;
    if !out.status.success() {
        return None;
    }
    parse_remote_url(String::from_utf8_lossy(&out.stdout).trim())
}

// ------------------------------------------------------------ state mapping

fn str_field<'a>(json: &'a Value, key: &str) -> Option<&'a str> {
    json.get(key).and_then(Value::as_str)
}

/// Draft > Conflict > Approved > ReviewPending; Unknown if nothing is readable.
pub fn az_state(json: &Value) -> PrState {
    let readable = ["isDraft", "mergeStatus", "reviewers"].iter().any(|k| json.get(k).is_some());
    if !readable {
        return PrState::Unknown;
    }
    if json.get("isDraft").and_then(Value::as_bool) == Some(true) {
        return PrState::Draft;
    }
    if str_field(json, "mergeStatus").is_some_and(|s| s.eq_ignore_ascii_case("conflicts")) {
        return PrState::Conflict;
    }
    let votes: Vec<i64> = json
        .get("reviewers")
        .and_then(Value::as_array)
        .map(|rs| rs.iter().filter_map(|r| r.get("vote").and_then(Value::as_i64)).collect())
        .unwrap_or_default();
    if votes.iter().any(|v| *v >= 10) && votes.iter().all(|v| *v > -5) {
        PrState::Approved
    } else {
        PrState::ReviewPending
    }
}

/// Draft > Conflict > Approved > ReviewPending; Unknown otherwise.
pub fn gh_state(json: &Value) -> PrState {
    if json.get("isDraft").and_then(Value::as_bool) == Some(true) {
        return PrState::Draft;
    }
    if str_field(json, "mergeable") == Some("CONFLICTING") {
        return PrState::Conflict;
    }
    match str_field(json, "reviewDecision") {
        Some("APPROVED") => PrState::Approved,
        Some("REVIEW_REQUIRED" | "CHANGES_REQUESTED" | "") => PrState::ReviewPending,
        _ => PrState::Unknown,
    }
}

/// Picks the PR with the highest `number_key` from a JSON array.
fn pick_pr(text: &str, number_key: &str, state: fn(&Value) -> PrState) -> Result<Option<Pr>> {
    let v: Value = serde_json::from_str(text).map_err(|_| anyhow!("unreadable PR list output"))?;
    let items = v.as_array().ok_or_else(|| anyhow!("unexpected PR list output"))?;
    let best = items
        .iter()
        .filter_map(|i| Some((i.get(number_key)?.as_u64()?, i)))
        .max_by_key(|(n, _)| *n);
    Ok(best.map(|(number, item)| Pr {
        number,
        title: str_field(item, "title").unwrap_or_default().to_string(),
        state: state(item),
    }))
}

pub fn parse_az_list(text: &str) -> Result<Option<Pr>> {
    pick_pr(text, "pullRequestId", az_state)
}

pub fn parse_gh_list(text: &str) -> Result<Option<Pr>> {
    pick_pr(text, "number", gh_state)
}

// ---------------------------------------------------------------- providers

/// Azure DevOps CLI calls are slow on a cold start.
const AZ_TIMEOUT: Duration = Duration::from_secs(15);
const GH_TIMEOUT: Duration = Duration::from_secs(5);

type CliRunner<'a> = &'a dyn Fn(&str, &[&str], Duration) -> Result<String>;
type Detect<'a> = &'a dyn Fn(&Path) -> Option<Remote>;

/// `remote` is filled by the provider factory so a lookup runs
/// `git remote get-url` once; without it `find` detects on its own.
#[derive(Default)]
pub struct AzProvider {
    remote: Option<Remote>,
}

#[derive(Default)]
pub struct GhProvider {
    remote: Option<Remote>,
}

fn check_branch(branch: &str) -> Result<()> {
    if branch.is_empty() || branch.starts_with('-') || branch.chars().any(char::is_control) {
        return Err(anyhow!("unsupported branch name"));
    }
    Ok(())
}

/// Runs the CLI; error texts never include arguments (they may hold names).
fn run_cli(program: &str, args: &[&str], timeout: Duration) -> Result<String> {
    let out = run_bounded(program, args, None, timeout)
        .map_err(|_| anyhow!("{program} unavailable or timed out"))?;
    if !out.status.success() {
        return Err(anyhow!("{program} failed (not logged in?)"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

impl AzProvider {
    pub fn with_remote(remote: Remote) -> Self {
        Self { remote: Some(remote) }
    }

    fn remote_for(&self, repo_dir: &Path, detect: Detect) -> Option<Remote> {
        self.remote.clone().or_else(|| detect(repo_dir))
    }

    fn find_with(&self, remote: Option<Remote>, branch: &str, run: CliRunner) -> Result<Option<Pr>> {
        check_branch(branch)?;
        let Some(Remote::AzureDevOps { org, project, repo }) = remote else {
            return Err(anyhow!("origin is not an Azure DevOps remote"));
        };
        let org_url = format!("https://dev.azure.com/{org}");
        let text = run(
            "az",
            &[
                "repos", "pr", "list", "--organization", &org_url, "--project", &project,
                "--repository", &repo, "--source-branch", branch, "--status", "active",
                "--output", "json",
            ],
            AZ_TIMEOUT,
        )?;
        parse_az_list(&text)
    }
}

impl GhProvider {
    pub fn with_remote(remote: Remote) -> Self {
        Self { remote: Some(remote) }
    }

    fn remote_for(&self, repo_dir: &Path, detect: Detect) -> Option<Remote> {
        self.remote.clone().or_else(|| detect(repo_dir))
    }

    fn find_with(&self, remote: Option<Remote>, branch: &str, run: CliRunner) -> Result<Option<Pr>> {
        check_branch(branch)?;
        let Some(Remote::GitHub { owner, repo }) = remote else {
            return Err(anyhow!("origin is not a GitHub remote"));
        };
        let slug = format!("{owner}/{repo}");
        let text = run(
            "gh",
            &[
                "pr", "list", "--repo", &slug, "--head", branch, "--state", "open", "--json",
                "number,title,isDraft,reviewDecision,mergeable",
            ],
            GH_TIMEOUT,
        )?;
        parse_gh_list(&text)
    }
}

impl PrProvider for AzProvider {
    fn find(&self, repo_dir: &Path, branch: &str) -> Result<Option<Pr>> {
        self.find_with(self.remote_for(repo_dir, &detect_remote), branch, &run_cli)
    }
    fn tool(&self) -> &'static str {
        "az"
    }
}

impl PrProvider for GhProvider {
    fn find(&self, repo_dir: &Path, branch: &str) -> Result<Option<Pr>> {
        self.find_with(self.remote_for(repo_dir, &detect_remote), branch, &run_cli)
    }
    fn tool(&self) -> &'static str {
        "gh"
    }
}

/// Factory that detects the remote once and hands it to the provider.
fn providers_with(detect: Arc<dyn Fn(&Path) -> Option<Remote> + Send + Sync>) -> ProviderFactory {
    Arc::new(move |dir: &Path| match detect(dir)? {
        r @ Remote::AzureDevOps { .. } => Some(Box::new(AzProvider::with_remote(r)) as Box<dyn PrProvider>),
        r @ Remote::GitHub { .. } => Some(Box::new(GhProvider::with_remote(r)) as Box<dyn PrProvider>),
    })
}

/// Production wiring: `az` for Azure DevOps origins, `gh` for GitHub origins.
pub fn default_providers() -> ProviderFactory {
    providers_with(Arc::new(detect_remote))
}

// -------------------------------------------------------------------- cache

type Key = (PathBuf, String);

#[derive(Default)]
pub struct PrCache {
    map: Mutex<HashMap<Key, PrLookup>>,
    /// CLIs the current targets' remotes need (`az`/`gh`), sorted.
    tools: Mutex<Vec<&'static str>>,
}

impl PrCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<Key, PrLookup>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Short lock, never calls a provider. `Unknown` if never fetched.
    pub fn get(&self, repo_dir: &Path, branch: &str) -> PrLookup {
        self.lock()
            .get(&(repo_dir.to_path_buf(), branch.to_string()))
            .cloned()
            .unwrap_or(PrLookup::Unknown)
    }

    pub(crate) fn put(&self, key: Key, value: PrLookup) {
        self.lock().insert(key, value);
    }

    /// CLIs needed by the remotes seen in the last poll (empty before the
    /// first poll or while PR info is off).
    pub fn tools_seen(&self) -> Vec<&'static str> {
        self.tools.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_tools(&self, mut tools: Vec<&'static str>) {
        tools.sort_unstable();
        tools.dedup();
        *self.tools.lock().unwrap_or_else(|e| e.into_inner()) = tools;
    }

    fn retain_targets(&self, targets: &[Key]) {
        self.lock().retain(|k, _| targets.contains(k));
    }
}

/// Footer note while a PR lookup has not reported yet.
pub const PR_PENDING: &str = "PR: 조회 중…";

/// Short footer note for PR availability: why lookups are `Unavailable`
/// (most actionable reason first: login, missing tool, unsupported remote,
/// other), else [`PR_PENDING`] when some are still pending, else `None`.
pub fn pr_note(unavailable: &[String], pending: bool) -> Option<String> {
    let classify = |r: &str| -> (u8, String) {
        let tool = ["az", "gh"].into_iter().find(|t| r.starts_with(t));
        match tool {
            Some(t) if r.contains("not logged in") => (0, format!("PR: {t} 로그인 필요")),
            Some(t) if r.contains("unavailable or timed out") => (1, format!("PR: {t} 없음 또는 응답 없음")),
            _ if r == "no supported remote" => (2, "PR: 지원하지 않는 저장소".into()),
            _ => (3, "PR: 조회 실패".into()),
        }
    };
    match unavailable.iter().map(|r| classify(r)).min_by_key(|(rank, _)| *rank) {
        Some((_, note)) => Some(note),
        None => pending.then(|| PR_PENDING.to_string()),
    }
}

fn short_reason(msg: &str) -> String {
    msg.chars().take(MAX_REASON).collect()
}

fn lookup(providers: &ProviderFactory, repo_dir: &Path, branch: &str, tools: &Mutex<Vec<&'static str>>) -> PrLookup {
    let Some(provider) = providers(repo_dir) else {
        return PrLookup::Unavailable("no supported remote".into());
    };
    tools.lock().unwrap_or_else(|e| e.into_inner()).push(provider.tool());
    match provider.find(repo_dir, branch) {
        Ok(Some(pr)) => PrLookup::Found(pr),
        Ok(None) => PrLookup::None,
        Err(e) => PrLookup::Unavailable(short_reason(&e.to_string())),
    }
}

fn poll_once(
    cache: &PrCache,
    targets: &Mutex<Vec<Key>>,
    providers: &ProviderFactory,
    stop: &AtomicBool,
) {
    let mut snapshot: Vec<Key> = targets.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut seen = Vec::new();
    snapshot.retain(|k| {
        let fresh = !seen.contains(k);
        seen.push(k.clone());
        fresh
    });
    let tools = Mutex::new(Vec::new());
    for key in &snapshot {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        // The cache lock is not held while the provider runs.
        let result = catch_unwind(AssertUnwindSafe(|| lookup(providers, &key.0, &key.1, &tools)))
            .unwrap_or_else(|_| PrLookup::Unavailable("lookup failed".into()));
        cache.put(key.clone(), result);
    }
    cache.retain_targets(&snapshot);
    cache.set_tools(tools.into_inner().unwrap_or_else(|e| e.into_inner()));
}

/// Polls every `interval` (first round immediately) until `stop` is set.
/// Targets are queried sequentially; a failing target does not affect others.
///
/// On exit callers must set `stop` and drop the handle rather than join it (a
/// provider call can take up to its CLI timeout). A provider panic is caught,
/// but the default panic hook still prints to stderr; the app (Task 8) installs
/// a quiet hook.
pub fn spawn_poller(
    cache: Arc<PrCache>,
    targets: Arc<Mutex<Vec<(PathBuf, String)>>>,
    interval: Duration,
    providers: ProviderFactory,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let _ = catch_unwind(AssertUnwindSafe(|| poll_once(&cache, &targets, &providers, &stop)));
            let next = Instant::now() + interval;
            while Instant::now() < next && !stop.load(Ordering::Relaxed) {
                std::thread::sleep(STOP_SLICE.min(next.saturating_duration_since(Instant::now())));
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn az(org: &str, project: &str, repo: &str) -> Option<Remote> {
        Some(Remote::AzureDevOps { org: org.into(), project: project.into(), repo: repo.into() })
    }

    fn gh(owner: &str, repo: &str) -> Option<Remote> {
        Some(Remote::GitHub { owner: owner.into(), repo: repo.into() })
    }

    #[test]
    fn parse_remote_url_azure_https() {
        assert_eq!(parse_remote_url("https://dev.azure.com/acme/proj/_git/repo"), az("acme", "proj", "repo"));
    }

    #[test]
    fn parse_remote_url_azure_userinfo_and_encoded_project() {
        assert_eq!(
            parse_remote_url("https://acme@dev.azure.com/acme/My%20Project/_git/my-repo"),
            az("acme", "My Project", "my-repo")
        );
        assert_eq!(
            parse_remote_url("https://user:tok3n@dev.azure.com/acme/proj/_git/repo"),
            az("acme", "proj", "repo")
        );
    }

    #[test]
    fn parse_remote_url_azure_ssh() {
        assert_eq!(
            parse_remote_url("git@ssh.dev.azure.com:v3/acme/My%20Project/repo"),
            az("acme", "My Project", "repo")
        );
    }

    #[test]
    fn parse_remote_url_github_https_and_ssh() {
        assert_eq!(parse_remote_url("https://github.com/octo/hello.git"), gh("octo", "hello"));
        assert_eq!(parse_remote_url("https://github.com/octo/hello"), gh("octo", "hello"));
        assert_eq!(parse_remote_url("git@github.com:octo/hello.git"), gh("octo", "hello"));
        assert_eq!(parse_remote_url("https://tok3n@github.com/octo/hello.git"), gh("octo", "hello"));
    }

    #[test]
    fn parse_remote_url_unknown_is_none() {
        for u in [
            "",
            "https://example.com/a/b",
            "https://gitlab.com/a/b.git",
            "https://dev.azure.com/acme/proj/repo",
            "https://github.com/onlyone",
            "https://github.com/a/b/c",
            "ftp://github.com/a/b",
            "/local/path/repo",
            "https://dev.azure.com/acme/%ZZ/_git/repo",
            "https://dev.azure.com/-bad/proj/_git/repo",
        ] {
            assert_eq!(parse_remote_url(u), None, "{u}");
        }
    }

    #[test]
    fn az_state_draft() {
        assert_eq!(az_state(&json!({"isDraft": true, "mergeStatus": "conflicts"})), PrState::Draft);
    }

    #[test]
    fn az_state_conflict() {
        let j = json!({"isDraft": false, "mergeStatus": "conflicts", "reviewers": [{"vote": 10}]});
        assert_eq!(az_state(&j), PrState::Conflict);
    }

    #[test]
    fn az_state_approved_and_review_pending() {
        let ok = json!({"isDraft": false, "mergeStatus": "succeeded", "reviewers": [{"vote": 10}, {"vote": 0}]});
        assert_eq!(az_state(&ok), PrState::Approved);
        let suggestions = json!({"isDraft": false, "reviewers": [{"vote": 5}]});
        assert_eq!(az_state(&suggestions), PrState::ReviewPending);
        let waiting = json!({"isDraft": false, "reviewers": [{"vote": 10}, {"vote": -5}]});
        assert_eq!(az_state(&waiting), PrState::ReviewPending);
        let none = json!({"isDraft": false, "reviewers": []});
        assert_eq!(az_state(&none), PrState::ReviewPending);
        assert_eq!(az_state(&json!({})), PrState::Unknown);
    }

    #[test]
    fn gh_state_approved() {
        assert_eq!(gh_state(&json!({"reviewDecision": "APPROVED", "mergeable": "MERGEABLE"})), PrState::Approved);
    }

    #[test]
    fn gh_state_conflict_draft_and_review_required() {
        let c = json!({"reviewDecision": "APPROVED", "mergeable": "CONFLICTING"});
        assert_eq!(gh_state(&c), PrState::Conflict);
        let d = json!({"isDraft": true, "reviewDecision": "APPROVED", "mergeable": "CONFLICTING"});
        assert_eq!(gh_state(&d), PrState::Draft);
        for rd in ["REVIEW_REQUIRED", "CHANGES_REQUESTED", ""] {
            assert_eq!(gh_state(&json!({"reviewDecision": rd})), PrState::ReviewPending, "{rd}");
        }
        assert_eq!(gh_state(&json!({})), PrState::Unknown);
        assert_eq!(gh_state(&json!({"reviewDecision": "WHATEVER"})), PrState::Unknown);
    }

    #[test]
    fn list_parsing_picks_highest_number_and_handles_empty() {
        let az_out = r#"[{"pullRequestId":3,"title":"old"},{"pullRequestId":9,"title":"new","isDraft":true}]"#;
        assert_eq!(
            parse_az_list(az_out).unwrap(),
            Some(Pr { number: 9, title: "new".into(), state: PrState::Draft })
        );
        assert_eq!(parse_az_list("[]").unwrap(), None);
        assert!(parse_az_list("not json").is_err());
        assert!(parse_gh_list("{}").is_err());
        let gh_out = r#"[{"number":4,"title":"t","reviewDecision":"APPROVED"}]"#;
        assert_eq!(
            parse_gh_list(gh_out).unwrap(),
            Some(Pr { number: 4, title: "t".into(), state: PrState::Approved })
        );
    }

    // ---- cache / poller

    struct Fake(Box<dyn Fn(&Path, &str) -> Result<Option<Pr>> + Send + Sync>);

    impl PrProvider for Fake {
        fn find(&self, d: &Path, b: &str) -> Result<Option<Pr>> {
            (self.0)(d, b)
        }
        fn tool(&self) -> &'static str {
            "gh"
        }
    }

    #[test]
    fn pr_note_explains_unavailable_lookups() {
        assert_eq!(pr_note(&["az failed (not logged in?)".into()], false).as_deref(), Some("PR: az 로그인 필요"));
        assert_eq!(pr_note(&["gh unavailable or timed out".into()], false).as_deref(), Some("PR: gh 없음 또는 응답 없음"));
        assert_eq!(pr_note(&["no supported remote".into()], false).as_deref(), Some("PR: 지원하지 않는 저장소"));
        assert_eq!(pr_note(&["unreadable PR list output".into()], false).as_deref(), Some("PR: 조회 실패"));
        // login beats the rest, whatever the order
        let mixed = ["no supported remote".to_string(), "az failed (not logged in?)".to_string()];
        assert_eq!(pr_note(&mixed, true).as_deref(), Some("PR: az 로그인 필요"));
        assert_eq!(pr_note(&[], true).as_deref(), Some(PR_PENDING));
        assert_eq!(pr_note(&[], false), None);
    }

    #[test]
    fn cache_records_the_tool_each_remote_needs() {
        let (cache, _t, stop, h) = start(vec![("/r", "b")], factory(|_, _| Ok(None)));
        assert!(wait_until(|| cache.get(Path::new("/r"), "b") == PrLookup::None));
        assert_eq!(cache.tools_seen(), vec!["gh"]);
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        assert_eq!(AzProvider::default().tool(), "az");
        assert_eq!(GhProvider::default().tool(), "gh");
    }

    fn factory(f: impl Fn(&Path, &str) -> Result<Option<Pr>> + Send + Sync + Clone + 'static) -> ProviderFactory {
        Arc::new(move |_| Some(Box::new(Fake(Box::new(f.clone()))) as Box<dyn PrProvider>))
    }

    fn pr(n: u64) -> Pr {
        Pr { number: n, title: format!("pr {n}"), state: PrState::ReviewPending }
    }

    fn start(
        targets: Vec<(&str, &str)>,
        providers: ProviderFactory,
    ) -> (Arc<PrCache>, Arc<Mutex<Vec<Key>>>, Arc<AtomicBool>, JoinHandle<()>) {
        let cache = Arc::new(PrCache::new());
        let t = Arc::new(Mutex::new(
            targets.into_iter().map(|(d, b)| (PathBuf::from(d), b.to_string())).collect::<Vec<_>>(),
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let h = spawn_poller(cache.clone(), t.clone(), Duration::from_millis(30), providers, stop.clone());
        (cache, t, stop, h)
    }

    fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn cache_get_is_nonblocking_while_provider_hangs() {
        let hang = factory(|_, _| {
            std::thread::sleep(Duration::from_secs(3));
            Ok(None)
        });
        let (cache, _t, stop, h) = start(vec![("/r", "b")], hang);
        std::thread::sleep(Duration::from_millis(100));
        let t0 = Instant::now();
        assert_eq!(cache.get(Path::new("/r"), "b"), PrLookup::Unknown);
        assert!(t0.elapsed() < Duration::from_millis(200), "{:?}", t0.elapsed());
        stop.store(true, Ordering::Relaxed);
        drop(h); // do not wait for the hanging provider
    }

    #[test]
    fn cache_unavailable_when_provider_errors() {
        let (cache, _t, stop, h) = start(vec![("/r", "b")], factory(|_, _| Err(anyhow!("az failed (not logged in?)"))));
        assert!(wait_until(|| matches!(cache.get(Path::new("/r"), "b"), PrLookup::Unavailable(_))));
        assert_eq!(
            cache.get(Path::new("/r"), "b"),
            PrLookup::Unavailable("az failed (not logged in?)".into())
        );
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
    }

    #[test]
    fn lookup_none_when_no_pr() {
        let (cache, _t, stop, h) = start(vec![("/r", "b")], factory(|_, _| Ok(None)));
        assert!(wait_until(|| cache.get(Path::new("/r"), "b") == PrLookup::None));
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
    }

    #[test]
    fn poller_isolates_failing_target() {
        let f = factory(|_, b| match b {
            "boom" => Err(anyhow!("boom")),
            "panic" => panic!("provider bug"),
            _ => Ok(Some(pr(7))),
        });
        let (cache, _t, stop, h) = start(vec![("/r", "boom"), ("/r", "panic"), ("/r", "ok")], f);
        assert!(wait_until(|| cache.get(Path::new("/r"), "ok") == PrLookup::Found(pr(7))));
        assert_eq!(cache.get(Path::new("/r"), "boom"), PrLookup::Unavailable("boom".into()));
        assert!(matches!(cache.get(Path::new("/r"), "panic"), PrLookup::Unavailable(_)));
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
    }

    #[test]
    fn poller_picks_up_new_targets_and_drops_removed_ones() {
        let (cache, targets, stop, h) = start(vec![("/r", "a")], factory(|_, _| Ok(Some(pr(1)))));
        assert!(wait_until(|| cache.get(Path::new("/r"), "a") != PrLookup::Unknown));
        *targets.lock().unwrap() = vec![(PathBuf::from("/r"), "b".to_string())];
        assert!(wait_until(|| cache.get(Path::new("/r"), "b") != PrLookup::Unknown));
        assert!(wait_until(|| cache.get(Path::new("/r"), "a") == PrLookup::Unknown));
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
    }

    #[test]
    fn poller_stop_ends_thread() {
        let cache = Arc::new(PrCache::new());
        let stop = Arc::new(AtomicBool::new(false));
        let h = spawn_poller(
            cache,
            Arc::new(Mutex::new(Vec::new())),
            Duration::from_secs(60),
            factory(|_, _| Ok(None)),
            stop.clone(),
        );
        std::thread::sleep(Duration::from_millis(100));
        let t0 = Instant::now();
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
    }

    #[test]
    fn credentials_in_origin_never_leak_into_lookup() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let git = |args: &[&str]| {
            let d = dir.to_string_lossy();
            let mut a = vec!["-C", &*d];
            a.extend_from_slice(args);
            assert!(run_bounded("git", &a, None, Duration::from_secs(5)).unwrap().status.success());
        };
        git(&["init", "-q"]);
        git(&["remote", "add", "origin", "https://user:s3cretT0ken@example.com/o/r.git"]);
        assert_eq!(detect_remote(dir), None);
        let (cache, _t, stop, h) = start(vec![(dir.to_str().unwrap(), "b")], default_providers());
        assert!(wait_until(|| cache.get(dir, "b") != PrLookup::Unknown));
        let shown = format!("{:?}", cache.get(dir, "b"));
        assert!(shown.contains("Unavailable") && !shown.contains("s3cretT0ken"), "{shown}");
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();

        // Supported host with credentials: parsed value holds names only.
        let r = parse_remote_url("https://user:s3cretT0ken@dev.azure.com/acme/proj/_git/repo").unwrap();
        assert!(!format!("{r:?}").contains("s3cretT0ken"));
        // Provider errors do not echo the URL either.
        git(&["remote", "set-url", "origin", "https://user:s3cretT0ken@dev.azure.com/acme/proj/_git/repo"]);
        // Azure remote is read for real; the run closure fails like a
        // not-logged-in az, and `GhProvider` sees a non-GitHub remote.
        let remote = detect_remote(dir);
        assert!(remote.is_some());
        let fail = |_: &str, _: &[&str], _: Duration| -> Result<String> { Err(anyhow!("az failed")) };
        let err = AzProvider::default().find_with(remote.clone(), "b", &fail).unwrap_err().to_string();
        assert!(!err.contains("s3cretT0ken"), "{err}");
        let err = GhProvider::default().find_with(remote, "b", &fail).unwrap_err().to_string();
        assert!(!err.contains("s3cretT0ken"), "{err}");
    }

    #[test]
    fn providers_use_their_own_cli_timeouts() {
        use std::cell::RefCell;
        let seen: RefCell<Vec<(String, Duration)>> = RefCell::new(Vec::new());
        let run = |prog: &str, _: &[&str], t: Duration| -> Result<String> {
            seen.borrow_mut().push((prog.to_string(), t));
            Ok("[]".into())
        };
        let a = Some(Remote::AzureDevOps { org: "o".into(), project: "p".into(), repo: "r".into() });
        let g = Some(Remote::GitHub { owner: "o".into(), repo: "r".into() });
        assert_eq!(AzProvider::default().find_with(a, "b", &run).unwrap(), None);
        assert_eq!(GhProvider::default().find_with(g, "b", &run).unwrap(), None);
        assert_eq!(
            *seen.borrow(),
            vec![("az".to_string(), Duration::from_secs(15)), ("gh".to_string(), Duration::from_secs(5))]
        );
    }

    #[test]
    fn remote_is_detected_once_per_lookup() {
        use std::sync::atomic::AtomicUsize;
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let f = providers_with(Arc::new(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
            Some(Remote::GitHub { owner: "o".into(), repo: "r".into() })
        }));
        assert!(f(Path::new("/r")).is_some());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // A provider holding its remote never detects again.
        let p = GhProvider::with_remote(Remote::GitHub { owner: "o".into(), repo: "r".into() });
        let never = |_: &Path| -> Option<Remote> { panic!("detect called") };
        assert!(p.remote_for(Path::new("/r"), &never).is_some());
    }
}
