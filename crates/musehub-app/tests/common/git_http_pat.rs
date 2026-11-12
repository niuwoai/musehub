//! PAT-backed smart HTTP git helpers (0600 config, never token in remote URL).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Output;

use super::git_ssh_util::{git_timeout_secs, run_bounded};
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
                "[credential]\n\thelper =\n[http]\n\textraHeader = Authorization: Basic {basic}\n"
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

    pub fn run_git(&self, cwd: Option<&Path>, args: &[&str]) -> Output {
        let mut cmd = Command::new("git");
        if let Some(d) = cwd {
            cmd.current_dir(d);
        }
        cmd.env("GIT_CONFIG_GLOBAL", self.config_path.to_str().expect("cfg"))
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "")
            .args(args);
        run_bounded(cmd, git_timeout_secs()).into_output()
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
