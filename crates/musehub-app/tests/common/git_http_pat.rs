//! PAT-backed smart HTTP git helpers (0600 config, never token in remote URL).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use super::git_ssh_util::{git_timeout_secs, run_bounded};

fn shell_word(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+:@%/=".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}
use std::sync::atomic::{AtomicBool, Ordering};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

pub struct GitHttpPatSandbox {
    pub pat: String,
    pub token_id: Uuid,
    pub config_dir: PathBuf,
    pub config_path: PathBuf,
    cookie: String,
    app: axum::Router,
    revoked: AtomicBool,
}

impl GitHttpPatSandbox {
    pub async fn mint(
        app: &axum::Router,
        cookie: &str,
        org: &str,
        repo_id: Uuid,
        scopes: &[&str],
    ) -> Self {
        let scopes_json: Vec<String> = scopes.iter().map(|s| (*s).to_string()).collect();
        let body = serde_json::json!({
            "name": format!("e2e-{}", Uuid::new_v4().simple()),
            "org": org,
            "scopes": scopes_json,
            "scope_mode": "selected",
            "repo_ids": [repo_id],
        });
        let pat_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/tokens")
                    .header(header::COOKIE, cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let st = pat_resp.status();
        assert!(
            st == StatusCode::CREATED || st == StatusCode::OK,
            "unexpected mint status {st}"
        );
        let pat_json: serde_json::Value =
            serde_json::from_slice(&pat_resp.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let pat = pat_json["token"].as_str().expect("pat").to_string();
        let token_id = Uuid::parse_str(pat_json["id"].as_str().expect("token id")).expect("uuid");
        let config_dir = tempfile::tempdir().expect("git cfg dir").keep();
        std::fs::set_permissions(&config_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let config_path = config_dir.join("config");
        let basic = base64::engine::general_purpose::STANDARD.encode(format!("admin:{pat}"));
        std::fs::write(
            &config_path,
            format!(
                "[credential]\n\thelper =\n[http]\n\texpect = false\n\textraHeader = Authorization: Basic {basic}\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        Self {
            pat,
            token_id,
            config_dir,
            config_path,
            cookie: cookie.to_string(),
            app: app.clone(),
            revoked: AtomicBool::new(false),
        }
    }

    pub fn redact_output(output: &str, pat: &str) -> String {
        let mut out = output.to_string();
        if !pat.is_empty() {
            out = out.replace(pat, "[REDACTED_PAT]");
            let basic = base64::engine::general_purpose::STANDARD.encode(format!("admin:{pat}"));
            out = out.replace(&basic, "[REDACTED_BASIC]");
            let enc = urlencoding::encode(pat);
            if !enc.is_empty() {
                out = out.replace(enc.as_ref(), "[REDACTED_PAT]");
            }
        }
        while let Some(start) = out.find("mhp_") {
            let rest = &out[start + 4..];
            let len = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .count();
            let end = start + 4 + len;
            if end > start + 4 {
                out.replace_range(start..end, "[REDACTED_PAT]");
            } else {
                break;
            }
        }
        out
    }

    fn assert_no_pat_leak(&self, blob: &str) {
        assert!(
            !blob.contains(&self.pat),
            "leaked PAT: {}",
            Self::redact_output(blob, &self.pat)
        );
        let basic = base64::engine::general_purpose::STANDARD.encode(format!("admin:{}", self.pat));
        assert!(!blob.contains(&basic), "leaked basic auth");
    }

    fn shell_for_blocking(&self) -> Self {
        Self {
            pat: self.pat.clone(),
            token_id: self.token_id,
            config_dir: self.config_dir.clone(),
            config_path: self.config_path.clone(),
            cookie: String::new(),
            app: axum::Router::new(),
            revoked: AtomicBool::new(true),
        }
    }

    pub async fn revoke(&self) {
        if self.revoked.swap(true, Ordering::SeqCst) {
            return;
        }
        let resp = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/tokens/{}", self.token_id))
                    .header(header::COOKIE, &self.cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("revoke token request");
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    fn http_extra_header_config(&self) -> String {
        let basic = base64::engine::general_purpose::STANDARD.encode(format!("admin:{}", self.pat));
        format!("http.extraHeader=Authorization: Basic {basic}")
    }

    fn build_git_command(&self, cwd: Option<&Path>, args: &[&str]) -> Command {
        let header_cfg = self.http_extra_header_config();
        let git_global = self.config_path.to_str().expect("cfg");
        let mut inner = Command::new("git");
        if let Some(d) = cwd {
            inner.current_dir(d);
        }
        inner
            .args(["-c", &header_cfg])
            .env("GIT_CONFIG_GLOBAL", git_global)
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "")
            .env_remove("GIT_TRACE")
            .env_remove("GIT_TRACE_PACKET")
            .env_remove("GIT_CURL_VERBOSE");
        if std::env::var("MUSEHUB_TEST_GIT_HTTP_TRACE").is_ok() {
            inner
                .env("GIT_TRACE", "1")
                .env("GIT_TRACE_PACKET", "1")
                .env("GIT_CURL_VERBOSE", "1");
        }
        inner.args(args);
        if let Ok(group) = std::env::var("MUSEHUB_GIT_STORAGE_GROUP") {
            if !group.is_empty()
                && Command::new("sg")
                    .arg("--help")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
            {
                let mut script = format!(
                    "export GIT_CONFIG_GLOBAL={} GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0 GIT_ASKPASS=",
                    shell_word(git_global),
                );
                if let Some(d) = cwd {
                    script.push_str(&format!("; cd {}", shell_word(&d.display().to_string())));
                }
                script.push_str(&format!("; git -c {}", shell_word(&header_cfg)));
                for arg in args {
                    script.push_str(&format!(" {}", shell_word(arg)));
                }
                if std::env::var("MUSEHUB_TEST_GIT_HTTP_TRACE").is_ok() {
                    script = format!(
                        "export GIT_TRACE=1 GIT_TRACE_PACKET=1 GIT_CURL_VERBOSE=1; {script}"
                    );
                }
                let mut cmd = Command::new("sg");
                cmd.arg(&group).arg("-c").arg(script);
                return cmd;
            }
        }
        inner
    }

    pub fn run_git(&self, cwd: Option<&Path>, args: &[&str]) -> Output {
        let cmd = self.build_git_command(cwd, args);
        let bounded = run_bounded(cmd, git_timeout_secs());
        let timed_out = bounded.timed_out;
        let mut out = bounded.into_output();
        if timed_out {
            let note = format!(
                "\n[git subprocess timed out after {}s]\n",
                git_timeout_secs()
            );
            out.stderr.extend_from_slice(note.as_bytes());
        }
        out
    }

    pub fn run_git_ok(&self, cwd: Option<&Path>, args: &[&str]) {
        let out = self.run_git(cwd, args);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "{}",
            Self::redact_output(
                &format!("git {} failed: {} {}", args.join(" "), stderr, stdout),
                &self.pat
            )
        );
        self.assert_no_pat_leak(&format!("{stdout}{stderr}"));
    }

    pub fn credential_free_url(http_base: &str, org: &str, repo: &str) -> String {
        format!("{http_base}/git/{org}/{repo}.git")
    }

    /// Loopback-only: git subprocesses may ignore `http.extraHeader` for some hosts;
    /// embed PAT in URL for alternating HTTP/SSH e2e (never log or persist).
    pub fn loopback_authenticated_url(http_base: &str, org: &str, repo: &str, pat: &str) -> String {
        let host = http_base
            .strip_prefix("http://")
            .or_else(|| http_base.strip_prefix("https://"))
            .unwrap_or(http_base);
        let user = urlencoding::encode("admin");
        let pass = urlencoding::encode(pat);
        format!("http://{user}:{pass}@{host}/git/{org}/{repo}.git")
    }

    pub fn clone_repo(&self, url: &str, dest: &Path) {
        self.run_git_ok(None, &["clone", url, dest.to_str().unwrap()]);
        let cfg = dest.join(".git/config");
        let body = std::fs::read_to_string(cfg).expect("git config");
        self.assert_no_pat_leak(&body);
        assert!(
            !body.contains("extraHeader"),
            "global auth must not leak into repo config"
        );
    }

    /// Git subprocess polls block a Tokio worker; run clone off the async pool so axum can serve.
    pub async fn clone_repo_async(&self, url: &str, dest: &Path) {
        let url = url.to_string();
        let dest = dest.to_path_buf();
        let sandbox = self.shell_for_blocking();
        tokio::task::spawn_blocking(move || sandbox.clone_repo(&url, &dest))
            .await
            .expect("clone join")
    }

    pub fn commit_all(&self, work: &Path, msg: &str) {
        self.run_git_ok(
            Some(work),
            &[
                "-c",
                "user.email=e2e@musehub.local",
                "-c",
                "user.name=e2e",
                "commit",
                "-m",
                msg,
                "--no-gpg-sign",
            ],
        );
    }

    pub fn push_branch(&self, work: &Path, branch: &str) -> String {
        self.run_git_ok(
            Some(work),
            &["push", "origin", &format!("HEAD:refs/heads/{branch}")],
        );
        let out = self.run_git(Some(work), &["rev-parse", "HEAD"]);
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    pub fn fetch_branch(&self, work: &Path, branch: &str) {
        self.run_git_ok(Some(work), &["fetch", "origin", branch]);
    }

    pub fn merge_ff_only(&self, work: &Path, branch: &str) {
        self.run_git_ok(
            Some(work),
            &["merge", "--ff-only", &format!("origin/{branch}")],
        );
    }

    pub async fn push_branch_async(&self, work: &Path, branch: &str) -> String {
        let work = work.to_path_buf();
        let branch = branch.to_string();
        let sandbox = self.shell_for_blocking();
        tokio::task::spawn_blocking(move || sandbox.push_branch(&work, &branch))
            .await
            .expect("push join")
    }

    pub async fn fetch_branch_async(&self, work: &Path, branch: &str) {
        let work = work.to_path_buf();
        let branch = branch.to_string();
        let sandbox = self.shell_for_blocking();
        tokio::task::spawn_blocking(move || sandbox.fetch_branch(&work, &branch))
            .await
            .expect("fetch join")
    }

    pub async fn merge_ff_only_async(&self, work: &Path, branch: &str) {
        let work = work.to_path_buf();
        let branch = branch.to_string();
        let sandbox = self.shell_for_blocking();
        tokio::task::spawn_blocking(move || sandbox.merge_ff_only(&work, &branch))
            .await
            .expect("merge join")
    }
}

impl Drop for GitHttpPatSandbox {
    fn drop(&mut self) {
        if !self.revoked.load(Ordering::SeqCst) {
            let app = self.app.clone();
            let cookie = self.cookie.clone();
            let token_id = self.token_id;
            // Best-effort revoke on panic/early exit. `revoke().await` in tests is required for
            // a guaranteed cleanup before process exit; Drop may not finish if the runtime stops first.
            let _ = std::thread::spawn(move || {
                if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    let _ = rt.block_on(async move {
                        app.oneshot(
                            Request::builder()
                                .method("DELETE")
                                .uri(format!("/api/v1/tokens/{token_id}"))
                                .header(header::COOKIE, cookie)
                                .body(Body::empty())
                                .unwrap(),
                        )
                        .await
                    });
                }
            })
            .join();
        }
        let _ = std::fs::remove_dir_all(&self.config_dir);
    }
}
