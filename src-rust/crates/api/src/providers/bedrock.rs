// providers/bedrock.rs — Amazon Bedrock provider adapter.
//
// Uses the Bedrock Converse Streaming API which accepts a unified message
// format similar to Anthropic's, making it straightforward to map from
// our internal ProviderRequest.
//
// Endpoint:
//   POST https://bedrock-runtime.{region}.amazonaws.com/model/{model_id}/converse-stream
//
// Auth:
//   - If AWS_BEARER_TOKEN_BEDROCK is set: Authorization: Bearer <token>
//   - Otherwise: AWS SigV4 signed request using access key + secret
//
// Only Claude models on Bedrock are officially supported by this adapter.

use std::pin::Pin;

use async_stream::stream;
use async_trait::async_trait;
use claurst_core::provider_id::{ModelId, ProviderId};
use claurst_core::types::{ContentBlock, MessageContent, Role, ToolResultContent, UsageInfo};
use futures::Stream;
use serde_json::{json, Value};
use tracing::debug;

use crate::error_handling::parse_error_response;
use crate::provider::{LlmProvider, ModelInfo};
use crate::provider_error::ProviderError;
use crate::provider_types::{
    ProviderCapabilities, ProviderRequest, ProviderResponse, ProviderStatus, StopReason,
    StreamEvent, SystemPrompt, SystemPromptStyle,
};

use super::message_normalization::remove_empty_messages;
use super::request_options::merge_bedrock_options;

// ---------------------------------------------------------------------------
// BedrockProvider
// ---------------------------------------------------------------------------

pub struct BedrockProvider {
    id: ProviderId,
    region: String,
    http_client: reqwest::Client,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    session_token: Option<String>,
    bearer_token: Option<String>,
}

impl BedrockProvider {
    pub fn from_env() -> Option<Self> {
        let region = std::env::var("AWS_REGION")
            .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|_| "us-east-1".to_string());

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .expect("failed to build reqwest client");

        // Bearer token takes priority over SigV4 credentials.
        if let Ok(token) = std::env::var("AWS_BEARER_TOKEN_BEDROCK") {
            return Some(Self {
                id: ProviderId::new(ProviderId::AMAZON_BEDROCK),
                region,
                http_client,
                access_key_id: None,
                secret_access_key: None,
                session_token: None,
                bearer_token: Some(token),
            });
        }

        // Standard SigV4 credentials.
        let key = std::env::var("AWS_ACCESS_KEY_ID").ok()?;
        let secret = std::env::var("AWS_SECRET_ACCESS_KEY").ok()?;
        let session = std::env::var("AWS_SESSION_TOKEN").ok();

        Some(Self {
            id: ProviderId::new(ProviderId::AMAZON_BEDROCK),
            region,
            http_client,
            access_key_id: Some(key),
            secret_access_key: Some(secret),
            session_token: session,
            bearer_token: None,
        })
    }

    /// Add a regional cross-inference prefix for models that support it.
    fn model_id_with_prefix(&self, model: &str) -> String {
        // Skip if already has a dot-separated prefix (e.g. "us.anthropic.claude-...")
        if model.contains('.') {
            return model.to_string();
        }
        let region = &self.region;
        if region.starts_with("us-") && !region.contains("gov") {
            if model.contains("claude") || model.contains("nova") {
                return format!("us.{}", model);
            }
        } else if region.starts_with("eu-") && model.contains("claude") {
            return format!("eu.{}", model);
        }
        model.to_string()
    }

    fn endpoint_url(&self, model_id: &str) -> String {
        format!(
            "https://bedrock-runtime.{}.amazonaws.com/model/{}/converse-stream",
            self.region,
            urlencoding::encode(model_id)
        )
    }

    // -----------------------------------------------------------------------
    // AWS SigV4 signing
    // -----------------------------------------------------------------------

    fn sign_request(
        &self,
        method: &str,
        url_str: &str,
        body: &str,
        date: &chrono::DateTime<chrono::Utc>,
    ) -> std::collections::HashMap<String, String> {
        use hmac::{Hmac, Mac};
        use sha2::{Digest, Sha256};

        type HmacSha256 = Hmac<Sha256>;

        let mut headers = std::collections::HashMap::new();

        // If we have a bearer token, skip SigV4.
        if let Some(ref token) = self.bearer_token {
            headers.insert("Authorization".to_string(), format!("Bearer {}", token));
            return headers;
        }

        let access_key = match &self.access_key_id {
            Some(k) => k.clone(),
            None => return headers,
        };
        let secret_key = match &self.secret_access_key {
            Some(s) => s.clone(),
            None => return headers,
        };

        let date_str = date.format("%Y%m%d").to_string();
        let datetime_str = date.format("%Y%m%dT%H%M%SZ").to_string();
        let service = "bedrock";
        let region = &self.region;

        // Parse path and query from URL.
        let parsed = url::Url::parse(url_str).unwrap_or_else(|_| {
            url::Url::parse("https://bedrock-runtime.us-east-1.amazonaws.com/").unwrap()
        });
        let canonical_uri = {
            let p = parsed.path();
            if p.is_empty() {
                "/".to_string()
            } else {
                p.to_string()
            }
        };
        let canonical_query = parsed.query().unwrap_or("").to_string();

        // Body hash.
        let body_hash = hex::encode(Sha256::digest(body.as_bytes()));

        // Canonical headers (must be sorted, lowercased).
        let host = parsed.host_str().unwrap_or_default().to_string();
        let content_type = "application/json";

        // Build canonical headers string and signed headers list.
        // Include: content-type, host, x-amz-content-sha256, x-amz-date,
        // and optionally x-amz-security-token.
        let mut canonical_headers = format!(
            "content-type:{}\nhost:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\n",
            content_type, host, body_hash, datetime_str
        );
        let mut signed_headers = "content-type;host;x-amz-content-sha256;x-amz-date".to_string();

        if let Some(ref tok) = self.session_token {
            canonical_headers.push_str(&format!("x-amz-security-token:{}\n", tok));
            signed_headers.push_str(";x-amz-security-token");
        }

        // Canonical request.
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method, canonical_uri, canonical_query, canonical_headers, signed_headers, body_hash
        );

        // String to sign.
        let credential_scope = format!("{}/{}/{}/aws4_request", date_str, region, service);
        let canonical_request_hash = hex::encode(Sha256::digest(canonical_request.as_bytes()));
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            datetime_str, credential_scope, canonical_request_hash
        );

        // Signing key: HMAC-SHA256 chain.
        let sign_key = {
            let k_date = {
                let mut mac = HmacSha256::new_from_slice(format!("AWS4{}", secret_key).as_bytes())
                    .expect("HMAC init failed");
                mac.update(date_str.as_bytes());
                mac.finalize().into_bytes()
            };
            let k_region = {
                let mut mac = HmacSha256::new_from_slice(&k_date).expect("HMAC init failed");
                mac.update(region.as_bytes());
                mac.finalize().into_bytes()
            };
            let k_service = {
                let mut mac = HmacSha256::new_from_slice(&k_region).expect("HMAC init failed");
                mac.update(service.as_bytes());
                mac.finalize().into_bytes()
            };
            let k_signing = {
                let mut mac = HmacSha256::new_from_slice(&k_service).expect("HMAC init failed");
                mac.update(b"aws4_request");
                mac.finalize().into_bytes()
            };
            k_signing
        };

        let signature = {
            let mut mac = HmacSha256::new_from_slice(&sign_key).expect("HMAC init failed");
            mac.update(string_to_sign.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        };

        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            access_key, credential_scope, signed_headers, signature
        );

        headers.insert("Authorization".to_string(), authorization);
        headers.insert("x-amz-date".to_string(), datetime_str);
        headers.insert("x-amz-content-sha256".to_string(), body_hash);
        if let Some(ref tok) = self.session_token {
            headers.insert("x-amz-security-token".to_string(), tok.clone());
        }

        headers
    }

    // -----------------------------------------------------------------------
    // Request body builders
    // -----------------------------------------------------------------------

    fn build_converse_body(request: &ProviderRequest) -> Value {
        let messages = Self::build_converse_messages(request);
        let mut body = json!({
            "messages": messages,
            "inferenceConfig": {
                "maxTokens": request.max_tokens,
                "temperature": request.temperature.unwrap_or(0.7),
                "topP": request.top_p.unwrap_or(0.9),
                "stopSequences": request.stop_sequences,
            }
        });

        // System prompt.
        if let Some(sys) = &request.system_prompt {
            let sys_text = match sys {
                SystemPrompt::Text(t) => t.clone(),
                SystemPrompt::Blocks(blocks) => blocks
                    .iter()
                    .map(|b| b.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            body["system"] = json!([{ "text": sys_text }]);
        }

        // Tool definitions.
        if !request.tools.is_empty() {
            let tool_specs: Vec<Value> = request
                .tools
                .iter()
                .map(|td| {
                    json!({
                        "toolSpec": {
                            "name": td.name,
                            "description": td.description,
                            "inputSchema": {
                                "json": td.input_schema
                            }
                        }
                    })
                })
                .collect();
            body["toolConfig"] = json!({ "tools": tool_specs });
        }

        if let Some(thinking) = &request.thinking {
            body["reasoningConfig"] = json!({
                "type": "enabled",
                "budgetTokens": thinking.budget_tokens,
            });
        }

        merge_bedrock_options(&mut body, &request.provider_options);

        body
    }

    fn build_converse_messages(request: &ProviderRequest) -> Vec<Value> {
        remove_empty_messages(&request.messages)
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };
                let content = Self::message_content_to_converse(&msg.content, &msg.role);
                json!({ "role": role, "content": content })
            })
            .collect()
    }

    fn message_content_to_converse(content: &MessageContent, role: &Role) -> Vec<Value> {
        match content {
            MessageContent::Text(t) => vec![json!({ "text": t })],
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| Self::content_block_to_converse(b, role))
                .collect(),
        }
    }

    fn content_block_to_converse(block: &ContentBlock, role: &Role) -> Option<Value> {
        match block {
            ContentBlock::Text { text } => Some(json!({ "text": text })),
            ContentBlock::Image { source } => {
                // Bedrock Converse image format.
                let media_type = source
                    .media_type
                    .as_deref()
                    .unwrap_or("image/png")
                    .replace("image/", "");
                if let Some(data) = &source.data {
                    Some(json!({
                        "image": {
                            "format": media_type,
                            "source": {
                                "bytes": data
                            }
                        }
                    }))
                } else if let Some(url) = &source.url {
                    // Bedrock doesn't support URL images natively; skip.
                    debug!("Bedrock does not support URL images: {}", url);
                    None
                } else {
                    None
                }
            }
            ContentBlock::ToolUse { id, name, input } => Some(json!({
                "toolUse": {
                    "toolUseId": id,
                    "name": name,
                    "input": input
                }
            })),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let result_content = match content {
                    ToolResultContent::Text(t) => vec![json!({ "text": t })],
                    ToolResultContent::Blocks(inner) => inner
                        .iter()
                        .filter_map(|b| Self::content_block_to_converse(b, role))
                        .collect(),
                };
                let status = if is_error.unwrap_or(false) {
                    "error"
                } else {
                    "success"
                };
                Some(json!({
                    "toolResult": {
                        "toolUseId": tool_use_id,
                        "content": result_content,
                        "status": status
                    }
                }))
            }
            ContentBlock::Thinking { thinking, .. } => Some(json!({ "text": thinking })),
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // HTTP helpers
    // -----------------------------------------------------------------------

    fn map_http_error(&self, status: u16, body: &str) -> ProviderError {
        parse_error_response(status, body, &self.id)
    }

    // -----------------------------------------------------------------------
    // Send helpers
    // -----------------------------------------------------------------------

    async fn send_streaming(
        &self,
        request: &ProviderRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        let bedrock_model = self.model_id_with_prefix(&request.model);
        let url = self.endpoint_url(&bedrock_model);

        let body = Self::build_converse_body(request);
        let body_str = serde_json::to_string(&body).unwrap_or_default();

        let now = chrono::Utc::now();
        let auth_headers = self.sign_request("POST", &url, &body_str, &now);

        let mut req_builder = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/vnd.amazon.eventstream");

        for (k, v) in &auth_headers {
            req_builder = req_builder.header(k.as_str(), v.as_str());
        }

        let resp = req_builder
            .body(body_str)
            .send()
            .await
            .map_err(|e| ProviderError::Other {
                provider: self.id.clone(),
                message: format!("HTTP request failed: {}", e),
                status: None,
                body: None,
            })?;

        let status = resp.status().as_u16();
        if !(200..300).contains(&(status as usize)) {
            let text = resp.text().await.unwrap_or_default();
            return Err(self.map_http_error(status, &text));
        }

        Ok(resp)
    }

    async fn send_non_streaming(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        let bedrock_model = self.model_id_with_prefix(&request.model);
        // Non-streaming uses /converse (not /converse-stream)
        let url = format!(
            "https://bedrock-runtime.{}.amazonaws.com/model/{}/converse",
            self.region,
            urlencoding::encode(&bedrock_model)
        );

        let body = Self::build_converse_body(request);
        let body_str = serde_json::to_string(&body).unwrap_or_default();

        let now = chrono::Utc::now();
        let auth_headers = self.sign_request("POST", &url, &body_str, &now);

        let mut req_builder = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json");

        for (k, v) in &auth_headers {
            req_builder = req_builder.header(k.as_str(), v.as_str());
        }

        let resp = req_builder
            .body(body_str)
            .send()
            .await
            .map_err(|e| ProviderError::Other {
                provider: self.id.clone(),
                message: format!("HTTP request failed: {}", e),
                status: None,
                body: None,
            })?;

        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(|e| ProviderError::Other {
            provider: self.id.clone(),
            message: format!("Failed to read response body: {}", e),
            status: Some(status),
            body: None,
        })?;

        if !(200..300).contains(&(status as usize)) {
            return Err(self.map_http_error(status, &text));
        }

        let json_val: Value = serde_json::from_str(&text).map_err(|e| ProviderError::Other {
            provider: self.id.clone(),
            message: format!("Failed to parse response JSON: {}", e),
            status: Some(status),
            body: Some(text.clone()),
        })?;

        Self::parse_converse_response(&json_val, &self.id)
    }

    fn parse_converse_response(
        json: &Value,
        provider_id: &ProviderId,
    ) -> Result<ProviderResponse, ProviderError> {
        // Bedrock Converse non-streaming response shape:
        // { "output": { "message": { "role": "assistant", "content": [...] } },
        //   "stopReason": "end_turn",
        //   "usage": { "inputTokens": N, "outputTokens": M } }

        let message = json
            .get("output")
            .and_then(|o| o.get("message"))
            .ok_or_else(|| ProviderError::Other {
                provider: provider_id.clone(),
                message: "No output.message in Bedrock response".to_string(),
                status: None,
                body: None,
            })?;

        let content_blocks =
            Self::parse_converse_content(message.get("content").and_then(|c| c.as_array()));

        let stop_reason_str = json
            .get("stopReason")
            .and_then(|v| v.as_str())
            .unwrap_or("end_turn");
        let stop_reason = Self::map_stop_reason(stop_reason_str);

        let usage = Self::parse_converse_usage(json.get("usage"));

        Ok(ProviderResponse {
            id: uuid::Uuid::new_v4().to_string(),
            content: content_blocks,
            stop_reason,
            usage,
            model: json
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        })
    }

    fn parse_converse_content(content: Option<&Vec<Value>>) -> Vec<ContentBlock> {
        let blocks = match content {
            Some(b) => b,
            None => return vec![],
        };

        blocks
            .iter()
            .filter_map(|b| {
                if let Some(text) = b.get("text").and_then(|v| v.as_str()) {
                    return Some(ContentBlock::Text {
                        text: text.to_string(),
                    });
                }
                if let Some(tu) = b.get("toolUse") {
                    let id = tu
                        .get("toolUseId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = tu
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = tu.get("input").cloned().unwrap_or(json!({}));
                    return Some(ContentBlock::ToolUse { id, name, input });
                }
                None
            })
            .collect()
    }

    fn map_stop_reason(reason: &str) -> StopReason {
        match reason {
            "end_turn" => StopReason::EndTurn,
            "max_tokens" => StopReason::MaxTokens,
            "tool_use" => StopReason::ToolUse,
            "stop_sequence" => StopReason::StopSequence,
            "content_filtered" => StopReason::ContentFiltered,
            other => StopReason::Other(other.to_string()),
        }
    }

    fn parse_converse_usage(usage: Option<&Value>) -> UsageInfo {
        let u = match usage {
            Some(v) => v,
            None => return UsageInfo::default(),
        };
        UsageInfo {
            input_tokens: u.get("inputTokens").and_then(|v| v.as_u64()).unwrap_or(0),
            output_tokens: u.get("outputTokens").and_then(|v| v.as_u64()).unwrap_or(0),
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// LlmProvider impl
// ---------------------------------------------------------------------------

#[async_trait]
impl LlmProvider for BedrockProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn name(&self) -> &str {
        "Amazon Bedrock"
    }

    async fn create_message(
        &self,
        request: ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        self.send_non_streaming(&request).await
    }

    async fn create_message_stream(
        &self,
        request: ProviderRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>
    {
        let resp = self.send_streaming(&request).await?;
        let provider_id = self.id.clone();

        // Bedrock Converse streaming uses AWS EventStream binary framing.
        // For simplicity we parse the JSON chunks that appear within the
        // event payload bytes.  Each event is a binary-framed blob containing
        // a JSON payload under the ":event-type" header.
        //
        // We fall back to text-based JSON parsing by scanning for JSON objects
        // in the raw bytes, which works reliably for the common text delta events.
        let s = stream! {
            use futures::StreamExt;

            let mut byte_stream = resp.bytes_stream();
            let mut buf: Vec<u8> = Vec::new();
            let mut message_started = false;

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        yield Err(ProviderError::StreamError {
                            provider: provider_id.clone(),
                            message: format!("Stream read error: {}", e),
                            partial_response: None,
                        });
                        return;
                    }
                };

                buf.extend_from_slice(&chunk);

                // Extract all complete JSON objects from the buffer.
                // The AWS event-stream format prefixes each event with a
                // 12-byte prelude (total-len + headers-len + crc32) followed
                // by variable-length headers and then a JSON payload.  Rather
                // than fully parsing the binary framing we scan for JSON
                // object boundaries which is sufficient for the text events.
                loop {
                    // Find the first '{' in the buffer.
                    let start = match buf.iter().position(|&b| b == b'{') {
                        Some(p) => p,
                        None => {
                            buf.clear();
                            break;
                        }
                    };

                    // Drain everything before the opening brace.
                    buf.drain(..start);

                    // Try to parse a complete JSON object.
                    match serde_json::from_slice::<Value>(&buf) {
                        Ok(val) => {
                            let consumed = serde_json::to_vec(&val)
                                .map(|v| v.len())
                                .unwrap_or(buf.len());
                            buf.drain(..consumed);
                            // Process the event.
                            for ev in parse_bedrock_event(&val, &provider_id, &mut message_started) {
                                yield ev;
                            }
                        }
                        Err(e) if e.is_eof() => {
                            // Incomplete — wait for more data.
                            break;
                        }
                        Err(_) => {
                            // Invalid JSON at this position — skip one byte and retry.
                            if !buf.is_empty() {
                                buf.drain(..1);
                            } else {
                                break;
                            }
                        }
                    }
                }
            }

            // Drain any remaining complete JSON in the buffer.
            loop {
                let start = match buf.iter().position(|&b| b == b'{') {
                    Some(p) => p,
                    None => break,
                };
                buf.drain(..start);
                match serde_json::from_slice::<Value>(&buf) {
                    Ok(val) => {
                        let consumed = serde_json::to_vec(&val)
                            .map(|v| v.len())
                            .unwrap_or(buf.len());
                        buf.drain(..consumed);
                        for ev in parse_bedrock_event(&val, &provider_id, &mut message_started) {
                            yield ev;
                        }
                    }
                    Err(_) => break,
                }
            }

            if message_started {
                yield Ok(StreamEvent::MessageStop);
            }
        };

        Ok(Box::pin(s))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(vec![
            ModelInfo {
                id: ModelId::new("anthropic.claude-opus-4-6"),
                provider_id: self.id.clone(),
                name: "Claude Opus 4.6 (Bedrock)".to_string(),
                context_window: 200_000,
                max_output_tokens: 32_000,
            },
            ModelInfo {
                id: ModelId::new("anthropic.claude-sonnet-4-6"),
                provider_id: self.id.clone(),
                name: "Claude Sonnet 4.6 (Bedrock)".to_string(),
                context_window: 200_000,
                max_output_tokens: 16_000,
            },
            ModelInfo {
                id: ModelId::new("anthropic.claude-haiku-4-5-20251001"),
                provider_id: self.id.clone(),
                name: "Claude Haiku 4.5 (Bedrock)".to_string(),
                context_window: 200_000,
                max_output_tokens: 8_192,
            },
        ])
    }

    async fn health_check(&self) -> Result<ProviderStatus, ProviderError> {
        // Lightweight check: GET the list-foundation-models endpoint.
        let url = format!(
            "https://bedrock.{}.amazonaws.com/foundation-models",
            self.region
        );
        let now = chrono::Utc::now();
        // For health check, sign an empty GET body.
        let auth_headers = self.sign_request("GET", &url, "", &now);

        let mut req_builder = self.http_client.get(&url);
        for (k, v) in &auth_headers {
            req_builder = req_builder.header(k.as_str(), v.as_str());
        }

        let resp = req_builder.send().await;
        match resp {
            Ok(r) if r.status().is_success() => Ok(ProviderStatus::Healthy),
            Ok(r) if r.status().as_u16() == 401 || r.status().as_u16() == 403 => {
                Ok(ProviderStatus::Unavailable {
                    reason: "authentication failed".to_string(),
                })
            }
            Ok(r) => Ok(ProviderStatus::Degraded {
                reason: format!("foundation-models returned {}", r.status()),
            }),
            Err(e) => Ok(ProviderStatus::Unavailable {
                reason: e.to_string(),
            }),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            tool_calling: true,
            thinking: true,
            image_input: true,
            pdf_input: true,
            audio_input: false,
            video_input: false,
            caching: true,
            structured_output: false,
            system_prompt_style: SystemPromptStyle::TopLevel,
        }
    }
}

// ---------------------------------------------------------------------------
// Bedrock event parsing helper (free function so it can be used in stream!)
// ---------------------------------------------------------------------------

fn parse_bedrock_event(
    val: &Value,
    provider_id: &ProviderId,
    message_started: &mut bool,
) -> Vec<Result<StreamEvent, ProviderError>> {
    let mut events = Vec::new();

    // Bedrock Converse streaming events come in several shapes.
    // We check for the most common ones:

    // messageStart
    if let Some(msg_start) = val.get("messageStart") {
        let role = msg_start
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("assistant");
        let _ = role;
        if !*message_started {
            events.push(Ok(StreamEvent::MessageStart {
                id: uuid::Uuid::new_v4().to_string(),
                model: String::new(),
                usage: UsageInfo::default(),
            }));
            *message_started = true;
        }
        return events;
    }

    // contentBlockStart
    if let Some(cb_start) = val.get("contentBlockStart") {
        let index = cb_start
            .get("contentBlockIndex")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        if !*message_started {
            events.push(Ok(StreamEvent::MessageStart {
                id: uuid::Uuid::new_v4().to_string(),
                model: String::new(),
                usage: UsageInfo::default(),
            }));
            *message_started = true;
        }
        let start_val = cb_start.get("start");
        if let Some(tool_use) = start_val.and_then(|s| s.get("toolUse")) {
            let id = tool_use
                .get("toolUseId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = tool_use
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            events.push(Ok(StreamEvent::ContentBlockStart {
                index,
                content_block: ContentBlock::ToolUse {
                    id,
                    name,
                    input: json!({}),
                },
            }));
        } else {
            events.push(Ok(StreamEvent::ContentBlockStart {
                index,
                content_block: ContentBlock::Text {
                    text: String::new(),
                },
            }));
        }
        return events;
    }

    // contentBlockDelta
    if let Some(cb_delta) = val.get("contentBlockDelta") {
        let index = cb_delta
            .get("contentBlockIndex")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        if !*message_started {
            events.push(Ok(StreamEvent::MessageStart {
                id: uuid::Uuid::new_v4().to_string(),
                model: String::new(),
                usage: UsageInfo::default(),
            }));
            events.push(Ok(StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlock::Text {
                    text: String::new(),
                },
            }));
            *message_started = true;
        }
        if let Some(delta) = cb_delta.get("delta") {
            if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    events.push(Ok(StreamEvent::TextDelta {
                        index,
                        text: text.to_string(),
                    }));
                }
            } else if let Some(json_frag) = delta
                .get("toolUse")
                .and_then(|tu| tu.get("input"))
                .and_then(|v| v.as_str())
            {
                if !json_frag.is_empty() {
                    events.push(Ok(StreamEvent::InputJsonDelta {
                        index,
                        partial_json: json_frag.to_string(),
                    }));
                }
            }
        }
        return events;
    }

    // contentBlockStop
    if let Some(cb_stop) = val.get("contentBlockStop") {
        let index = cb_stop
            .get("contentBlockIndex")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        events.push(Ok(StreamEvent::ContentBlockStop { index }));
        return events;
    }

    // messageStop
    if let Some(msg_stop) = val.get("messageStop") {
        let stop_reason_str = msg_stop
            .get("stopReason")
            .and_then(|v| v.as_str())
            .unwrap_or("end_turn");
        let stop_reason = match stop_reason_str {
            "end_turn" => StopReason::EndTurn,
            "max_tokens" => StopReason::MaxTokens,
            "tool_use" => StopReason::ToolUse,
            "stop_sequence" => StopReason::StopSequence,
            other => StopReason::Other(other.to_string()),
        };
        events.push(Ok(StreamEvent::MessageDelta {
            stop_reason: Some(stop_reason),
            usage: None,
        }));
        events.push(Ok(StreamEvent::MessageStop));
        return events;
    }

    // metadata (usage)
    if let Some(metadata) = val.get("metadata") {
        if let Some(usage_val) = metadata.get("usage") {
            let usage = UsageInfo {
                input_tokens: usage_val
                    .get("inputTokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                output_tokens: usage_val
                    .get("outputTokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            };
            events.push(Ok(StreamEvent::MessageDelta {
                stop_reason: None,
                usage: Some(usage),
            }));
        }
        return events;
    }

    // internalServerException / throttlingException
    if let Some(err) = val
        .get("internalServerException")
        .or_else(|| val.get("throttlingException"))
        .or_else(|| val.get("modelStreamErrorException"))
        .or_else(|| val.get("validationException"))
    {
        let message = err
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown Bedrock error")
            .to_string();
        events.push(Err(ProviderError::StreamError {
            provider: provider_id.clone(),
            message,
            partial_response: None,
        }));
    }

    events
}
