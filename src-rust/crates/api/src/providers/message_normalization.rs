use claurst_core::types::{ContentBlock, Message, MessageContent};

pub(crate) fn remove_empty_messages(messages: &[Message]) -> Vec<Message> {
    messages.iter().filter_map(remove_empty_message).collect()
}

pub(crate) fn normalize_anthropic_messages(messages: &[Message]) -> Vec<Message> {
    scrub_tool_ids(&remove_empty_messages(messages), scrub_anthropic_tool_id)
}

pub(crate) fn scrub_tool_ids<F>(messages: &[Message], scrub: F) -> Vec<Message>
where
    F: Fn(&str) -> String + Copy,
{
    messages
        .iter()
        .cloned()
        .map(|mut message| {
            if let MessageContent::Blocks(blocks) = &mut message.content {
                for block in blocks.iter_mut() {
                    match block {
                        ContentBlock::ToolUse { id, .. } => {
                            *id = scrub(id);
                        }
                        ContentBlock::ToolResult { tool_use_id, .. } => {
                            *tool_use_id = scrub(tool_use_id);
                        }
                        _ => {}
                    }
                }
            }
            message
        })
        .collect()
}

pub(crate) fn scrub_anthropic_tool_id(id: &str) -> String {
    id.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn remove_empty_message(message: &Message) -> Option<Message> {
    match &message.content {
        MessageContent::Text(text) if text.is_empty() => None,
        MessageContent::Text(_) => Some(message.clone()),
        MessageContent::Blocks(blocks) => {
            let filtered: Vec<ContentBlock> =
                blocks.iter().filter_map(remove_empty_block).collect();
            if filtered.is_empty() {
                None
            } else {
                let mut cloned = message.clone();
                cloned.content = MessageContent::Blocks(filtered);
                Some(cloned)
            }
        }
    }
}

fn remove_empty_block(block: &ContentBlock) -> Option<ContentBlock> {
    match block {
        ContentBlock::Text { text } if text.is_empty() => None,
        ContentBlock::Thinking { thinking, .. } if thinking.is_empty() => None,
        _ => Some(block.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claurst_core::types::{Message, Role, ToolResultContent};
    use serde_json::json;

    #[test]
    fn remove_empty_messages_filters_empty_text_and_thinking() {
        let messages = vec![
            Message::user(""),
            Message::assistant_blocks(vec![
                ContentBlock::Text {
                    text: String::new(),
                },
                ContentBlock::Thinking {
                    thinking: String::new(),
                    signature: "sig".to_string(),
                },
            ]),
            Message::user_blocks(vec![
                ContentBlock::Text {
                    text: "kept".to_string(),
                },
                ContentBlock::Thinking {
                    thinking: String::new(),
                    signature: "sig".to_string(),
                },
            ]),
        ];

        let normalized = remove_empty_messages(&messages);
        assert_eq!(normalized.len(), 1);
        assert!(matches!(&normalized[0].role, Role::User));
        let MessageContent::Blocks(blocks) = &normalized[0].content else {
            panic!("expected block message");
        };
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "kept"));
    }

    #[test]
    fn normalize_anthropic_messages_scrubs_tool_ids() {
        let messages = vec![
            Message::assistant_blocks(vec![ContentBlock::ToolUse {
                id: "call:1/abc".to_string(),
                name: "search".to_string(),
                input: json!({"q": "test"}),
            }]),
            Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "call:1/abc".to_string(),
                content: ToolResultContent::Text("done".to_string()),
                is_error: Some(false),
            }]),
        ];

        let normalized = normalize_anthropic_messages(&messages);
        let MessageContent::Blocks(assistant_blocks) = &normalized[0].content else {
            panic!("expected assistant blocks");
        };
        let MessageContent::Blocks(user_blocks) = &normalized[1].content else {
            panic!("expected user blocks");
        };

        assert!(matches!(
            &assistant_blocks[0],
            ContentBlock::ToolUse { id, .. } if id == "call_1_abc"
        ));
        assert!(matches!(
            &user_blocks[0],
            ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_1_abc"
        ));
    }
}
