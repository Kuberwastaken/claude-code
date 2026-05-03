// WebSearch tool: search the web using SearXNG (preferred), Brave Search API,
// or DuckDuckGo Instant Answers as a fallback.
//
// Priority order:
// 1. SearXNG (self-hosted meta-search, aggregates Google/Bing/DDG/Brave/etc.)
// 2. Brave Search API (requires BRAVE_SEARCH_API_KEY)
// 3. DuckDuckGo Instant Answer API (limited, no real search results)

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{debug, warn};

const DEFAULT_SEARXNG_URL: &str = "http://localhost:8888";

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
                    "description": "Number of results to return (default: 5, max: 20)"
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

        let num_results = params.num_results.min(20).max(1);
        debug!(query = %params.query, num_results, "Web search");

        // 1. Try SearXNG (self-hosted meta-search)
        let searxng_url = std::env::var("SEARXNG_URL")
            .unwrap_or_else(|_| DEFAULT_SEARXNG_URL.to_string());

        match search_searxng(&params.query, num_results, &searxng_url).await {
            Ok(result) => return result,
            Err(e) => {
                debug!(error = %e, "SearXNG unavailable, trying next backend");
            }
        }

        // 2. Try Brave Search API
        if let Some(api_key) = std::env::var("BRAVE_SEARCH_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
        {
            return search_brave(&params.query, num_results, &api_key).await;
        }

        // 3. Fall back to DuckDuckGo Instant Answers
        warn!("No search backend available (SearXNG down, no Brave API key) — using DuckDuckGo fallback");
        search_duckduckgo(&params.query, num_results).await
    }
}

// ---------------------------------------------------------------------------
// SearXNG — self-hosted meta-search engine
// ---------------------------------------------------------------------------

async fn search_searxng(
    query: &str,
    num_results: usize,
    base_url: &str,
) -> Result<ToolResult, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let url = format!(
        "{}/search?q={}&format=json&categories=general&pageno=1",
        base_url.trim_end_matches('/'),
        urlencoding_simple(query),
    );

    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("SearXNG request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("SearXNG returned status {}", resp.status()));
    }

    let data: Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse SearXNG response: {}", e))?;

    Ok(format_searxng_results(&data, num_results))
}

fn format_searxng_results(data: &Value, max: usize) -> ToolResult {
    let results = match data.get("results").and_then(|r| r.as_array()) {
        Some(r) => r,
        None => return ToolResult::success("No results found.".to_string()),
    };

    if results.is_empty() {
        return ToolResult::success("No results found.".to_string());
    }

    let mut output = String::new();
    let total = data
        .get("number_of_results")
        .and_then(|n| n.as_u64())
        .unwrap_or(0);

    if total > 0 {
        output.push_str(&format!("About {} results found.\n\n", total));
    }

    for (i, item) in results.iter().take(max).enumerate() {
        let title = item
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("(No title)");
        let url = item.get("url").and_then(|u| u.as_str()).unwrap_or("");
        let content = item
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("");
        let engines: Vec<&str> = item
            .get("engines")
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect()
            })
            .unwrap_or_default();
        let score = item
            .get("score")
            .and_then(|s| s.as_f64())
            .unwrap_or(0.0);

        output.push_str(&format!(
            "{}. **{}**\n   URL: {}\n   {}\n   [engines: {} | score: {:.1}]\n\n",
            i + 1,
            title,
            url,
            content,
            engines.join(", "),
            score
        ));
    }

    ToolResult::success(output)
}

// ---------------------------------------------------------------------------
// Brave Search API
// ---------------------------------------------------------------------------

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
            let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("(No title)");
            let url = item.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let snippet = item.get("description").and_then(|s| s.as_str()).unwrap_or("");

            output.push_str(&format!("{}. **{}**\n   URL: {}\n   {}\n\n", i + 1, title, url, snippet));
        }
    }

    if output.is_empty() {
        "No results found.".to_string()
    } else {
        output
    }
}

// ---------------------------------------------------------------------------
// DuckDuckGo Instant Answer API (limited fallback)
// ---------------------------------------------------------------------------

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
            let source = data.get("AbstractSource").and_then(|s| s.as_str()).unwrap_or("");
            let url = data.get("AbstractURL").and_then(|u| u.as_str()).unwrap_or("");
            output.push_str(&format!("**{}**\n{}\nURL: {}\n\n", source, abstract_text, url));
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
            "No instant answer found for '{}'. Try running SearXNG \
             (set SEARXNG_URL env var) for full web search, or set \
             BRAVE_SEARCH_API_KEY for Brave Search.",
            data.get("QuerySearchQuery")
                .and_then(|q| q.as_str())
                .unwrap_or("your query")
        )
    } else {
        output
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
