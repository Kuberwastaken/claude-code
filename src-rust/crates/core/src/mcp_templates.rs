//! MCP resource prompt template rendering with variable substitution.
//!
//! Supports templates like:
//!   "Use {{name}} for {{description}}"
//!   "Access {{meta.author}} resources"
//!
//! Variables are substituted from resource properties, with support for
//! nested paths (e.g., `{{meta.author}}` accesses `resource["meta"]["author"]`).

use serde_json::Value;
use tracing::debug;

/// Renders template strings with variable substitution from a context object.
///
/// Templates use `{{variable_name}}` or `{{nested.path}}` syntax.
/// Missing variables are left as-is (not replaced).
pub struct TemplateRenderer;

impl TemplateRenderer {
    /// Render a template string by substituting {{variable}} placeholders
    /// with values from the context.
    ///
    /// # Arguments
    /// * `template` - Template string with {{var}} placeholders
    /// * `context` - JSON object providing values for substitution
    ///
    /// # Returns
    /// The rendered string with substitutions applied.
    /// Missing variables are left as `{{variable}}`.
    pub fn render(template: &str, context: &Value) -> String {
        let mut result = template.to_string();
        let mut pos = 0;

        loop {
            // Find next {{
            let search_from = &result[pos..];
            match search_from.find("{{") {
                None => break,
                Some(rel_start) => {
                    let start = pos + rel_start;

                    // Find matching }}
                    match search_from[rel_start..].find("}}") {
                        None => break,
                        Some(rel_end) => {
                            let end = pos + rel_start + rel_end;
                            let var_name = &result[start + 2..end];

                            // Get value from context
                            if let Some(value) = Self::get_nested_value(context, var_name) {
                                let replacement = Self::value_to_string(&value);
                                result = format!(
                                    "{}{}{}",
                                    &result[..start],
                                    replacement,
                                    &result[end + 2..]
                                );
                                pos = start + replacement.len();
                            } else {
                                // Variable not found, leave as-is and skip past it
                                debug!("Template variable not found in context: {}", var_name);
                                pos = end + 2;
                            }
                        }
                    }
                }
            }
        }

        result
    }

    /// Get a nested value from JSON using dot notation.
    ///
    /// # Arguments
    /// * `value` - JSON value to access
    /// * `path` - Dot-separated path like "meta.author" or simple "name"
    ///
    /// # Returns
    /// The value at the path, or None if not found.
    fn get_nested_value(value: &Value, path: &str) -> Option<Value> {
        let parts: Vec<&str> = path.split('.').collect();
        let mut current = value;

        for part in parts {
            current = match current {
                Value::Object(map) => map.get(part)?,
                _ => return None,
            };
        }

        Some(current.clone())
    }

    /// Convert a JSON value to a string representation.
    fn value_to_string(value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => "null".to_string(),
            Value::Array(arr) => {
                // Join array elements with comma
                arr.iter()
                    .map(Self::value_to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            }
            Value::Object(_) => "[object]".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_simple_substitution() {
        let context = json!({
            "name": "Database",
            "description": "Query operations"
        });
        let result = TemplateRenderer::render("Use {{name}} for {{description}}", &context);
        assert_eq!(result, "Use Database for Query operations");
    }

    #[test]
    fn test_nested_path() {
        let context = json!({
            "meta": {
                "author": "Alice",
                "version": "1.0"
            }
        });
        let result =
            TemplateRenderer::render("Created by {{meta.author}} (v{{meta.version}})", &context);
        assert_eq!(result, "Created by Alice (v1.0)");
    }

    #[test]
    fn test_missing_variable() {
        let context = json!({
            "name": "Database"
        });
        let result = TemplateRenderer::render("Use {{name}} with {{api_key}}", &context);
        // Missing {{api_key}} is left as-is
        assert_eq!(result, "Use Database with {{api_key}}");
    }

    #[test]
    fn test_no_substitution() {
        let context = json!({ "name": "Test" });
        let result = TemplateRenderer::render("Plain text without variables", &context);
        assert_eq!(result, "Plain text without variables");
    }

    #[test]
    fn test_number_and_bool_values() {
        let context = json!({
            "count": 42,
            "enabled": true
        });
        let result = TemplateRenderer::render("Count: {{count}}, Enabled: {{enabled}}", &context);
        assert_eq!(result, "Count: 42, Enabled: true");
    }

    #[test]
    fn test_array_value() {
        let context = json!({
            "tags": ["rust", "mcp", "templates"]
        });
        let result = TemplateRenderer::render("Tags: {{tags}}", &context);
        assert_eq!(result, "Tags: rust, mcp, templates");
    }

    #[test]
    fn test_empty_context() {
        let context = json!({});
        let result = TemplateRenderer::render("Hello {{name}}", &context);
        assert_eq!(result, "Hello {{name}}");
    }
}
