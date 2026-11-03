//! Guards the public repository against leaking local paths, real identifiers
//! and private names. Scans every text file that is tracked or
//! untracked-but-not-ignored by git. Fails (rather than silently passing) when
//! git is unavailable, unless `ALLOW_NO_GIT` is set.
//!
//! Built-in checks: home-directory paths (`/Users/`), real-looking UUIDs and
//! `dev.azure.com/<org>` organisations other than the fake ones.
//!
//! Private names (company, team, colleague, handle …) are never written into
//! this file — that would publish exactly what the guard hides. Maintainers
//! add their own patterns locally, matched case-insensitively:
//!
//! - env var `CCMON_FORBIDDEN`, comma-separated: `CCMON_FORBIDDEN=foo-inc,bar cargo test`
//! - file `.personal-info-patterns` at the repository root (gitignored), one
//!   pattern per line, lines starting with `#` are comments.

use std::path::Path;
use std::process::Command;

/// Built-in forbidden substrings (case-sensitive).
const FORBIDDEN: &[&str] = &["/Users/"];
const UUID_FAKE_PREFIXES: &[&str] = &["00000000-", "aaaaaaaa-", "bbbbbbbb-"];
/// This file spells out the built-in patterns as test data, so for it only the
/// maintainer's own (extra) patterns are checked.
const GUARD_FILE: &str = "tests/no_personal_info.rs";
/// Optional, gitignored file with extra patterns (see the module docs).
const LOCAL_PATTERNS_FILE: &str = ".personal-info-patterns";
const ENV_PATTERNS: &str = "CCMON_FORBIDDEN";

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

/// UUID-shaped substrings (8-4-4-4-12 hex digits).
fn uuids(line: &str) -> Vec<&str> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let shape = [8usize, 4, 4, 4, 12];
    let total = 36;
    let mut i = 0;
    while i + total <= b.len() {
        let mut pos = i;
        let mut ok = true;
        for (n, len) in shape.iter().enumerate() {
            if n > 0 {
                if b[pos] != b'-' {
                    ok = false;
                    break;
                }
                pos += 1;
            }
            if !b[pos..pos + len].iter().all(|c| is_hex(*c)) {
                ok = false;
                break;
            }
            pos += len;
        }
        let bounded = (i == 0 || !is_hex(b[i - 1])) && (i + total == b.len() || !is_hex(b[i + total]));
        if ok && bounded {
            out.push(&line[i..i + total]);
            i += total;
        } else {
            i += 1;
        }
    }
    out
}

/// Organisations after `dev.azure.com/` other than the fake ones.
fn bad_azure_org(line: &str) -> Option<String> {
    let mut rest = line;
    while let Some(idx) = rest.find("dev.azure.com") {
        rest = &rest[idx + "dev.azure.com".len()..];
        let org: String = rest
            .trim_start_matches(['/', ':'])
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if !org.is_empty() && !org.starts_with('-') && org != "acme" && org != "example" && org != "v3" {
            return Some(org);
        }
    }
    None
}

/// Extra forbidden substrings, lowercased: comma-separated `env` value first,
/// then the lines of the local patterns file (`#` lines and blanks skipped).
fn extra_patterns(env: Option<&str>, file: Option<&str>) -> Vec<String> {
    let from_env = env.unwrap_or_default().split(',');
    let from_file = file
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'));
    from_env
        .chain(from_file)
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect()
}

const REPO_URL: &str = "https://github.com/jeanboy44/cmux-cc-sessions-monitor";
const COPYRIGHT: &str = "Copyright (c) 2026 jeanboy44";
/// The public handle is intentionally public (repository URL, copyright). If a
/// maintainer adds it as an extra pattern, only these exact substrings are
/// exempt, in any file; anything else on the same line is still checked.
const PUBLIC: &[&str] = &[REPO_URL, COPYRIGHT];

fn strip_allowed(line: &str) -> String {
    PUBLIC.iter().fold(line.to_string(), |l, p| l.replace(p, ""))
}

/// Decides what to do with the result of listing files via git.
/// `Ok(Some(files))` = scan them, `Ok(None)` = skipped (explicitly allowed),
/// `Err` = the guard cannot run and must fail.
fn file_list_or_fail(git_result: Result<Vec<String>, String>, allow_no_git: bool) -> Result<Option<Vec<String>>, String> {
    match git_result {
        Ok(files) => Ok(Some(files)),
        Err(_) if allow_no_git => {
            eprintln!("WARNING: personal-info guard SKIPPED (git unavailable, ALLOW_NO_GIT set)");
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

fn findings_for(rel: &str, text: &str, extra: &[String]) -> Vec<String> {
    let builtin = rel != GUARD_FILE;
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let n = n + 1;
        let rest = strip_allowed(line);
        let lower = rest.to_lowercase();
        for pat in extra {
            if lower.contains(pat.as_str()) {
                // The pattern itself is not echoed: CI logs are public too.
                out.push(format!("{rel}:{n}: contains a locally forbidden pattern"));
            }
        }
        if !builtin {
            continue;
        }
        for pat in FORBIDDEN {
            if rest.contains(pat) {
                out.push(format!("{rel}:{n}: contains `{pat}`"));
            }
        }
        for u in uuids(line) {
            let lower = u.to_ascii_lowercase();
            if !UUID_FAKE_PREFIXES.iter().any(|p| lower.starts_with(p)) {
                out.push(format!("{rel}:{n}: real-looking UUID `{u}`"));
            }
        }
        if let Some(org) = bad_azure_org(line) {
            out.push(format!("{rel}:{n}: dev.azure.com org `{org}` (use acme/example)"));
        }
    }
    out
}

fn git_files(root: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(format!("git ls-files failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect())
}

#[test]
fn tracked_files_contain_no_personal_info() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let allow = std::env::var_os("ALLOW_NO_GIT").is_some();
    let files = match file_list_or_fail(git_files(root), allow) {
        Ok(Some(files)) => files,
        Ok(None) => return,
        Err(msg) => panic!("personal-info guard could not run: {msg} (set ALLOW_NO_GIT=1 to skip)"),
    };
    let env = std::env::var(ENV_PATTERNS).ok();
    let local = std::fs::read_to_string(root.join(LOCAL_PATTERNS_FILE)).ok();
    let extra = extra_patterns(env.as_deref(), local.as_deref());
    let mut findings = Vec::new();
    for rel in &files {
        let Ok(bytes) = std::fs::read(root.join(rel)) else { continue };
        let Ok(text) = String::from_utf8(bytes) else { continue };
        findings.extend(findings_for(rel, &text, &extra));
    }
    assert!(findings.is_empty(), "personal info found:\n{}", findings.join("\n"));
}

#[test]
fn local_patterns_file_is_gitignored() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ignore = std::fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
    assert!(ignore.lines().any(|l| l.trim() == LOCAL_PATTERNS_FILE), ".gitignore must list {LOCAL_PATTERNS_FILE}");
}

#[test]
fn git_failure_fails_unless_allowed() {
    let ok = file_list_or_fail(Ok(vec!["a".into()]), false);
    assert_eq!(ok, Ok(Some(vec!["a".to_string()])));
    assert!(file_list_or_fail(Err("no git".into()), false).is_err());
    assert_eq!(file_list_or_fail(Err("no git".into()), true), Ok(None));
    assert_eq!(file_list_or_fail(Ok(vec![]), true), Ok(Some(vec![])));
}

#[test]
fn allowed_substring_does_not_exempt_rest_of_line() {
    let none: &[String] = &[];
    let url = REPO_URL;
    assert!(findings_for("README.md", &format!("install {url}"), none).is_empty());
    assert!(!findings_for("README.md", &format!("{url} /Users/x"), none).is_empty());
    assert!(!findings_for("Cargo.toml", &format!("repository = \"{url}\" # /Users/x"), none).is_empty());
    let extra = vec!["corp-example".to_string()];
    assert!(!findings_for("LICENSE", &format!("{COPYRIGHT} corp-example"), &extra).is_empty());
    // A maintainer adding the public handle as a pattern keeps the exemptions.
    let handle = REPO_URL.split('/').nth(3).unwrap().to_string();
    let extra = vec![handle.clone()];
    assert!(findings_for("LICENSE", COPYRIGHT, &extra).is_empty());
    assert!(findings_for("README.md", &format!("install {url}"), &extra).is_empty());
    assert!(!findings_for("src/a.rs", &format!("// by {handle}"), &extra).is_empty());
}

#[test]
fn extra_patterns_come_from_env_and_local_file() {
    let file = "# comment\n\n  Corp-Example  \nsecret-team # trailing words stay\n";
    let got = extra_patterns(Some("Foo-Inc, ,bar-labs"), Some(file));
    assert_eq!(got, vec!["foo-inc", "bar-labs", "corp-example", "secret-team # trailing words stay"]);
    assert!(extra_patterns(None, None).is_empty());
    assert!(extra_patterns(Some(""), Some("# only comments\n")).is_empty());
}

#[test]
fn extra_patterns_match_case_insensitively() {
    let extra = extra_patterns(Some("corp-example"), None);
    assert!(!findings_for("a.md", "Made at CORP-Example", &extra).is_empty());
    assert!(findings_for("a.md", "Made at corp-sample", &extra).is_empty());
    // Without extras the built-in checks still run.
    assert!(findings_for("a.md", "Made at corp-example", &[]).is_empty());
    // Findings never echo the private pattern.
    assert!(findings_for("a.md", "corp-example", &extra).iter().all(|f| !f.contains("corp-example")));
}

#[test]
fn guard_file_itself_is_checked_for_extra_patterns() {
    let extra = vec!["corp-example".to_string()];
    // Built-in fixtures in the guard file are exempt ...
    assert!(findings_for(GUARD_FILE, "const X: &str = \"/Users/\";", &extra).is_empty());
    // ... but a maintainer's own patterns are not.
    assert!(!findings_for(GUARD_FILE, "const X: &str = \"corp-example\";", &extra).is_empty());
}

#[test]
fn detector_flags_and_allows_expected_cases() {
    let none: &[String] = &[];
    assert_eq!(uuids("id 3f2b8c1e-9a4d-4e7b-8c21-5d6e7f8a9b0c x").len(), 1);
    assert_eq!(uuids("AAAAAAAA-0000-0000-0000-000000000001").len(), 1);
    assert!(findings_for("a.md", "AAAAAAAA-0000-0000-0000-000000000001 00000000-0000-0000-0000-000000000002", none).is_empty());
    assert!(!findings_for("a.md", "3f2b8c1e-9a4d-4e7b-8c21-5d6e7f8a9b0c", none).is_empty());
    assert!(findings_for("a.md", "https://dev.azure.com/acme/p/_git/r", none).is_empty());
    assert!(!findings_for("a.md", "https://dev.azure.com/realorg/p/_git/r", none).is_empty());
    assert!(!findings_for("a.md", "/Users/someone/x", none).is_empty());
    assert!(findings_for("LICENSE", COPYRIGHT, none).is_empty());
}
