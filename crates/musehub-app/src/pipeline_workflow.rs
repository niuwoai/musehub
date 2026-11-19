//! Strict, versioned workflow definitions used by the pipeline scheduler.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

const WORKFLOW_VERSION: u8 = 1;
const MAX_SOURCE_BYTES: usize = 64 * 1024;
const MAX_JOBS: usize = 32;
const MAX_NEEDS_PER_JOB: usize = 32;
const MAX_COMMAND_ARGS: usize = 64;
const MAX_ARTIFACTS_PER_JOB: usize = 16;
const MAX_CACHE_PATHS: usize = 16;
const MAX_FIELD_BYTES: usize = 1024;
const IMAGE_DIGEST_PREFIX: &str = "@sha256:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowEvent {
    Push,
    PullRequest,
    Manual,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowJob {
    pub id: String,
    pub needs: Vec<String>,
    pub image: String,
    /// Executed as argv inside the job container; never passed through a host shell.
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<WorkflowArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reports: Option<WorkflowReports>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<WorkflowCache>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowArtifact {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReports {
    pub junit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCache {
    /// User-controlled namespace prefix; trust and repository boundaries are added by control plane.
    pub key: String,
    pub lockfile: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    pub version: u8,
    pub events: Vec<WorkflowEvent>,
    pub jobs: Vec<WorkflowJob>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatedWorkflow {
    pub definition: WorkflowDefinition,
    pub sha256: String,
    /// Stable topological ordering.
    pub topological_order: Vec<String>,
    /// Jobs grouped into dependency layers for bounded parallel execution.
    pub execution_layers: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWorkflow {
    version: u8,
    #[serde(rename = "on")]
    events: Vec<WorkflowEvent>,
    jobs: Vec<RawJob>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJob {
    id: String,
    #[serde(default)]
    needs: Vec<String>,
    image: String,
    command: Vec<String>,
    #[serde(default)]
    artifacts: Vec<WorkflowArtifact>,
    #[serde(default)]
    reports: Option<WorkflowReports>,
    #[serde(default)]
    cache: Option<WorkflowCache>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WorkflowError {
    #[error("workflow source exceeds the {MAX_SOURCE_BYTES}-byte limit")]
    SourceTooLarge,
    #[error("workflow source contains a NUL byte")]
    NulByte,
    #[error("workflow YAML is invalid: {0}")]
    Yaml(String),
    #[error("workflow version {0} is unsupported")]
    UnsupportedVersion(u8),
    #[error("workflow must declare at least one supported event")]
    NoEvents,
    #[error("workflow declares a duplicate event")]
    DuplicateEvent,
    #[error("workflow must declare at least one job")]
    NoJobs,
    #[error("workflow contains more than {MAX_JOBS} jobs")]
    TooManyJobs,
    #[error("job {job} declares more than {MAX_NEEDS_PER_JOB} dependencies")]
    TooManyDependencies { job: String },
    #[error("workflow job id is invalid: {0}")]
    InvalidJobId(String),
    #[error("workflow contains duplicate job id: {0}")]
    DuplicateJobId(String),
    #[error("job {job} contains an invalid or duplicate dependency: {dependency}")]
    InvalidDependency { job: String, dependency: String },
    #[error("job {job} depends on missing job {dependency}")]
    MissingDependency { job: String, dependency: String },
    #[error("workflow dependency graph contains a cycle")]
    DependencyCycle,
    #[error("job {job} has an invalid pinned image reference")]
    InvalidImage { job: String },
    #[error("job {job} has an invalid command")]
    InvalidCommand { job: String },
    #[error("job {job} declares invalid or duplicate artifacts")]
    InvalidArtifacts { job: String },
    #[error("job {job} declares an invalid JUnit report path")]
    InvalidReport { job: String },
    #[error("job {job} declares invalid cache settings")]
    InvalidCache { job: String },
}

pub fn parse_workflow(source: &str) -> Result<ValidatedWorkflow, WorkflowError> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(WorkflowError::SourceTooLarge);
    }
    if source.as_bytes().contains(&0) {
        return Err(WorkflowError::NulByte);
    }

    let raw: RawWorkflow =
        serde_yaml_ng::from_str(source).map_err(|error| WorkflowError::Yaml(error.to_string()))?;
    if raw.version != WORKFLOW_VERSION {
        return Err(WorkflowError::UnsupportedVersion(raw.version));
    }
    if raw.events.is_empty() {
        return Err(WorkflowError::NoEvents);
    }
    if raw.jobs.is_empty() {
        return Err(WorkflowError::NoJobs);
    }
    if raw.jobs.len() > MAX_JOBS {
        return Err(WorkflowError::TooManyJobs);
    }

    let event_count = raw.events.len();
    let events = raw.events.into_iter().collect::<BTreeSet<_>>();
    if events.len() != event_count {
        return Err(WorkflowError::DuplicateEvent);
    }
    let mut jobs = BTreeMap::new();
    for raw_job in raw.jobs {
        if !valid_identifier(&raw_job.id) {
            return Err(WorkflowError::InvalidJobId(raw_job.id));
        }
        if jobs.contains_key(&raw_job.id) {
            return Err(WorkflowError::DuplicateJobId(raw_job.id));
        }
        if raw_job.needs.len() > MAX_NEEDS_PER_JOB {
            return Err(WorkflowError::TooManyDependencies { job: raw_job.id });
        }
        let needs_count = raw_job.needs.len();
        let needs = raw_job.needs.into_iter().collect::<BTreeSet<_>>();
        if needs.len() != needs_count {
            return Err(WorkflowError::InvalidDependency {
                job: raw_job.id,
                dependency: "duplicate dependency".to_string(),
            });
        }
        if !valid_digest_image(&raw_job.image) {
            return Err(WorkflowError::InvalidImage { job: raw_job.id });
        }
        if raw_job.command.is_empty()
            || raw_job.command.len() > MAX_COMMAND_ARGS
            || raw_job
                .command
                .iter()
                .any(|arg| arg.is_empty() || arg.len() > MAX_FIELD_BYTES || arg.contains('\0'))
        {
            return Err(WorkflowError::InvalidCommand { job: raw_job.id });
        }
        let mut artifact_names = BTreeSet::new();
        if raw_job.artifacts.len() > MAX_ARTIFACTS_PER_JOB
            || raw_job.artifacts.iter().any(|artifact| {
                !valid_artifact_name(&artifact.name)
                    || !valid_workspace_path(&artifact.path)
                    || !artifact_names.insert(artifact.name.as_str())
            })
        {
            return Err(WorkflowError::InvalidArtifacts { job: raw_job.id });
        }
        if raw_job.reports.as_ref().is_some_and(|reports| {
            !valid_workspace_path(&reports.junit) || artifact_names.contains("junit-report.xml")
        }) {
            return Err(WorkflowError::InvalidReport { job: raw_job.id });
        }
        if raw_job.cache.as_ref().is_some_and(|cache| {
            let paths = cache.paths.iter().collect::<BTreeSet<_>>();
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
        }) {
            return Err(WorkflowError::InvalidCache { job: raw_job.id });
        }
        let job = WorkflowJob {
            id: raw_job.id.clone(),
            needs: needs.into_iter().collect(),
            image: raw_job.image,
            command: raw_job.command,
            artifacts: raw_job.artifacts,
            reports: raw_job.reports,
            cache: raw_job.cache,
        };
        jobs.insert(raw_job.id, job);
    }

    for job in jobs.values() {
        for dependency in &job.needs {
            if dependency == &job.id || !valid_identifier(dependency) {
                return Err(WorkflowError::InvalidDependency {
                    job: job.id.clone(),
                    dependency: dependency.clone(),
                });
            }
            if !jobs.contains_key(dependency) {
                return Err(WorkflowError::MissingDependency {
                    job: job.id.clone(),
                    dependency: dependency.clone(),
                });
            }
        }
    }

    let (topological_order, execution_layers) = stable_topological_order(&jobs)?;
    let definition = WorkflowDefinition {
        version: raw.version,
        events: events.into_iter().collect(),
        jobs: jobs.into_values().collect(),
    };
    let canonical = serde_json::to_vec(&definition)
        .expect("serializing the typed workflow definition to JSON cannot fail");
    let sha256 = format!("{:x}", Sha256::digest(canonical));
    Ok(ValidatedWorkflow {
        definition,
        sha256,
        topological_order,
        execution_layers,
    })
}

fn stable_topological_order(
    jobs: &BTreeMap<String, WorkflowJob>,
) -> Result<(Vec<String>, Vec<Vec<String>>), WorkflowError> {
    let mut indegree = jobs
        .iter()
        .map(|(id, job)| (id.clone(), job.needs.len()))
        .collect::<BTreeMap<_, _>>();
    let mut dependents = BTreeMap::<String, Vec<String>>::new();
    for (id, job) in jobs {
        for dependency in &job.needs {
            dependents
                .entry(dependency.clone())
                .or_default()
                .push(id.clone());
        }
    }
    for values in dependents.values_mut() {
        values.sort();
    }

    let mut ready = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(jobs.len());
    let mut layers = Vec::new();
    while !ready.is_empty() {
        let layer = std::mem::take(&mut ready).into_iter().collect::<Vec<_>>();
        for id in &layer {
            order.push(id.clone());
            if let Some(children) = dependents.get(id) {
                for child in children {
                    let degree = indegree.get_mut(child).expect("known workflow job");
                    *degree -= 1;
                    if *degree == 0 {
                        ready.insert(child.clone());
                    }
                }
            }
        }
        layers.push(layer);
    }
    if order.len() != jobs.len() {
        return Err(WorkflowError::DependencyCycle);
    }
    Ok((order, layers))
}

fn valid_identifier(value: &str) -> bool {
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
        && value.len() <= MAX_FIELD_BYTES
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
    let Some((name, digest)) = value.split_once(IMAGE_DIGEST_PREFIX) else {
        return false;
    };
    !name.is_empty()
        && name.len() <= MAX_FIELD_BYTES
        && !name.chars().any(char::is_whitespace)
        && digest.len() == 64
        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::{parse_workflow, WorkflowError};

    const IMAGE: &str = "registry.example/node@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn workflow(jobs: &str) -> String {
        format!("version: 1\non: [push, pull_request, manual, tag]\njobs:\n{jobs}")
    }

    #[test]
    fn validates_dag_and_hashes_semantic_definition() {
        let input = workflow(&format!(
            "  - id: test\n    image: {IMAGE}\n    command: [npm, test]\n  - id: lint\n    image: {IMAGE}\n    command: [npm, run, lint]\n  - id: package\n    needs: [lint, test]\n    image: {IMAGE}\n    command: [npm, pack]\n"
        ));
        let parsed = parse_workflow(&input).unwrap();
        assert_eq!(parsed.topological_order, ["lint", "test", "package"]);
        assert_eq!(
            parsed.execution_layers,
            vec![
                vec!["lint".to_string(), "test".to_string()],
                vec!["package".to_string()]
            ]
        );

        let reordered = workflow(&format!(
            "  - id: package\n    needs: [test, lint]\n    image: {IMAGE}\n    command: [npm, pack]\n  - id: test\n    image: {IMAGE}\n    command: [npm, test]\n  - id: lint\n    image: {IMAGE}\n    command: [npm, run, lint]\n"
        ));
        assert_eq!(parsed.sha256, parse_workflow(&reordered).unwrap().sha256);
    }

    #[test]
    fn rejects_duplicate_events() {
        let duplicate = "version: 1\non: [push, push]\njobs:\n  - id: test\n    image: registry.example/node@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n    command: [npm, test]\n";
        assert_eq!(
            parse_workflow(duplicate),
            Err(WorkflowError::DuplicateEvent)
        );
    }

    #[test]
    fn rejects_unknown_fields_and_cycles() {
        let unknown = workflow(&format!(
            "  - id: test\n    image: {IMAGE}\n    command: [npm, test]\n    shell: true\n"
        ));
        assert!(matches!(
            parse_workflow(&unknown),
            Err(WorkflowError::Yaml(_))
        ));

        let cycle = workflow(&format!(
            "  - id: first\n    needs: [second]\n    image: {IMAGE}\n    command: [npm, test]\n  - id: second\n    needs: [first]\n    image: {IMAGE}\n    command: [npm, test]\n"
        ));
        assert_eq!(parse_workflow(&cycle), Err(WorkflowError::DependencyCycle));
    }

    #[test]
    fn rejects_duplicate_ids_missing_needs_and_unpinned_images() {
        let duplicate = workflow(&format!(
            "  - id: test\n    image: {IMAGE}\n    command: [npm, test]\n  - id: test\n    image: {IMAGE}\n    command: [npm, test]\n"
        ));
        assert_eq!(
            parse_workflow(&duplicate),
            Err(WorkflowError::DuplicateJobId("test".to_string()))
        );

        let missing = workflow(&format!(
            "  - id: test\n    needs: [missing]\n    image: {IMAGE}\n    command: [npm, test]\n"
        ));
        assert!(matches!(
            parse_workflow(&missing),
            Err(WorkflowError::MissingDependency { .. })
        ));

        let unpinned = workflow("  - id: test\n    image: node:24\n    command: [npm, test]\n");
        assert_eq!(
            parse_workflow(&unpinned),
            Err(WorkflowError::InvalidImage {
                job: "test".to_string()
            })
        );
    }

    #[test]
    fn validates_artifact_report_and_cache_declarations() {
        let configured = workflow(&format!(
            "  - id: test\n    image: {IMAGE}\n    command: [cargo, test]\n    artifacts:\n      - name: test-log.txt\n        path: target/test.log\n    reports:\n      junit: target/junit.xml\n    cache:\n      key: cargo-v1\n      lockfile: Cargo.lock\n      paths: [.cargo/registry, target]\n"
        ));
        let parsed = parse_workflow(&configured).expect("valid workflow outputs");
        let job = &parsed.definition.jobs[0];
        assert_eq!(job.artifacts[0].name, "test-log.txt");
        assert_eq!(
            job.reports.as_ref().map(|reports| reports.junit.as_str()),
            Some("target/junit.xml")
        );
        assert_eq!(
            job.cache.as_ref().map(|cache| cache.key.as_str()),
            Some("cargo-v1")
        );

        let traversal = workflow(&format!(
            "  - id: test\n    image: {IMAGE}\n    command: [cargo, test]\n    cache:\n      key: cargo-v1\n      lockfile: Cargo.lock\n      paths: [../shared]\n"
        ));
        assert!(matches!(
            parse_workflow(&traversal),
            Err(WorkflowError::InvalidCache { .. })
        ));
    }
}
