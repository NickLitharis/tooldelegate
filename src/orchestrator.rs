use crate::{
    executor::{self, Limits},
    feedback::{self, Mode},
    metrics::{Metrics, Pricing},
    model_adapter::{self, Action, ModelAdapter},
    suite, verifier,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    error::Error,
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn environment() -> Value {
    use sha2::{Digest, Sha256};
    let mut versions = serde_json::Map::new();
    for program in [
        "bwrap", "prlimit", "python3", "sort", "mv", "mkdir", "cat", "sed",
    ] {
        let value = std::process::Command::new(format!("/usr/bin/{program}"))
            .arg("--version")
            .output()
            .ok()
            .filter(|r| r.status.success())
            .map(|r| String::from_utf8_lossy(&r.stdout).into_owned());
        versions.insert(program.into(), json!(value));
    }
    let kernel = std::process::Command::new("/usr/bin/uname")
        .arg("-srv")
        .output()
        .ok()
        .filter(|r| r.status.success())
        .map(|r| String::from_utf8_lossy(&r.stdout).into_owned());
    let binary = std::env::current_exe()
        .ok()
        .and_then(|p| fs::read(p).ok())
        .map(|data| format!("{:x}", Sha256::digest(data)));
    json!({"os":std::env::consts::OS,"architecture":std::env::consts::ARCH,"harness_version":env!("CARGO_PKG_VERSION"),"compiler":env!("VERIFIED_RUSTC"),"kernel":kernel,"cli_versions":versions,"binary_sha256":binary,"cargo_lock_sha256":format!("{:x}",Sha256::digest(include_bytes!("../Cargo.lock")))})
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Debug)]
pub enum Arm {
    A,
    B,
    C,
    D,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub model: String,
    pub repetition: u64,
    pub decoding: Value,
    pub max_calls: u64,
    pub max_retries: u64,
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
    pub max_context_tokens: u64,
    pub max_request_bytes: usize,
    pub max_commands_per_attempt: usize,
    pub task_wall_seconds: u64,
    pub feedback_bytes: usize,
    pub pricing: Option<Pricing>,
}

fn safe(root: &Path, name: &str) -> Result<PathBuf> {
    if name.is_empty()
        || !Path::new(name)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err("invalid relative path".into());
    }
    let mut path = root.to_owned();
    for part in Path::new(name).components() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() || (!m.is_dir() && !m.is_file()) => {
                return Err("unsafe path".into());
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(path)
}

fn event(file: &mut fs::File, value: Value) -> Result<()> {
    serde_json::to_writer(&mut *file, &value)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

pub(crate) fn validate(config: &Config) -> Result<()> {
    if config.model.is_empty()
        || !config.decoding.is_object()
        || config.max_calls == 0
        || config.task_wall_seconds == 0
        || config.feedback_bytes < 196
        || config.feedback_bytes > 65536
        || config.max_commands_per_attempt == 0
        || config.max_request_bytes == 0
        || config.max_context_tokens == 0
        || config.max_input_tokens == 0
        || config.max_output_tokens == 0
    {
        return Err("invalid run configuration".into());
    }
    if let Some(p) = &config.pricing
        && (p.currency.is_empty()
            || p.source.is_empty()
            || [
                p.input_per_million,
                p.cached_input_per_million,
                p.output_per_million,
            ]
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0))
    {
        return Err("invalid recorded pricing".into());
    }
    Ok(())
}

/// One run. Paths/config are trusted harness inputs; model actions never choose them.
pub fn run(
    task: &Path,
    root: &Path,
    arm: Arm,
    config: &Config,
    adapter: &mut dyn ModelAdapter,
) -> Result<Value> {
    let start = Instant::now();
    let started_unix_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos()
        .to_string();
    validate(config)?;
    let manifest: Value = serde_json::from_slice(&fs::read(task.join("manifest.json"))?)?;
    if serde_json::to_value(crate::filesystem_task::snapshot(&task.join("input"))?)?
        != manifest["input_hashes"]
    {
        return Err("input hash mismatch".into());
    }
    fs::create_dir(root)?;
    let work = executor::fresh_work(task, &root.join("work"))?;
    let mut trace = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(root.join("trace.jsonl"))?;
    let synthetic = adapter.synthetic();
    let schema = model_adapter::schema(arm == Arm::A, read_limit(config.feedback_bytes));
    let mut instruction = "Complete the task using only the supplied action interface. Read original inputs with read (byte offsets); results return UTF-8 text, or base64 bytes when the requested range is not valid UTF-8. Artifacts live outside context. Submit each complete attempt; at most max_retries additional submissions are allowed. Tool arms submit a complete ordered command batch; verification happens after that batch. Finish only after your final submission. No hidden oracle feedback is available. Resource limits and truncation are failures, not completion. Every response must be exactly one action JSON object.".to_owned();
    if arm == Arm::A {
        instruction.push_str(" Arm A has no command execution. Write the complete artifact into the work tree with put (append later chunks with append=true); submit only acknowledges delivery and creates no files. Finish only after the artifact is written and submitted.");
    }
    let mut messages = vec![
        json!({"role":"system","content":instruction}),
        json!({"role":"user","content":{"requirement":manifest["requirement"],"input_paths":manifest["input_hashes"].as_object().ok_or("missing input inventory")?.keys().collect::<Vec<_>>()}}),
    ];
    event(
        &mut trace,
        json!({"event":"start","started_unix_nanos":started_unix_nanos,"synthetic":synthetic,"arm":arm,"adapter":adapter.metadata(),"config":config,"task":manifest,"schema":schema,"schema_bytes":serde_json::to_vec(&schema)?.len(),"system":instruction,"environment":environment()}),
    )?;
    let mut metrics = Metrics::start();
    let mut termination = "call_limit".to_owned();
    let mut finished = false;
    let mut tool_index = 0;
    let mut dirty = false;
    for call in 0..config.max_calls {
        if start.elapsed().as_secs() >= config.task_wall_seconds {
            termination = "resource_limit".into();
            break;
        }
        let request = json!({"model":config.model,"decoding":config.decoding,"arm":arm,"schema":schema,"messages":messages,"limits":config,"remaining_wall_seconds":config.task_wall_seconds as f64-start.elapsed().as_secs_f64(),"synthetic":synthetic});
        if serde_json::to_vec(&request)?.len() > config.max_request_bytes {
            termination = "context_limit".into();
            break;
        }
        event(
            &mut trace,
            json!({"event":"model_request","call":call,"request":request}),
        )?;
        let call_start = Instant::now();
        metrics.calls += 1;
        let reply = match adapter.respond(&request) {
            Ok(reply) => reply,
            Err(error) => {
                metrics.usage_complete = false;
                event(
                    &mut trace,
                    json!({"event":"adapter_error","call":call,"error":error.to_string(),"latency_seconds":call_start.elapsed().as_secs_f64()}),
                )?;
                termination = "adapter_error".into();
                break;
            }
        };
        event(
            &mut trace,
            json!({"event":"model_response","call":call,"reply":reply,"latency_seconds":call_start.elapsed().as_secs_f64()}),
        )?;
        if metrics.add(&reply.usage).is_err() {
            metrics.usage_complete = false;
            termination = "usage_error".into();
            break;
        }
        if reply.model != config.model || reply.decoding != config.decoding {
            termination = "model_configuration_mismatch".into();
            break;
        }
        if !synthetic && reply.usage.source != "api_reported" {
            metrics.usage_complete = false;
            termination = "usage_error".into();
            break;
        }
        if metrics.input_tokens > config.max_input_tokens
            || metrics.output_tokens > config.max_output_tokens
            || reply.usage.input_tokens + reply.usage.output_tokens > config.max_context_tokens
            || start.elapsed().as_secs() >= config.task_wall_seconds
            || reply.finish_reason != "stop"
        {
            termination = "resource_limit".into();
            break;
        }
        let history=match serde_json::from_str::<Action>(&reply.action) {
            Ok(Action::Put {path,content,append})=>json!({"type":"put","path":path,"append":append,"content_omitted":true,"bytes":content.len()}).to_string(),
            _=>reply.action.clone(),
        };
        messages.push(json!({"role":"assistant","content":history}));
        let mut response = match serde_json::from_str::<Action>(&reply.action) {
            Ok(Action::Finish {}) => {
                if metrics.attempts == 0 || dirty {
                    json!({"ok":false,"code":"SUBMISSION_REQUIRED"}).to_string()
                } else {
                    finished = true;
                    termination = "finished".into();
                    break;
                }
            }
            Ok(action) => {
                let submitting = matches!(action, Action::Submit { .. });
                if matches!(
                    action,
                    Action::Put { .. } | Action::Remove { .. } | Action::Mkdir { .. }
                ) {
                    dirty = true;
                }
                match apply(
                    action,
                    task,
                    &work,
                    root,
                    arm,
                    config,
                    &mut metrics,
                    &mut tool_index,
                    start,
                    &mut trace,
                ) {
                    Ok(response) => {
                        if submitting {
                            dirty = false;
                        }
                        response
                    }
                    Err(error) => {
                        metrics.tool_errors += 1;
                        event(
                            &mut trace,
                            json!({"event":"action_error","call":call,"error":error.to_string()}),
                        )?;
                        json!({"ok":false,"code":"ACTION_REJECTED","error":error.to_string()})
                            .to_string()
                    }
                }
            }
            Err(error) => {
                event(
                    &mut trace,
                    json!({"event":"malformed_action","call":call,"error":error.to_string()}),
                )?;
                json!({"ok":false,"code":"MALFORMED_ACTION","error":error.to_string()}).to_string()
            }
        };
        if response.len() > config.feedback_bytes {
            let mut end = config.feedback_bytes;
            while !response.is_char_boundary(end) {
                end -= 1;
            }
            response.truncate(end);
        }
        metrics.returned_feedback_bytes += response.len() as u64;
        event(
            &mut trace,
            json!({"event":"feedback","call":call,"bytes":response.len(),"content":response,"tokens":null}),
        )?;
        messages.push(json!({"role":"tool","content":response}));
    }
    let oracle_start = Instant::now();
    let safe_final = executor::tree_budget(&work, &Limits::default()).is_ok();
    let oracle = if safe_final {
        suite::score_isolated(task, &work)
    } else {
        Err("unsafe or oversized final tree".into())
    };
    let (oracle_pass, oracle_error) = match oracle {
        Ok(pass) => (pass, None),
        Err(error) => (false, Some(error.to_string())),
    };
    let oracle_elapsed = oracle_start.elapsed().as_secs_f64();
    let final_hashes = if safe_final {
        crate::filesystem_task::snapshot(&work).ok()
    } else {
        None
    };
    metrics.end_to_end_seconds = start.elapsed().as_secs_f64();
    metrics.price(config.pricing.as_ref());
    if termination == "model_configuration_mismatch" {
        metrics.monetary_cost = None;
    }
    let false_positive = if oracle_error.is_none() {
        metrics.last_verifier_ok.map(|pass| pass && !oracle_pass)
    } else {
        None
    };
    let false_negative = if oracle_error.is_none() {
        metrics.last_verifier_ok.map(|pass| !pass && oracle_pass)
    } else {
        None
    };
    let summary = json!({"event":"summary","run_id":format!("{}-{}",started_unix_nanos,std::process::id()),"started_unix_nanos":started_unix_nanos,"repetition":config.repetition,"synthetic":synthetic,"arm":arm,"task_id":manifest["id"],"task":manifest,"config":config,"model":config.model,"decoding":config.decoding,"pricing":config.pricing,"termination":termination,"complete":finished,"oracle_pass":if oracle_error.is_none(){Some(oracle_pass)}else{None},"success":finished&&oracle_pass,"oracle_error":oracle_error,"oracle_elapsed_seconds":oracle_elapsed,"verifier_false_positive":false_positive,"verifier_false_negative":false_negative,"metrics":metrics,"final_hashes":final_hashes,"input_hashes":manifest["input_hashes"]});
    event(&mut trace, summary.clone())?;
    fs::write(
        root.join("summary.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    fs::write(
        root.join("record.jsonl"),
        format!("{}\n", serde_json::to_string(&summary)?),
    )?;
    Ok(summary)
}

/// Maximum bytes a single read may return while the encoded response fits the
/// feedback budget. UTF-8 text expands once; base64 bytes expand by 4/3.
pub fn read_limit(feedback_bytes: usize) -> usize {
    feedback_bytes.saturating_sub(96) * 3 / 4
}

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied();
        let third = chunk.get(2).copied();
        out.push(ALPHABET[(first >> 2) as usize] as char);
        out.push(ALPHABET[(((first & 0b11) << 4) | (second.unwrap_or(0) >> 4)) as usize] as char);
        out.push(match second {
            Some(second) => {
                ALPHABET[(((second & 0b1111) << 2) | (third.unwrap_or(0) >> 6)) as usize] as char
            }
            None => '=',
        });
        out.push(match third {
            Some(third) => ALPHABET[(third & 0b111111) as usize] as char,
            None => '=',
        });
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn apply(
    action: Action,
    task: &Path,
    work: &Path,
    root: &Path,
    arm: Arm,
    config: &Config,
    metrics: &mut Metrics,
    tool_index: &mut u64,
    start: Instant,
    trace: &mut fs::File,
) -> Result<String> {
    let ack = || json!({"ok":true,"code":"STORED"}).to_string();
    match action {
        Action::Read {
            path,
            offset,
            bytes,
        } => {
            if bytes == 0 || bytes > read_limit(config.feedback_bytes) {
                return Err("read exceeds byte budget".into());
            }
            let mut file = fs::File::open(safe(&task.join("input"), &path)?)?;
            if offset as u64 > file.metadata()?.len() {
                return Err("read offset exceeds file size".into());
            }
            file.seek(SeekFrom::Start(offset as u64))?;
            let mut data = vec![0; bytes];
            let length = file.read(&mut data)?;
            data.truncate(length);
            let eof = offset as u64 + length as u64 >= file.metadata()?.len();
            // JSON byte arrays expand context roughly fourfold. Return UTF-8 text when
            // possible and base64 bytes otherwise, so direct generation is not taxed by
            // a representation unrelated to the transformation.
            match std::str::from_utf8(&data) {
                Ok(text) => Ok(
                    json!({"encoding":"utf-8","data":text,"offset":offset,"eof":eof}).to_string(),
                ),
                Err(_) => Ok(
                    json!({"encoding":"base64","data":base64(&data),"offset":offset,"eof":eof})
                        .to_string(),
                ),
            }
        }
        Action::Put {
            path,
            content,
            append,
        } if arm == Arm::A => {
            let path = safe(work, &path)?;
            let size = if append {
                fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
            } else {
                0
            };
            if size.saturating_add(content.len() as u64) > Limits::default().file_bytes {
                return Err("artifact exceeds file budget".into());
            }
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .append(append)
                .truncate(!append)
                .open(path)?
                .write_all(content.as_bytes())?;
            executor::tree_budget(work, &Limits::default())?;
            Ok(ack())
        }
        Action::Remove { path } if arm == Arm::A => {
            fs::remove_file(safe(work, &path)?)?;
            Ok(ack())
        }
        Action::Mkdir { path } if arm == Arm::A => {
            fs::create_dir(safe(work, &path)?)?;
            Ok(ack())
        }
        Action::Submit { commands } => {
            if metrics.attempts > config.max_retries {
                return Err("retry budget exhausted".into());
            }
            if arm == Arm::A && commands.is_some() {
                return Err("direct generation cannot execute commands".into());
            }
            if arm != Arm::A
                && commands
                    .as_ref()
                    .is_none_or(|c| c.is_empty() || c.len() > config.max_commands_per_attempt)
            {
                return Err("invalid command batch".into());
            }
            metrics.attempts += 1;
            metrics.retries = metrics.attempts - 1;
            if metrics.attempts > 1 && metrics.last_verifier_ok == Some(false) {
                metrics.retries_after_negative_verification += 1;
            }
            if arm == Arm::A {
                return Ok(json!({"ok":true,"code":"SUBMITTED"}).to_string());
            }
            let mut raw = Vec::new();
            for command in commands.unwrap() {
                let remaining = config.task_wall_seconds as f64 - start.elapsed().as_secs_f64();
                if remaining <= 0.0 {
                    return Err("task wall budget exceeded".into());
                }
                let limits = Limits {
                    wall: Duration::from_secs_f64(remaining.min(60.0)),
                    feedback_bytes: config.feedback_bytes,
                    ..Limits::default()
                };
                *tool_index += 1;
                event(
                    trace,
                    json!({"event":"tool_request","index":tool_index,"args":command}),
                )?;
                let execution = executor::execute(work, &command, &limits)?;
                executor::save_trace(&execution, &root.join(format!("tool-{tool_index}.json")))?;
                metrics.tool_output_bytes += execution.stdout_bytes + execution.stderr_bytes;
                let bad = execution.status != Some(0)
                    || execution.timed_out
                    || execution.resource_limited;
                if bad {
                    metrics.tool_errors += 1;
                }
                if arm == Arm::B {
                    raw.extend(feedback::render(
                        Mode::RawTool,
                        &execution,
                        None,
                        config.feedback_bytes,
                    )?);
                    raw.truncate(config.feedback_bytes);
                }
                if bad {
                    break;
                }
            }
            if arm == Arm::B {
                return Ok(String::from_utf8_lossy(&raw).into_owned());
            }
            let verification = verifier::verify_isolated(task, work);
            metrics.last_verifier_ok =
                if verification.code == crate::certificates::Code::VerifierError {
                    None
                } else {
                    Some(verification.code == crate::certificates::Code::Pass)
                };
            metrics.verifier_elapsed_seconds += verification.elapsed_seconds;
            if let Some(cpu) = verification.cpu_seconds {
                metrics.verifier_cpu_seconds += cpu;
            } else {
                metrics.verifier_metrics_complete = false;
            }
            event(
                trace,
                json!({"event":"verification","record":verification.record()}),
            )?;
            Ok(if arm == Arm::C {
                verification.raw()
            } else {
                verification.certificate()
            })
        }
        _ => Err("action unavailable in this arm".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{base64, read_limit};

    #[test]
    fn read_budget_leaves_room_for_base64_expansion() {
        assert_eq!(read_limit(65536), 49080);
        assert_eq!(read_limit(196), 75);
        let bytes = vec![0xff; read_limit(65536)];
        assert!(base64(&bytes).len() <= 65536 - 96 + 4);
    }

    #[test]
    fn base64_matches_standard_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }
}
