use anyhow::Result;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData as McpError, ServerHandler, ServiceExt,
};
use std::path::PathBuf;
use tokio::process::Command;
use tracing_subscriber::EnvFilter;

// ── Parameter types ──────────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct AddTaskRequest {
    /// Task description
    description: String,
    /// Project this task belongs to (REQUIRED). Use dot-notation for subprojects, e.g. "Work.Backend".
    /// Every task must be filed under a project — untagged tasks pollute the global list.
    project: String,
    /// Due date/time: "today", "tomorrow", "eow", "eom", "friday", "2025-06-15", "2025-06-15T14:30"
    due: Option<String>,
    /// Tags to apply, without the + prefix (e.g. ["urgent", "blocked"])
    tags: Option<Vec<String>>,
    /// Priority: H (high), M (medium), or L (low)
    priority: Option<String>,
    /// Wait date — task is hidden from reports until this date
    wait: Option<String>,
    /// Scheduled date — when you plan to start (distinct from due = must finish by)
    scheduled: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ListTasksRequest {
    /// Project to scope this query to (REQUIRED). Use dot-notation, e.g. "Work" or "Work.Backend".
    /// All list operations are project-scoped by default to avoid dumping thousands of unrelated
    /// tasks into context. Set all_projects=true only for genuine cross-project needs.
    project: String,
    /// Additional filter tokens beyond the project scope, e.g. "+urgent", "priority:H", "+OVERDUE"
    filter: Option<String>,
    /// Report to run: next (default, urgency-sorted), list, all, completed, waiting, blocked
    report: Option<String>,
    /// Override project scoping and query ALL projects. Only use when the request is explicitly
    /// cross-project (e.g. "show me everything overdue across all projects").
    all_projects: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchTasksRequest {
    /// Regex pattern to match against task descriptions and annotations
    pattern: String,
    /// Project to scope this search to (REQUIRED). Set all_projects=true to search globally.
    project: String,
    /// Additional filter tokens to narrow results, e.g. "priority:H"
    filter: Option<String>,
    /// Override project scoping and search ALL projects.
    all_projects: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct TaskIdRequest {
    /// Task ID (numeric) or UUID
    id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ModifyTaskRequest {
    /// Task ID (numeric) or UUID
    id: String,
    /// Space-separated modification tokens, e.g. "due:friday priority:H +urgent -old project:Work".
    /// Clear a field by omitting its value: "due: priority:"
    modifications: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct AnnotateTaskRequest {
    /// Task ID (numeric) or UUID
    id: String,
    /// Note text to attach; timestamped automatically by Taskwarrior
    note: String,
}

// ── Server ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct TaskWarriorServer {
    tool_router: ToolRouter<TaskWarriorServer>,
    /// Overrides the taskwarrior data directory. Used in tests for isolation.
    data_dir: Option<PathBuf>,
}

impl TaskWarriorServer {
    async fn run(&self, args: &[&str]) -> Result<String, McpError> {
        let mut cmd = Command::new("task");
        cmd.arg("rc.confirmation=no");
        if let Some(dir) = &self.data_dir {
            cmd.arg(format!("rc.data.location={}", dir.display()));
        }
        cmd.args(args);

        let output = cmd
            .output()
            .await
            .map_err(|e| McpError::internal_error(format!("Failed to run task: {e}"), None))?;

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

        if !output.status.success() && stdout.is_empty() {
            return Err(McpError::internal_error(
                if !stderr.is_empty() {
                    stderr
                } else {
                    format!("task exited with status {}", output.status)
                },
                None,
            ));
        }

        Ok(if !stdout.is_empty() { stdout } else { stderr })
    }
}

#[cfg(test)]
impl TaskWarriorServer {
    fn with_data_dir(dir: &std::path::Path) -> Self {
        Self {
            tool_router: Self::tool_router(),
            data_dir: Some(dir.to_path_buf()),
        }
    }
}

#[tool_router]
impl TaskWarriorServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            data_dir: None,
        }
    }

    #[tool(description = "\
        Add a new task. `project` is REQUIRED — every task must belong to a project. \
        Supports due dates (today/tomorrow/eow/eom/friday/ISO datetime), tags, \
        dot-notation subprojects (e.g. Work.Backend), priorities (H/M/L), \
        wait dates (hide until actionable), and scheduled dates (when you plan to start).")]
    async fn add_task(
        &self,
        Parameters(req): Parameters<AddTaskRequest>,
    ) -> Result<CallToolResult, McpError> {
        let mut args = vec!["add".to_string(), req.description];
        args.push(format!("project:{}", req.project));
        if let Some(v) = req.due {
            args.push(format!("due:{v}"));
        }
        if let Some(v) = req.priority {
            args.push(format!("priority:{v}"));
        }
        if let Some(v) = req.wait {
            args.push(format!("wait:{v}"));
        }
        if let Some(v) = req.scheduled {
            args.push(format!("scheduled:{v}"));
        }
        if let Some(tags) = req.tags {
            for t in tags {
                args.push(format!("+{t}"));
            }
        }

        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        Ok(CallToolResult::success(vec![Content::text(
            self.run(&refs).await?,
        )]))
    }

    #[tool(description = "\
        List tasks sorted by urgency. `project` is REQUIRED and is automatically prepended \
        as a filter to prevent loading thousands of unrelated tasks into context. \
        Use `filter` for additional narrowing (+urgent, priority:H, +OVERDUE, +DUE, +READY, +BLOCKED). \
        Use `report` to switch views: next (default), list, all, completed, waiting, blocked. \
        Only set `all_projects=true` for explicit cross-project requests.")]
    async fn list_tasks(
        &self,
        Parameters(req): Parameters<ListTasksRequest>,
    ) -> Result<CallToolResult, McpError> {
        let mut args: Vec<String> = Vec::new();

        if !req.all_projects.unwrap_or(false) {
            args.push(format!("project:{}", req.project));
        }
        if let Some(f) = req.filter {
            args.extend(f.split_whitespace().map(str::to_string));
        }
        args.push(req.report.unwrap_or_else(|| "next".to_string()));

        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self
            .run(&refs)
            .await
            .unwrap_or_else(|_| "No tasks found.".to_string());
        Ok(CallToolResult::success(vec![Content::text(
            if out.is_empty() {
                "No tasks found.".to_string()
            } else {
                out
            },
        )]))
    }

    #[tool(description = "\
        Search tasks by regex pattern across descriptions and annotations. \
        `project` is REQUIRED and automatically scopes the search. \
        Only set `all_projects=true` for explicit cross-project searches.")]
    async fn search_tasks(
        &self,
        Parameters(req): Parameters<SearchTasksRequest>,
    ) -> Result<CallToolResult, McpError> {
        let mut args: Vec<String> = Vec::new();

        if !req.all_projects.unwrap_or(false) {
            args.push(format!("project:{}", req.project));
        }
        if let Some(f) = req.filter {
            args.extend(f.split_whitespace().map(str::to_string));
        }
        args.push(format!("/{}/", req.pattern));
        args.push("list".to_string());

        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self
            .run(&refs)
            .await
            .unwrap_or_else(|_| "No matching tasks.".to_string());
        Ok(CallToolResult::success(vec![Content::text(
            if out.is_empty() {
                "No matching tasks.".to_string()
            } else {
                out
            },
        )]))
    }

    #[tool(description = "\
        Get full details of a task by ID or UUID: all attributes, annotations, \
        urgency score, dependencies, and timestamps.")]
    async fn get_task(
        &self,
        Parameters(req): Parameters<TaskIdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let out = self.run(&[&req.id, "information"]).await?;
        Ok(CallToolResult::success(vec![Content::text(out)]))
    }

    #[tool(description = "\
        Modify a task's attributes. Pass modifications as a space-separated string: \
        'due:friday priority:H +newtag -oldtag project:Work'. \
        Clear a field by omitting its value: 'due: priority:'.")]
    async fn modify_task(
        &self,
        Parameters(req): Parameters<ModifyTaskRequest>,
    ) -> Result<CallToolResult, McpError> {
        let mut args = vec![req.id, "modify".to_string()];
        args.extend(req.modifications.split_whitespace().map(str::to_string));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        Ok(CallToolResult::success(vec![Content::text(
            self.run(&refs).await?,
        )]))
    }

    #[tool(description = "Mark a task as completed.")]
    async fn complete_task(
        &self,
        Parameters(req): Parameters<TaskIdRequest>,
    ) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(
            self.run(&[&req.id, "done"]).await?,
        )]))
    }

    #[tool(description = "Permanently delete a task.")]
    async fn delete_task(
        &self,
        Parameters(req): Parameters<TaskIdRequest>,
    ) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(
            self.run(&[&req.id, "delete"]).await?,
        )]))
    }

    #[tool(description = "\
        Attach a timestamped annotation (note) to a task. \
        Use for progress updates, links, or context that shouldn't be lost.")]
    async fn annotate_task(
        &self,
        Parameters(req): Parameters<AnnotateTaskRequest>,
    ) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(
            self.run(&[&req.id, "annotate", &req.note]).await?,
        )]))
    }
}

#[tool_handler]
impl ServerHandler for TaskWarriorServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: env!("CARGO_PKG_NAME").to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                title: None,
                description: None,
                icons: None,
                website_url: None,
            },
            instructions: Some(
                "Taskwarrior MCP server. PROJECT SCOPING IS MANDATORY: \
                add_task requires `project`, list_tasks and search_tasks require `project` and \
                automatically prepend it as a filter — this prevents thousands of unrelated tasks \
                from flooding context. Only pass all_projects=true when the user explicitly asks \
                for a cross-project view. \
                Tools: add_task · list_tasks · search_tasks · get_task · modify_task · complete_task · delete_task · annotate_task. \
                Date syntax: today · tomorrow · eow · eom · friday · 2025-06-15 · 2025-06-15T14:30. \
                Virtual filter tags: +OVERDUE · +DUE · +READY · +BLOCKED · +BLOCKING · +ACTIVE · +WAITING · +TODAY."
                .to_string(),
            ),
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    tracing::info!("Starting task-warrior-mcp");

    let service = TaskWarriorServer::new()
        .serve(stdio())
        .await
        .inspect_err(|e| tracing::error!("Serving error: {e:?}"))?;

    service.waiting().await?;
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── helpers ───────────────────────────────────────────────────────────────

    fn test_server() -> (TempDir, TaskWarriorServer) {
        let dir = TempDir::new().expect("failed to create temp dir");
        let server = TaskWarriorServer::with_data_dir(dir.path());
        (dir, server)
    }

    fn text_of(result: &CallToolResult) -> &str {
        result
            .content
            .first()
            .expect("no content")
            .as_text()
            .expect("expected text content")
            .text
            .as_str()
    }

    /// Parse "Created task 5." → "5"
    fn created_id(output: &str) -> String {
        output
            .split_whitespace()
            .nth(2)
            .unwrap_or("1")
            .trim_end_matches('.')
            .to_string()
    }

    async fn add_task(server: &TaskWarriorServer, desc: &str, project: &str) -> String {
        let result = server
            .add_task(Parameters(AddTaskRequest {
                description: desc.to_string(),
                project: project.to_string(),
                due: None,
                tags: None,
                priority: None,
                wait: None,
                scheduled: None,
            }))
            .await
            .expect("add_task failed");
        created_id(text_of(&result))
    }

    // ── add_task ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_add_task_basic() {
        let (_dir, server) = test_server();
        let result = server
            .add_task(Parameters(AddTaskRequest {
                description: "Write the docs".to_string(),
                project: "myproject".to_string(),
                due: None,
                tags: None,
                priority: None,
                wait: None,
                scheduled: None,
            }))
            .await
            .unwrap();

        assert!(!result.is_error.unwrap_or(false));
        assert!(text_of(&result).contains("Created task"));
    }

    #[tokio::test]
    async fn test_add_task_with_all_fields() {
        let (_dir, server) = test_server();
        let result = server
            .add_task(Parameters(AddTaskRequest {
                description: "Full featured task".to_string(),
                project: "myproject".to_string(),
                due: Some("tomorrow".to_string()),
                tags: Some(vec!["urgent".to_string(), "review".to_string()]),
                priority: Some("H".to_string()),
                wait: None,
                scheduled: None,
            }))
            .await
            .unwrap();

        assert!(!result.is_error.unwrap_or(false));
        assert!(text_of(&result).contains("Created task"));
    }

    #[tokio::test]
    async fn test_add_task_project_is_stored() {
        let (_dir, server) = test_server();
        let id = add_task(&server, "Verify project stored", "stored-proj").await;

        let info = server
            .get_task(Parameters(TaskIdRequest { id }))
            .await
            .unwrap();

        assert!(text_of(&info).contains("stored-proj"));
    }

    // ── list_tasks ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_tasks_scoped_to_project() {
        let (_dir, server) = test_server();
        add_task(&server, "Task in alpha", "alpha").await;
        add_task(&server, "Task in beta", "beta").await;

        let result = server
            .list_tasks(Parameters(ListTasksRequest {
                project: "alpha".to_string(),
                filter: None,
                report: Some("list".to_string()),
                all_projects: None,
            }))
            .await
            .unwrap();

        let out = text_of(&result);
        assert!(out.contains("Task in alpha"), "alpha task must appear");
        assert!(
            !out.contains("Task in beta"),
            "beta task must not leak into alpha scope"
        );
    }

    #[tokio::test]
    async fn test_list_tasks_all_projects_override() {
        let (_dir, server) = test_server();
        add_task(&server, "Task in alpha", "alpha").await;
        add_task(&server, "Task in beta", "beta").await;

        let result = server
            .list_tasks(Parameters(ListTasksRequest {
                project: "alpha".to_string(),
                filter: None,
                report: Some("list".to_string()),
                all_projects: Some(true),
            }))
            .await
            .unwrap();

        let out = text_of(&result);
        assert!(out.contains("Task in alpha"));
        assert!(
            out.contains("Task in beta"),
            "all_projects=true should surface both projects"
        );
    }

    #[tokio::test]
    async fn test_list_tasks_with_filter() {
        let (_dir, server) = test_server();
        add_task(&server, "High priority task", "filter-test").await;
        add_task(&server, "Low priority task", "filter-test").await;

        // Modify the first to H priority so we can filter on it
        server
            .modify_task(Parameters(ModifyTaskRequest {
                id: "1".to_string(),
                modifications: "priority:H".to_string(),
            }))
            .await
            .unwrap();

        let result = server
            .list_tasks(Parameters(ListTasksRequest {
                project: "filter-test".to_string(),
                filter: Some("priority:H".to_string()),
                report: Some("list".to_string()),
                all_projects: None,
            }))
            .await
            .unwrap();

        let out = text_of(&result);
        assert!(out.contains("High priority task"));
        assert!(!out.contains("Low priority task"));
    }

    // ── search_tasks ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_search_tasks_finds_match() {
        let (_dir, server) = test_server();
        add_task(&server, "Fix the flibbertigibbet bug", "search-test").await;
        add_task(&server, "Unrelated task", "search-test").await;

        let result = server
            .search_tasks(Parameters(SearchTasksRequest {
                pattern: "flibbertigibbet".to_string(),
                project: "search-test".to_string(),
                filter: None,
                all_projects: None,
            }))
            .await
            .unwrap();

        let out = text_of(&result);
        assert!(
            out.contains("flibbertigibbet"),
            "should find the matching task"
        );
        assert!(
            !out.contains("Unrelated task"),
            "should not include non-matching tasks"
        );
    }

    #[tokio::test]
    async fn test_search_tasks_scoped_to_project() {
        let (_dir, server) = test_server();
        add_task(&server, "needle in project A", "proj-a").await;
        add_task(&server, "needle in project B", "proj-b").await;

        let result = server
            .search_tasks(Parameters(SearchTasksRequest {
                pattern: "needle".to_string(),
                project: "proj-a".to_string(),
                filter: None,
                all_projects: None,
            }))
            .await
            .unwrap();

        let out = text_of(&result);
        assert!(
            out.contains("needle in project A"),
            "should find proj-a task"
        );
        assert!(
            !out.contains("needle in project B"),
            "should not leak proj-b results"
        );
    }

    // ── get_task ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_task_returns_details() {
        let (_dir, server) = test_server();
        let id = add_task(&server, "Fetch me by ID", "get-test").await;

        let result = server
            .get_task(Parameters(TaskIdRequest { id }))
            .await
            .unwrap();

        assert!(!result.is_error.unwrap_or(false));
        assert!(text_of(&result).contains("Fetch me by ID"));
    }

    // ── modify_task ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_modify_task_changes_priority() {
        let (_dir, server) = test_server();
        let id = add_task(&server, "Task to modify", "modify-test").await;

        let result = server
            .modify_task(Parameters(ModifyTaskRequest {
                id: id.clone(),
                modifications: "priority:H".to_string(),
            }))
            .await
            .unwrap();

        assert!(!result.is_error.unwrap_or(false));

        let info = server
            .get_task(Parameters(TaskIdRequest { id }))
            .await
            .unwrap();

        // taskwarrior shows "H" or "High" in task info
        let out = text_of(&info);
        assert!(out.contains('H'), "priority should be H after modify");
    }

    #[tokio::test]
    async fn test_modify_task_adds_tag() {
        let (_dir, server) = test_server();
        let id = add_task(&server, "Tag me", "tag-test").await;

        server
            .modify_task(Parameters(ModifyTaskRequest {
                id: id.clone(),
                modifications: "+newtag".to_string(),
            }))
            .await
            .unwrap();

        let info = server
            .get_task(Parameters(TaskIdRequest { id }))
            .await
            .unwrap();

        assert!(text_of(&info).contains("newtag"));
    }

    // ── complete_task ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_complete_task_removes_from_pending() {
        let (_dir, server) = test_server();
        let id = add_task(&server, "Task to complete", "done-test").await;

        let result = server
            .complete_task(Parameters(TaskIdRequest { id }))
            .await
            .unwrap();

        assert!(!result.is_error.unwrap_or(false));

        // Should no longer appear in the pending list
        let list = server
            .list_tasks(Parameters(ListTasksRequest {
                project: "done-test".to_string(),
                filter: None,
                report: Some("list".to_string()),
                all_projects: None,
            }))
            .await
            .unwrap();

        assert!(!text_of(&list).contains("Task to complete"));
    }

    // ── delete_task ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_delete_task_removes_from_list() {
        let (_dir, server) = test_server();
        let id = add_task(&server, "Task to delete", "delete-test").await;

        let result = server
            .delete_task(Parameters(TaskIdRequest { id }))
            .await
            .unwrap();

        assert!(!result.is_error.unwrap_or(false));

        let list = server
            .list_tasks(Parameters(ListTasksRequest {
                project: "delete-test".to_string(),
                filter: None,
                report: Some("list".to_string()),
                all_projects: None,
            }))
            .await
            .unwrap();

        assert!(!text_of(&list).contains("Task to delete"));
    }

    // ── annotate_task ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_annotate_task_appears_in_info() {
        let (_dir, server) = test_server();
        let id = add_task(&server, "Task to annotate", "annotate-test").await;

        let result = server
            .annotate_task(Parameters(AnnotateTaskRequest {
                id: id.clone(),
                note: "Important context note xyzzy".to_string(),
            }))
            .await
            .unwrap();

        assert!(!result.is_error.unwrap_or(false));

        let info = server
            .get_task(Parameters(TaskIdRequest { id }))
            .await
            .unwrap();

        assert!(text_of(&info).contains("Important context note xyzzy"));
    }
}
