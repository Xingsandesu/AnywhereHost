use crate::codex::RpcError;
use serde_json::{Value, json};

/// Codex keeps a newly started thread in memory before its first user message.
/// That precise lifecycle state means an empty first page, not a failed read.
pub fn normalize(
    method: &str,
    params: &Value,
    result: Result<Value, RpcError>,
) -> Result<Value, RpcError> {
    if method == "thread/turns/list"
        && params["cursor"].is_null()
        && let Some(id) = params["threadId"].as_str()
        && let Err(error) = &result
        && error.code == -32600
        && error.message
            == format!(
                "thread {id} is not materialized yet; thread/turns/list is unavailable before first user message"
            )
    {
        return Ok(json!({
            "data": [], "nextCursor": null, "backwardsCursor": null,
            "historyState": "unmaterialized"
        }));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn empty_error() -> RpcError {
        RpcError::new(
            -32600,
            "thread t is not materialized yet; thread/turns/list is unavailable before first user message",
        )
    }
    #[test]
    fn first_message_pending_has_an_empty_page() {
        let page = normalize(
            "thread/turns/list",
            &json!({"threadId":"t"}),
            Err(empty_error()),
        )
        .unwrap();
        assert_eq!(page["data"], json!([]));
        assert!(page["nextCursor"].is_null());
        assert_eq!(page["historyState"], "unmaterialized");
    }
    #[test]
    fn unrelated_errors_and_successful_pages_are_preserved() {
        for (method, params, error) in [
            ("thread/read", json!({"threadId":"t"}), empty_error()),
            (
                "thread/turns/list",
                json!({"threadId":"other"}),
                empty_error(),
            ),
            (
                "thread/turns/list",
                json!({"threadId":"t","cursor":"older"}),
                empty_error(),
            ),
            (
                "thread/turns/list",
                json!({"threadId":"t"}),
                RpcError::new(-32600, "thread not found"),
            ),
            (
                "thread/turns/list",
                json!({"threadId":"t"}),
                RpcError::new(-32002, empty_error().message),
            ),
        ] {
            let actual = normalize(method, &params, Err(error.clone())).unwrap_err();
            assert_eq!(actual.code, error.code);
            assert_eq!(actual.message, error.message);
        }
        let page = json!({"data":[{"id":"turn"}],"nextCursor":"older"});
        assert_eq!(
            normalize(
                "thread/turns/list",
                &json!({"threadId":"t"}),
                Ok(page.clone())
            )
            .unwrap(),
            page
        );
    }
}
