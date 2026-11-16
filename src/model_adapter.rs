//! A trusted adapter, not model-generated code, translates a provider API to this contract.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    error::Error,
    io::Write,
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub schema_tokens: Option<u64>,
    /// Feedback repeated in this call's context, not merely the latest message.
    pub feedback_input_tokens: Option<u64>,
    pub verification_input_tokens: Option<u64>,
    pub source: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub model: String,
    pub decoding: Value,
    /// Original structured action text. Parsing errors remain visible, never repaired.
    pub action: String,
    pub usage: Usage,
    pub finish_reason: String,
    /// Full provider response, minus credentials. Must not be echoed to the model.
    pub provider_response: Value,
    pub provider_request: Value,
}

pub trait ModelAdapter {
    fn respond(&mut self, request: &Value) -> Result<Reply, Box<dyn Error>>;
    fn synthetic(&self) -> bool;
    fn metadata(&self) -> Value;
}

pub struct Replay {
    replies: VecDeque<Reply>,
}
impl Replay {
    pub fn load(path: &Path) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            replies: std::fs::read_to_string(path)?
                .lines()
                .map(serde_json::from_str)
                .collect::<Result<_, _>>()?,
        })
    }
}
impl ModelAdapter for Replay {
    fn respond(&mut self, _: &Value) -> Result<Reply, Box<dyn Error>> {
        self.replies
            .pop_front()
            .ok_or_else(|| "replay exhausted".into())
    }
    fn synthetic(&self) -> bool {
        true
    }
    fn metadata(&self) -> Value {
        json!({"kind":"replay"})
    }
}

/// User-configured executable outside the sandbox. Never accept its path from model output.
#[derive(Clone)]
pub struct Connector {
    pub program: String,
    pub args: Vec<String>,
    pub timeout: Duration,
}
impl Connector {
    /// Builds a connector from trusted operator configuration. The program must be an
    /// absolute path; model output can never supply or alter it.
    pub fn from_config(config: &Value, timeout: Duration) -> Result<Self, Box<dyn Error>> {
        let program = config["program"]
            .as_str()
            .ok_or("connector program missing")?
            .to_owned();
        if !Path::new(&program).is_absolute() {
            return Err("connector program must be an absolute trusted path".into());
        }
        let args = config["args"]
            .as_array()
            .ok_or("connector args missing")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or("connector arguments must be strings")
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            program,
            args,
            timeout,
        })
    }
}
impl ModelAdapter for Connector {
    fn respond(&mut self, request: &Value) -> Result<Reply, Box<dyn Error>> {
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        let pid = child.id();
        let mut stdin = child.stdin.take().ok_or("adapter stdin missing")?;
        let bytes = serde_json::to_vec(request)?;
        let writer = thread::spawn(move || stdin.write_all(&bytes));
        let stdout = child.stdout.take().ok_or("adapter stdout missing")?;
        let reader = thread::spawn(move || crate::executor::drain(stdout, 4 * 1024 * 1024));
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if start.elapsed().as_secs_f64()
                > self
                    .timeout
                    .as_secs_f64()
                    .min(request["remaining_wall_seconds"].as_f64().unwrap_or(0.0))
            {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
                child.kill().ok();
                break child.wait()?;
            }
            thread::sleep(Duration::from_millis(5));
        };
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        let written = writer.join().map_err(|_| "adapter writer panicked")?;
        let (count, raw) = reader.join().map_err(|_| "adapter reader panicked")??;
        if !status.success() || count > 4 * 1024 * 1024 || written.is_err() {
            return Err(json!({"error":"adapter failed, timed out, or exceeded response budget; usage unknown","raw_adapter_response_bounded":String::from_utf8_lossy(&raw),"stdout_bytes":count,"truncated":count>raw.len() as u64}).to_string().into());
        }
        serde_json::from_slice(&raw).map_err(|error| {
            json!({"error":error.to_string(),"raw_adapter_response":String::from_utf8_lossy(&raw)})
                .to_string()
                .into()
        })
    }
    fn synthetic(&self) -> bool {
        false
    }
    fn metadata(&self) -> Value {
        json!({"kind":"trusted_connector","program":self.program,"args":self.args,"timeout_seconds":self.timeout.as_secs_f64()})
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Read {
        path: String,
        offset: usize,
        bytes: usize,
    },
    Put {
        path: String,
        content: String,
        append: bool,
    },
    Remove {
        path: String,
    },
    Mkdir {
        path: String,
    },
    Submit {
        commands: Option<Vec<Vec<String>>>,
    },
    Finish {},
}

pub fn schema(direct: bool) -> Value {
    let rule = "Return exactly one JSON object per turn with a type field at its top level. Do not wrap the action in another object or an array. Do not return prose, multiple objects, or any field not listed here.";
    if direct {
        json!({
            "response_rule": rule,
            "allowed_actions": [
                {"type":"read","path":"relative input file","offset":"integer byte offset >= 0","bytes":"positive integer byte count"},
                {"type":"put","path":"relative artifact path","content":"UTF-8 chunk","append":"boolean; false truncates"},
                {"type":"remove","path":"relative file"},
                {"type":"mkdir","path":"relative directory"},
                {"type":"submit","commands":null},
                {"type":"finish"}
            ],
            "examples": [
                {"type":"read","path":"input.csv","offset":0,"bytes":65536},
                {"type":"put","path":"output.csv","content":"row\n","append":false},
                {"type":"submit","commands":null},
                {"type":"finish"}
            ]
        })
    } else {
        json!({
            "response_rule": rule,
            "allowed_programs": crate::executor::ALLOWLIST,
            "allowed_actions": [
                {"type":"read","path":"relative input file","offset":"integer byte offset >= 0","bytes":"positive integer byte count"},
                {"type":"submit","commands":"nonempty list of argument arrays; executed sequentially, then arm feedback is returned. This object has no other fields."},
                {"type":"finish"}
            ],
            "examples": [
                {"type":"read","path":"input.csv","offset":0,"bytes":65536},
                {"type":"submit","commands":[["python3","-I","-c","print(1)"]]},
                {"type":"finish"}
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_actions_and_bounded_connector_transport() {
        assert!(serde_json::from_str::<Action>(r#"{"type":"finish","extra":1}"#).is_err());
        assert!(serde_json::from_str::<Action>(r#"{"type":"unknown"}"#).is_err());
        let mut connector = Connector {
            program: "/usr/bin/python3".into(),
            args: vec![
                "-I".into(),
                "-c".into(),
                "import sys;sys.stdin.read();print('malformed')".into(),
            ],
            timeout: Duration::from_secs(2),
        };
        let error = connector
            .respond(&json!({"remaining_wall_seconds":2}))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("raw_adapter_response"));
        assert!(error.contains("malformed"));
        connector.args = vec!["-I".into(), "-c".into(), "import time;time.sleep(5)".into()];
        connector.timeout = Duration::from_millis(50);
        let start = Instant::now();
        assert!(
            connector
                .respond(&json!({"remaining_wall_seconds":2}))
                .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn action_schema_examples_are_flat_and_parseable() {
        for direct in [true, false] {
            let schema = schema(direct);
            assert!(schema.get("actions").is_none());
            assert!(
                schema["response_rule"]
                    .as_str()
                    .unwrap()
                    .contains("Do not wrap")
            );
            for example in schema["examples"].as_array().unwrap() {
                serde_json::from_value::<Action>(example.clone())
                    .unwrap_or_else(|error| panic!("example {example} does not parse: {error}"));
            }
            for action in schema["allowed_actions"].as_array().unwrap() {
                assert!(action["type"].is_string());
                assert!(action.get("fields").is_none());
                assert!(action.get("allowlist").is_none());
            }
        }
        assert!(schema(false)["allowed_programs"].is_array());
        assert!(schema(true).get("allowed_programs").is_none());
    }
}
