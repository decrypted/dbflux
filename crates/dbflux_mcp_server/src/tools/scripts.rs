//! Script management tools for MCP server.
//!
//! Provides CRUD operations and execution for database scripts stored in
//! the platform-specific scripts directory (~/.local/share/dbflux/scripts/).
//!
//! All tools use `ScriptsDirectory` from `dbflux_core` to manage file operations.

use crate::{
    DbFluxServer,
    governance::{AuditDetails, GovernanceMiddleware, current_tool_call_arguments},
    helper::{IntoErrorData, to_json_content},
    state::ServerState,
};
use dbflux_core::{
    Connection, DbError, HookKind, LanguageService, QueryLanguage, QueryRequest,
    ReadOnlyEnforcement, ScriptSource, ScriptsDirectory, script_content_sha256,
};
use dbflux_mcp::server::authorization::AuthorizationOutcome;
use dbflux_policy::ExecutionClassification;
use rmcp::{
    ErrorData,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    schemars::JsonSchema,
    tool, tool_router,
};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListScriptsParams {
    #[schemars(description = "Optional subfolder path to list (relative to scripts root)")]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetScriptParams {
    #[schemars(description = "Path to the script file (relative to scripts root)")]
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateScriptParams {
    #[schemars(description = "Script name (without extension)")]
    pub name: String,

    #[schemars(description = "Script content")]
    pub content: String,

    #[schemars(description = "File extension (e.g., 'sql', 'js', 'lua', 'py', 'sh')")]
    pub extension: String,

    #[schemars(description = "Optional subfolder path (relative to scripts root)")]
    pub folder: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateScriptParams {
    #[schemars(description = "Path to the script file (relative to scripts root)")]
    pub path: String,

    #[schemars(description = "New script content")]
    pub content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteScriptParams {
    #[schemars(description = "Path to the script file (relative to scripts root)")]
    pub path: String,

    #[schemars(description = "Confirmation string - must match filename (not full path)")]
    pub confirm: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExecuteScriptParams {
    #[schemars(description = "Path to the script file (relative to scripts root)")]
    pub path: String,

    #[schemars(description = "Connection ID from DBFlux configuration")]
    pub connection_id: String,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)] // Used by list_scripts tool via serde serialization
struct ScriptEntryDto {
    path: String,
    name: String,
    #[serde(rename = "type")]
    entry_type: String,
    extension: Option<String>,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)] // Used by get_script tool via serde serialization
struct ScriptContentDto {
    path: String,
    name: String,
    content: String,
    language: String,
    size: usize,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)] // Used by create_script tool via serde serialization
struct ScriptCreatedDto {
    path: String,
    name: String,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)] // Used by update_script tool via serde serialization
struct ScriptUpdatedDto {
    path: String,
    size: usize,
}

pub const DELETE_CONFIRMATION_ERROR: &str =
    "Confirmation string must match filename (not full path)";

pub fn validate_delete_params(params: &DeleteScriptParams) -> Result<(), String> {
    let path = Path::new(&params.path);
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Invalid path")?;

    if params.confirm != filename {
        return Err(DELETE_CONFIRMATION_ERROR.to_string());
    }
    Ok(())
}

#[tool_router(router = scripts_router, vis = "pub")]
impl DbFluxServer {
    #[tool(description = "List scripts in the scripts directory")]
    async fn list_scripts(
        &self,
        Parameters(params): Parameters<ListScriptsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        use dbflux_policy::ExecutionClassification;

        let state = self.state.clone();
        let path = params.path.clone();

        self.governance
            .authorize_and_execute(
                "list_scripts",
                None, // Global operation
                ExecutionClassification::Metadata,
                move || async move {
                    let result = Self::list_scripts_impl(state, path.as_deref())
                        .await
                        .map_err(|e| e.into_error_data())?;

                    Ok(CallToolResult::success(vec![to_json_content(&result)?]))
                },
            )
            .await
    }

    #[tool(description = "Get script content and metadata")]
    async fn get_script(
        &self,
        Parameters(params): Parameters<GetScriptParams>,
    ) -> Result<CallToolResult, ErrorData> {
        use dbflux_policy::ExecutionClassification;

        let state = self.state.clone();
        let path = params.path.clone();

        self.governance
            .authorize_and_execute(
                "get_script",
                None, // Global operation
                ExecutionClassification::Read,
                move || async move {
                    let result = Self::get_script_impl(state, &path)
                        .await
                        .map_err(|e| e.into_error_data())?;

                    Ok(CallToolResult::success(vec![to_json_content(&result)?]))
                },
            )
            .await
    }

    #[tool(description = "Create a new script file")]
    async fn create_script(
        &self,
        Parameters(params): Parameters<CreateScriptParams>,
    ) -> Result<CallToolResult, ErrorData> {
        use dbflux_policy::ExecutionClassification;

        let state = self.state.clone();
        let name = params.name.clone();
        let content = params.content.clone();
        let extension = params.extension.clone();
        let folder = params.folder.clone();

        self.governance
            .authorize_and_execute(
                "create_script",
                None, // Global operation
                ExecutionClassification::Write,
                move || async move {
                    let result = Self::create_script_impl(
                        state,
                        &name,
                        &content,
                        &extension,
                        folder.as_deref(),
                    )
                    .await
                    .map_err(|e| e.into_error_data())?;

                    Ok(CallToolResult::success(vec![to_json_content(&result)?]))
                },
            )
            .await
    }

    #[tool(description = "Update script content")]
    async fn update_script(
        &self,
        Parameters(params): Parameters<UpdateScriptParams>,
    ) -> Result<CallToolResult, ErrorData> {
        use dbflux_policy::ExecutionClassification;

        let state = self.state.clone();
        let path = params.path.clone();
        let content = params.content.clone();

        self.governance
            .authorize_and_execute(
                "update_script",
                None, // Global operation
                ExecutionClassification::Write,
                move || async move {
                    let result = Self::update_script_impl(state, &path, &content)
                        .await
                        .map_err(|e| e.into_error_data())?;

                    Ok(CallToolResult::success(vec![to_json_content(&result)?]))
                },
            )
            .await
    }

    #[tool(description = "Delete a script file (requires confirmation)")]
    async fn delete_script(
        &self,
        Parameters(params): Parameters<DeleteScriptParams>,
    ) -> Result<CallToolResult, ErrorData> {
        use dbflux_policy::ExecutionClassification;

        // Validate confirmation matches filename
        validate_delete_params(&params).map_err(|e| ErrorData::invalid_params(e, None))?;

        let state = self.state.clone();
        let path = params.path.clone();

        self.governance
            .authorize_and_execute(
                "delete_script",
                None, // Global operation
                ExecutionClassification::Admin,
                move || async move {
                    Self::delete_script_impl(state, &path)
                        .await
                        .map_err(|e| e.into_error_data())?;

                    Ok(CallToolResult::success(vec![ContentBlock::text(
                        r#"{"status": "Script deleted successfully"}"#,
                    )]))
                },
            )
            .await
    }

    #[tool(description = "Execute a script against a database connection")]
    async fn execute_script(
        &self,
        Parameters(params): Parameters<ExecuteScriptParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.state.clone();
        let connection_id = params.connection_id.as_str();

        // Read once: the text that is classified, hashed into the approval
        // and run is the same, whatever happens to the file meanwhile.
        let (content, language) = Self::read_script_for_execution(&state, &params.path)
            .await
            .map_err(|e| e.into_error_data())?;
        let authorizations = ScriptAuthorizations::new(&self.governance, connection_id, &content);

        let mut connection = None;
        if runs_database_query(&language) {
            connection = Self::cached_connection(&state, connection_id).await;

            // Connecting runs the profile's hooks, SSO login, SSH tunnel and
            // ProxyCommand, so a connection that is not open yet is only
            // opened for a call authorized by what its text alone shows.
            if connection.is_none() {
                let unconnected = Self::detect_execution_classification(&content, &language, None);
                let outcome = authorizations.authorize(unconnected).await?;

                match Self::get_or_connect(state.clone(), connection_id).await {
                    Ok(opened) => connection = Some(opened),
                    Err(error) => {
                        return self
                            .governance
                            .run_authorized_audited(
                                "execute_script",
                                Some(connection_id),
                                &outcome,
                                || async move { Err(error.into_error_data()) },
                            )
                            .await;
                    }
                }
            }
        }

        // The driver's language service can see more than the generic
        // heuristic, so the class is decided again with the connection.
        let classification = Self::detect_execution_classification(
            &content,
            &language,
            connection
                .as_deref()
                .map(|connection| connection.language_service()),
        );
        let enforces_read_only = connection
            .as_deref()
            .is_some_and(|connection| connection.metadata().enforces_read_only());
        let (classification, read_only) = govern_read_only(classification, enforces_read_only);

        let governance = &self.governance;
        let authorizations = &authorizations;

        govern_script_attempts(classification, read_only, |classification, read_only| {
            let content = content.clone();
            let language = language.clone();
            let connection = connection.clone();

            async move {
                let outcome = match authorizations.authorize(classification).await {
                    Ok(outcome) => outcome,
                    Err(error) => return ScriptAttempt::Completed(Err(error)),
                };

                let refusal: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
                let handler_refusal = refusal.clone();

                let result = governance
                    .run_authorized_audited(
                        "execute_script",
                        Some(connection_id),
                        &outcome,
                        move || async move {
                            let outcome = Self::execute_script_impl(
                                connection,
                                &content,
                                &language,
                                classification,
                                read_only,
                            )
                            .await
                            .map_err(|e| e.into_error_data())?;

                            match outcome {
                                ScriptOutcome::Completed(value) => Ok((
                                    CallToolResult::success(vec![to_json_content(&value)?]),
                                    AuditDetails::default(),
                                )),
                                ScriptOutcome::ReadOnlyRefused(reason) => {
                                    if let Ok(mut slot) = handler_refusal.lock() {
                                        *slot = Some(reason.clone());
                                    }
                                    Err(ErrorData::internal_error(reason, None))
                                }
                            }
                        },
                    )
                    .await;

                let refused = refusal.lock().ok().and_then(|mut slot| slot.take());
                match refused {
                    Some(reason) => ScriptAttempt::ReadOnlyRefused(reason),
                    None => ScriptAttempt::Completed(result),
                }
            }
        })
        .await
    }
}

/// Whether `execute_script` runs a script in `language` as a database query,
/// and so needs a connection. Other languages are refused after governance.
fn runs_database_query(language: &QueryLanguage) -> bool {
    matches!(
        language,
        QueryLanguage::Sql
            | QueryLanguage::CloudWatchLogsInsightsQl
            | QueryLanguage::OpenSearchPpl
            | QueryLanguage::OpenSearchSql
            | QueryLanguage::MongoQuery
            | QueryLanguage::RedisCommands
            | QueryLanguage::Cql
            | QueryLanguage::Cypher
            | QueryLanguage::InfluxQuery
            | QueryLanguage::Flux
    )
}

/// Longest script text an approval shows. The hash covers all of it.
const APPROVAL_SCRIPT_TEXT_BYTES: usize = 16 * 1024;

/// What an `execute_script` approval is queued with and matched against: the
/// call's arguments plus the script's hash, its class and its text.
///
/// The arguments name only a path, and the file can change after a person
/// approves it, so the hash makes changed content need a new approval. The
/// class is part of it because the same text can be authorized under
/// several classes in one call (before and after connecting), and an
/// approval given for one must not authorize another.
fn script_approval_payload(
    arguments: serde_json::Value,
    content: &str,
    classification: ExecutionClassification,
) -> serde_json::Value {
    let mut payload = match arguments {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };

    let mut text_end = APPROVAL_SCRIPT_TEXT_BYTES.min(content.len());
    while !content.is_char_boundary(text_end) {
        text_end -= 1;
    }

    payload.insert(
        "script_sha256".to_string(),
        serde_json::Value::String(script_content_sha256(content)),
    );
    payload.insert(
        "script_classification".to_string(),
        serde_json::to_value(classification).unwrap_or(serde_json::Value::Null),
    );
    payload.insert(
        "script_text".to_string(),
        serde_json::Value::String(content.get(..text_end).unwrap_or_default().to_string()),
    );
    if text_end < content.len() {
        payload.insert(
            "script_text_truncated".to_string(),
            serde_json::Value::Bool(true),
        );
    }

    serde_json::Value::Object(payload)
}

/// The authorizations one `execute_script` call has obtained, by class.
///
/// A call can be authorized before it connects (by the text alone) and
/// again once the driver has classified it, and a read the driver refused to
/// enforce is retried as a write. Each class is authorized once per call.
/// Classes are compared for equality only: policies decide per class, so an
/// authorization for one class never stands in for another.
struct ScriptAuthorizations<'a> {
    governance: &'a GovernanceMiddleware,
    connection_id: &'a str,
    content: &'a str,
    granted: Mutex<Vec<(ExecutionClassification, AuthorizationOutcome)>>,
}

impl<'a> ScriptAuthorizations<'a> {
    fn new(governance: &'a GovernanceMiddleware, connection_id: &'a str, content: &'a str) -> Self {
        Self {
            governance,
            connection_id,
            content,
            granted: Mutex::new(Vec::new()),
        }
    }

    async fn authorize(
        &self,
        classification: ExecutionClassification,
    ) -> Result<AuthorizationOutcome, ErrorData> {
        let earlier = self.granted.lock().ok().and_then(|granted| {
            granted
                .iter()
                .find(|(granted_class, _)| *granted_class == classification)
                .map(|(_, outcome)| outcome.clone())
        });
        if let Some(outcome) = earlier {
            return Ok(outcome);
        }

        let payload =
            script_approval_payload(current_tool_call_arguments(), self.content, classification);
        let outcome = self
            .governance
            .authorize_with_payload(
                "execute_script",
                Some(self.connection_id),
                classification,
                payload,
            )
            .await?;

        if let Ok(mut granted) = self.granted.lock() {
            granted.push((classification, outcome.clone()));
        }

        Ok(outcome)
    }
}

/// What running a script's query produced.
#[derive(Debug)]
enum ScriptOutcome {
    /// The query ran; the value is its serialized result.
    Completed(serde_json::Value),

    /// The driver refused to enforce read-only execution and ran nothing.
    ReadOnlyRefused(String),
}

/// What one governed attempt at a script produced.
enum ScriptAttempt {
    /// The attempt was authorized or denied and, when authorized, ran.
    Completed(Result<CallToolResult, ErrorData>),

    /// The attempt was authorized as a read, but the driver refused to
    /// enforce read-only execution and ran nothing.
    ReadOnlyRefused(String),
}

/// Decides how a script is governed and run from its classification and
/// whether the connection's driver enforces read-only execution.
///
/// A `Read` or `Metadata` script runs with
/// [`ReadOnlyEnforcement::Required`] when the driver enforces it. Otherwise
/// it is governed as `Write`, so the policy decides whether it may run
/// without the database refusing writes. A script is never run as a read
/// without that enforcement. Other classifications are unchanged.
fn govern_read_only(
    classification: ExecutionClassification,
    enforces_read_only: bool,
) -> (ExecutionClassification, ReadOnlyEnforcement) {
    match classification {
        ExecutionClassification::Read | ExecutionClassification::Metadata if enforces_read_only => {
            (classification, ReadOnlyEnforcement::Required)
        }
        ExecutionClassification::Read | ExecutionClassification::Metadata => {
            (ExecutionClassification::Write, ReadOnlyEnforcement::None)
        }
        _ => (classification, ReadOnlyEnforcement::None),
    }
}

/// Runs a governed script attempt and, when the driver refused read-only
/// enforcement at execution time (for example because the session is inside
/// a transaction), runs it once more governed as `Write` without it.
///
/// The refused attempt ran nothing, and the second attempt goes through the
/// policy again, so a refusal can only lead to a run the policy allows for a
/// write: approval, denial, or execution.
async fn govern_script_attempts<F, Fut>(
    classification: ExecutionClassification,
    read_only: ReadOnlyEnforcement,
    mut attempt: F,
) -> Result<CallToolResult, ErrorData>
where
    F: FnMut(ExecutionClassification, ReadOnlyEnforcement) -> Fut,
    Fut: Future<Output = ScriptAttempt>,
{
    let reason = match attempt(classification, read_only).await {
        ScriptAttempt::Completed(result) => return result,
        ScriptAttempt::ReadOnlyRefused(reason) => reason,
    };

    log::info!(
        "execute_script: read-only enforcement was refused ({reason}); governing the script as a write"
    );

    match attempt(ExecutionClassification::Write, ReadOnlyEnforcement::None).await {
        ScriptAttempt::Completed(result) => result,
        ScriptAttempt::ReadOnlyRefused(reason) => Err(ErrorData::internal_error(reason, None)),
    }
}

/// Runs a script's query on `connection`, reporting a read-only refusal
/// apart from other failures.
///
/// `confirmed_ceiling` and `read_only` come from the policy decision this
/// call follows, never from the MCP client. See
/// `QueryRequest::confirmed_ceiling` and `QueryRequest::read_only`. With
/// `Required`, any `NotSupported` means the driver ran nothing, so the caller
/// may govern the script again.
async fn run_script_query(
    connection: Arc<dyn Connection>,
    query: &str,
    classification: ExecutionClassification,
    read_only: ReadOnlyEnforcement,
) -> Result<ScriptOutcome, String> {
    use crate::helper::{script_failure_message, serialize_query_result};

    let request = QueryRequest {
        sql: query.to_string(),
        params: Vec::new(),
        limit: None,
        offset: None,
        statement_timeout: None,
        database: None,
        execution_context: None,
        confirmed_ceiling: Some(classification),
        read_only,
    };

    let outcome = DbFluxServer::execute_connection_blocking(connection, move |connection| {
        match connection.execute(&request) {
            Ok(result) => Ok(Ok(result)),
            Err(DbError::NotSupported(reason)) if request.read_only.is_required() => {
                Ok(Err(reason))
            }
            Err(error) => Err(format!("Query execution failed: {}", error)),
        }
    })
    .await?;

    let result = match outcome {
        Ok(result) => result,
        Err(reason) => return Ok(ScriptOutcome::ReadOnlyRefused(reason)),
    };

    if let Some(failure) = script_failure_message(&result) {
        return Err(failure);
    }

    Ok(ScriptOutcome::Completed(serialize_query_result(&result)))
}

/// Resolves a client-supplied path against the scripts root.
///
/// `Path::starts_with` compares components without resolving `..`, so a lexical
/// prefix check alone lets `../outside` through. Only plain components are
/// accepted, and the path must then pass [`ensure_resolves_inside_root`].
fn resolve_in_scripts_root(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let has_only_plain_components = Path::new(relative)
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));

    if !has_only_plain_components {
        return Err(OUTSIDE_SCRIPTS_ROOT.to_string());
    }

    let full_path = root.join(relative);
    ensure_resolves_inside_root(root, &full_path)?;

    Ok(full_path)
}

const OUTSIDE_SCRIPTS_ROOT: &str = "Path is outside scripts root";

/// Checks that the deepest part of `path` that exists on disk lies inside
/// `root` once symlinks are followed, so a path that does not exist yet is
/// judged by the folder it would be created in.
///
/// An entry that exists but cannot be resolved is a dangling symlink, and is
/// refused because writing through it creates its target, wherever that is.
fn ensure_resolves_inside_root(root: &Path, path: &Path) -> Result<(), String> {
    let resolve_error = |error: std::io::Error| format!("Failed to resolve script path: {}", error);

    for candidate in path.ancestors() {
        match std::fs::symlink_metadata(candidate) {
            Ok(_) => {
                return match std::fs::canonicalize(candidate) {
                    Ok(resolved) if resolved.starts_with(root) => Ok(()),
                    Ok(_) => Err(OUTSIDE_SCRIPTS_ROOT.to_string()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        Err(OUTSIDE_SCRIPTS_ROOT.to_string())
                    }
                    Err(error) => Err(resolve_error(error)),
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(resolve_error(error)),
        }
    }

    Err(OUTSIDE_SCRIPTS_ROOT.to_string())
}

/// Rejects a script name or extension that is not a single plain path segment,
/// since both are joined into the file name of the created script.
fn validate_script_file_name(name: &str, extension: &str) -> Result<(), String> {
    let is_single_segment = |value: &str| {
        let mut components = Path::new(value).components();

        matches!(
            (components.next(), components.next()),
            (Some(Component::Normal(_)), None)
        )
    };

    if !is_single_segment(name) || !is_single_segment(extension) {
        return Err("Script name and extension must not contain path separators".to_string());
    }

    Ok(())
}

fn open_scripts_directory(state: &ServerState) -> Result<ScriptsDirectory, String> {
    match &state.scripts_root {
        Some(root) => ScriptsDirectory::new_at(root.clone()),
        None => ScriptsDirectory::new(),
    }
    .map_err(|e| format!("Failed to initialize scripts directory: {}", e))
}

/// The folder under the scripts root that holds file-backed connection hooks.
const HOOKS_FOLDER: &str = "hooks";

const HOOK_SCRIPT_REFUSED: &str =
    "Connection hook scripts cannot be created, changed or deleted over MCP; edit hooks in DBFlux";

/// What a script tool is about to do to a path.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HookWrite {
    Create,
    Overwrite,
    Delete,
}

/// Refuses a script tool write that would change what a connection hook runs.
///
/// Hooks run with the user's rights on the next connect, which an agent can
/// trigger, so writing one is running code. Protected are the hooks folder
/// and every file a loaded profile's hooks name, compared after symlinks are
/// followed, plus (for an overwrite) any file with a second hard link, which
/// may be a hook file under another name.
async fn ensure_not_hook_script(
    state: &ServerState,
    root: &Path,
    path: &Path,
    write: HookWrite,
) -> Result<(), String> {
    let resolved = resolve_existing_prefix(path)?;

    let in_hooks_folder = resolved
        .strip_prefix(root)
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|first| {
            first
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(HOOKS_FOLDER)
        });
    if in_hooks_folder {
        return Err(HOOK_SCRIPT_REFUSED.to_string());
    }

    if referenced_hook_files(state)
        .await
        .iter()
        .any(|hook_file| hook_file.starts_with(&resolved))
    {
        return Err(HOOK_SCRIPT_REFUSED.to_string());
    }

    #[cfg(unix)]
    if write == HookWrite::Overwrite {
        use std::os::unix::fs::MetadataExt;

        if std::fs::metadata(&resolved).is_ok_and(|metadata| metadata.nlink() > 1) {
            return Err(
                "Script has other hard links, which may be a connection hook; it cannot be changed over MCP"
                    .to_string(),
            );
        }
    }

    #[cfg(not(unix))]
    let _ = write;

    Ok(())
}

/// `path` with symlinks in its existing part followed and the part that does
/// not exist yet appended unchanged. Callers have already limited that part
/// to plain components inside the scripts root.
fn resolve_existing_prefix(path: &Path) -> Result<PathBuf, String> {
    let resolve_error = |error: std::io::Error| format!("Failed to resolve script path: {}", error);
    let mut missing = Vec::new();
    let mut candidate = path;

    loop {
        match std::fs::symlink_metadata(candidate) {
            Ok(_) => {
                let resolved = std::fs::canonicalize(candidate).map_err(resolve_error)?;
                return Ok(missing
                    .iter()
                    .rev()
                    .fold(resolved, |resolved, part| resolved.join(part)));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let (Some(parent), Some(name)) = (candidate.parent(), candidate.file_name()) else {
                    return Err(OUTSIDE_SCRIPTS_ROOT.to_string());
                };
                missing.push(name.to_os_string());
                candidate = parent;
            }
            Err(error) => return Err(resolve_error(error)),
        }
    }
}

/// Every existing file the loaded profiles' hooks run or pass to a program,
/// canonicalized: file-backed script and Lua hooks, and command hooks'
/// program and arguments that name a file.
async fn referenced_hook_files(state: &ServerState) -> Vec<PathBuf> {
    let mut named = Vec::new();

    {
        let profile_manager = state.profile_manager.read().await;
        for hooks in profile_manager
            .profiles
            .iter()
            .filter_map(|profile| profile.hooks.as_ref())
        {
            for hook in hooks
                .pre_connect
                .iter()
                .chain(&hooks.post_connect)
                .chain(&hooks.pre_disconnect)
                .chain(&hooks.post_disconnect)
            {
                let base = hook.cwd.clone();
                match &hook.kind {
                    HookKind::Script {
                        source: ScriptSource::File { path },
                        ..
                    }
                    | HookKind::Lua {
                        source: ScriptSource::File { path },
                        ..
                    } => named.push((base, path.clone())),
                    HookKind::Command { command, args } => {
                        for word in std::iter::once(command).chain(args) {
                            if word.contains(std::path::MAIN_SEPARATOR) || word.contains('/') {
                                named.push((base.clone(), PathBuf::from(word)));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    named
        .into_iter()
        .filter_map(|(base, path)| {
            let path = match base {
                Some(base) if path.is_relative() => base.join(path),
                _ => path,
            };
            std::fs::canonicalize(path).ok()
        })
        .collect()
}

// Implementation methods
// Note: These implementation methods are called by the #[tool] macro-generated code.
// Clippy cannot detect this usage, so we suppress dead_code warnings.
impl DbFluxServer {
    #[allow(dead_code)]
    async fn list_scripts_impl(
        state: ServerState,
        subfolder: Option<&str>,
    ) -> Result<Vec<ScriptEntryDto>, String> {
        let scripts_dir = open_scripts_directory(&state)?;
        let root = scripts_dir.root_path();

        // Determine the directory to list
        let target_dir = if let Some(path) = subfolder {
            resolve_in_scripts_root(root, path)?
        } else {
            root.to_path_buf()
        };

        // Convert entries to DTOs
        let entries: Vec<ScriptEntryDto> = scripts_dir
            .entries()
            .iter()
            .filter_map(|entry| {
                // Filter entries that match the target directory
                if subfolder.is_some() && !entry.path().starts_with(&target_dir) {
                    return None;
                }

                let relative_path = entry.path().strip_prefix(root).ok()?.to_str()?.to_string();

                match entry {
                    dbflux_core::ScriptEntry::File {
                        name, extension, ..
                    } => Some(ScriptEntryDto {
                        path: relative_path,
                        name: name.clone(),
                        entry_type: "file".to_string(),
                        extension: Some(extension.clone()),
                    }),
                    dbflux_core::ScriptEntry::Folder { name, .. } => Some(ScriptEntryDto {
                        path: relative_path,
                        name: name.clone(),
                        entry_type: "folder".to_string(),
                        extension: None,
                    }),
                }
            })
            .collect();

        Ok(entries)
    }

    #[allow(dead_code)]
    async fn get_script_impl(
        state: ServerState,
        script_path: &str,
    ) -> Result<ScriptContentDto, String> {
        let scripts_dir = open_scripts_directory(&state)?;
        let root = scripts_dir.root_path();
        let full_path = resolve_in_scripts_root(root, script_path)?;

        if !full_path.exists() {
            return Err("Script not found".to_string());
        }

        if !full_path.is_file() {
            return Err("Path is not a file".to_string());
        }

        let content = std::fs::read_to_string(&full_path)
            .map_err(|e| format!("Failed to read script: {}", e))?;

        let filename = full_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "Invalid filename".to_string())?
            .to_string();

        let language = QueryLanguage::from_path(&full_path)
            .map(|l| l.display_name().to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let size = content.len();

        Ok(ScriptContentDto {
            path: script_path.to_string(),
            name: filename,
            content,
            language,
            size,
        })
    }

    #[allow(dead_code)]
    async fn create_script_impl(
        state: ServerState,
        name: &str,
        content: &str,
        extension: &str,
        folder: Option<&str>,
    ) -> Result<ScriptCreatedDto, String> {
        let mut scripts_dir = open_scripts_directory(&state)?;
        let root = scripts_dir.root_path().to_path_buf();

        // Determine parent directory
        validate_script_file_name(name, extension)?;

        let parent = folder
            .map(|folder_path| resolve_in_scripts_root(&root, folder_path))
            .transpose()?;

        // Mirrors the file name `ScriptsDirectory::create_file` builds, so a
        // dangling symlink with that name is refused before it is written through.
        let file_name = if name.contains('.') {
            name.to_string()
        } else {
            format!("{}.{}", name, extension)
        };
        let target_dir = parent.as_deref().unwrap_or(&root);
        let target = target_dir.join(file_name);
        ensure_resolves_inside_root(&root, &target)?;
        ensure_not_hook_script(&state, &root, &target, HookWrite::Create).await?;

        // Create the file
        let created_path = scripts_dir
            .create_file(parent.as_deref(), name, extension)
            .map_err(|e| format!("Failed to create script file: {}", e))?;

        // Write content
        std::fs::write(&created_path, content)
            .map_err(|e| format!("Failed to write script content: {}", e))?;

        let relative_path = created_path
            .strip_prefix(&root)
            .map_err(|_| "Failed to compute relative path".to_string())?
            .to_str()
            .ok_or_else(|| "Invalid path encoding".to_string())?
            .to_string();

        let filename = created_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "Invalid filename".to_string())?
            .to_string();

        Ok(ScriptCreatedDto {
            path: relative_path,
            name: filename,
        })
    }

    #[allow(dead_code)]
    async fn update_script_impl(
        state: ServerState,
        script_path: &str,
        content: &str,
    ) -> Result<ScriptUpdatedDto, String> {
        let scripts_dir = open_scripts_directory(&state)?;
        let root = scripts_dir.root_path();
        let full_path = resolve_in_scripts_root(root, script_path)?;

        if !full_path.exists() {
            return Err("Script not found".to_string());
        }

        if !full_path.is_file() {
            return Err("Path is not a file".to_string());
        }

        ensure_not_hook_script(&state, root, &full_path, HookWrite::Overwrite).await?;

        std::fs::write(&full_path, content)
            .map_err(|e| format!("Failed to write script: {}", e))?;

        Ok(ScriptUpdatedDto {
            path: script_path.to_string(),
            size: content.len(),
        })
    }

    #[allow(dead_code)]
    async fn delete_script_impl(state: ServerState, script_path: &str) -> Result<(), String> {
        let mut scripts_dir = open_scripts_directory(&state)?;
        let root = scripts_dir.root_path();
        let full_path = resolve_in_scripts_root(root, script_path)?;

        if !full_path.exists() {
            return Err("Script not found".to_string());
        }

        ensure_not_hook_script(&state, root, &full_path, HookWrite::Delete).await?;

        scripts_dir
            .delete(&full_path)
            .map_err(|e| format!("Failed to delete script: {}", e))?;

        Ok(())
    }

    #[allow(dead_code)]
    async fn read_script_for_execution(
        state: &ServerState,
        script_path: &str,
    ) -> Result<(String, QueryLanguage), String> {
        let scripts_dir = open_scripts_directory(state)?;
        let root = scripts_dir.root_path();
        let full_path = resolve_in_scripts_root(root, script_path)?;

        if !full_path.exists() {
            return Err("Script not found".to_string());
        }

        let content = std::fs::read_to_string(&full_path)
            .map_err(|e| format!("Failed to read script: {}", e))?;

        let language = QueryLanguage::from_path(&full_path)
            .ok_or_else(|| "Cannot detect query language from file extension".to_string())?;

        Ok((content, language))
    }

    fn detect_execution_classification(
        content: &str,
        language: &QueryLanguage,
        service: Option<&dyn LanguageService>,
    ) -> dbflux_policy::ExecutionClassification {
        use dbflux_core::classify_query_for_governance;
        use dbflux_policy::ExecutionClassification;

        // Non-query scripts are always classified as Admin
        match language {
            QueryLanguage::Lua | QueryLanguage::Python | QueryLanguage::Bash => {
                return ExecutionClassification::Admin;
            }
            _ => {}
        }

        // For query languages, use the safety module to classify, consulting
        // the driver's language service when one is available.
        classify_query_for_governance(language, content, service)
    }

    #[allow(dead_code)]
    async fn execute_script_impl(
        connection: Option<Arc<dyn Connection>>,
        content: &str,
        language: &QueryLanguage,
        classification: dbflux_policy::ExecutionClassification,
        read_only: ReadOnlyEnforcement,
    ) -> Result<ScriptOutcome, String> {
        match language {
            QueryLanguage::Lua | QueryLanguage::Python | QueryLanguage::Bash => Err(
                "Script language not supported for execution (only database queries)".to_string(),
            ),
            QueryLanguage::Custom(_) => {
                Err("Custom language not supported for execution".to_string())
            }
            _ => {
                let connection =
                    connection.ok_or_else(|| "No connection to run the script on".to_string())?;
                run_script_query(connection, content, classification, read_only).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_in_scripts_root, validate_script_file_name};
    use std::fs;

    fn scripts_root() -> (tempfile::TempDir, std::path::PathBuf) {
        let parent = tempfile::tempdir().expect("create temp dir");
        let root = parent.path().join("scripts");
        fs::create_dir(&root).expect("create scripts root");
        let root = fs::canonicalize(&root).expect("canonicalize scripts root");

        (parent, root)
    }

    #[test]
    fn resolves_a_nested_script_inside_the_root() {
        let (_parent, root) = scripts_root();

        let resolved = resolve_in_scripts_root(&root, "reports/daily.sql").expect("inside root");

        assert_eq!(resolved, root.join("reports/daily.sql"));
    }

    #[test]
    fn rejects_parent_directory_traversal() {
        let (parent, root) = scripts_root();
        fs::write(parent.path().join("secret.txt"), "secret").expect("write outside file");

        for path in [
            "../secret.txt",
            "reports/../../secret.txt",
            "./../secret.txt",
        ] {
            assert!(
                resolve_in_scripts_root(&root, path).is_err(),
                "{path} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_absolute_paths() {
        let (parent, root) = scripts_root();
        let absolute = parent.path().join("secret.txt");

        let result = resolve_in_scripts_root(&root, absolute.to_str().expect("utf-8 path"));

        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlink_that_leads_out_of_the_root() {
        let (parent, root) = scripts_root();
        let outside = parent.path().join("secret.sql");
        fs::write(&outside, "SELECT 1;").expect("write outside file");
        std::os::unix::fs::symlink(&outside, root.join("escape.sql")).expect("create symlink");

        assert!(resolve_in_scripts_root(&root, "escape.sql").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_missing_path_below_a_symlink_that_leads_out_of_the_root() {
        let (parent, root) = scripts_root();
        let outside = parent.path().join("outside");
        fs::create_dir(&outside).expect("create outside dir");
        std::os::unix::fs::symlink(&outside, root.join("escape")).expect("create symlink");

        assert!(resolve_in_scripts_root(&root, "escape/new-folder").is_err());
        assert!(resolve_in_scripts_root(&root, "escape/new-folder/new.sql").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_dangling_symlink() {
        let (parent, root) = scripts_root();
        let missing_target = parent.path().join("created-outside.sql");
        std::os::unix::fs::symlink(&missing_target, root.join("dangling.sql"))
            .expect("create symlink");

        assert!(resolve_in_scripts_root(&root, "dangling.sql").is_err());
    }

    #[test]
    fn accepts_a_missing_path_below_an_existing_folder() {
        let (_parent, root) = scripts_root();
        fs::create_dir(root.join("reports")).expect("create folder");

        let resolved =
            resolve_in_scripts_root(&root, "reports/new/weekly.sql").expect("inside root");

        assert_eq!(resolved, root.join("reports/new/weekly.sql"));
    }

    #[test]
    fn accepts_a_plain_file_name_and_extension() {
        assert!(validate_script_file_name("weekly", "sql").is_ok());
        assert!(validate_script_file_name("weekly.sql", "sql").is_ok());
    }

    #[test]
    fn rejects_file_names_and_extensions_with_path_segments() {
        let cases = [
            ("../escape", "sql"),
            ("../../escape.sql", "sql"),
            ("nested/escape", "sql"),
            ("weekly", "sql/../../escape"),
            ("..", "sql"),
            ("", "sql"),
        ];

        for (name, extension) in cases {
            assert!(
                validate_script_file_name(name, extension).is_err(),
                "{name:?} with {extension:?} must be rejected"
            );
        }
    }

    mod read_only {
        use super::super::{
            ScriptAttempt, ScriptOutcome, govern_read_only, govern_script_attempts,
            run_script_query,
        };
        use dbflux_core::{
            Connection, ConnectionProfile, DbConfig, DbKind, ExecutionClassification,
            ReadOnlyEnforcement,
        };
        use dbflux_test_support::FakeDriver;
        use rmcp::model::CallToolResult;
        use std::sync::{Arc, Mutex};

        type Attempts = Arc<Mutex<Vec<(ExecutionClassification, ReadOnlyEnforcement)>>>;

        fn connect(driver: &FakeDriver) -> Arc<dyn Connection> {
            let profile = ConnectionProfile::new("fake", DbConfig::default_postgres());
            driver.connect_arc(&profile).expect("fake connection")
        }

        fn received_flags(driver: &FakeDriver) -> Vec<ReadOnlyEnforcement> {
            driver
                .stats()
                .executed_requests
                .iter()
                .map(|request| request.read_only)
                .collect()
        }

        /// Governs and runs `SELECT 1` the way `execute_script` does, with
        /// the authorization step replaced by a recorder of the
        /// classification and read-only flag each attempt was governed with.
        async fn govern_and_run(
            driver: &FakeDriver,
            classification: ExecutionClassification,
        ) -> Vec<(ExecutionClassification, ReadOnlyEnforcement)> {
            let connection = connect(driver);
            let (classification, read_only) =
                govern_read_only(classification, connection.metadata().enforces_read_only());

            let attempts: Attempts = Arc::new(Mutex::new(Vec::new()));
            let result =
                govern_script_attempts(classification, read_only, |classification, read_only| {
                    attempts
                        .lock()
                        .expect("attempts mutex")
                        .push((classification, read_only));
                    let connection = connection.clone();
                    async move {
                        match run_script_query(connection, "SELECT 1", classification, read_only)
                            .await
                        {
                            Ok(ScriptOutcome::Completed(_)) => {
                                ScriptAttempt::Completed(Ok(CallToolResult::success(Vec::new())))
                            }
                            Ok(ScriptOutcome::ReadOnlyRefused(reason)) => {
                                ScriptAttempt::ReadOnlyRefused(reason)
                            }
                            Err(error) => ScriptAttempt::Completed(Err(
                                rmcp::ErrorData::internal_error(error, None),
                            )),
                        }
                    }
                })
                .await;

            assert!(result.is_ok(), "the script should run: {result:?}");
            attempts.lock().expect("attempts mutex").clone()
        }

        #[test]
        fn read_scripts_require_enforcement_only_where_the_driver_provides_it() {
            for classification in [
                ExecutionClassification::Read,
                ExecutionClassification::Metadata,
            ] {
                assert_eq!(
                    govern_read_only(classification, true),
                    (classification, ReadOnlyEnforcement::Required)
                );
                assert_eq!(
                    govern_read_only(classification, false),
                    (ExecutionClassification::Write, ReadOnlyEnforcement::None)
                );
            }

            for classification in [
                ExecutionClassification::Write,
                ExecutionClassification::Destructive,
                ExecutionClassification::AdminSafe,
                ExecutionClassification::Admin,
                ExecutionClassification::AdminDestructive,
            ] {
                for enforces in [true, false] {
                    assert_eq!(
                        govern_read_only(classification, enforces),
                        (classification, ReadOnlyEnforcement::None)
                    );
                }
            }
        }

        #[tokio::test]
        async fn a_read_script_on_an_enforcing_driver_carries_required() {
            let driver = FakeDriver::new(DbKind::Postgres).with_read_only_enforcement();

            let attempts = govern_and_run(&driver, ExecutionClassification::Read).await;

            assert_eq!(
                attempts,
                vec![(ExecutionClassification::Read, ReadOnlyEnforcement::Required)]
            );
            assert_eq!(received_flags(&driver), vec![ReadOnlyEnforcement::Required]);
        }

        #[tokio::test]
        async fn a_read_script_on_a_mongodb_connection_runs_as_a_read_with_required() {
            let driver = FakeDriver::new(DbKind::MongoDB);

            let attempts = govern_and_run(&driver, ExecutionClassification::Read).await;

            assert_eq!(
                attempts,
                vec![(ExecutionClassification::Read, ReadOnlyEnforcement::Required)]
            );
            assert_eq!(received_flags(&driver), vec![ReadOnlyEnforcement::Required]);
            assert_eq!(
                driver.stats().executed_requests[0].confirmed_ceiling,
                Some(ExecutionClassification::Read)
            );
        }

        #[tokio::test]
        async fn a_read_script_on_a_redis_connection_runs_as_a_read_with_required() {
            let driver = FakeDriver::new(DbKind::Redis);

            let attempts = govern_and_run(&driver, ExecutionClassification::Read).await;

            assert_eq!(
                attempts,
                vec![(ExecutionClassification::Read, ReadOnlyEnforcement::Required)]
            );
            assert_eq!(received_flags(&driver), vec![ReadOnlyEnforcement::Required]);
        }

        #[tokio::test]
        async fn a_read_script_on_a_non_enforcing_driver_is_governed_as_a_write() {
            let driver = FakeDriver::new(DbKind::Postgres);

            let attempts = govern_and_run(&driver, ExecutionClassification::Read).await;

            assert_eq!(
                attempts,
                vec![(ExecutionClassification::Write, ReadOnlyEnforcement::None)]
            );
            assert_eq!(received_flags(&driver), vec![ReadOnlyEnforcement::None]);
        }

        #[tokio::test]
        async fn a_read_script_refused_at_execution_is_governed_again_as_a_write() {
            let driver = FakeDriver::new(DbKind::Postgres).with_read_only_refusal();

            let attempts = govern_and_run(&driver, ExecutionClassification::Metadata).await;

            assert_eq!(
                attempts,
                vec![
                    (
                        ExecutionClassification::Metadata,
                        ReadOnlyEnforcement::Required
                    ),
                    (ExecutionClassification::Write, ReadOnlyEnforcement::None),
                ]
            );
            assert_eq!(
                received_flags(&driver),
                vec![ReadOnlyEnforcement::Required, ReadOnlyEnforcement::None]
            );
        }

        #[tokio::test]
        async fn a_script_stopped_mid_run_is_reported_as_an_error() {
            let mut stopped = dbflux_core::QueryResult::empty();
            stopped.metadata_extra = Some(std::collections::HashMap::from([(
                "script_failure".to_string(),
                serde_json::json!({
                    "index": 1,
                    "message": ".aggregate() is classified Write, which exceeds the confirmed Read ceiling for this run",
                }),
            )]));
            let driver = FakeDriver::new(DbKind::MongoDB).with_default_result(stopped);
            let connection = connect(&driver);

            let outcome = run_script_query(
                connection,
                "db.a.find({}); db.a.aggregate([{$merge: 'x'}]);",
                ExecutionClassification::Read,
                ReadOnlyEnforcement::Required,
            )
            .await;

            assert!(
                matches!(outcome, Err(ref message) if message.contains("exceeds the confirmed Read ceiling")),
                "{outcome:?}"
            );
        }

        #[tokio::test]
        async fn other_failures_are_reported_without_a_second_attempt() {
            let driver = FakeDriver::new(DbKind::Postgres)
                .with_read_only_enforcement()
                .with_default_error("boom");
            let connection = connect(&driver);

            let outcome = run_script_query(
                connection,
                "SELECT 1",
                ExecutionClassification::Read,
                ReadOnlyEnforcement::Required,
            )
            .await;

            assert!(
                matches!(outcome, Err(ref message) if message.contains("boom")),
                "{outcome:?}"
            );
        }
    }
}
