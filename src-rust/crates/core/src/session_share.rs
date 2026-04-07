// session_share.rs — Session sharing and local export utilities.
//
// Provides `share_session` for uploading a session to a configurable
// share endpoint, and `export_session_text` as a plain-text fallback
// when no endpoint is configured.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareRequest {
    pub session_id: String,
    pub title: Option<String>,
    pub messages: Vec<serde_json::Value>,
    pub created_at: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareResponse {
    pub share_id: String,
    pub url: String,
}

/// Upload a session to the share service. Returns the share URL on success.
pub async fn share_session(
    messages: &[crate::types::Message],
    session_id: &str,
    title: Option<&str>,
    model: &str,
    share_endpoint: &str,
) -> Result<String, String> {
    let req = ShareRequest {
        session_id: session_id.to_string(),
        title: title.map(|t| t.to_string()),
        messages: messages
            .iter()
            .map(|m| serde_json::to_value(m).unwrap_or(serde_json::Value::Null))
            .collect(),
        created_at: chrono::Utc::now().to_rfc3339(),
        model: model.to_string(),
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .post(share_endpoint)
        .json(&req)
        .send()
        .await
        .map_err(|e| format!("Failed to connect to share service: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Share service returned {}", resp.status()));
    }

    let share: ShareResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse share response: {}", e))?;

    Ok(share.url)
}

/// Generate a simple local Markdown export of the session (fallback when
/// no share endpoint is configured).
pub fn export_session_text(messages: &[crate::types::Message], title: Option<&str>) -> String {
    let mut out = String::new();

    out.push_str("# Claurst Conversation Export\n\n");
    if let Some(t) = title {
        out.push_str(&format!("**{}**\n\n", t));
    }
    out.push_str(&format!(
        "*Exported at {}*\n\n",
        chrono::Utc::now().format("%Y-%m-%d %H:%M UTC")
    ));
    out.push_str("---\n\n");

    for msg in messages {
        let role = match msg.role {
            crate::types::Role::User => "**User**",
            crate::types::Role::Assistant => "**Assistant**",
        };
        out.push_str(&format!("{}\n\n", role));

        use crate::types::MessageContent;
        match &msg.content {
            MessageContent::Text(t) => out.push_str(&format!("{}\n\n", t)),
            MessageContent::Blocks(blocks) => {
                for block in blocks {
                    if let crate::types::ContentBlock::Text { text } = block {
                        out.push_str(&format!("{}\n\n", text));
                    }
                }
            }
        }
        out.push_str("---\n\n");
    }

    out
}
