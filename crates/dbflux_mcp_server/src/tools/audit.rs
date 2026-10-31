//! Audit log tools for MCP server.
//!
//! Provides tools for querying and exporting audit logs:
//! - `query_audit_logs`: Query audit logs with filters
//! - `get_audit_entry`: Get a specific audit entry by ID
//! - `export_audit_logs`: Export audit logs in CSV or JSON format

use dbflux_audit::export::AuditExportFormat;
use dbflux_audit::query::AuditQueryFilter;
use rmcp::{
    ErrorData,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    schemars::JsonSchema,
    tool, tool_router,
};
use serde::Deserialize;

use crate::{
    helper::{IntoErrorData, to_json_content},
    server::DbFluxServer,
    state::ServerState,
};
use dbflux_audit::{AuditEvent, AuditEventDto, AuditService};
use dbflux_core::LogErr;
use std::collections::HashSet;

/// Rows read per page while filtering audit events by visibility.
const VISIBILITY_PAGE_SIZE: usize = 500;

/// Which audit events the calling client may read through one audit tool.
///
/// The audit log also holds the GUI's activity and other clients' calls on
/// every connection, so a client sees its own events, and other events only on
/// connections it may use that tool on (MCP-enabled, and a policy assigned on
/// that connection allows the tool). Global scope is not enough, because every
/// trusted client gets the read-only role there.
pub(crate) struct AuditVisibility {
    client_id: String,
    connections: HashSet<String>,
}

impl AuditVisibility {
    pub(crate) async fn for_tool(state: &ServerState, tool_id: &str) -> Self {
        let connection_ids: Vec<String> = state
            .profile_manager
            .read()
            .await
            .profiles
            .iter()
            .map(|profile| profile.id.to_string())
            .collect();

        let policy_engine = {
            let runtime = state.runtime.read().await;
            dbflux_policy::PolicyEngine::new(
                runtime.policy_assignments_for_engine(),
                runtime.roles_for_engine(),
                runtime.policies_for_engine(),
            )
        };

        let mut connections = HashSet::new();
        for connection_id in connection_ids {
            if !state.is_mcp_enabled_for_connection(&connection_id).await {
                continue;
            }

            let decision = policy_engine
                .evaluate(&dbflux_policy::PolicyEvaluationRequest {
                    actor_id: state.client_id.clone(),
                    connection_id: connection_id.clone(),
                    tool_id: tool_id.to_string(),
                    classification: dbflux_policy::ExecutionClassification::Read,
                })
                .log_err_with("Failed to evaluate audit visibility for a connection");

            if matches!(decision, Some(dbflux_policy::PolicyDecision::Allow)) {
                connections.insert(connection_id);
            }
        }

        Self {
            client_id: state.client_id.clone(),
            connections,
        }
    }

    pub(crate) fn allows(&self, event: &AuditEventDto) -> bool {
        event.actor_id == self.client_id
            || event
                .connection_id
                .as_deref()
                .is_some_and(|connection_id| self.connections.contains(connection_id))
    }

    /// The newest `filter.limit` visible matches (all when `None`), oldest
    /// first like `AuditService::query`. Pages through the matches so the
    /// limit counts visible events, not rows before filtering.
    fn query(
        &self,
        audit_service: &AuditService,
        filter: &AuditQueryFilter,
    ) -> Result<Vec<AuditEvent>, String> {
        let wanted = filter.limit;
        let page_filter = AuditQueryFilter {
            limit: Some(VISIBILITY_PAGE_SIZE),
            ..filter.clone()
        };

        let mut visible = Vec::new();
        let mut offset = 0;

        loop {
            let page = audit_service
                .query_extended_page(&page_filter, offset)
                .map_err(|e| format!("Failed to query audit logs: {}", e))?;
            let page_len = page.len();

            for event in page.into_iter().filter(|event| self.allows(event)) {
                visible.push(legacy_event(event));

                if wanted.is_some_and(|wanted| visible.len() >= wanted) {
                    visible.reverse();
                    return Ok(visible);
                }
            }

            if page_len < VISIBILITY_PAGE_SIZE {
                break;
            }
            offset += page_len;
        }

        visible.reverse();
        Ok(visible)
    }
}

fn legacy_event(event: AuditEventDto) -> AuditEvent {
    AuditEvent {
        tool_id: event.legacy_tool_id(),
        decision: event.legacy_decision(),
        id: event.id,
        actor_id: event.actor_id,
        reason: event.reason,
        created_at_epoch_ms: event.created_at_epoch_ms,
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QueryAuditLogsParams {
    #[schemars(description = "Filter by actor ID (optional)")]
    pub actor_id: Option<String>,

    #[schemars(description = "Filter by tool ID (optional)")]
    pub tool_id: Option<String>,

    #[schemars(
        description = "Filter by decision (e.g., 'allow', 'deny', 'approved', 'rejected') (optional)"
    )]
    pub decision: Option<String>,

    #[schemars(
        description = "Filter by start date in ISO8601 format (e.g., '2024-03-20T10:00:00Z') (optional)"
    )]
    pub start_date: Option<String>,

    #[schemars(
        description = "Filter by end date in ISO8601 format (e.g., '2024-03-20T23:59:59Z') (optional)"
    )]
    pub end_date: Option<String>,

    #[schemars(description = "Maximum number of entries to return (optional)")]
    pub limit: Option<usize>,

    #[schemars(description = "Filter by category (e.g., 'mcp', 'query', 'governance') (optional)")]
    pub category: Option<String>,

    #[schemars(description = "Filter by level (e.g., 'info', 'warn', 'error') (optional)")]
    pub level: Option<String>,

    #[schemars(
        description = "Filter by outcome (e.g., 'success', 'failure', 'cancelled') (optional)"
    )]
    pub outcome: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetAuditEntryParams {
    #[schemars(description = "Audit entry ID")]
    pub id: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportAuditLogsParams {
    #[schemars(description = "Filter by actor ID (optional)")]
    pub actor_id: Option<String>,

    #[schemars(description = "Filter by tool ID (optional)")]
    pub tool_id: Option<String>,

    #[schemars(
        description = "Filter by decision (e.g., 'allow', 'deny', 'approved', 'rejected') (optional)"
    )]
    pub decision: Option<String>,

    #[schemars(
        description = "Filter by start date in ISO8601 format (e.g., '2024-03-20T10:00:00Z') (optional)"
    )]
    pub start_date: Option<String>,

    #[schemars(
        description = "Filter by end date in ISO8601 format (e.g., '2024-03-20T23:59:59Z') (optional)"
    )]
    pub end_date: Option<String>,

    #[schemars(description = "Maximum number of entries to return (optional)")]
    pub limit: Option<usize>,

    #[schemars(description = "Filter by category (e.g., 'mcp', 'query', 'governance') (optional)")]
    pub category: Option<String>,

    #[schemars(description = "Filter by level (e.g., 'info', 'warn', 'error') (optional)")]
    pub level: Option<String>,

    #[schemars(
        description = "Filter by outcome (e.g., 'success', 'failure', 'cancelled') (optional)"
    )]
    pub outcome: Option<String>,

    #[schemars(description = "Export format: 'csv' or 'json'")]
    pub format: String,
}

#[allow(dead_code)] // Used by query_audit_logs and export_audit_logs tools
fn parse_iso8601_to_epoch_ms(date_str: &str) -> Result<i64, String> {
    use chrono::DateTime;

    DateTime::parse_from_rfc3339(date_str)
        .map(|dt| dt.timestamp_millis())
        .map_err(|e| format!("Invalid ISO8601 date '{}': {}", date_str, e))
}

/// Common filter fields shared by query and export audit params.
struct AuditFilterInput {
    actor_id: Option<String>,
    tool_id: Option<String>,
    decision: Option<String>,
    start_date: Option<String>,
    end_date: Option<String>,
    limit: Option<usize>,
    category: Option<String>,
    level: Option<String>,
    outcome: Option<String>,
}

impl AuditFilterInput {
    fn from_query(p: &QueryAuditLogsParams) -> Self {
        Self {
            actor_id: p.actor_id.clone(),
            tool_id: p.tool_id.clone(),
            decision: p.decision.clone(),
            start_date: p.start_date.clone(),
            end_date: p.end_date.clone(),
            limit: p.limit,
            category: p.category.clone(),
            level: p.level.clone(),
            outcome: p.outcome.clone(),
        }
    }

    fn from_export(p: &ExportAuditLogsParams) -> Self {
        Self {
            actor_id: p.actor_id.clone(),
            tool_id: p.tool_id.clone(),
            decision: p.decision.clone(),
            start_date: p.start_date.clone(),
            end_date: p.end_date.clone(),
            limit: p.limit,
            category: p.category.clone(),
            level: p.level.clone(),
            outcome: p.outcome.clone(),
        }
    }

    fn into_filter(self) -> Result<AuditQueryFilter, String> {
        let start_epoch_ms = self
            .start_date
            .as_deref()
            .map(parse_iso8601_to_epoch_ms)
            .transpose()?;

        let end_epoch_ms = self
            .end_date
            .as_deref()
            .map(parse_iso8601_to_epoch_ms)
            .transpose()?;

        Ok(AuditQueryFilter {
            actor_id: self.actor_id,
            tool_id: self.tool_id,
            decision: self.decision,
            start_epoch_ms,
            end_epoch_ms,
            limit: self.limit,
            level: self.level,
            category: self.category,
            action: None,
            source_id: None,
            outcome: self.outcome,
            object_type: None,
            free_text: None,
            correlation_id: None,
        })
    }
}

#[tool_router(router = audit_router, vis = "pub")]
impl DbFluxServer {
    #[tool(
        description = "Query audit logs with optional filters (actor, tool, decision, date range, limit)"
    )]
    async fn query_audit_logs(
        &self,
        Parameters(params): Parameters<QueryAuditLogsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        use dbflux_policy::ExecutionClassification;

        let filter_input = AuditFilterInput::from_query(&params);

        let state = self.state.clone();

        self.governance
            .authorize_and_execute(
                "query_audit_logs",
                None,
                ExecutionClassification::Read,
                move || async move {
                    let filter = filter_input
                        .into_filter()
                        .map_err(|e| e.into_error_data())?;

                    let visibility = AuditVisibility::for_tool(&state, "query_audit_logs").await;
                    let runtime = state.runtime.read().await;

                    let events = visibility
                        .query(runtime.audit_service(), &filter)
                        .map_err(|e| e.into_error_data())?;

                    Ok(CallToolResult::success(vec![to_json_content(&events)?]))
                },
            )
            .await
    }

    #[tool(description = "Get a specific audit entry by ID")]
    async fn get_audit_entry(
        &self,
        Parameters(params): Parameters<GetAuditEntryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        use dbflux_policy::ExecutionClassification;

        let id = params.id;

        let state = self.state.clone();

        self.governance
            .authorize_and_execute(
                "get_audit_entry",
                None,
                ExecutionClassification::Read,
                move || async move {
                    let visibility = AuditVisibility::for_tool(&state, "get_audit_entry").await;
                    let runtime = state.runtime.read().await;
                    let audit_service = runtime.audit_service();

                    let event = audit_service
                        .get_extended(id)
                        .map_err(|e| format!("Failed to get audit entry: {}", e))
                        .map_err(|e| e.into_error_data())?;

                    // An entry the client may not see answers like a missing
                    // one, so its existence does not leak either.
                    match event.filter(|entry| visibility.allows(entry)) {
                        Some(entry) => Ok(CallToolResult::success(vec![to_json_content(
                            &legacy_event(entry),
                        )?])),
                        None => {
                            Err(format!("Audit entry with ID {} not found", id).into_error_data())
                        }
                    }
                },
            )
            .await
    }

    #[tool(description = "Export audit logs in CSV or JSON format with optional filters")]
    async fn export_audit_logs(
        &self,
        Parameters(params): Parameters<ExportAuditLogsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        use dbflux_policy::ExecutionClassification;

        let filter_input = AuditFilterInput::from_export(&params);
        let format = params.format;

        let state = self.state.clone();

        self.governance
            .authorize_and_execute(
                "export_audit_logs",
                None,
                ExecutionClassification::Read,
                move || async move {
                    let filter = filter_input
                        .into_filter()
                        .map_err(|e| e.into_error_data())?;

                    let export_format = match format.to_lowercase().as_str() {
                        "csv" => AuditExportFormat::Csv,
                        "json" => AuditExportFormat::Json,
                        _ => {
                            return Err(format!(
                                "Invalid export format '{}'. Must be 'csv' or 'json'",
                                format
                            )
                            .into_error_data());
                        }
                    };

                    let visibility = AuditVisibility::for_tool(&state, "export_audit_logs").await;
                    let runtime = state.runtime.read().await;

                    let events = visibility
                        .query(runtime.audit_service(), &filter)
                        .map_err(|e| e.into_error_data())?;
                    let output = dbflux_audit::export::export_entries(&events, export_format)
                        .map_err(|e| format!("Failed to export audit logs: {}", e))
                        .map_err(|e| e.into_error_data())?;

                    Ok(CallToolResult::success(vec![ContentBlock::text(output)]))
                },
            )
            .await
    }
}
