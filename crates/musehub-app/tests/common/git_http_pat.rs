//! PAT-backed smart HTTP git helpers (0600 config, never token in remote URL).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
        }
    }

    pub async fn revoke(&self) {
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
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    fn redact(output: &str) -> String {
        let mut out = output.to_string();
        out = out.replace("mhp_", "[REDACTED_PAT_PREFIX]");
        out
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
        cmd.output().expect("spawn git")
    }

    pub fn run_git_ok(&self, cwd: Option<&Path>, args: &[&str]) {
        let out = self.run_git(cwd, args);
        assert!(
            out.status.success(),
            "{}",
            Self::redact(&format!(
                "git {} failed: {} {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr),
                String::from_utf8_lossy(&out.stdout)
            ))
        );
        let blob = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !blob.contains(&self.pat),
            "git output leaked PAT: {}",
            Self::redact(&blob)
        );
    }

    pub fn credential_free_url(http_base: &str, org: &str, repo: &str) -> String {
        format!("{http_base}/git/{org}/{repo}.git")
    }

    pub fn clone_repo(&self, url: &str, dest: &Path) {
        self.run_git_ok(None, &["clone", url, dest.to_str().unwrap()]);
        let cfg = dest.join(".git/config");
        let body = std::fs::read_to_string(cfg).expect("git config");
        assert!(
            !body.contains(&self.pat),
            "PAT must not appear in git config"
        );
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
}

impl Drop for GitHttpPatSandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.config_dir);
    }
}
