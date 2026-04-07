// cc-mcp: Official MCP server registry.
//
// Mirrors the TS officialRegistry.ts, but instead of fetching the live
// Anthropic registry at runtime (which requires network access and an API key),
// we maintain a static list of well-known MCP servers.  The live-registry URL
// check from TS is replicated by `is_official_mcp_url`, which performs a
// lazy HTTP fetch and caches the result.

use once_cell::sync::OnceCell;
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Static registry
// ---------------------------------------------------------------------------

/// A description of one official MCP server.
pub struct OfficialMcpServer {
    pub name: &'static str,
    pub description: &'static str,
    pub homepage: &'static str,
    /// Optional npx / uvx / docker install command.
    pub install_command: Option<&'static str>,
    pub categories: &'static [&'static str],
}

/// All officially-supported MCP servers known at compile time.
pub const OFFICIAL_SERVERS: &[OfficialMcpServer] = &[
    OfficialMcpServer {
        name: "filesystem",
        description: "Provides read/write access to the local filesystem.",
        homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/filesystem",
        install_command: Some("npx -y @modelcontextprotocol/server-filesystem"),
        categories: &["files", "local"],
    },
    OfficialMcpServer {
        name: "github",
        description: "Interact with GitHub repositories, issues, pull requests, and code search.",
        homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/github",
        install_command: Some("npx -y @modelcontextprotocol/server-github"),
        categories: &["vcs", "remote"],
    },
    OfficialMcpServer {
        name: "gitlab",
        description: "Interact with GitLab repositories and merge requests.",
        homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/gitlab",
        install_command: Some("npx -y @modelcontextprotocol/server-gitlab"),
        categories: &["vcs", "remote"],
    },
    OfficialMcpServer {
        name: "google-drive",
        description: "Read and search files stored in Google Drive.",
        homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/gdrive",
        install_command: Some("npx -y @modelcontextprotocol/server-gdrive"),
        categories: &["files", "remote"],
    },
    OfficialMcpServer {
        name: "google-maps",
        description: "Geocoding, directions, and Places API via Google Maps.",
        homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/google-maps",
        install_command: Some("npx -y @modelcontextprotocol/server-google-maps"),
        categories: &["maps", "remote"],
    },
    OfficialMcpServer {
        name: "postgres",
        description: "Run read-only SQL queries against a PostgreSQL database.",
        homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/postgres",
        install_command: Some("npx -y @modelcontextprotocol/server-postgres"),
        categories: &["database", "local"],
    },
    OfficialMcpServer {
        name: "sqlite",
        description: "Interact with a SQLite database file.",
        homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/sqlite",
        install_command: Some("npx -y @modelcontextprotocol/server-sqlite"),
        categories: &["database", "local"],
    },
    OfficialMcpServer {
        name: "slack",
        description: "Post messages, list channels, and search Slack workspaces.",
        homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/slack",
        install_command: Some("npx -y @modelcontextprotocol/server-slack"),
        categories: &["communication", "remote"],
    },
    OfficialMcpServer {
        name: "memory",
        description: "Persistent key-value memory store across conversations.",
        homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/memory",
        install_command: Some("npx -y @modelcontextprotocol/server-memory"),
        categories: &["memory", "local"],
    },
    OfficialMcpServer {
        name: "sequential-thinking",
        description: "Structured chain-of-thought reasoning tool.",
        homepage:
            "https://github.com/modelcontextprotocol/servers/tree/main/src/sequentialthinking",
        install_command: Some("npx -y @modelcontextprotocol/server-sequential-thinking"),
        categories: &["reasoning"],
    },
    OfficialMcpServer {
        name: "brave-search",
        description: "Web and local search via the Brave Search API.",
        homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/brave-search",
        install_command: Some("npx -y @modelcontextprotocol/server-brave-search"),
        categories: &["search", "remote"],
    },
    OfficialMcpServer {
        name: "fetch",
        description: "Fetch content from URLs (web pages, APIs, RSS feeds).",
        homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/fetch",
        install_command: Some("uvx mcp-server-fetch"),
        categories: &["http", "remote"],
    },
    OfficialMcpServer {
        name: "puppeteer",
        description: "Browser automation and web scraping via Puppeteer.",
        homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/puppeteer",
        install_command: Some("npx -y @modelcontextprotocol/server-puppeteer"),
        categories: &["browser", "automation"],
    },
    OfficialMcpServer {
        name: "aws-kb-retrieval",
        description: "Retrieve knowledge from AWS Bedrock Knowledge Bases.",
        homepage:
            "https://github.com/modelcontextprotocol/servers/tree/main/src/aws-kb-retrieval-server",
        install_command: Some("npx -y @modelcontextprotocol/server-aws-kb-retrieval"),
        categories: &["aws", "remote"],
    },
    OfficialMcpServer {
        name: "everything",
        description: "Reference / test server exposing all MCP capabilities.",
        homepage: "https://github.com/modelcontextprotocol/servers/tree/main/src/everything",
        install_command: Some("npx -y @modelcontextprotocol/server-everything"),
        categories: &["testing"],
    },
];

// ---------------------------------------------------------------------------
// Search helpers
// ---------------------------------------------------------------------------

/// Return all servers whose name or any category contains `query` (case-insensitive).
pub fn search_registry(query: &str) -> Vec<&'static OfficialMcpServer> {
    let q = query.to_lowercase();
    OFFICIAL_SERVERS
        .iter()
        .filter(|s| {
            s.name.to_lowercase().contains(&q)
                || s.description.to_lowercase().contains(&q)
                || s.categories.iter().any(|c| c.to_lowercase().contains(&q))
        })
        .collect()
}

/// Return the server with an exact name match, if any.
pub fn find_server(name: &str) -> Option<&'static OfficialMcpServer> {
    OFFICIAL_SERVERS.iter().find(|s| s.name == name)
}

// ---------------------------------------------------------------------------
// Live-registry URL check (mirrors TS isOfficialMcpUrl / prefetchOfficialMcpUrls)
// ---------------------------------------------------------------------------

/// Cached set of normalized URLs fetched from the Anthropic MCP registry.
/// `None` means the fetch has not been attempted yet or was disabled.
static OFFICIAL_URLS: OnceCell<HashSet<String>> = OnceCell::new();

/// Normalize a URL: strip query-string and trailing slash.
fn normalize_url(url: &str) -> Option<String> {
    let mut u = url::Url::parse(url).ok()?;
    u.set_query(None);
    u.set_fragment(None);
    let s = u.to_string();
    Some(s.trim_end_matches('/').to_string())
}

/// Fire-and-forget fetch of `https://api.anthropic.com/mcp-registry/v0/servers`.
/// Populates `OFFICIAL_URLS` so that `is_official_mcp_url` works.
///
/// Skipped when `CLAURST_DISABLE_NONESSENTIAL_TRAFFIC` is set.
pub async fn prefetch_official_mcp_urls() {
    if std::env::var("CLAURST_DISABLE_NONESSENTIAL_TRAFFIC").is_ok() {
        return;
    }

    // Only fetch once.
    if OFFICIAL_URLS.get().is_some() {
        return;
    }

    let result: anyhow::Result<HashSet<String>> = async {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let resp: serde_json::Value = client
            .get("https://api.anthropic.com/mcp-registry/v0/servers?version=latest&visibility=commercial")
            .send()
            .await?
            .json()
            .await?;

        let mut urls = HashSet::new();
        if let Some(servers) = resp.get("servers").and_then(|s| s.as_array()) {
            for entry in servers {
                if let Some(remotes) = entry
                    .get("server")
                    .and_then(|s| s.get("remotes"))
                    .and_then(|r| r.as_array())
                {
                    for remote in remotes {
                        if let Some(url) = remote.get("url").and_then(|u| u.as_str()) {
                            if let Some(normalized) = normalize_url(url) {
                                urls.insert(normalized);
                            }
                        }
                    }
                }
            }
        }
        Ok(urls)
    }
    .await;

    match result {
        Ok(urls) => {
            let count = urls.len();
            let _ = OFFICIAL_URLS.set(urls);
            tracing::debug!(count, "[mcp-registry] Loaded official MCP URLs");
        }
        Err(e) => {
            tracing::debug!(error = %e, "[mcp-registry] Failed to fetch MCP registry");
        }
    }
}

/// Returns `true` iff `normalized_url` appears in the official registry.
/// Returns `false` when the registry has not been fetched yet (fail-closed).
pub fn is_official_mcp_url(normalized_url: &str) -> bool {
    OFFICIAL_URLS
        .get()
        .map(|set| set.contains(normalized_url))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_server() {
        let s = find_server("filesystem").unwrap();
        assert_eq!(s.name, "filesystem");
        assert!(s.install_command.is_some());
    }

    #[test]
    fn test_search_registry_by_category() {
        let results = search_registry("database");
        let names: Vec<_> = results.iter().map(|s| s.name).collect();
        assert!(names.contains(&"postgres"));
        assert!(names.contains(&"sqlite"));
    }

    #[test]
    fn test_search_registry_by_name() {
        let results = search_registry("github");
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "github");
    }

    #[test]
    fn test_normalize_url() {
        let n = normalize_url("https://example.com/path?q=1#frag").unwrap();
        assert_eq!(n, "https://example.com/path");
    }
}
