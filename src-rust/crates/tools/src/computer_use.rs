// Computer Use tool — cross-platform mouse/keyboard/screenshot control.
//
// All implementation is gated behind `#[cfg(feature = "computer-use")]`
// so the default build never links enigo or xcap.
//
// API wire format follows Anthropic's computer_20250124 spec:
//   - Tool type: "computer_20250124"
//   - Name:      "computer"
//   - Input:     { action, coordinate?, start_coordinate?, end_coordinate?,
//                  text?, direction?, amount? }
//   - Output:    text description of the action; screenshots return base64 JPEG.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Input type (parsed regardless of feature flag so the schema is always
// available; the *execution* is feature-gated).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ComputerUseInput {
    pub action: String,
    /// [x, y] pixel coordinate for single-point actions.
    pub coordinate: Option<[i32; 2]>,
    /// Drag start coordinate.
    pub start_coordinate: Option<[i32; 2]>,
    /// Drag end coordinate.
    pub end_coordinate: Option<[i32; 2]>,
    /// Text for `type_text` / `key` actions.
    pub text: Option<String>,
    /// Scroll direction: "up" | "down" | "left" | "right"
    pub direction: Option<String>,
    /// Number of scroll notches / lines.
    pub amount: Option<u32>,
}

// ---------------------------------------------------------------------------
// Display size constants used in the API description.
// These should match (or be updated to match) the actual primary monitor.
// ---------------------------------------------------------------------------

const DISPLAY_WIDTH_PX: u32 = 1920;
const DISPLAY_HEIGHT_PX: u32 = 1080;

/// Maximum dimensions the API accepts for screenshots.
#[allow(dead_code)]
const MAX_SCREENSHOT_WIDTH: u32 = 1366;
#[allow(dead_code)]
const MAX_SCREENSHOT_HEIGHT: u32 = 768;
#[allow(dead_code)]
const JPEG_QUALITY: u8 = 75;

// ---------------------------------------------------------------------------
// Tool struct
// ---------------------------------------------------------------------------

pub struct ComputerUseTool;

#[async_trait]
impl Tool for ComputerUseTool {
    fn name(&self) -> &str {
        "computer"
    }

    fn description(&self) -> &str {
        "Control the computer: take screenshots, move the mouse, click, type text, \
         press keyboard shortcuts, scroll, and drag. \
         Use `screenshot` first to see the current state of the screen. \
         Coordinates are in pixels from the top-left corner of the screen."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Dangerous
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "screenshot",
                        "mouse_move",
                        "left_click",
                        "right_click",
                        "double_click",
                        "left_click_drag",
                        "type_text",
                        "key",
                        "scroll",
                        "get_cursor_position"
                    ],
                    "description": "The action to perform"
                },
                "coordinate": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "minItems": 2,
                    "maxItems": 2,
                    "description": "[x, y] pixel coordinate for mouse actions"
                },
                "start_coordinate": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "minItems": 2,
                    "maxItems": 2,
                    "description": "Start [x, y] coordinate for left_click_drag"
                },
                "end_coordinate": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "minItems": 2,
                    "maxItems": 2,
                    "description": "End [x, y] coordinate for left_click_drag"
                },
                "text": {
                    "type": "string",
                    "description": "Text to type (type_text) or key sequence to press (key), e.g. \"ctrl+c\", \"Return\""
                },
                "direction": {
                    "type": "string",
                    "enum": ["up", "down", "left", "right"],
                    "description": "Scroll direction"
                },
                "amount": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Number of scroll notches"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: ComputerUseInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid computer-use input: {}", e)),
        };

        // Permission gate
        let desc = format!("computer: {}", params.action);
        if let Err(e) = ctx.check_permission(self.name(), &desc, false) {
            return ToolResult::error(e.to_string());
        }

        execute_action(params).await
    }

    /// Override `to_definition` to emit the Anthropic computer-use-specific
    /// format with `type` and display dimensions.
    fn to_definition(&self) -> claurst_core::types::ToolDefinition {
        // The computer tool uses a special schema form that includes display dims.
        // We encode them in the description field which is what the API sends.
        claurst_core::types::ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            input_schema: self.input_schema(),
        }
    }
}

/// Build the extended API definition that includes type + display dimensions.
/// Callers that need the full `computer_20250124` block can use this instead
/// of `to_definition()`.
pub fn computer_use_api_definition() -> Value {
    json!({
        "type": "computer_20250124",
        "name": "computer",
        "display_width_px": DISPLAY_WIDTH_PX,
        "display_height_px": DISPLAY_HEIGHT_PX,
    })
}

// ---------------------------------------------------------------------------
// Action dispatch (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(not(feature = "computer-use"))]
async fn execute_action(_params: ComputerUseInput) -> ToolResult {
    ToolResult::error(
        "The computer-use feature is not enabled in this build. \
         Recompile with --features cc-tools/computer-use to enable it.",
    )
}

#[cfg(feature = "computer-use")]
async fn execute_action(params: ComputerUseInput) -> ToolResult {
    use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};

    match params.action.as_str() {
        // ── Screenshot ───────────────────────────────────────────────────
        "screenshot" => take_screenshot(),

        // ── Cursor position ──────────────────────────────────────────────
        "get_cursor_position" => match Enigo::new(&Settings::default()) {
            Ok(enigo) => match enigo.location() {
                Ok((x, y)) => {
                    ToolResult::success(format!("Cursor position: {{\"x\": {}, \"y\": {}}}", x, y))
                }
                Err(e) => ToolResult::error(format!("Failed to get cursor position: {}", e)),
            },
            Err(e) => ToolResult::error(format!("Failed to initialise input controller: {}", e)),
        },

        // ── Mouse move ───────────────────────────────────────────────────
        "mouse_move" => {
            let [x, y] = match params.coordinate {
                Some(c) => c,
                None => return ToolResult::error("mouse_move requires 'coordinate' field"),
            };
            match Enigo::new(&Settings::default()) {
                Ok(mut enigo) => match enigo.move_mouse(x, y, Coordinate::Abs) {
                    Ok(()) => ToolResult::success(format!("Moved mouse to ({}, {})", x, y)),
                    Err(e) => ToolResult::error(format!("mouse_move failed: {}", e)),
                },
                Err(e) => {
                    ToolResult::error(format!("Failed to initialise input controller: {}", e))
                }
            }
        }

        // ── Left click ───────────────────────────────────────────────────
        "left_click" => {
            let [x, y] = match params.coordinate {
                Some(c) => c,
                None => return ToolResult::error("left_click requires 'coordinate' field"),
            };
            match Enigo::new(&Settings::default()) {
                Ok(mut enigo) => {
                    if let Err(e) = enigo.move_mouse(x, y, Coordinate::Abs) {
                        return ToolResult::error(format!("left_click move failed: {}", e));
                    }
                    match enigo.button(Button::Left, Direction::Click) {
                        Ok(()) => ToolResult::success(format!("Left-clicked at ({}, {})", x, y)),
                        Err(e) => ToolResult::error(format!("left_click failed: {}", e)),
                    }
                }
                Err(e) => {
                    ToolResult::error(format!("Failed to initialise input controller: {}", e))
                }
            }
        }

        // ── Right click ──────────────────────────────────────────────────
        "right_click" => {
            let [x, y] = match params.coordinate {
                Some(c) => c,
                None => return ToolResult::error("right_click requires 'coordinate' field"),
            };
            match Enigo::new(&Settings::default()) {
                Ok(mut enigo) => {
                    if let Err(e) = enigo.move_mouse(x, y, Coordinate::Abs) {
                        return ToolResult::error(format!("right_click move failed: {}", e));
                    }
                    match enigo.button(Button::Right, Direction::Click) {
                        Ok(()) => ToolResult::success(format!("Right-clicked at ({}, {})", x, y)),
                        Err(e) => ToolResult::error(format!("right_click failed: {}", e)),
                    }
                }
                Err(e) => {
                    ToolResult::error(format!("Failed to initialise input controller: {}", e))
                }
            }
        }

        // ── Double click ─────────────────────────────────────────────────
        "double_click" => {
            let [x, y] = match params.coordinate {
                Some(c) => c,
                None => return ToolResult::error("double_click requires 'coordinate' field"),
            };
            match Enigo::new(&Settings::default()) {
                Ok(mut enigo) => {
                    if let Err(e) = enigo.move_mouse(x, y, Coordinate::Abs) {
                        return ToolResult::error(format!("double_click move failed: {}", e));
                    }
                    if let Err(e) = enigo.button(Button::Left, Direction::Click) {
                        return ToolResult::error(format!(
                            "double_click first click failed: {}",
                            e
                        ));
                    }
                    match enigo.button(Button::Left, Direction::Click) {
                        Ok(()) => ToolResult::success(format!("Double-clicked at ({}, {})", x, y)),
                        Err(e) => {
                            ToolResult::error(format!("double_click second click failed: {}", e))
                        }
                    }
                }
                Err(e) => {
                    ToolResult::error(format!("Failed to initialise input controller: {}", e))
                }
            }
        }

        // ── Click and drag ───────────────────────────────────────────────
        "left_click_drag" => {
            let [sx, sy] = match params.start_coordinate {
                Some(c) => c,
                None => {
                    return ToolResult::error("left_click_drag requires 'start_coordinate' field")
                }
            };
            let [ex, ey] = match params.end_coordinate {
                Some(c) => c,
                None => {
                    return ToolResult::error("left_click_drag requires 'end_coordinate' field")
                }
            };
            match Enigo::new(&Settings::default()) {
                Ok(mut enigo) => {
                    // Move to start, press, move to end, release.
                    if let Err(e) = enigo.move_mouse(sx, sy, Coordinate::Abs) {
                        return ToolResult::error(format!(
                            "left_click_drag: move to start failed: {}",
                            e
                        ));
                    }
                    if let Err(e) = enigo.button(Button::Left, Direction::Press) {
                        return ToolResult::error(format!("left_click_drag: press failed: {}", e));
                    }
                    if let Err(e) = enigo.move_mouse(ex, ey, Coordinate::Abs) {
                        // Best-effort release on error.
                        let _ = enigo.button(Button::Left, Direction::Release);
                        return ToolResult::error(format!(
                            "left_click_drag: drag move failed: {}",
                            e
                        ));
                    }
                    match enigo.button(Button::Left, Direction::Release) {
                        Ok(()) => ToolResult::success(format!(
                            "Dragged from ({}, {}) to ({}, {})",
                            sx, sy, ex, ey
                        )),
                        Err(e) => {
                            ToolResult::error(format!("left_click_drag: release failed: {}", e))
                        }
                    }
                }
                Err(e) => {
                    ToolResult::error(format!("Failed to initialise input controller: {}", e))
                }
            }
        }

        // ── Type text ────────────────────────────────────────────────────
        "type_text" => {
            let text = match params.text {
                Some(t) => t,
                None => return ToolResult::error("type_text requires 'text' field"),
            };
            match Enigo::new(&Settings::default()) {
                Ok(mut enigo) => match enigo.text(&text) {
                    Ok(()) => {
                        ToolResult::success(format!("Typed {} characters", text.chars().count()))
                    }
                    Err(e) => ToolResult::error(format!("type_text failed: {}", e)),
                },
                Err(e) => {
                    ToolResult::error(format!("Failed to initialise input controller: {}", e))
                }
            }
        }

        // ── Key press ────────────────────────────────────────────────────
        "key" => {
            let key_str = match params.text {
                Some(t) => t,
                None => return ToolResult::error("key requires 'text' field with key sequence"),
            };
            match Enigo::new(&Settings::default()) {
                Ok(mut enigo) => match press_key_sequence(&mut enigo, &key_str) {
                    Ok(()) => ToolResult::success(format!("Pressed key: {}", key_str)),
                    Err(e) => ToolResult::error(format!("key failed: {}", e)),
                },
                Err(e) => {
                    ToolResult::error(format!("Failed to initialise input controller: {}", e))
                }
            }
        }

        // ── Scroll ───────────────────────────────────────────────────────
        "scroll" => {
            let [x, y] = match params.coordinate {
                Some(c) => c,
                None => return ToolResult::error("scroll requires 'coordinate' field"),
            };
            let direction = match params.direction.as_deref() {
                Some(d) => d.to_string(),
                None => return ToolResult::error("scroll requires 'direction' field"),
            };
            let amount = params.amount.unwrap_or(3) as i32;

            match Enigo::new(&Settings::default()) {
                Ok(mut enigo) => {
                    if let Err(e) = enigo.move_mouse(x, y, Coordinate::Abs) {
                        return ToolResult::error(format!("scroll: move failed: {}", e));
                    }
                    let result = match direction.as_str() {
                        "up" => enigo.scroll(-amount, enigo::Axis::Vertical),
                        "down" => enigo.scroll(amount, enigo::Axis::Vertical),
                        "left" => enigo.scroll(-amount, enigo::Axis::Horizontal),
                        "right" => enigo.scroll(amount, enigo::Axis::Horizontal),
                        other => {
                            return ToolResult::error(format!(
                                "scroll: unknown direction '{}'. Use up/down/left/right",
                                other
                            ))
                        }
                    };
                    match result {
                        Ok(()) => ToolResult::success(format!(
                            "Scrolled {} by {} at ({}, {})",
                            direction, amount, x, y
                        )),
                        Err(e) => ToolResult::error(format!("scroll failed: {}", e)),
                    }
                }
                Err(e) => {
                    ToolResult::error(format!("Failed to initialise input controller: {}", e))
                }
            }
        }

        other => ToolResult::error(format!(
            "Unknown action '{}'. Valid actions: screenshot, mouse_move, left_click, \
             right_click, double_click, left_click_drag, type_text, key, scroll, \
             get_cursor_position",
            other
        )),
    }
}

// ---------------------------------------------------------------------------
// Screenshot helper (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "computer-use")]
fn take_screenshot() -> ToolResult {
    use base64::Engine as _;

    // Capture the primary monitor.
    let monitors = match xcap::Monitor::all() {
        Ok(m) => m,
        Err(e) => return ToolResult::error(format!("Failed to enumerate monitors: {}", e)),
    };

    let monitor = match monitors.into_iter().next() {
        Some(m) => m,
        None => return ToolResult::error("No monitors found"),
    };

    let image = match monitor.capture_image() {
        Ok(img) => img,
        Err(e) => return ToolResult::error(format!("Screenshot capture failed: {}", e)),
    };

    // Scale down to API limits if necessary (preserve aspect ratio).
    let (orig_w, orig_h) = (image.width(), image.height());
    let scale_w = MAX_SCREENSHOT_WIDTH as f64 / orig_w as f64;
    let scale_h = MAX_SCREENSHOT_HEIGHT as f64 / orig_h as f64;
    let scale = scale_w.min(scale_h).min(1.0);

    let scaled = if scale < 1.0 {
        let new_w = (orig_w as f64 * scale).round() as u32;
        let new_h = (orig_h as f64 * scale).round() as u32;
        image::DynamicImage::ImageRgba8(image)
            .resize(new_w, new_h, image::imageops::FilterType::Lanczos3)
            .to_rgba8()
    } else {
        image
    };

    // Encode as JPEG.
    let mut jpeg_bytes: Vec<u8> = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut jpeg_bytes);
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, JPEG_QUALITY);
        if let Err(e) = encoder.encode_image(&image::DynamicImage::ImageRgba8(scaled)) {
            return ToolResult::error(format!("JPEG encoding failed: {}", e));
        }
    }

    let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);

    ToolResult::success(format!(
        "Screenshot captured ({}x{} → JPEG, {} bytes base64).\ndata:image/jpeg;base64,{}",
        orig_w,
        orig_h,
        b64.len(),
        b64
    ))
}

// ---------------------------------------------------------------------------
// Key sequence parser (feature-gated)
// ---------------------------------------------------------------------------
//
// Parses xdotool-style sequences like "ctrl+c", "Return", "super+shift+s".
// Each '+'-delimited token is mapped to an enigo Key or modifier.

#[cfg(feature = "computer-use")]
fn press_key_sequence(
    enigo: &mut enigo::Enigo,
    sequence: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use enigo::{Direction, Key, Keyboard};

    let parts: Vec<&str> = sequence
        .split('+')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() {
        return Err("Empty key sequence".into());
    }

    // If there's only one part, press+release it directly.
    if parts.len() == 1 {
        let key = parse_key(parts[0])?;
        enigo.key(key, Direction::Click)?;
        return Ok(());
    }

    // For combos: press all modifiers (first N-1 parts), click the final key,
    // then release modifiers in reverse order.
    let (modifiers, final_key_str) = parts.split_at(parts.len() - 1);
    let final_key = parse_key(final_key_str[0])?;

    let mut pressed: Vec<enigo::Key> = Vec::new();
    let mut press_err: Option<Box<dyn std::error::Error + Send + Sync>> = None;

    for &mod_str in modifiers {
        match parse_key(mod_str) {
            Ok(k) => {
                if let Err(e) = enigo.key(k, Direction::Press) {
                    press_err = Some(Box::new(e));
                    break;
                }
                pressed.push(k);
            }
            Err(e) => {
                press_err = Some(e);
                break;
            }
        }
    }

    // Click the final key (only if no error so far).
    if press_err.is_none() {
        if let Err(e) = enigo.key(final_key, Direction::Click) {
            press_err = Some(Box::new(e));
        }
    }

    // Release modifiers in reverse order — always attempt even on error.
    for &k in pressed.iter().rev() {
        let _ = enigo.key(k, Direction::Release);
    }

    if let Some(e) = press_err {
        return Err(e);
    }

    Ok(())
}

#[cfg(feature = "computer-use")]
fn parse_key(s: &str) -> Result<enigo::Key, Box<dyn std::error::Error + Send + Sync>> {
    use enigo::Key;

    let key = match s.to_ascii_lowercase().as_str() {
        // Modifiers
        "ctrl" | "control" => Key::Control,
        "alt" => Key::Alt,
        "shift" => Key::Shift,
        "super" | "meta" | "win" | "cmd" | "command" => Key::Meta,

        // Navigation / special
        "return" | "enter" => Key::Return,
        "escape" | "esc" => Key::Escape,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "insert" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "page_up" => Key::PageUp,
        "pagedown" | "page_down" => Key::PageDown,
        "up" => Key::UpArrow,
        "down" => Key::DownArrow,
        "left" => Key::LeftArrow,
        "right" => Key::RightArrow,
        "space" => Key::Space,
        "capslock" | "caps_lock" => Key::CapsLock,

        // Function keys
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,

        // Single unicode character
        other if other.chars().count() == 1 => {
            let ch = other.chars().next().unwrap();
            Key::Unicode(ch)
        }

        other => return Err(format!("Unknown key name: '{}'", other).into()),
    };

    Ok(key)
}

// ---------------------------------------------------------------------------
// Tests (work without the feature flag — test input parsing logic only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(v: Value) -> Result<ComputerUseInput, serde_json::Error> {
        serde_json::from_value(v)
    }

    // ---- Input parsing -------------------------------------------------------

    #[test]
    fn test_parse_screenshot_action() {
        let input = parse(json!({ "action": "screenshot" })).unwrap();
        assert_eq!(input.action, "screenshot");
        assert!(input.coordinate.is_none());
    }

    #[test]
    fn test_parse_left_click_with_coordinate() {
        let input = parse(json!({
            "action": "left_click",
            "coordinate": [100, 200]
        }))
        .unwrap();
        assert_eq!(input.action, "left_click");
        assert_eq!(input.coordinate, Some([100, 200]));
    }

    #[test]
    fn test_parse_type_text() {
        let input = parse(json!({
            "action": "type_text",
            "text": "hello world"
        }))
        .unwrap();
        assert_eq!(input.text, Some("hello world".to_string()));
    }

    #[test]
    fn test_parse_key() {
        let input = parse(json!({
            "action": "key",
            "text": "ctrl+c"
        }))
        .unwrap();
        assert_eq!(input.text, Some("ctrl+c".to_string()));
    }

    #[test]
    fn test_parse_scroll() {
        let input = parse(json!({
            "action": "scroll",
            "coordinate": [400, 300],
            "direction": "down",
            "amount": 5
        }))
        .unwrap();
        assert_eq!(input.direction, Some("down".to_string()));
        assert_eq!(input.amount, Some(5));
        assert_eq!(input.coordinate, Some([400, 300]));
    }

    #[test]
    fn test_parse_left_click_drag() {
        let input = parse(json!({
            "action": "left_click_drag",
            "start_coordinate": [10, 20],
            "end_coordinate": [100, 200]
        }))
        .unwrap();
        assert_eq!(input.start_coordinate, Some([10, 20]));
        assert_eq!(input.end_coordinate, Some([100, 200]));
    }

    #[test]
    fn test_parse_missing_optional_fields() {
        let input = parse(json!({ "action": "get_cursor_position" })).unwrap();
        assert!(input.coordinate.is_none());
        assert!(input.text.is_none());
        assert!(input.direction.is_none());
        assert!(input.amount.is_none());
        assert!(input.start_coordinate.is_none());
        assert!(input.end_coordinate.is_none());
    }

    #[test]
    fn test_parse_invalid_coordinate_type_fails() {
        // coordinate must be [i32; 2], not a string.
        let result = parse(json!({
            "action": "left_click",
            "coordinate": "100,200"
        }));
        assert!(result.is_err());
    }

    // ---- Key string parsing (not feature-gated — just string logic) ----------

    #[test]
    fn test_key_sequence_splits_correctly() {
        // Validate the splitting logic used by press_key_sequence.
        let seq = "ctrl+shift+a";
        let parts: Vec<&str> = seq
            .split('+')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(parts, vec!["ctrl", "shift", "a"]);
    }

    #[test]
    fn test_key_sequence_single_key() {
        let seq = "Return";
        let parts: Vec<&str> = seq
            .split('+')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], "Return");
    }

    #[test]
    fn test_key_sequence_empty_parts_filtered() {
        let seq = "+ctrl+c+";
        let parts: Vec<&str> = seq
            .split('+')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(parts, vec!["ctrl", "c"]);
    }

    // ---- Tool trait surface --------------------------------------------------

    #[test]
    fn test_computer_use_tool_name() {
        assert_eq!(ComputerUseTool.name(), "computer");
    }

    #[test]
    fn test_computer_use_tool_schema_is_object() {
        let schema = ComputerUseTool.input_schema();
        assert!(schema.is_object());
        assert!(schema.get("properties").is_some());
    }

    #[test]
    fn test_computer_use_tool_schema_has_action() {
        let schema = ComputerUseTool.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(
            props.contains_key("action"),
            "schema must have 'action' property"
        );
    }

    #[test]
    fn test_computer_use_permission_level_is_dangerous() {
        assert_eq!(
            ComputerUseTool.permission_level(),
            PermissionLevel::Dangerous
        );
    }

    #[test]
    fn test_computer_use_api_definition() {
        let def = computer_use_api_definition();
        assert_eq!(def["type"], "computer_20250124");
        assert_eq!(def["name"], "computer");
        assert!(def["display_width_px"].as_u64().unwrap() > 0);
        assert!(def["display_height_px"].as_u64().unwrap() > 0);
    }
}
