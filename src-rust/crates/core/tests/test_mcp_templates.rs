// Integration tests for MCP template rendering

use claurst_core::mcp_templates::TemplateRenderer;
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
fn test_resource_context() {
    // Simulate an MCP resource context
    let context = json!({
        "uri": "file:///database.json",
        "name": "Database",
        "description": "Primary data store",
        "mimeType": "application/json"
    });

    let template = "Access {{name}} at {{uri}} - {{description}}";
    let result = TemplateRenderer::render(template, &context);
    assert_eq!(
        result,
        "Access Database at file:///database.json - Primary data store"
    );
}

#[test]
fn test_multiple_occurrences() {
    let context = json!({
        "resource": "users",
        "action": "read"
    });

    let template = "{{resource}} {{action}} and {{resource}} {{action}}";
    let result = TemplateRenderer::render(template, &context);
    assert_eq!(result, "users read and users read");
}

#[test]
fn test_numeric_and_bool_values() {
    let context = json!({
        "count": 42,
        "enabled": true,
        "version": 1.5
    });

    let template = "Items: {{count}}, Enabled: {{enabled}}, Version: {{version}}";
    let result = TemplateRenderer::render(template, &context);
    assert_eq!(result, "Items: 42, Enabled: true, Version: 1.5");
}
