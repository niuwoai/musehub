//! Runs a versioned MuseHub workflow in disposable, resource-limited Docker jobs.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const MAX_JOBS: usize = 32;
const MAX_PARALLEL_JOBS: usize = 4;
const MAX_JOB_OUTPUT_BYTES: usize = 512 * 1024;
const MAX_TOTAL_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ARTIFACTS_PER_JOB: usize = 16;
const MAX_CACHE_PATHS: usize = 16;
const MAX_CACHE_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CACHE_FILES: usize = 8192;
const MAX_CACHE_ENTRIES: usize = 32;
const MAX_CACHE_TOTAL_ENTRIES: usize = 512;
const MAX_CACHE_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const CACHE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const JOB_IMAGE_SUFFIX: &str = "@sha256:";
const DOCKER_CLEANUP_TIMEOUT: Duration = Duration::from_secs(15);
const DOCKER_CLEANUP_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowEnvelope {
    definition: WorkflowDefinition,
    sha256: String,
    topological_order: Vec<String>,
    execution_layers: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkflowDefinition {
    version: u8,
    events: Vec<String>,
    jobs: Vec<WorkflowJob>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkflowJob {
    id: String,
    needs: Vec<String>,
    image: String,
    command: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    artifacts: Vec<WorkflowArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reports: Option<WorkflowReports>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache: Option<WorkflowCache>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkflowArtifact {
    name: String,
    path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkflowReports {
    junit: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkflowCache {
    key: String,
    lockfile: String,
    paths: Vec<String>,
}

#[derive(Clone)]
struct Config {
    docker_bin: PathBuf,
    workspace_root: PathBuf,
    workspace_host_root: PathBuf,
    allowed_images: Vec<String>,
    bare_git: PathBuf,
    artifact_root: Option<PathBuf>,
    org_id: Uuid,
    repo_id: Uuid,
    trust_class: String,
    attempt_ids: std::collections::BTreeMap<String, Uuid>,
    source_oid: String,
    run_id: Uuid,
    runner_id: Uuid,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let result = match args.next().as_deref() {
        Some("--cleanup") => cleanup_orphaned_jobs(None),
        Some("--cancel") => args
            .next()
            .ok_or("run_id_missing")
            .and_then(|value| value.parse::<Uuid>().map_err(|_| "run_id_invalid"))
            .and_then(|run_id| cleanup_orphaned_jobs(Some(run_id))),
        _ => run(),
    };
    if let Err(code) = result {
        eprintln!("musehub pipeline job failed: {code}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), &'static str> {
    let config = load_config()?;
    let workflow_json = std::env::var("MUSEHUB_PIPELINE_WORKFLOW_JSON")
        .map_err(|_| "workflow_definition_missing")?;
    if workflow_json.len() > 128 * 1024 {
        return Err("workflow_definition_too_large");
    }
    let workflow: WorkflowEnvelope =
        serde_json::from_str(&workflow_json).map_err(|_| "workflow_definition_invalid")?;
    validate_workflow(&workflow, &config.allowed_images)?;
    let selected = match std::env::var("MUSEHUB_PIPELINE_WORKFLOW_SELECTED_KEYS") {
        Ok(value) => {
            serde_json::from_str::<Vec<String>>(&value).map_err(|_| "workflow_selection_invalid")?
        }
        Err(_) => workflow
            .definition
            .jobs
            .iter()
            .map(|job| job.id.clone())
            .collect(),
    };
    let succeeded = match std::env::var("MUSEHUB_PIPELINE_WORKFLOW_SUCCEEDED_KEYS") {
        Ok(value) => {
            serde_json::from_str::<Vec<String>>(&value).map_err(|_| "workflow_selection_invalid")?
        }
        Err(_) => Vec::new(),
    };
    if selected.is_empty() {
        return Err("workflow_selection_empty");
    }
    let selected_set = selected
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let succeeded_set = succeeded
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    if selected_set.len() != selected.len()
        || selected
            .iter()
            .any(|id| !workflow.definition.jobs.iter().any(|job| &job.id == id))
        || succeeded
            .iter()
            .any(|id| !workflow.definition.jobs.iter().any(|job| &job.id == id))
    {
        return Err("workflow_selection_invalid");
    }
    for job in &workflow.definition.jobs {
        if selected_set.contains(&job.id)
            && job.needs.iter().any(|dependency| {
                !selected_set.contains(dependency) && !succeeded_set.contains(dependency)
            })
        {
            return Err("workflow_selection_dependency_missing");
        }
        if selected_set.contains(&job.id)
            && (!job.artifacts.is_empty() || job.reports.is_some())
            && !config.attempt_ids.contains_key(&job.id)
        {
            return Err("workflow_attempt_id_missing");
        }
    }

    println!("workflow_sha256={}", workflow.sha256);
    let mut total_output = 0usize;
    for layer in &workflow.execution_layers {
        let mut jobs = Vec::with_capacity(layer.len());
        for job_id in layer {
            if !selected_set.contains(job_id) {
                continue;
            }
            let job = workflow
                .definition
                .jobs
                .iter()
                .find(|job| &job.id == job_id)
                .ok_or("workflow_layer_invalid")?;
            jobs.push(job.clone());
        }
        if jobs.is_empty() {
            continue;
        }
        for batch in jobs.chunks(MAX_PARALLEL_JOBS) {
            let results = thread::scope(|scope| {
                let handles = batch
                    .iter()
                    .map(|job| {
                        let config = config.clone();
                        scope.spawn(move || execute_job(&config, job))
                    })
                    .collect::<Vec<_>>();
                handles
                    .into_iter()
                    .map(|handle| handle.join().unwrap_or(Err("job_thread_panicked")))
                    .collect::<Vec<_>>()
            });

            let mut results = results;
            results.sort_by(|left, right| job_result_id(left).cmp(job_result_id(right)));
            for result in results {
                let (job_id, output, truncated) = result?;
                println!("--- job {job_id} stdout ---");
                print_limited(&output.stdout, &mut total_output);
                println!("--- job {job_id} stderr ---");
                print_limited(&output.stderr, &mut total_output);
                let workflow_log = serde_json::json!({
                    "job_key": job_id,
                    "stdout": String::from_utf8_lossy(&output.stdout),
                    "stderr": String::from_utf8_lossy(&output.stderr),
                });
                println!("MUSEHUB_WORKFLOW_LOG:{}:{}", config.run_id, workflow_log);
                if truncated {
                    println!("[job output truncated at configured limit]");
                }
                if !output.status.success() {
                    println!("job {job_id} failed with status {}", output.status);
                    return Err("workflow_job_failed");
                }
            }
        }
    }
    println!("workflow_result=succeeded");
    Ok(())
}

fn load_config() -> Result<Config, &'static str> {
    let docker_bin = std::env::var_os("MUSEHUB_JOB_DOCKER_BIN")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("job_docker_bin_not_configured")?;
    let workspace_root = required_absolute_path("MUSEHUB_JOB_WORKSPACE_ROOT")?;
    let workspace_host_root = required_absolute_path("MUSEHUB_JOB_WORKSPACE_HOST_ROOT")?;
    let allowed_images = std::env::var("MUSEHUB_JOB_ALLOWED_IMAGES")
        .map_err(|_| "job_image_allowlist_not_configured")?
        .split(',')
        .map(str::trim)
        .filter(|image| !image.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if allowed_images.is_empty() {
        return Err("job_image_allowlist_not_configured");
    }
    let bare_git = std::env::var_os("MUSEHUB_PIPELINE_BARE_GIT")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("bare_git_path_invalid")?;
    let source_oid =
        std::env::var("MUSEHUB_PIPELINE_SOURCE_OID").map_err(|_| "source_oid_missing")?;
    if source_oid.len() != 40 || !source_oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("source_oid_invalid");
    }
    let run_id = std::env::var("MUSEHUB_PIPELINE_RUN_ID")
        .map_err(|_| "run_id_missing")?
        .parse()
        .map_err(|_| "run_id_invalid")?;
    let runner_id = std::env::var("MUSEHUB_RUNNER_INSTANCE_ID")
        .map_err(|_| "runner_id_missing")?
        .parse()
        .map_err(|_| "runner_id_invalid")?;
    let org_id = std::env::var("MUSEHUB_PIPELINE_ORG_ID")
        .map_err(|_| "org_id_missing")?
        .parse()
        .map_err(|_| "org_id_invalid")?;
    let repo_id = std::env::var("MUSEHUB_PIPELINE_REPO_ID")
        .map_err(|_| "repo_id_missing")?
        .parse()
        .map_err(|_| "repo_id_invalid")?;
    let trust_class =
        std::env::var("MUSEHUB_PIPELINE_TRUST_CLASS").map_err(|_| "trust_class_missing")?;
    if !matches!(trust_class.as_str(), "trusted" | "untrusted") {
        return Err("trust_class_invalid");
    }
    let artifact_root = std::env::var_os("MUSEHUB_ARTIFACT_ROOT")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute()
                && !path
                    .components()
                    .any(|component| component == std::path::Component::ParentDir)
            {
                Ok(path)
            } else {
                Err("artifact_root_invalid")
            }
        })
        .transpose()?;
    let attempt_ids = std::env::var("MUSEHUB_PIPELINE_WORKFLOW_ATTEMPT_IDS")
        .ok()
        .map(|value| serde_json::from_str(&value).map_err(|_| "workflow_attempt_ids_invalid"))
        .transpose()?
        .unwrap_or_default();
    Ok(Config {
        docker_bin,
        workspace_root,
        workspace_host_root,
        allowed_images,
        bare_git,
        artifact_root,
        org_id,
        repo_id,
        trust_class,
        attempt_ids,
        source_oid,
        run_id,
        runner_id,
    })
}

fn required_absolute_path(name: &str) -> Result<PathBuf, &'static str> {
    let path = std::env::var_os(name)
        .map(PathBuf::from)
        .ok_or("job_workspace_path_missing")?;
    if !path.is_absolute()
        || path
            .components()
            .any(|component| component == std::path::Component::ParentDir)
    {
        return Err("job_workspace_path_invalid");
    }
    Ok(path)
}

fn validate_workflow(
    workflow: &WorkflowEnvelope,
    allowed_images: &[String],
) -> Result<(), &'static str> {
    if workflow.definition.version != 1
        || workflow.definition.jobs.is_empty()
        || workflow.definition.jobs.len() > MAX_JOBS
        || workflow.definition.events.is_empty()
        || workflow.sha256.len() != 64
        || !workflow.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("workflow_definition_invalid");
    }
    let mut ids: Vec<&str> = Vec::with_capacity(workflow.definition.jobs.len());
    for job in &workflow.definition.jobs {
        let artifact_names = job
            .artifacts
            .iter()
            .map(|artifact| artifact.name.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let cache_invalid = job.cache.as_ref().is_some_and(|cache| {
            let paths = cache
                .paths
                .iter()
                .collect::<std::collections::BTreeSet<_>>();
            cache.key.is_empty()
                || cache.key.len() > 128
                || !cache
                    .key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                || !valid_workspace_path(&cache.lockfile)
                || cache.paths.is_empty()
                || cache.paths.len() > MAX_CACHE_PATHS
                || paths.len() != cache.paths.len()
                || cache_paths_overlap(&cache.paths)
                || cache.paths.iter().any(|path| !valid_workspace_path(path))
        });
        if !valid_job_id(&job.id)
            || job.needs.len() > MAX_JOBS
            || job.command.is_empty()
            || job.command.len() > 64
            || job
                .command
                .iter()
                .any(|arg| arg.is_empty() || arg.len() > 1024 || arg.contains('\0'))
            || !valid_digest_image(&job.image)
            || !allowed_images.iter().any(|allowed| allowed == &job.image)
            || job.artifacts.len() > MAX_ARTIFACTS_PER_JOB
            || artifact_names.len() != job.artifacts.len()
            || job.artifacts.iter().any(|artifact| {
                !valid_artifact_name(&artifact.name) || !valid_workspace_path(&artifact.path)
            })
            || job.reports.as_ref().is_some_and(|reports| {
                !valid_workspace_path(&reports.junit) || artifact_names.contains("junit-report.xml")
            })
            || cache_invalid
        {
            return Err("workflow_job_invalid_or_image_not_allowed");
        }
        if ids.contains(&job.id.as_str()) {
            return Err("workflow_duplicate_job");
        }
        ids.push(job.id.as_str());
    }
    let mut layer_ids = Vec::new();
    for layer in &workflow.execution_layers {
        if layer.is_empty() || layer.len() > MAX_JOBS {
            return Err("workflow_layer_invalid");
        }
        for id in layer {
            if !ids.contains(&id.as_str()) || layer_ids.contains(&id.as_str()) {
                return Err("workflow_layer_invalid");
            }
            layer_ids.push(id.as_str());
        }
    }
    if layer_ids.len() != ids.len()
        || workflow
            .topological_order
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != layer_ids
    {
        return Err("workflow_layer_incomplete");
    }
    let mut positions = std::collections::BTreeMap::new();
    for (index, id) in layer_ids.iter().enumerate() {
        positions.insert(*id, index);
    }
    for job in &workflow.definition.jobs {
        for dependency in &job.needs {
            if !ids.contains(&dependency.as_str()) || dependency == &job.id {
                return Err("workflow_dependency_invalid");
            }
            if positions.get(dependency.as_str()) >= positions.get(job.id.as_str()) {
                return Err("workflow_dependency_order_invalid");
            }
        }
    }
    let canonical =
        serde_json::to_vec(&workflow.definition).map_err(|_| "workflow_definition_invalid")?;
    let actual_hash = format!("{:x}", Sha256::digest(canonical));
    if workflow.sha256 != actual_hash {
        return Err("workflow_hash_mismatch");
    }
    Ok(())
}

fn valid_job_id(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some('a'..='z'))
        && value.len() <= 64
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
}

fn valid_artifact_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn valid_workspace_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && !value.starts_with('/')
        && !value.contains('\\')
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && part != ".git")
}

fn cache_paths_overlap(paths: &[String]) -> bool {
    paths.iter().enumerate().any(|(index, path)| {
        paths.iter().skip(index + 1).any(|other| {
            path.starts_with(&format!("{other}/")) || other.starts_with(&format!("{path}/"))
        })
    })
}

fn valid_digest_image(value: &str) -> bool {
    let Some((name, digest)) = value.split_once(JOB_IMAGE_SUFFIX) else {
        return false;
    };
    !name.is_empty()
        && !name.chars().any(char::is_whitespace)
        && digest.len() == 64
        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn execute_job(config: &Config, job: &WorkflowJob) -> Result<(String, Output, bool), &'static str> {
    let result = (|| {
        let job_root = config
            .workspace_root
            .join(config.run_id.to_string())
            .join(&job.id);
        let host_job_root = config
            .workspace_host_root
            .join(config.run_id.to_string())
            .join(&job.id);
        if job_root.exists() {
            std::fs::remove_dir_all(&job_root).map_err(|_| "job_workspace_cleanup_failed")?;
        }
        std::fs::create_dir_all(job_root.parent().ok_or("job_workspace_path_invalid")?)
            .map_err(|_| "job_workspace_create_failed")?;
        let _workspace_guard = WorkspaceGuard(job_root.clone());
        stage_source(config, &job_root)?;
        let cache = job
            .cache
            .as_ref()
            .and_then(|cache| cache_identity(config, job, cache, &job_root));
        let cache_hit = cache
            .as_ref()
            .is_some_and(|identity| restore_workflow_cache(identity, job, &job_root));
        println!(
            "workflow_cache={}:{}",
            job.id,
            if cache_hit { "hit" } else { "miss" }
        );
        let attempt_id = config.attempt_ids.get(&job.id).copied();
        if !job.artifacts.is_empty() || job.reports.is_some() {
            mark_workflow_attempt_started(config, attempt_id)?;
        }
        let (output, truncated) = docker_run(config, job, &job_root, &host_job_root)?;
        let attempt_id = config.attempt_ids.get(&job.id).copied();
        collect_declared_outputs(config, job, &job_root, attempt_id)?;
        if output.status.success() {
            if let (Some(identity), Some(cache)) = (cache.as_ref(), job.cache.as_ref()) {
                if publish_workflow_cache(identity, cache, &job_root) {
                    println!("workflow_cache_published={}", job.id);
                } else {
                    println!("workflow_cache_publish_skipped={}", job.id);
                }
            }
        }
        Ok((job.id.clone(), output, truncated))
    })();
    let state = match &result {
        Ok((_, output, _)) if output.status.success() => "succeeded",
        _ => "failed",
    };
    println!(
        "MUSEHUB_WORKFLOW_RESULT:{}:{}:{}",
        config.run_id, job.id, state
    );
    result
}

#[derive(Clone)]
struct CacheIdentity {
    root: PathBuf,
    namespace: PathBuf,
    entry: PathBuf,
    lock: PathBuf,
}

struct CacheLock(File);

impl CacheLock {
    fn acquire(identity: &CacheIdentity) -> Option<Self> {
        ensure_cache_directory(&identity.root).ok()?;
        ensure_cache_subdirectories(&identity.root, &identity.namespace).ok()?;
        let parent = identity.lock.parent()?;
        ensure_cache_subdirectories(&identity.root, parent).ok()?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&identity.lock).ok()?;
        #[cfg(unix)]
        if unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&file), libc::LOCK_EX) } != 0 {
            return None;
        }
        Some(Self(file))
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::flock(std::os::fd::AsRawFd::as_raw_fd(&self.0), libc::LOCK_UN);
        }
    }
}

fn cache_identity(
    config: &Config,
    job: &WorkflowJob,
    cache: &WorkflowCache,
    workspace: &Path,
) -> Option<CacheIdentity> {
    let artifact_root = config.artifact_root.as_deref()?;
    let lockfile = read_workspace_file(workspace, &cache.lockfile).ok()?;
    let lockfile_hash = format!("{:x}", Sha256::digest(lockfile));
    let key_material = format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{}",
        config.org_id,
        config.repo_id,
        config.trust_class,
        job.image,
        lockfile_hash,
        cache.key,
        cache.paths.join("\0")
    );
    let key_hash = format!("{:x}", Sha256::digest(key_material.as_bytes()));
    let root = artifact_root.join(".workflow-cache");
    let namespace = root
        .join(config.org_id.to_string())
        .join(config.repo_id.to_string())
        .join(&config.trust_class);
    Some(CacheIdentity {
        lock: root.join(".locks").join("global.lock"),
        entry: namespace.join(key_hash),
        namespace,
        root,
    })
}

fn restore_workflow_cache(identity: &CacheIdentity, job: &WorkflowJob, workspace: &Path) -> bool {
    let Some(cache) = job.cache.as_ref() else {
        return false;
    };
    let Some(_lock) = CacheLock::acquire(identity) else {
        return false;
    };
    if !cache_entry_valid(identity) {
        if fs::symlink_metadata(&identity.entry)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        {
            let _ = fs::remove_dir_all(&identity.entry);
        }
        return false;
    }
    let data = identity.entry.join("data");
    let mut targets = Vec::new();
    for path in &cache.paths {
        let source = data.join(path);
        if fs::symlink_metadata(&source).is_err() {
            continue;
        }
        let Some(destination) = safe_workspace_destination(workspace, path, true) else {
            return false;
        };
        if fs::symlink_metadata(&destination).is_ok() {
            return false;
        }
        targets.push((path.clone(), destination));
    }
    if targets.is_empty() {
        return false;
    }
    let staging = workspace.join(format!(".musehub-cache-restore-{}", Uuid::new_v4()));
    if fs::create_dir(&staging).is_err() {
        return false;
    }
    let mut budget = CacheBudget::default();
    for (path, _) in &targets {
        let source = data.join(path);
        let Some(destination) = safe_workspace_destination(&staging, path, true) else {
            let _ = fs::remove_dir_all(&staging);
            return false;
        };
        if copy_cache_entry(&source, &destination, &mut budget).is_err() {
            let _ = fs::remove_dir_all(&staging);
            return false;
        }
    }
    let mut installed = Vec::<PathBuf>::new();
    for (path, destination) in &targets {
        let Some(staged) = safe_workspace_destination(&staging, path, false) else {
            for path in installed {
                let _ = remove_cache_target(&path);
            }
            let _ = fs::remove_dir_all(&staging);
            return false;
        };
        if let Some(parent) = destination.parent() {
            if fs::create_dir_all(parent).is_err() {
                for path in installed {
                    let _ = remove_cache_target(&path);
                }
                let _ = fs::remove_dir_all(&staging);
                return false;
            }
        }
        if fs::rename(staged, destination).is_err() {
            for path in installed {
                let _ = remove_cache_target(&path);
            }
            let _ = fs::remove_dir_all(&staging);
            return false;
        }
        installed.push(destination.clone());
    }
    let _ = fs::remove_dir_all(&staging);
    true
}

fn publish_workflow_cache(
    identity: &CacheIdentity,
    cache: &WorkflowCache,
    workspace: &Path,
) -> bool {
    let Some(_lock) = CacheLock::acquire(identity) else {
        return false;
    };
    if std::fs::symlink_metadata(&identity.entry)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
    {
        return true;
    }
    let staging = identity.namespace.join(format!(
        ".cache-{}.{}.tmp",
        identity
            .entry
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("entry"),
        Uuid::new_v4()
    ));
    if fs::create_dir(&staging).is_err() {
        return false;
    }
    let data = staging.join("data");
    if fs::create_dir(&data).is_err() {
        let _ = fs::remove_dir_all(&staging);
        return false;
    }
    let mut budget = CacheBudget::default();
    for path in &cache.paths {
        let Some(source) = safe_workspace_destination(workspace, path, false) else {
            continue;
        };
        if std::fs::symlink_metadata(&source).is_err() {
            continue;
        }
        let Some(destination) = safe_workspace_destination(&data, path, true) else {
            let _ = fs::remove_dir_all(&staging);
            return false;
        };
        if copy_cache_entry(&source, &destination, &mut budget).is_err() {
            let _ = fs::remove_dir_all(&staging);
            return false;
        }
    }
    if budget.files == 0 {
        let _ = fs::remove_dir_all(&staging);
        return false;
    }
    let Some(digest) = cache_tree_digest(&data) else {
        let _ = fs::remove_dir_all(&staging);
        return false;
    };
    let manifest_path = staging.join(".manifest");
    let Ok(mut manifest) = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&manifest_path)
    else {
        let _ = fs::remove_dir_all(&staging);
        return false;
    };
    if manifest
        .write_all(digest.as_bytes())
        .and_then(|_| manifest.sync_all())
        .is_err()
    {
        let _ = fs::remove_dir_all(&staging);
        return false;
    }
    drop(manifest);
    if fs::rename(&staging, &identity.entry).is_err() {
        let _ = fs::remove_dir_all(&staging);
        return false;
    }
    enforce_cache_capacity(identity);
    true
}

#[derive(Default)]
struct CacheBudget {
    files: usize,
    bytes: u64,
}

fn ensure_cache_directory(path: &Path) -> Result<(), ()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|_| ())
        }
        Err(_) => Err(()),
    }
}

fn ensure_cache_subdirectories(root: &Path, target: &Path) -> Result<(), ()> {
    let relative = target.strip_prefix(root).map_err(|_| ())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if let std::path::Component::Normal(component) = component {
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => return Err(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&current).map_err(|_| ())?
                }
                Err(_) => return Err(()),
            }
        } else {
            return Err(());
        }
    }
    Ok(())
}

fn cache_entry_valid(identity: &CacheIdentity) -> bool {
    let Ok(metadata) = fs::symlink_metadata(&identity.entry) else {
        return false;
    };
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata
            .modified()
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_none_or(|age| age > CACHE_TTL)
    {
        return false;
    }
    let manifest = identity.entry.join(".manifest");
    let Ok(metadata) = fs::symlink_metadata(&manifest) else {
        return false;
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 128 {
        return false;
    }
    let Ok(expected) = fs::read_to_string(manifest) else {
        return false;
    };
    cache_tree_digest(&identity.entry.join("data")).is_some_and(|actual| actual == expected)
}

fn cache_tree_digest(root: &Path) -> Option<String> {
    let mut files = Vec::<(String, Vec<u8>)>::new();
    let mut budget = CacheBudget::default();
    collect_cache_files(root, root, &mut budget, &mut files).ok()?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    for (relative, bytes) in files {
        digest.update((relative.len() as u64).to_be_bytes());
        digest.update(relative.as_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(&bytes);
    }
    Some(format!("{:x}", digest.finalize()))
}

fn collect_cache_files(
    root: &Path,
    current: &Path,
    budget: &mut CacheBudget,
    files: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), ()> {
    let metadata = fs::symlink_metadata(current).map_err(|_| ())?;
    if metadata.file_type().is_symlink() {
        return Err(());
    }
    if metadata.is_file() {
        budget.files += 1;
        budget.bytes = budget.bytes.checked_add(metadata.len()).ok_or(())?;
        if budget.files > MAX_CACHE_FILES || budget.bytes > MAX_CACHE_ENTRY_BYTES {
            return Err(());
        }
        let relative = current
            .strip_prefix(root)
            .map_err(|_| ())?
            .to_str()
            .ok_or(())?
            .replace('\\', "/");
        let bytes = read_workspace_file(root, &relative).map_err(|_| ())?;
        files.push((relative, bytes));
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(());
    }
    let mut entries = fs::read_dir(current)
        .map_err(|_| ())?
        .map(|entry| entry.map_err(|_| ()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        collect_cache_files(root, &entry.path(), budget, files)?;
    }
    Ok(())
}

fn copy_cache_entry(source: &Path, destination: &Path, budget: &mut CacheBudget) -> Result<(), ()> {
    let metadata = fs::symlink_metadata(source).map_err(|_| ())?;
    if metadata.file_type().is_symlink() {
        return Err(());
    }
    if metadata.is_dir() {
        fs::create_dir_all(destination).map_err(|_| ())?;
        let mut entries = fs::read_dir(source)
            .map_err(|_| ())?
            .map(|entry| entry.map_err(|_| ()))
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            copy_cache_entry(&entry.path(), &destination.join(entry.file_name()), budget)?;
        }
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(());
    }
    budget.files += 1;
    budget.bytes = budget.bytes.checked_add(metadata.len()).ok_or(())?;
    if budget.files > MAX_CACHE_FILES || budget.bytes > MAX_CACHE_ENTRY_BYTES {
        return Err(());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|_| ())?;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let input = options.open(source).map_err(|_| ())?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| ())?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    input
        .take(MAX_CACHE_ENTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() as u64 != metadata.len() || bytes.len() as u64 > MAX_CACHE_ENTRY_BYTES {
        return Err(());
    }
    output.write_all(&bytes).map_err(|_| ())?;
    output.sync_all().map_err(|_| ())?;
    Ok(())
}

fn safe_workspace_destination(
    root: &Path,
    relative: &str,
    create_parents: bool,
) -> Option<PathBuf> {
    if !valid_workspace_path(relative) {
        return None;
    }
    let parts = relative.split('/').collect::<Vec<_>>();
    let mut current = root.to_path_buf();
    for (index, component) in parts.iter().enumerate() {
        current.push(component);
        let final_part = index + 1 == parts.len();
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return None,
            Ok(metadata) if !final_part && !metadata.is_dir() => return None,
            Ok(_) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && !final_part
                    && create_parents =>
            {
                fs::create_dir(&current).ok()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && final_part => {}
            Err(_) => return None,
        }
    }
    Some(current)
}

fn remove_cache_target(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
        Ok(metadata) if metadata.file_type().is_symlink() => false,
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path).is_ok(),
        Ok(_) => fs::remove_file(path).is_ok(),
    }
}

fn enforce_cache_capacity(identity: &CacheIdentity) {
    let Ok(directory_entries) = fs::read_dir(&identity.namespace) else {
        return;
    };
    let now = SystemTime::now();
    let mut entries = Vec::new();
    for entry in directory_entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if now
            .duration_since(modified)
            .map_or(true, |age| age > CACHE_TTL)
        {
            let _ = fs::remove_dir_all(path);
        } else {
            entries.push((modified, path));
        }
    }
    entries.sort_by_key(|entry| entry.0);
    while entries.len() > MAX_CACHE_ENTRIES {
        let (_, oldest) = entries.remove(0);
        let _ = fs::remove_dir_all(oldest);
    }
    enforce_global_cache_capacity(identity);
}

fn enforce_global_cache_capacity(identity: &CacheIdentity) {
    let mut entries = Vec::new();
    let Ok(orgs) = fs::read_dir(&identity.root) else {
        return;
    };
    for org in orgs.filter_map(Result::ok) {
        if org.file_name() == ".locks" || !is_real_directory(&org.path()) {
            continue;
        }
        let Ok(repos) = fs::read_dir(org.path()) else {
            continue;
        };
        for repo in repos.filter_map(Result::ok) {
            if !is_real_directory(&repo.path()) {
                continue;
            }
            let Ok(trust_classes) = fs::read_dir(repo.path()) else {
                continue;
            };
            for trust in trust_classes.filter_map(Result::ok) {
                if !is_real_directory(&trust.path()) {
                    continue;
                }
                let Ok(caches) = fs::read_dir(trust.path()) else {
                    continue;
                };
                for cache in caches.filter_map(Result::ok) {
                    let path = cache.path();
                    if !is_real_directory(&path) || !path.join(".manifest").is_file() {
                        continue;
                    }
                    let modified = fs::symlink_metadata(&path)
                        .ok()
                        .and_then(|metadata| metadata.modified().ok())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    if SystemTime::now()
                        .duration_since(modified)
                        .map_or(true, |age| age > CACHE_TTL)
                    {
                        let _ = fs::remove_dir_all(path);
                    } else if let Some(bytes) = cache_directory_size(&path) {
                        entries.push((modified, path, bytes));
                    } else {
                        let _ = fs::remove_dir_all(path);
                    }
                }
            }
        }
    }
    entries.sort_by_key(|entry| entry.0);
    let mut total_bytes = entries.iter().map(|entry| entry.2).sum::<u64>();
    while entries.len() > MAX_CACHE_TOTAL_ENTRIES || total_bytes > MAX_CACHE_TOTAL_BYTES {
        let (_, oldest, bytes) = entries.remove(0);
        let _ = fs::remove_dir_all(oldest);
        total_bytes = total_bytes.saturating_sub(bytes);
    }
}

fn cache_directory_size(root: &Path) -> Option<u64> {
    let metadata = fs::symlink_metadata(root).ok()?;
    if metadata.file_type().is_symlink() {
        return None;
    }
    if metadata.is_file() {
        return Some(metadata.len());
    }
    if !metadata.is_dir() {
        return None;
    }
    let mut total = 0u64;
    for entry in fs::read_dir(root).ok()? {
        total = total.checked_add(cache_directory_size(&entry.ok()?.path())?)?;
    }
    Some(total)
}

fn is_real_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
}

fn mark_workflow_attempt_started(
    config: &Config,
    attempt_id: Option<Uuid>,
) -> Result<(), &'static str> {
    let attempt_id = attempt_id.ok_or("workflow_attempt_id_missing")?;
    let artifact_root = config
        .artifact_root
        .as_deref()
        .ok_or("artifact_root_not_configured")?;
    let pending = ensure_pending_directory(artifact_root, config.run_id, attempt_id)?;
    write_pending_file(&pending, ".executed", b"started\n")
}

fn collect_declared_outputs(
    config: &Config,
    job: &WorkflowJob,
    workspace: &Path,
    attempt_id: Option<Uuid>,
) -> Result<(), &'static str> {
    if job.artifacts.is_empty() && job.reports.is_none() {
        return Ok(());
    }
    let attempt_id = attempt_id.ok_or("workflow_attempt_id_missing")?;
    let artifact_root = config
        .artifact_root
        .as_deref()
        .ok_or("artifact_root_not_configured")?;
    let pending = ensure_pending_directory(artifact_root, config.run_id, attempt_id)?;
    for artifact in &job.artifacts {
        let bytes = read_workspace_file(workspace, &artifact.path)?;
        write_pending_file(&pending, &artifact.name, &bytes)?;
    }
    if let Some(reports) = &job.reports {
        let bytes = read_workspace_file(workspace, &reports.junit)?;
        write_pending_file(&pending, "reports/junit.xml", &bytes)?;
    }
    Ok(())
}

fn ensure_pending_directory(
    artifact_root: &Path,
    run_id: Uuid,
    attempt_id: Uuid,
) -> Result<PathBuf, &'static str> {
    std::fs::create_dir_all(artifact_root).map_err(|_| "artifact_root_unavailable")?;
    let run_id_component = run_id.to_string();
    let attempt_id_component = attempt_id.to_string();
    let mut current = artifact_root.to_path_buf();
    for component in [
        ".workflow-pending",
        run_id_component.as_str(),
        attempt_id_component.as_str(),
    ] {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err("workflow_artifact_path_unsafe"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current).map_err(|_| "workflow_artifact_directory_failed")?;
            }
            Err(_) => return Err("workflow_artifact_directory_failed"),
        }
    }
    Ok(current)
}

fn read_workspace_file(workspace: &Path, relative: &str) -> Result<Vec<u8>, &'static str> {
    if !valid_workspace_path(relative) {
        return Err("workflow_artifact_path_invalid");
    }
    let parts = relative.split('/').collect::<Vec<_>>();
    let mut path = workspace.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        path.push(part);
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| "workflow_artifact_missing")?;
        if metadata.file_type().is_symlink()
            || (index + 1 == parts.len() && !metadata.is_file())
            || (index + 1 < parts.len() && !metadata.is_dir())
        {
            return Err("workflow_artifact_path_unsafe");
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(&path)
        .map_err(|_| "workflow_artifact_read_failed")?;
    let metadata = file
        .metadata()
        .map_err(|_| "workflow_artifact_read_failed")?;
    if !metadata.is_file() || metadata.len() > MAX_ARTIFACT_BYTES {
        return Err("workflow_artifact_too_large_or_not_file");
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_ARTIFACT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "workflow_artifact_read_failed")?;
    if bytes.len() as u64 > MAX_ARTIFACT_BYTES {
        return Err("workflow_artifact_too_large_or_not_file");
    }
    Ok(bytes)
}

fn write_pending_file(base: &Path, relative: &str, bytes: &[u8]) -> Result<(), &'static str> {
    if !valid_workspace_path(relative) || bytes.len() as u64 > MAX_ARTIFACT_BYTES {
        return Err("workflow_artifact_invalid");
    }
    let parts = relative.split('/').collect::<Vec<_>>();
    let (filename, parents) = parts.split_last().ok_or("workflow_artifact_path_invalid")?;
    let mut parent = base.to_path_buf();
    for component in parents {
        parent.push(component);
        match std::fs::symlink_metadata(&parent) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err("workflow_artifact_path_unsafe"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&parent).map_err(|_| "workflow_artifact_directory_failed")?;
            }
            Err(_) => return Err("workflow_artifact_directory_failed"),
        }
    }
    let destination = parent.join(filename);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| "workflow_artifact_write_failed")?;
    use std::io::Write;
    file.write_all(bytes)
        .map_err(|_| "workflow_artifact_write_failed")?;
    file.sync_all()
        .map_err(|_| "workflow_artifact_write_failed")?;
    Ok(())
}

struct WorkspaceGuard(PathBuf);

impl Drop for WorkspaceGuard {
    fn drop(&mut self) {
        if self.0.exists() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn stage_source(config: &Config, workspace: &Path) -> Result<(), &'static str> {
    let clone = Command::new("git")
        .args([
            "clone",
            "--no-hardlinks",
            "--no-checkout",
            "--quiet",
            "--",
            config.bare_git.to_str().ok_or("bare_git_path_invalid")?,
        ])
        .arg(workspace)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", "/tmp")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .map_err(|_| "git_clone_failed")?;
    if !clone.status.success() {
        return Err("git_clone_failed");
    }
    let checkout = Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null", "-C"])
        .arg(workspace)
        .args(["checkout", "--quiet", "--detach", &config.source_oid])
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", "/tmp")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .map_err(|_| "git_checkout_failed")?;
    if !checkout.status.success() {
        return Err("git_checkout_failed");
    }
    let head = Command::new("git")
        .args(["-C"])
        .arg(workspace)
        .args(["rev-parse", "HEAD"])
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", "/tmp")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .map_err(|_| "git_head_check_failed")?;
    if !head.status.success() || String::from_utf8_lossy(&head.stdout).trim() != config.source_oid {
        return Err("git_head_mismatch");
    }
    std::fs::remove_dir_all(workspace.join(".git")).map_err(|_| "git_metadata_removal_failed")?;
    Ok(())
}

fn docker_run(
    config: &Config,
    job: &WorkflowJob,
    _workspace: &Path,
    host_workspace: &Path,
) -> Result<(Output, bool), &'static str> {
    let container_name = format!("musehub-ci-{}-{}", config.run_id, job.id);
    let mut command = Command::new(&config.docker_bin);
    command
        .args([
            "run",
            "--rm",
            "--init",
            "--pull=never",
            "--name",
            &container_name,
            "--label",
            "musehub.managed=true",
            "--label",
            &format!("musehub.run_id={}", config.run_id),
            "--label",
            &format!("musehub.runner_id={}", config.runner_id),
            "--network",
            "none",
            "--read-only",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges=true",
            "--user",
            "10001:10001",
            "--memory",
            "512m",
            "--cpus",
            "1",
            "--pids-limit",
            "128",
            "--shm-size",
            "16m",
            "--tmpfs",
            "/tmp:rw,noexec,nosuid,nodev,size=64m",
            "--mount",
        ])
        .arg(format!(
            "type=bind,source={},target=/workspace",
            host_workspace.display()
        ))
        .args(["--workdir", "/workspace", "--env", "HOME=/tmp", "--env"])
        .arg("PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .arg(&job.image)
        .args(&job.command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|_| "docker_run_failed")?;
    let stdout = child.stdout.take().ok_or("docker_stdout_unavailable")?;
    let stderr = child.stderr.take().ok_or("docker_stderr_unavailable")?;
    let stdout_reader = thread::spawn(move || read_limited(stdout, MAX_JOB_OUTPUT_BYTES / 2));
    let stderr_reader = thread::spawn(move || read_limited(stderr, MAX_JOB_OUTPUT_BYTES / 2));
    let status = child.wait().map_err(|_| "docker_wait_failed")?;
    let (stdout, stdout_truncated) = stdout_reader.join().map_err(|_| "docker_output_failed")??;
    let (stderr, stderr_truncated) = stderr_reader.join().map_err(|_| "docker_output_failed")??;
    Ok((
        Output {
            status,
            stdout,
            stderr,
        },
        stdout_truncated || stderr_truncated,
    ))
}

fn read_limited(mut reader: impl Read, limit: usize) -> Result<(Vec<u8>, bool), &'static str> {
    let mut kept = Vec::with_capacity(limit.min(16 * 1024));
    let mut truncated = false;
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|_| "docker_output_read_failed")?;
        if count == 0 {
            break;
        }
        let available = limit.saturating_sub(kept.len());
        let retain = available.min(count);
        kept.extend_from_slice(&buffer[..retain]);
        truncated |= retain < count;
    }
    Ok((kept, truncated))
}

fn run_docker_cleanup_command(
    command: &mut Command,
    operation: &str,
) -> Result<Vec<u8>, &'static str> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| {
            eprintln!("could not start Docker cleanup {operation}: {error}");
            "docker_cleanup_command_start_failed"
        })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("docker_cleanup_command_stdout_unavailable")?;
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).map(|_| output)
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = reader
                    .join()
                    .map_err(|_| "docker_cleanup_command_stdout_failed")?
                    .map_err(|_| "docker_cleanup_command_stdout_failed")?;
                if status.success() {
                    return Ok(output);
                }
                eprintln!("Docker cleanup {operation} exited with {status}");
                return Err("docker_cleanup_command_failed");
            }
            Ok(None) if started.elapsed() < DOCKER_CLEANUP_TIMEOUT => {
                thread::sleep(DOCKER_CLEANUP_POLL_INTERVAL);
            }
            Ok(None) => {
                eprintln!(
                    "Docker cleanup {operation} exceeded {} seconds; terminating Docker CLI",
                    DOCKER_CLEANUP_TIMEOUT.as_secs()
                );
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err("docker_cleanup_command_timeout");
            }
            Err(error) => {
                eprintln!("could not wait for Docker cleanup {operation}: {error}");
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err("docker_cleanup_command_wait_failed");
            }
        }
    }
}

fn cleanup_orphaned_jobs(run_id: Option<Uuid>) -> Result<(), &'static str> {
    let docker_bin = std::env::var_os("MUSEHUB_JOB_DOCKER_BIN")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("job_docker_bin_not_configured")?;
    let runner_id = std::env::var("MUSEHUB_RUNNER_INSTANCE_ID")
        .map_err(|_| "runner_id_missing")?
        .parse::<Uuid>()
        .map_err(|_| "runner_id_invalid")?;
    let mut list = Command::new(&docker_bin);
    list.args([
        "ps",
        "-aq",
        "--filter",
        "label=musehub.managed=true",
        "--filter",
        &format!("label=musehub.runner_id={runner_id}"),
    ]);
    if let Some(run_id) = run_id {
        list.args(["--filter", &format!("label=musehub.run_id={run_id}")]);
    }
    let listed = run_docker_cleanup_command(&mut list, "listing")
        .map_err(|_| "docker_cleanup_list_failed")?;
    let listed_ids = String::from_utf8_lossy(&listed).into_owned();
    let ids = listed_ids
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if ids.len() == 128 {
        return Err("orphaned_job_container_limit_reached");
    }
    for id in ids {
        if !(12..=64).contains(&id.len()) || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("docker_cleanup_container_id_invalid");
        }
        let mut remove = Command::new(&docker_bin);
        remove.args(["rm", "-f", id.as_str()]);
        run_docker_cleanup_command(&mut remove, "remove")
            .map_err(|_| "docker_cleanup_remove_failed")?;
    }
    Ok(())
}

fn print_limited(bytes: &[u8], total: &mut usize) {
    let remaining = MAX_TOTAL_OUTPUT_BYTES.saturating_sub(*total);
    let retain = bytes.len().min(remaining);
    print!("{}", String::from_utf8_lossy(&bytes[..retain]));
    *total += retain;
    if retain < bytes.len() {
        println!("\n[workflow output total limit reached]");
    }
}

fn job_result_id<'a>(result: &'a Result<(String, Output, bool), &'static str>) -> &'a str {
    result.as_ref().map(|(id, _, _)| id.as_str()).unwrap_or("")
}

#[cfg(test)]
mod cache_tests {
    use super::{cache_paths_overlap, cache_tree_digest, copy_cache_entry, CacheBudget};

    #[test]
    fn cache_copy_preserves_content_and_integrity_digest_detects_changes() {
        let source = tempfile::tempdir().expect("cache source directory");
        let destination = tempfile::tempdir().expect("cache destination directory");
        std::fs::create_dir(source.path().join("target")).expect("target directory");
        std::fs::write(source.path().join("target/build.bin"), b"build output")
            .expect("cache source file");
        let target = destination.path().join("target");
        copy_cache_entry(
            &source.path().join("target"),
            &target,
            &mut CacheBudget::default(),
        )
        .expect("copy cache data");
        assert_eq!(
            cache_tree_digest(source.path()),
            cache_tree_digest(destination.path())
        );
        std::fs::write(target.join("build.bin"), b"changed output").expect("mutate cached file");
        assert_ne!(
            cache_tree_digest(source.path()),
            cache_tree_digest(destination.path())
        );
    }

    #[test]
    fn rejects_overlapping_cache_paths() {
        assert!(cache_paths_overlap(&[
            "target".into(),
            "target/debug".into()
        ]));
        assert!(!cache_paths_overlap(&[
            "target".into(),
            ".cargo/registry".into()
        ]));
    }

    #[cfg(unix)]
    #[test]
    fn cache_copy_rejects_symlinks() {
        let source = tempfile::tempdir().expect("cache source directory");
        let destination = tempfile::tempdir().expect("cache destination directory");
        std::fs::write(source.path().join("outside"), b"outside").expect("target file");
        std::os::unix::fs::symlink(source.path().join("outside"), source.path().join("link"))
            .expect("symlink");
        assert!(copy_cache_entry(
            &source.path().join("link"),
            &destination.path().join("link"),
            &mut CacheBudget::default(),
        )
        .is_err());
    }
}
