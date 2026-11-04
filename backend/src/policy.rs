use crate::{
    codex::{Codex, RpcError},
    events::Events,
};
use serde_json::Value;
// Deliberate API surface: no command shell, credential export, arbitrary config writes,
// memory reset or destructive filesystem RPC. Expand with UI and validation together.
pub const METHODS: &[&str] = &[
    "project/list",
    "project/read",
    "project/create",
    "project/update",
    "project/move",
    "thread/list",
    "thread/read",
    "thread/turns/list",
    "thread/items/list",
    "thread/loaded/list",
    "thread/start",
    "thread/resume",
    "thread/fork",
    "thread/archive",
    "thread/unarchive",
    "thread/name/set",
    "thread/metadata/update",
    "thread/settings/update",
    "thread/compact/start",
    "thread/goal/get",
    "thread/goal/set",
    "thread/goal/clear",
    "thread/queue/add",
    "thread/queue/list",
    "thread/queue/update",
    "thread/queue/delete",
    "thread/queue/reorder",
    "thread/queue/start",
    "thread/attachment/add",
    "thread/attachment/list",
    "thread/attachment/remove",
    "thread/backgroundTerminals/list",
    "thread/backgroundTerminals/terminate",
    "turn/start",
    "turn/steer",
    "turn/interrupt",
    "turn/settings/update",
    "review/start",
    "model/list",
    "permissionProfile/list",
    "skills/list",
    "plugin/list",
    "plugin/installed",
    "plugin/read",
    "mcpServerStatus/list",
    "account/read",
    "account/rateLimits/read",
    "account/usage/read",
    "collaborationMode/list",
];
pub fn read_only(method: &str) -> bool {
    matches!(
        method,
        "thread/read"
            | "project/read"
            | "thread/goal/get"
            | "plugin/read"
            | "account/read"
            | "account/rateLimits/read"
            | "account/usage/read"
            | "plugin/installed"
    ) || method.ends_with("/list")
}
pub fn validate(
    method: &str,
    params: &Value,
    codex: &Codex,
    events: &Events,
) -> Result<(), RpcError> {
    if !METHODS.contains(&method) || !codex.runtime.methods.contains(method) {
        return Err(RpcError::new(
            -32601,
            "This runtime does not expose this method through AnywhereHost",
        ));
    }
    if !params.is_object() {
        return Err(RpcError::new(-32602, "params must be a JSON object"));
    }
    if matches!(method, "thread/resume" | "thread/fork")
        && (params.get("path").is_some() || params.get("history").is_some())
    {
        return Err(RpcError::new(
            -32602,
            "Resume/fork must use the original threadId, without substituted history/path",
        ));
    }
    // Never let a missing, null, or differently typed identity skip the ownership
    // gate and leave target selection to a runtime default or alternate field.
    if !read_only(method)
        && method != "thread/start"
        && (method.starts_with("thread/")
            || method.starts_with("turn/")
            || method == "review/start")
        && !params["threadId"]
            .as_str()
            .is_some_and(|id| !id.is_empty() && id.len() <= 256)
    {
        return Err(RpcError::new(
            -32602,
            "Mutation requires an explicit threadId",
        ));
    }
    if !codex.runtime.shares_live_process
        && !read_only(method)
        && method != "thread/fork"
        && let Some(id) = params["threadId"].as_str()
        && !events.owns(id)
    {
        return Err(RpcError::new(
            -32009,
            "Existing desktop thread is read-only in this runtime. Attach to its shared control socket or fork it to continue safely.",
        ));
    }
    // Thread creation must name a real workspace; no implicit cwd inherited from a service.
    if method == "thread/start" {
        let cwd = params["cwd"]
            .as_str()
            .ok_or_else(|| RpcError::new(-32602, "thread/start requires an absolute cwd"))?;
        if !std::path::Path::new(cwd).is_absolute() || !std::path::Path::new(cwd).is_dir() {
            return Err(RpcError::new(
                -32602,
                "cwd must be an existing absolute directory",
            ));
        }
    }
    Ok(())
}
