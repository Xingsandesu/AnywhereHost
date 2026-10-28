use crate::codex::RpcError;
use serde_json::{Value, json};

/// The native launcher verifies its own non-inherited App Sandbox entitlement.
/// Require that parent contract AND the helper's signed entitlement, so running
/// an inherited helper directly from a terminal does not select externalSandbox.
#[cfg(target_os = "macos")]
pub fn enabled() -> bool {
    use std::{ffi::c_void, ptr};
    type Ref = *const c_void;
    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecTaskCreateFromSelf(allocator: Ref) -> Ref;
        fn SecTaskCopyValueForEntitlement(task: Ref, key: Ref, error: *mut Ref) -> Ref;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(allocator: Ref, bytes: *const i8, encoding: u32) -> Ref;
        fn CFBooleanGetValue(value: Ref) -> bool;
        fn CFGetTypeID(value: Ref) -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFRelease(value: Ref);
    }
    if std::env::var("ANYWHERE_HOST_PARENT_SANDBOXED").as_deref() != Ok("1") {
        return false;
    }
    // Every successful Create/Copy is balanced, and the value's type is checked.
    unsafe {
        let task = SecTaskCreateFromSelf(ptr::null());
        if task.is_null() {
            return false;
        }
        let key = CFStringCreateWithCString(
            ptr::null(),
            c"com.apple.security.app-sandbox".as_ptr(),
            0x08000100,
        );
        if key.is_null() {
            CFRelease(task);
            return false;
        }
        let value = SecTaskCopyValueForEntitlement(task, key, ptr::null_mut());
        let enabled = !value.is_null()
            && CFGetTypeID(value) == CFBooleanGetTypeID()
            && CFBooleanGetValue(value);
        if !value.is_null() {
            CFRelease(value);
        }
        CFRelease(key);
        CFRelease(task);
        enabled
    }
}
#[cfg(not(target_os = "macos"))]
pub fn enabled() -> bool {
    false
}

pub fn policy() -> Value {
    json!({"type":"externalSandbox", "networkAccess":"enabled"})
}

/// Explicit requests for different guarantees must fail, not be silently weakened.
pub fn prepare(method: &str, params: &mut Value) -> Result<(), RpcError> {
    let config_overrides = params
        .get("config")
        .and_then(Value::as_object)
        .is_some_and(|config| {
            config
                .keys()
                .any(|key| key.starts_with("sandbox") || key.starts_with("permissions"))
        });
    if params.get("sandbox").is_some_and(|v| !v.is_null())
        || params.get("permissions").is_some_and(|v| !v.is_null())
        || config_overrides
        || params
            .get("sandboxPolicy")
            .is_some_and(|v| !v.is_null() && v != &policy())
    {
        return Err(RpcError::new(
            -32602,
            "This Host executes inside macOS App Sandbox with network access and user-authorized folders. Nested Codex sandbox modes/permission profiles are unsupported; omit the override or request externalSandbox with networkAccess enabled.",
        ));
    }
    if matches!(method, "turn/start" | "thread/settings/update") {
        params["sandboxPolicy"] = policy();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_different_security_guarantees() {
        for mut params in [
            json!({"sandbox":"read-only"}),
            json!({"sandbox":"workspace-write"}),
            json!({"sandbox":"danger-full-access"}),
            json!({"permissions":"restricted"}),
            json!({"sandboxPolicy":{"type":"externalSandbox","networkAccess":"restricted"}}),
            json!({"sandboxPolicy":{"type":"workspaceWrite"}}),
            json!({"config":{"sandbox_mode":"read-only"}}),
        ] {
            assert!(prepare("turn/start", &mut params).is_err());
        }
    }
    #[test]
    fn preserves_approvals_and_uses_os_sandbox() {
        let mut params = json!({"approvalPolicy":"on-request","threadId":"owned"});
        prepare("turn/start", &mut params).unwrap();
        assert_eq!(params["approvalPolicy"], "on-request");
        assert_eq!(params["sandboxPolicy"], policy());
        prepare("turn/start", &mut params).unwrap();
    }
}
