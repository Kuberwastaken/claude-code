// WebSearch tool: search the web using Brave Search API or fallback to DuckDuckGo.
//
// Mirrors the TypeScript WebSearch tool behaviour:
// - Accepts a query string
// - Returns a list of results with title, url, and snippet
// - Falls back to DuckDuckGo if no search API key is configured

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::debug;

pub struct WebSearchTool;

#[derive(Debug, Deserialize)]
struct WebSearchInput {
    query: String,
    #[serde(default = "default_num_results")]
    num_results: usize,
}

fn default_num_results() -> usize {
    5
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        claurst_core::constants::TOOL_NAME_WEB_SEARCH
    }

    fn description(&self) -> &str {
        "Search the web for information. Returns a list of relevant web pages with \
         titles, URLs, and snippets. Use this when you need current information \
         not available in your training data, or when searching for documentation, \
         examples, or news."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "num_results": {
                    "type": "number",
                    "description": "Number of results to return (default: 5, max: 10)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let params: WebSearchInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        let num_results = params.num_results.min(10).max(1);
        debug!(query = %params.query, num_results, "Web search");

        // Try Brave Search API first, then fall back to DuckDuckGo
        if let Some(api_key) = std::env::var("BRAVE_SEARCH_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
        {
            search_brave(&params.query, num_results, &api_key).await
        } else {
            search_duckduckgo(&params.query, num_results).await
        }
    }
}

/// Search using the Brave Search API.
async fn search_brave(query: &str, num_results: usize, api_key: &str) -> ToolResult {
    let client = reqwest::Client::new();
    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
        urlencoding_simple(query),
        num_results
    );

    let resp = match client
        .get(&url)
        .header("Accept", "application/json")
        .header("Accept-Encoding", "gzip")
        .header("X-Subscription-Token", api_key)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return ToolResult::error(format!("Search request failed: {}", e)),
    };

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        return ToolResult::error(format!("Brave Search API returned status {}", status));
    }

    let data: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return ToolResult::error(format!("Failed to parse response: {}", e)),
    };

    let results = format_brave_results(&data, num_results);
    ToolResult::success(results)
}

fn format_brave_results(data: &Value, max: usize) -> String {
    let mut output = String::new();
    let web_results = data
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(|r| r.as_array());

    if let Some(items) = web_results {
        for (i, item) in items.iter().take(max).enumerate() {
            let title = item
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("(No title)");
            let url = item.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let snippet = item
                .get("description")
                .and_then(|s| s.as_str())
                .unwrap_or("");

            output.push_str(&format!(
                "{}. **{}**\n   URL: {}\n   {}\n\n",
                i + 1,
                title,
                url,
                snippet
            ));
        }
    }

    if output.is_empty() {
        "No results found.".to_string()
    } else {
        output
    }
}

/// Fallback: DuckDuckGo Instant Answer API.
/// Note: this doesn't return full search results, only instant answers.
async fn search_duckduckgo(query: &str, num_results: usize) -> ToolResult {
    let client = reqwest::Client::new();
    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
        urlencoding_simple(query)
    );

    let resp = match client
        .get(&url)
        .header("User-Agent", "Claurst/1.0")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return ToolResult::error(format!("Search request failed: {}", e)),
    };

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        return ToolResult::error(format!("DuckDuckGo API returned status {}", status));
    }

    let data: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return ToolResult::error(format!("Failed to parse response: {}", e)),
    };

    let output = format_ddg_results(&data, num_results);
    ToolResult::success(output)
}

fn format_ddg_results(data: &Value, max: usize) -> String {
    let mut output = String::new();
    let mut count = 0;

    // Abstract (main answer)
    if let Some(abstract_text) = data.get("Abstract").and_then(|a| a.as_str()) {
        if !abstract_text.is_empty() {
            let source = data
                .get("AbstractSource")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            let url = data
                .get("AbstractURL")
                .and_then(|u| u.as_str())
                .unwrap_or("");
            output.push_str(&format!(
                "**{}**\n{}\nURL: {}\n\n",
                source, abstract_text, url
            ));
            count += 1;
        }
    }

    // Related topics
    if let Some(topics) = data.get("RelatedTopics").and_then(|t| t.as_array()) {
        for topic in topics.iter().take(max.saturating_sub(count)) {
            if let Some(text) = topic.get("Text").and_then(|t| t.as_str()) {
                if !text.is_empty() {
                    let url = topic.get("FirstURL").and_then(|u| u.as_str()).unwrap_or("");
                    output.push_str(&format!("- {}\n  {}\n\n", text, url));
                }
            }
        }
    }

    if output.is_empty() {
        format!(
            "No instant answer found for '{}'. Try using the Brave Search API \
             by setting the BRAVE_SEARCH_API_KEY environment variable for full web search.",
            data.get("QuerySearchQuery")
                .and_then(|q| q.as_str())
                .unwrap_or("your query")
        )
    } else {
        output
    }
}

/// Minimal percent-encoding for URL query parameters.
fn urlencoding_simple(s: &str) -> String {
    let mut encoded = String::new();
    for ch in s.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                encoded.push(ch);
            }
            ' ' => encoded.push('+'),
            _ => {
                for byte in ch.to_string().as_bytes() {
                    encoded.push_str(&format!("%{:02X}", byte));
                }
            }
        }
    }
    encoded
}
