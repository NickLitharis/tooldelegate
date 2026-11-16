//! Trusted connector for GPT-6 Luna through the ChatGPT Codex Responses endpoint.
//!
//! This is host code outside the sandbox. It reads one harness request on stdin and
//! writes one `Reply` on stdout. Credentials are never placed in argv, replies or
//! traces. Usage comes from the provider's own `response.usage`; it is never estimated.
//!
//! The endpoint and credential broker are environment-specific. `LUNA_ACCESS_TOKEN`
//! and `LUNA_ACCOUNT_ID` may be supplied instead of the broker for operator use and
//! tests. `LUNA_BASE_URL` exists so tests can point at a local server.

use crate::model_adapter::{Reply, Usage};
use serde_json::{Map, Value, json};
use std::{
    env,
    error::Error,
    fs,
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 64 * 1024;
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
const DEFAULT_TIMEOUT_SECONDS: f64 = 85.0;
const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

fn err(message: impl Into<String>) -> Box<dyn Error> {
    message.into().into()
}

/// Entry point for the `luna-connector` subcommand. A rejection is reported as a
/// bounded JSON object on stdout; the nonzero exit keeps the harness from treating
/// any part of it as a model reply.
pub fn serve() -> Result<(), Box<dyn Error>> {
    match exchange() {
        Ok(reply) => {
            println!("{}", serde_json::to_string(&reply)?);
            Ok(())
        }
        Err(error) => {
            println!("{}", json!({"error": error.to_string()}));
            Err(err("connector rejected the provider exchange; see stdout"))
        }
    }
}

fn exchange() -> Result<Reply, Box<dyn Error>> {
    let mut stdin = std::io::stdin().take(MAX_REQUEST_BYTES as u64 + 1);
    let mut raw = String::new();
    stdin.read_to_string(&mut raw)?;
    if raw.len() > MAX_REQUEST_BYTES {
        return Err(err("harness request exceeds connector budget"));
    }
    let request: Value =
        serde_json::from_str(&raw).map_err(|error| err(format!("invalid request: {error}")))?;
    let model = request["model"]
        .as_str()
        .filter(|model| !model.is_empty())
        .ok_or_else(|| err("request model missing"))?
        .to_owned();
    let decoding = request["decoding"]
        .as_object()
        .ok_or_else(|| err("request decoding must be an object"))?;
    let remaining = request["remaining_wall_seconds"]
        .as_f64()
        .filter(|seconds| *seconds > 0.0)
        .ok_or_else(|| err("remaining_wall_seconds missing"))?;
    // The harness kills the connector at the smaller of 90 seconds and the remaining
    // wall budget. Finish earlier so a timeout is reported as JSON instead of an
    // unexplained empty stdout.
    let timeout = (remaining - 5.0).clamp(1.0, DEFAULT_TIMEOUT_SECONDS);

    let (instructions, input) = map_messages(&request["messages"])?;
    let schema = request
        .get("schema")
        .ok_or_else(|| err("request schema missing"))?;
    let schema_text = serde_json::to_string(schema)?;
    let instructions = Some(match instructions {
        Some(system) => format!("{system}\n\nAction schema (JSON):\n{schema_text}"),
        None => format!("Action schema (JSON):\n{schema_text}"),
    });
    let body = provider_request(&model, instructions, input, decoding)?;
    let bytes = serde_json::to_vec(&body)?;
    let (access, account) = credential()?;
    let raw_response = post(&access, &account, &bytes, timeout)?;
    let (text, response) = parse_response(&raw_response, &model)?;
    let usage = usage(&response)?;
    let finish_reason = finish_reason(&response)?;
    Ok(Reply {
        model,
        decoding: Value::Object(decoding.clone()),
        action: text,
        usage,
        finish_reason,
        provider_response: bounded(response),
        provider_request: body,
    })
}

fn map_messages(messages: &Value) -> Result<(Option<String>, Vec<Value>), Box<dyn Error>> {
    let messages = messages
        .as_array()
        .ok_or_else(|| err("request messages must be an array"))?;
    let mut instructions = None;
    let mut input = Vec::new();
    for message in messages {
        let role = message["role"]
            .as_str()
            .ok_or_else(|| err("message role missing"))?;
        let content = &message["content"];
        let text = match content {
            Value::String(text) => text.clone(),
            Value::Object(_) => serde_json::to_string(content)?,
            _ => return Err(err(format!("message role {role} has unsupported content"))),
        };
        match role {
            "system" => {
                if instructions.is_some() {
                    return Err(err("only one system message is supported"));
                }
                instructions = Some(text);
            }
            "user" | "tool" => input.push(json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": text}]
            })),
            "assistant" => input.push(json!({
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": text}]
            })),
            other => return Err(err(format!("unsupported message role {other}"))),
        }
    }
    Ok((instructions, input))
}

fn provider_request(
    model: &str,
    instructions: Option<String>,
    input: Vec<Value>,
    decoding: &Map<String, Value>,
) -> Result<Value, Box<dyn Error>> {
    let mut body = json!({
        "model": model,
        "input": input,
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "store": false,
        "stream": true
    });
    if let Some(instructions) = instructions {
        body["instructions"] = Value::String(instructions);
    }
    // Never silently drop a decoding field: an unsupported field is a configuration
    // error, because the connector could not honor the recorded protocol.
    for (key, value) in decoding {
        match key.as_str() {
            "temperature" => body["temperature"] = value.clone(),
            "top_p" => body["top_p"] = value.clone(),
            "max_output_tokens" => body["max_output_tokens"] = value.clone(),
            "reasoning_effort" => {
                body["reasoning"] = json!({"effort": value, "summary": "auto"});
                body["include"] = json!(["reasoning.encrypted_content"]);
            }
            other => return Err(err(format!("unsupported decoding field {other}"))),
        }
    }
    Ok(body)
}

fn opencode_program() -> Option<PathBuf> {
    let name = "opencode";
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // Background jobs do not always inherit the interactive PATH.
    let home = env::var_os("HOME")?;
    [
        ".npm-global/bin/opencode",
        ".local/bin/opencode",
        ".bun/bin/opencode",
    ]
    .iter()
    .map(|relative| PathBuf::from(&home).join(relative))
    .find(|candidate| candidate.is_file())
}

fn credential() -> Result<(String, String), Box<dyn Error>> {
    if let Ok(access) = env::var("LUNA_ACCESS_TOKEN") {
        let account = env::var("LUNA_ACCOUNT_ID")
            .map_err(|_| err("LUNA_ACCOUNT_ID is required with LUNA_ACCESS_TOKEN"))?;
        if access.is_empty() || account.is_empty() {
            return Err(err("provided credential is empty"));
        }
        return Ok((access, account));
    }
    let program = opencode_program()
        .ok_or_else(|| err("credential broker unavailable: opencode not found"))?;
    let output = Command::new(program)
        .args(["api", "get", "/api/credential"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| err(format!("credential broker unavailable: {error}")))?;
    if !output.status.success() || output.stdout.len() > MAX_RESPONSE_BYTES {
        return Err(err("credential broker failed or exceeded response budget"));
    }
    let listing: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| err(format!("credential broker returned invalid JSON: {error}")))?;
    let entry = listing["data"]
        .as_array()
        .and_then(|entries| {
            entries.iter().find(|entry| {
                entry["integrationID"] == "openai"
                    && entry["active"] == true
                    && entry["value"]["type"] == "oauth"
            })
        })
        .ok_or_else(|| err("no active OpenAI OAuth credential"))?;
    let access = entry["value"]["access"]
        .as_str()
        .ok_or_else(|| err("OpenAI OAuth access token missing"))?
        .to_owned();
    let account = entry["value"]["metadata"]["accountID"]
        .as_str()
        .ok_or_else(|| err("OpenAI account id missing"))?
        .to_owned();
    let expires = entry["value"]["expires"].as_f64().unwrap_or(0.0) / 1000.0;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| err(error.to_string()))?
        .as_secs_f64();
    if expires <= now {
        return Err(err(
            "OpenAI OAuth credential is expired; refresh it in opencode",
        ));
    }
    Ok((access, account))
}

fn post(access: &str, account: &str, body: &[u8], timeout: f64) -> Result<Vec<u8>, Box<dyn Error>> {
    let directory = private_directory()?;
    let header_path = directory.join("authorization");
    let mut header = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&header_path)?;
    header.write_all(
        format!("Authorization: Bearer {access}\nchatgpt-account-id: {account}\n").as_bytes(),
    )?;
    header.sync_all()?;
    drop(header);

    let base = env::var("LUNA_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_owned());
    let url = format!("{}/responses", base.trim_end_matches('/'));
    let mut child = Command::new("curl")
        .args(["--silent", "--show-error", "--request", "POST"])
        .args(["--max-time", &format!("{timeout:.3}")])
        .args([
            "--header",
            &format!("@{}", header_path.display()),
            "--header",
            "Content-Type: application/json",
            "--header",
            "Accept: text/event-stream",
            "--header",
            "OpenAI-Beta: responses=experimental",
            "--header",
            "originator: opencode",
            "x-codex-beta-features: remote_compaction_v2",
            "--data-binary",
            "@-",
        ])
        .arg(&url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| err(format!("curl unavailable: {error}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| err("curl stdin missing"))?;
    let body = body.to_vec();
    let writer = thread::spawn(move || stdin.write_all(&body));
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| err("curl stdout missing"))?;
    let reader = thread::spawn(move || crate::executor::drain(stdout, MAX_RESPONSE_BYTES));
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| err("curl stderr missing"))?;
    let errors = thread::spawn(move || crate::executor::drain(stderr, MAX_STDERR_BYTES));
    let status = child.wait()?;
    let written = writer.join().map_err(|_| err("curl writer panicked"))?;
    let (count, raw) = reader.join().map_err(|_| err("curl reader panicked"))??;
    let (_, diagnostics) = errors
        .join()
        .map_err(|_| err("curl stderr reader panicked"))??;
    let _ = fs::remove_dir_all(&directory);
    if written.is_err() {
        return Err(err("failed to deliver provider request"));
    }
    if !status.success() {
        return Err(err(format!(
            "curl failed: {} {}",
            status,
            String::from_utf8_lossy(&diagnostics).trim()
        )));
    }
    if count > MAX_RESPONSE_BYTES as u64 {
        return Err(err("provider response exceeded connector budget"));
    }
    Ok(raw)
}

fn private_directory() -> Result<PathBuf, Box<dyn Error>> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| err(error.to_string()))?
        .as_nanos();
    let directory = env::temp_dir().join(format!("luna-connector-{}-{nonce}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&directory)?;
    Ok(directory)
}

fn parse_response(raw: &[u8], requested: &str) -> Result<(String, Value), Box<dyn Error>> {
    let raw = std::str::from_utf8(raw).map_err(|_| err("provider response is not UTF-8"))?;
    let mut events = 0usize;
    let mut text = String::new();
    let mut output_text = String::new();
    let mut settled: Option<Value> = None;
    let mut failure: Option<String> = None;
    for line in raw.lines() {
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let event: Value = serde_json::from_str(payload)
            .map_err(|_| err("provider emitted a malformed SSE event"))?;
        events += 1;
        match event["type"].as_str() {
            Some("response.output_text.delta") => {
                if let Some(delta) = event["delta"].as_str() {
                    text.push_str(delta);
                }
            }
            Some("response.output_text.done") => {
                if let Some(done) = event["text"].as_str() {
                    output_text.push_str(done);
                }
            }
            Some("response.completed") | Some("response.incomplete") => {
                settled = Some(event["response"].clone());
            }
            Some("response.failed") => {
                failure = Some(event["response"]["error"].to_string());
            }
            Some("error") => failure = Some(event.to_string()),
            _ => {}
        }
    }
    if events == 0 {
        // A non-stream body is an HTTP-level provider error even when curl is happy.
        let error: Value = serde_json::from_str(raw.trim())
            .map_err(|_| err("provider returned neither SSE events nor an error body"))?;
        let kind = error["error"]["type"]
            .as_str()
            .or_else(|| error["type"].as_str())
            .unwrap_or("provider_error");
        let message = error["error"]["message"]
            .as_str()
            .or_else(|| error["message"].as_str())
            .unwrap_or("no message");
        let resets = error["error"]["resets_at"]
            .as_i64()
            .map(|value| format!(" resets_at={value}"))
            .unwrap_or_default();
        let excerpt: String = raw.chars().take(512).collect();
        return Err(err(format!(
            "provider rejected the request: {kind}: {message}{resets} body={excerpt}"
        )));
    }
    if let Some(failure) = failure {
        return Err(err(format!("provider reported failure: {failure}")));
    }
    let response = settled.ok_or_else(|| err("provider stream ended without a final response"))?;
    let resolved = response["model"]
        .as_str()
        .ok_or_else(|| err("provider did not report the resolved model"))?;
    if resolved != requested {
        return Err(err(format!(
            "resolved model {resolved} does not match the pinned request {requested}"
        )));
    }
    if text.is_empty() {
        text = output_text;
    }
    if text.is_empty() {
        for item in response["output"].as_array().into_iter().flatten() {
            for part in item["content"].as_array().into_iter().flatten() {
                if part["type"] == "output_text"
                    && let Some(value) = part["text"].as_str()
                {
                    text.push_str(value);
                }
            }
        }
    }
    if text.is_empty() {
        return Err(err("provider returned no action text"));
    }
    Ok((text, response))
}

fn usage(response: &Value) -> Result<Usage, Box<dyn Error>> {
    let usage = &response["usage"];
    let input_tokens = usage["input_tokens"]
        .as_u64()
        .ok_or_else(|| err("provider did not report input_tokens"))?;
    let output_tokens = usage["output_tokens"]
        .as_u64()
        .ok_or_else(|| err("provider did not report output_tokens"))?;
    let cached = usage["input_tokens_details"]["cached_tokens"].as_u64();
    Ok(Usage {
        input_tokens,
        output_tokens,
        cached_input_tokens: cached,
        schema_tokens: None,
        feedback_input_tokens: None,
        verification_input_tokens: None,
        source: "api_reported".to_owned(),
    })
}

fn finish_reason(response: &Value) -> Result<String, Box<dyn Error>> {
    match response["status"].as_str() {
        Some("completed") => Ok("stop".to_owned()),
        Some("incomplete") => Ok(match response["incomplete_details"]["reason"].as_str() {
            Some("max_output_tokens") => "length".to_owned(),
            _ => "incomplete".to_owned(),
        }),
        Some(status) => Err(err(format!("provider status {status} is not completion"))),
        None => Err(err("provider did not report response status")),
    }
}

/// Keeps the raw envelope bounded without discarding the fields needed to audit usage.
fn bounded(response: Value) -> Value {
    let mut value = response;
    let oversized = serde_json::to_vec(&value)
        .map(|bytes| bytes.len() > MAX_PAYLOAD_BYTES)
        .unwrap_or(true);
    if oversized {
        value = json!({
            "truncated": true,
            "id": value["id"],
            "model": value["model"],
            "status": value["status"],
            "usage": value["usage"],
            "output_items": value["output"].as_array().map(Vec::len)
        });
    }
    value
}
