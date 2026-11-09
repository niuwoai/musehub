//! MCP Prompts catalog (`docs/04-mcp-api.md` §13) — templates only; no extra grants.

use rmcp::model::{
    GetPromptResponse, GetPromptResult, ListPromptsResult, Prompt, PromptMessage, Role,
};
use rmcp::ErrorData;

pub const PROMPT_NAMES: &[&str] = &[
    "plan_issue",
    "review_pull_request",
    "diagnose_pipeline_failure",
    "draft_release_notes",
];

pub fn list_prompts() -> ListPromptsResult {
    let items: Vec<Prompt> = PROMPT_NAMES
        .iter()
        .map(|name| {
            Prompt::new(
                *name,
                Some("MuseHub workflow template (does not grant permissions)"),
                None,
            )
        })
        .collect();
    ListPromptsResult::with_all_items(items)
}

pub fn get_prompt(name: &str) -> Result<GetPromptResponse, ErrorData> {
    if !PROMPT_NAMES.contains(&name) {
        return Err(ErrorData::invalid_params("unknown prompt", None));
    }
    let text = match name {
        "plan_issue" => {
            "Review the issue acceptance criteria and propose an implementation plan. Do not call write tools without explicit user approval."
        }
        "review_pull_request" => {
            "Summarize the PR diff risks, test gaps, and merge blockers. Use musehub_pr_get for authoritative state."
        }
        "diagnose_pipeline_failure" => {
            "Use musehub_pipeline_get and job logs tools to isolate failing job; prefer retry only when policy allows."
        }
        "draft_release_notes" => {
            "Draft release notes from merged changes; publishing requires release:publish scope and approval gates."
        }
        _ => unreachable!(),
    };
    Ok(GetPromptResult::new(vec![PromptMessage::new_text(Role::User, text)]).into())
}
