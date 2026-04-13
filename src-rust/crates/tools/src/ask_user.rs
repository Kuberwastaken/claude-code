// AskUserQuestion tool: ask the human operator a question and wait for a response.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::debug;

pub struct AskUserQuestionTool;

#[derive(Debug, Deserialize)]
struct AskUserInput {
    question: String,
    #[serde(default)]
    options: Option<Vec<String>>,
}

#[async_trait]
impl Tool for AskUserQuestionTool {
    fn name(&self) -> &str {
        claurst_core::constants::TOOL_NAME_ASK_USER
    }

    fn description(&self) -> &str {
        "Ask the user a question and wait for their response. Use this when you \
         need clarification, confirmation, or additional information from the user. \
         The question will be displayed and the user can type their answer."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of choices for multiple-choice questions"
                }
            },
            "required": ["question"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: AskUserInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        debug!(question = %params.question, "Asking user");

        // In non-interactive mode we cannot ask the user.
        if ctx.non_interactive {
            return ToolResult::error(
                "Cannot ask user questions in non-interactive mode".to_string(),
            );
        }

        // The actual prompt/response is handled at the TUI layer, which will
        // intercept this tool result and display the question.  We return a
        // placeholder that the query loop replaces.
        let meta = json!({
            "question": params.question,
            "options": params.options,
            "type": "ask_user",
        });

        ToolResult::success(format!("Question: {}", params.question)).with_metadata(meta)
    }
}
