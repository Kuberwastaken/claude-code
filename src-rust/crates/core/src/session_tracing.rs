//! Session Tracing — OpenTelemetry span stubs.
//!
//! Telemetry spans are no-ops: all span operations compile unchanged but
//! discard all tracing data.

use std::sync::Arc;

/// A no-op span that implements the minimal span interface.
#[derive(Debug, Clone)]
pub struct NoopSpan;

impl NoopSpan {
    /// Create a new no-op span.
    pub fn new() -> Self {
        Self
    }

    /// Set a single attribute (no-op).
    pub fn set_attribute(&self, _key: &str, _value: &str) {}

    /// Set multiple attributes (no-op).
    pub fn set_attributes(&self, _attrs: &[(&str, &str)]) {}

    /// Add an event to the span (no-op).
    pub fn add_event(&self, _name: &str) {}

    /// Record an exception (no-op).
    pub fn record_exception(&self, _error: &str) {}

    /// End the span (no-op).
    pub fn end(&self) {}
}

impl Default for NoopSpan {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Public Span API — all no-ops
// ---------------------------------------------------------------------------

/// Start an interaction span (root span for a user request).
/// Returns a no-op span; data is discarded.
pub fn start_interaction_span(_request_id: &str) -> Arc<NoopSpan> {
    Arc::new(NoopSpan::new())
}

/// End an interaction span (no-op).
pub fn end_interaction_span(_span: Arc<NoopSpan>) {}

/// Start an LLM request span (traces API calls).
/// Normally tracks TTFT, token counts, model, fast-mode status.
/// In free builds, this is a no-op.
pub fn start_llm_request_span(_model: &str, _max_tokens: u32) -> Arc<NoopSpan> {
    Arc::new(NoopSpan::new())
}

/// End an LLM request span (no-op).
pub fn end_llm_request_span(_span: Arc<NoopSpan>, _input_tokens: u32, _output_tokens: u32) {}

/// Start a tool execution span.
pub fn start_tool_span(_tool_name: &str) -> Arc<NoopSpan> {
    Arc::new(NoopSpan::new())
}

/// End a tool execution span (no-op).
pub fn end_tool_span(_span: Arc<NoopSpan>, _success: bool, _error: Option<&str>) {}

/// Start a permission dialog span.
pub fn start_permission_span(_tool_name: &str) -> Arc<NoopSpan> {
    Arc::new(NoopSpan::new())
}

/// End a permission dialog span (no-op).
pub fn end_permission_span(_span: Arc<NoopSpan>) {}

/// Start a hook execution span.
pub fn start_hook_span(_hook_name: &str) -> Arc<NoopSpan> {
    Arc::new(NoopSpan::new())
}

/// End a hook execution span (no-op).
pub fn end_hook_span(_span: Arc<NoopSpan>) {}

/// Add tool content event to span (no-op).
pub fn add_tool_content_event(_span: &Arc<NoopSpan>, _label: &str, _content: &str) {}

/// Execute an async operation within a span (no-op wrapper).
pub async fn execute_in_span<F, T>(f: F) -> T
where
    F: std::future::Future<Output = T>,
{
    f.await
}

/// Check if enhanced telemetry is enabled (always false).
pub fn is_enhanced_telemetry_enabled() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_span_methods() {
        let span = NoopSpan::new();
        span.set_attribute("key", "value");
        span.set_attributes(&[("k1", "v1"), ("k2", "v2")]);
        span.add_event("test_event");
        span.record_exception("test error");
        span.end();
        // If this test passes, the no-ops work correctly
    }

    #[test]
    fn test_span_functions() {
        let root = start_interaction_span("req-123");
        end_interaction_span(root);

        let llm = start_llm_request_span("claude-3", 4096);
        end_llm_request_span(llm, 100, 50);

        let tool = start_tool_span("bash");
        end_tool_span(tool, true, None);

        let perm = start_permission_span("read_file");
        end_permission_span(perm);

        let hook = start_hook_span("pre_request");
        end_hook_span(hook);

        assert!(!is_enhanced_telemetry_enabled());
    }

    #[tokio::test]
    async fn test_execute_in_span_async() {
        let result = execute_in_span(async { 42 }).await;
        assert_eq!(result, 42);
    }
}
