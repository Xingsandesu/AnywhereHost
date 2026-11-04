use serde_json::Value;
use std::collections::HashMap;

/// Positions are UTF-8 bytes in the same text projection as the mobile client.
/// Only streams observed from item/started can be assigned an exact position.
/// Already-journaled events retain their positions when replayed after reconnect.
#[derive(Default)]
pub struct TextOffsets {
    items: HashMap<(String, String, String), Position>,
}

enum Position {
    Text(usize),
    Summaries(Vec<usize>),
}

impl TextOffsets {
    pub fn annotate(&mut self, payload: &mut Value) {
        let method = payload["method"].as_str().unwrap_or("").to_owned();
        let p = &mut payload["params"];
        let Some(thread) = p["threadId"].as_str().map(str::to_owned) else {
            return;
        };
        let turn = p["turnId"].as_str().unwrap_or("").to_owned();
        if method == "turn/completed" {
            let completed = p["turn"]["id"].as_str().unwrap_or("");
            self.items
                .retain(|(t, r, _), _| t != &thread || r != completed);
            return;
        }
        if method == "item/started" || method == "item/completed" {
            let item = &p["item"];
            let Some(id) = item["id"].as_str() else {
                return;
            };
            let key = (thread, turn, id.to_owned());
            if method == "item/completed" {
                self.items.remove(&key);
                return;
            }
            let length = match item["type"].as_str() {
                Some("agentMessage" | "plan") => {
                    Position::Text(item["text"].as_str().unwrap_or("").len())
                }
                Some("commandExecution") => Position::Text(
                    item["command"]
                        .as_str()
                        .map_or(0, |command| command.len() + 1)
                        + item["aggregatedOutput"].as_str().unwrap_or("").len(),
                ),
                Some("reasoning") => {
                    Position::Summaries(item["summary"].as_array().map_or_else(Vec::new, |parts| {
                        parts
                            .iter()
                            .take(128)
                            .map(|part| part.as_str().unwrap_or("").len())
                            .collect()
                    }))
                }
                _ => return,
            };
            // Bound memory if a runtime never emits completion. Eviction only
            // removes the optimization; it must never invent a zero offset.
            if self.items.len() >= 1024 {
                self.items.clear();
            }
            self.items.insert(key, length);
        } else if matches!(
            method.as_str(),
            "item/agentMessage/delta"
                | "item/plan/delta"
                | "item/commandExecution/outputDelta"
                | "item/reasoning/summaryTextDelta"
        ) {
            let (Some(id), Some(delta)) = (p["itemId"].as_str(), p["delta"].as_str()) else {
                return;
            };
            let summary = if method == "item/reasoning/summaryTextDelta" {
                let Some(index) = p["summaryIndex"].as_u64().filter(|i| *i < 128) else {
                    return;
                };
                Some(index as usize)
            } else {
                None
            };
            let offset = match (self.items.get_mut(&(thread, turn, id.to_owned())), summary) {
                (Some(Position::Text(offset)), None) => offset,
                (Some(Position::Summaries(parts)), Some(index)) => {
                    parts.resize(parts.len().max(index + 1), 0);
                    &mut parts[index]
                }
                _ => return,
            };
            let start = *offset;
            *offset += delta.len();
            p["hostTextOffset"] = Value::from(start);
            if let Some(index) = summary {
                p["hostSummaryIndex"] = Value::from(index);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn offsets_are_utf8_bytes_and_scoped_to_thread_and_turn() {
        let mut offsets = TextOffsets::default();
        offsets.annotate(&mut json!({"method":"item/started","params":{"threadId":"a","turnId":"r","item":{"id":"i","type":"agentMessage","text":"你"}}}));
        let mut delta = json!({"method":"item/agentMessage/delta","params":{"threadId":"a","turnId":"r","itemId":"i","delta":"好🙂"}});
        offsets.annotate(&mut delta);
        assert_eq!(delta["params"]["hostTextOffset"], 3);
        offsets.annotate(&mut delta);
        assert_eq!(delta["params"]["hostTextOffset"], 10);
        let mut foreign = json!({"method":"item/agentMessage/delta","params":{"threadId":"b","turnId":"r","itemId":"i","delta":"x"}});
        offsets.annotate(&mut foreign);
        assert!(foreign["params"]["hostTextOffset"].is_null());
    }

    #[test]
    fn command_prefix_matches_rendering_and_completion_evicts_state() {
        let mut offsets = TextOffsets::default();
        offsets.annotate(&mut json!({"method":"item/started","params":{"threadId":"t","turnId":"r","item":{"id":"i","type":"commandExecution","command":"pwd"}}}));
        let mut delta = json!({"method":"item/commandExecution/outputDelta","params":{"threadId":"t","turnId":"r","itemId":"i","delta":"/tmp"}});
        offsets.annotate(&mut delta);
        assert_eq!(delta["params"]["hostTextOffset"], 4);
        offsets.annotate(
            &mut json!({"method":"turn/completed","params":{"threadId":"t","turn":{"id":"r"}}}),
        );
        assert!(offsets.items.is_empty());
    }

    #[test]
    fn missing_start_never_guesses_a_position() {
        let mut offsets = TextOffsets::default();
        let mut delta = json!({"method":"item/plan/delta","params":{"threadId":"t","turnId":"r","itemId":"i","delta":"already running"}});
        offsets.annotate(&mut delta);
        assert!(delta["params"]["hostTextOffset"].is_null());
    }

    #[test]
    fn reasoning_positions_are_per_summary_part() {
        let mut offsets = TextOffsets::default();
        offsets.annotate(&mut json!({"method":"item/started","params":{"threadId":"t","turnId":"r","item":{"id":"i","type":"reasoning","summary":["你"]}}}));
        let mut delta = json!({"method":"item/reasoning/summaryTextDelta","params":{"threadId":"t","turnId":"r","itemId":"i","summaryIndex":0,"delta":"好"}});
        offsets.annotate(&mut delta);
        assert_eq!(delta["params"]["hostTextOffset"], 3);
        assert_eq!(delta["params"]["hostSummaryIndex"], 0);
        delta["params"]["summaryIndex"] = json!(1);
        offsets.annotate(&mut delta);
        assert_eq!(delta["params"]["hostTextOffset"], 0);
        assert_eq!(delta["params"]["hostSummaryIndex"], 1);
    }
}
