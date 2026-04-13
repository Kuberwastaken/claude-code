// WebFetch tool: HTTP GET with HTML-to-text conversion and LLM-powered semantic extraction
// for edge cases (JS-heavy pages, minimal content).

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use tracing::{debug, warn};

pub struct WebFetchTool;

#[derive(Debug, Deserialize)]
struct WebFetchInput {
    url: String,
    #[serde(default)]
    #[allow(dead_code)]
    prompt: Option<String>,
}

/// Compute a simple hash of the URL for cache purposes.
fn url_hash(url: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Get the cache directory for web_fetch content.
fn get_cache_dir() -> PathBuf {
    let mut dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    dir.push(".claurst");
    dir.push("web_cache");
    dir
}

/// Attempt to load cached extracted content for a URL.
fn load_cached_extraction(url: &str) -> Option<String> {
    let cache_dir = get_cache_dir();
    let cache_file = cache_dir.join(format!("{}.txt", url_hash(url)));

    if cache_file.exists() {
        match fs::read_to_string(&cache_file) {
            Ok(content) => {
                debug!(file = ?cache_file, "Loaded cached web content");
                return Some(content);
            }
            Err(e) => {
                debug!(file = ?cache_file, error = %e, "Failed to load cache");
            }
        }
    }
    None
}

/// Save extracted content to cache.
fn save_cached_extraction(url: &str, content: &str) {
    let cache_dir = get_cache_dir();
    if let Err(e) = fs::create_dir_all(&cache_dir) {
        warn!(dir = ?cache_dir, error = %e, "Failed to create cache directory");
        return;
    }

    let cache_file = cache_dir.join(format!("{}.txt", url_hash(url)));
    if let Err(e) = fs::write(&cache_file, content) {
        warn!(file = ?cache_file, error = %e, "Failed to write cache file");
    } else {
        debug!(file = ?cache_file, "Cached extracted web content");
    }
}

/// Detect if HTML is likely a JS-heavy page with minimal semantic content.
fn is_edge_case_html(html: &str, extracted_text: &str) -> bool {
    // Check word count (rough estimate)
    let word_count = extracted_text.split_whitespace().count();
    if word_count < 100 {
        debug!(word_count, "Edge case: low word count");
        return true;
    }

    // Check for semantic HTML tags
    let lower = html.to_lowercase();
    let has_semantic =
        lower.contains("<article") || lower.contains("<main") || lower.contains("<body");

    if !has_semantic {
        debug!("Edge case: no semantic HTML tags");
        return true;
    }

    false
}

/// Call Claude Haiku to extract main content from HTML.
async fn semantic_extraction(html: &str, ctx: &ToolContext) -> Option<String> {
    // Try to create an Anthropic client from the config
    let client = match claurst_api::AnthropicClient::from_config(&ctx.config) {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "Failed to create Anthropic client for semantic extraction");
            return None;
        }
    };

    // Truncate HTML to avoid exceeding token limits
    let html_excerpt = if html.len() > 20000 {
        format!("{}...", &html[..20000])
    } else {
        html.to_string()
    };

    let system = "You are a content extraction expert. Given HTML, extract and return only the main text content. Return just plain text, no markdown or formatting.";
    let user_message = format!(
        "Extract the main content from this HTML and return only the text:\n\n{}",
        html_excerpt
    );

    // Use the builder API to construct the request
    let api_messages = vec![claurst_api::ApiMessage {
        role: "user".to_string(),
        content: serde_json::Value::String(user_message),
    }];

    let request = claurst_api::CreateMessageRequest::builder("claude-haiku-4-5", 2000)
        .messages(api_messages)
        .system(claurst_api::SystemPrompt::Text(system.to_string()))
        .build();

    match client.create_message(request).await {
        Ok(response) => {
            // Extract text from the response content (Vec<Value>)
            // Response content is JSON objects like {"type": "text", "text": "..."}
            let text = response.content.iter().find_map(|block| {
                if block.get("type")?.as_str()? == "text" {
                    block.get("text")?.as_str().map(str::to_owned)
                } else {
                    None
                }
            });

            if let Some(extracted) = text {
                debug!(
                    extracted_len = extracted.len(),
                    "Semantic extraction successful"
                );
                return Some(extracted);
            }

            warn!("No text block in semantic extraction response");
            None
        }
        Err(e) => {
            warn!(error = %e, "Semantic extraction API call failed");
            None
        }
    }
}

/// Naively strip HTML tags and decode common entities.
fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;

    let lower = html.to_lowercase();
    let chars: Vec<char> = html.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if !in_tag && chars[i] == '<' {
            in_tag = true;
            // Check for script/style open/close
            let rest: String = lower_chars[i..].iter().take(20).collect();
            if rest.starts_with("<script") {
                in_script = true;
            } else if rest.starts_with("</script") {
                in_script = false;
            } else if rest.starts_with("<style") {
                in_style = true;
            } else if rest.starts_with("</style") {
                in_style = false;
            }
            // Block tags => newline
            let block_tags = [
                "<br", "<p ", "<p>", "</p>", "<div", "</div>", "<h1", "<h2", "<h3", "<h4", "<h5",
                "<h6", "</h1", "</h2", "</h3", "</h4", "</h5", "</h6", "<li", "</li", "<tr",
                "</tr", "<hr",
            ];
            for tag in &block_tags {
                if rest.starts_with(tag) {
                    result.push('\n');
                    break;
                }
            }
            i += 1;
            continue;
        }

        if in_tag {
            if chars[i] == '>' {
                in_tag = false;
            }
            i += 1;
            continue;
        }

        if in_script || in_style {
            i += 1;
            continue;
        }

        // Decode basic entities
        if chars[i] == '&' {
            let rest: String = chars[i..].iter().take(10).collect();
            if rest.starts_with("&amp;") {
                result.push('&');
                i += 5;
            } else if rest.starts_with("&lt;") {
                result.push('<');
                i += 4;
            } else if rest.starts_with("&gt;") {
                result.push('>');
                i += 4;
            } else if rest.starts_with("&quot;") {
                result.push('"');
                i += 6;
            } else if rest.starts_with("&#39;") || rest.starts_with("&apos;") {
                result.push('\'');
                i += if rest.starts_with("&#39;") { 5 } else { 6 };
            } else if rest.starts_with("&nbsp;") {
                result.push(' ');
                i += 6;
            } else {
                result.push('&');
                i += 1;
            }
            continue;
        }

        result.push(chars[i]);
        i += 1;
    }

    // Collapse multiple blank lines
    let mut collapsed = String::new();
    let mut blank_count = 0;
    for line in result.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 2 {
                collapsed.push('\n');
            }
        } else {
            blank_count = 0;
            collapsed.push_str(trimmed);
            collapsed.push('\n');
        }
    }

    collapsed.trim().to_string()
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        claurst_core::constants::TOOL_NAME_WEB_FETCH
    }

    fn description(&self) -> &str {
        "Fetches a web page URL and returns its content as text. HTML is \
         automatically converted to plain text. Use this for reading documentation, \
         APIs, and other web resources."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "prompt": {
                    "type": "string",
                    "description": "Optional prompt for how to process the content"
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: WebFetchInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        // Permission check
        if let Err(e) = ctx.check_permission(
            self.name(),
            &format!("Fetch {}", params.url),
            true, // read-only
        ) {
            return ToolResult::error(e.to_string());
        }

        debug!(url = %params.url, "Fetching web page");

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build();

        let client = match client {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Failed to create HTTP client: {}", e)),
        };

        let resp = match client
            .get(&params.url)
            .header("User-Agent", "Claude-Code/1.0")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("Failed to fetch {}: {}", params.url, e)),
        };

        let status = resp.status();
        if !status.is_success() {
            return ToolResult::error(format!("HTTP {} when fetching {}", status, params.url));
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body = match resp.text().await {
            Ok(b) => b,
            Err(e) => return ToolResult::error(format!("Failed to read response body: {}", e)),
        };

        // Try to load from cache first
        if let Some(cached) = load_cached_extraction(&params.url) {
            return ToolResult::success(cached);
        }

        // Convert HTML to text if applicable
        let mut text = if content_type.contains("html") {
            strip_html(&body)
        } else {
            body.clone()
        };

        // Detect and handle edge cases with semantic extraction
        if content_type.contains("html") && is_edge_case_html(&body, &text) {
            debug!(url = %params.url, "Attempting semantic extraction for edge case");
            if let Some(extracted) = semantic_extraction(&body, ctx).await {
                text = extracted;
            } else {
                debug!("Semantic extraction failed, using basic HTML stripping");
            }
        }

        // Truncate very long content
        const MAX_LEN: usize = 100_000;
        let text = if text.len() > MAX_LEN {
            format!(
                "{}\n\n... (truncated, {} total characters)",
                &text[..MAX_LEN],
                text.len()
            )
        } else {
            text
        };

        // Cache the final result
        save_cached_extraction(&params.url, &text);

        ToolResult::success(text)
    }
}
