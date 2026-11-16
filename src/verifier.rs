//! Feedback verifier: does not call the reference transformation or hidden oracle.
use crate::{
    certificates::{Code, certificate},
    filesystem_task::snapshot,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, error::Error, fs, path::Path, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub struct Verification {
    pub code: Code,
    pub details: Value,
    pub elapsed_seconds: f64,
    pub cpu_seconds: Option<f64>,
    /// Process-wide high-water mark; not verifier-attributable memory usage.
    pub process_peak_rss_bytes: Option<u64>,
}

impl Verification {
    pub fn certificate(&self) -> String {
        certificate(self.code, None)
    }
    pub fn raw(&self) -> String {
        json!({"ok":self.code == Code::Pass,"code":self.code.name(),"details":self.details})
            .to_string()
    }
    pub fn record(&self) -> Value {
        json!({"code":self.code.name(),"details":self.details,"elapsed_seconds":self.elapsed_seconds,"cpu_seconds":self.cpu_seconds,"process_peak_rss_bytes":self.process_peak_rss_bytes})
    }
}

fn cpu_time() -> f64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time);
    }
    time.tv_sec as f64 + time.tv_nsec as f64 / 1e9
}

fn read_csv(path: &Path, header: &[&str]) -> Result<Vec<Vec<String>>> {
    let mut reader = csv::Reader::from_path(path)?;
    if reader.headers()?.iter().collect::<Vec<_>>() != header {
        return Err("schema mismatch".into());
    }
    reader
        .records()
        .map(|r| Ok(r?.iter().map(str::to_owned).collect()))
        .collect()
}

#[derive(Ord, PartialOrd, Eq, PartialEq)]
enum Field {
    Numeric(i64),
    Text(String),
}

fn sort_check(input: &Path, after: &Path, variant: u64) -> Result<Code> {
    let original = read_csv(
        &input.join("input.csv"),
        &["id", "value", "secondary", "payload"],
    )?;
    let actual = read_csv(
        &after.join("output.csv"),
        &["id", "value", "secondary", "payload"],
    )?;
    let mut keyed = Vec::new();
    for row in original {
        let null = row[1].is_empty();
        let primary = match variant {
            0 => Field::Numeric(if null { 0 } else { row[1].parse()? }),
            1 => Field::Text(row[1].clone()),
            2 => Field::Text(row[1].to_ascii_lowercase()),
            3 => Field::Numeric(if null {
                0
            } else if row[1].contains('T') {
                chrono::DateTime::parse_from_rfc3339(&row[1])?.timestamp()
            } else {
                chrono::NaiveDateTime::parse_from_str(&row[1], "%Y/%m/%d %H:%M:%S")?
                    .and_utc()
                    .timestamp()
            }),
            _ => return Err("unknown sorting variant".into()),
        };
        keyed.push((
            (
                null,
                primary,
                if variant == 2 {
                    -row[2].parse::<i64>()?
                } else {
                    0
                },
            ),
            row,
        ));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(if keyed.into_iter().map(|(_, r)| r).eq(actual) {
        Code::Pass
    } else {
        Code::OrderError
    })
}

/// SQLite aggregates are independent of the oracle's Rust per-group reductions.
fn query_check(input: &Path, after: &Path, variant: u64) -> Result<Code> {
    let rows = read_csv(
        &input.join("input.csv"),
        &["id", "group", "value", "active"],
    )?;
    let mut database = Connection::open_in_memory()?;
    database.execute_batch("CREATE TABLE data (g TEXT, v INTEGER, a INTEGER);")?;
    let transaction = database.transaction()?;
    {
        let mut insert = transaction.prepare("INSERT INTO data VALUES (?1,?2,?3)")?;
        for row in rows {
            let value = if row[2].is_empty() {
                None
            } else {
                Some(row[2].parse::<i64>()?)
            };
            insert.execute(params![row[1], value, row[3] == "true"])?;
        }
    }
    transaction.commit()?;
    let wanted = match variant {
        0 => {
            json!({"count":database.query_row("SELECT COUNT(*) FROM data WHERE g='A' AND a=1 AND v>50",[],|r|r.get::<_,i64>(0))?})
        }
        1 | 2 => {
            let sql = if variant == 1 {
                "SELECT g,COUNT(*),COALESCE(SUM(v),0),AVG(v) FROM data GROUP BY g"
            } else {
                "SELECT g,COUNT(*),COALESCE(SUM(v),0),AVG(v) FROM data WHERE a=1 GROUP BY g"
            };
            let mut statement = database.prepare(sql)?;
            let groups = statement
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Option<f64>>(3)?,
                    ))
                })?
                .map(|row| {
                    let (name, count, sum, average) = row?;
                    Ok((
                        name,
                        if variant == 1 {
                            json!({"count":count,"sum":sum})
                        } else {
                            json!({"sum":sum,"average":average})
                        },
                    ))
                })
                .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
            json!({"groups":groups})
        }
        3 => database.query_row(
            "SELECT COUNT(DISTINCT v),COUNT(*)-COUNT(v) FROM data WHERE a=1 AND g!='C'",
            [],
            |r| Ok(json!({"distinct":r.get::<_,i64>(0)?,"null_count":r.get::<_,i64>(1)?})),
        )?,
        _ => return Err("unknown aggregation variant".into()),
    };
    fn same(a: &Value, b: &Value, average: bool) -> bool {
        match (a, b) {
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len()
                    && a.iter().all(|(key, value)| {
                        b.get(key)
                            .is_some_and(|actual| same(value, actual, key == "average"))
                    })
            }
            (Value::Number(a), Value::Number(b)) if average => {
                (a.as_f64().unwrap() - b.as_f64().unwrap()).abs() <= 1e-9
            }
            _ => a == b,
        }
    }
    let actual = serde_json::from_slice(&fs::read(after.join("output.json"))?)?;
    Ok(if same(&wanted, &actual, false) {
        Code::Pass
    } else {
        Code::ContentError
    })
}

fn check(task: &Path, after: &Path) -> Result<(Code, Value)> {
    if let Err(error) = crate::executor::tree_budget(after, &crate::executor::Limits::default()) {
        return Ok((Code::SafetyError, json!({"reason":error.to_string()})));
    }
    let manifest: Value = serde_json::from_slice(&fs::read(task.join("manifest.json"))?)?;
    let input = task.join("input");
    let mut expected = snapshot(&input)?;
    if json!(expected) != manifest["input_hashes"] {
        return Ok((
            Code::VerifierError,
            json!({"reason":"original input hash mismatch"}),
        ));
    }
    let mut actual = match snapshot(after) {
        Ok(tree) => tree,
        Err(error) => return Ok((Code::SafetyError, json!({"reason":error.to_string()}))),
    };
    let family = manifest["family"].as_str().ok_or("missing family")?;
    let variant = manifest["variant"].as_u64().ok_or("missing variant")?;
    if variant > 3 {
        return Err("unknown variant".into());
    }
    let outcome = manifest["expected_outcome"]
        .as_str()
        .ok_or("missing outcome")?;
    if outcome != "success" {
        let code = match outcome {
            "collision_no_change" => "COLLISION",
            "INVALID_NUMBER" => "INVALID_NUMBER",
            "INVALID_UTF8" => "INVALID_UTF8",
            "INVALID_TIMESTAMP" => "INVALID_TIMESTAMP",
            _ => return Err("unknown outcome".into()),
        };
        let report: Value = serde_json::from_slice(&fs::read(after.join("result.json"))?)?;
        actual.remove("result.json");
        let result = if expected != actual {
            Code::PreservationError
        } else if report != json!({"error":code}) {
            Code::ContentError
        } else {
            Code::Pass
        };
        return Ok((
            result,
            json!({"check":"atomic rejection and explicit error report"}),
        ));
    }
    let result = match family {
        "sorting" => {
            actual.remove("output.csv");
            sort_check(&input, after, variant)?
        }
        "aggregation" => {
            actual.remove("output.json");
            query_check(&input, after, variant)?
        }
        "text" => {
            for name in expected.keys().cloned().collect::<Vec<_>>() {
                let target = match variant {
                    0 | 1 => name == "scope/a.txt",
                    2 => name == "scope/a.txt" || name == "scope/b.txt",
                    3 => name.starts_with("scope/") && name.ends_with(".txt"),
                    _ => false,
                };
                if !target {
                    continue;
                }
                let text = fs::read_to_string(input.join(&name))?;
                let chars: Vec<_> = text.chars().collect();
                let mut transformed = String::new();
                let mut index = 0;
                while index < chars.len() {
                    let matched = chars.get(index..index + 3).is_some_and(|s| {
                        if variant == 2 {
                            s.iter()
                                .zip(['r', 'e', 'd'])
                                .all(|(a, b)| a.eq_ignore_ascii_case(&b))
                        } else {
                            s == ['r', 'e', 'd']
                        }
                    });
                    let word = |c: &char| c.is_alphanumeric() || *c == '_';
                    let boundary = !matches!(variant, 1 | 3)
                        || (!index
                            .checked_sub(1)
                            .and_then(|i| chars.get(i))
                            .is_some_and(word)
                            && !chars.get(index + 3).is_some_and(word));
                    if matched && boundary {
                        transformed.push_str("blue");
                        index += 3;
                    } else {
                        transformed.push(chars[index]);
                        index += 1;
                    }
                }
                expected.insert(
                    name,
                    format!("{:x}", Sha256::digest(transformed.as_bytes())),
                );
            }
            Code::Pass
        }
        "filesystem" => {
            let renames = match variant {
                0 => vec![(
                    "work/source file [α].txt".into(),
                    "work/renamed file [α].txt".into(),
                )],
                1 => {
                    expected.insert("work/nested/".into(), "directory".into());
                    expected.insert("work/nested/deeper/".into(), "directory".into());
                    vec![(
                        "work/source file [α].txt".into(),
                        "work/nested/deeper/moved file [α].txt".into(),
                    )]
                }
                2 => (0..5)
                    .map(|i| {
                        (
                            format!("work/batch/old-{i}.txt"),
                            format!("work/batch/new-{i}.txt"),
                        )
                    })
                    .collect(),
                _ => return Err("unexpected success variant".into()),
            };
            for (source, destination) in renames {
                let hash = expected.remove(&source).ok_or("missing source")?;
                if expected.insert(destination, hash).is_some() {
                    return Err("unexpected collision".into());
                }
            }
            Code::Pass
        }
        _ => return Err("unknown family".into()),
    };
    let result = if expected != actual {
        Code::PreservationError
    } else {
        result
    };
    Ok((
        result,
        json!({"check":family,"expected_paths":expected.len(),"actual_paths":actual.len()}),
    ))
}

pub fn verify(task: &Path, after: &Path) -> Verification {
    let start = Instant::now();
    let cpu = cpu_time();
    let (code, details) = match check(task, after) {
        Ok(result) => result,
        Err(error) => (Code::FormatError, json!({"error":error.to_string()})),
    };
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let rss = unsafe {
        if libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) == 0 {
            usage.assume_init().ru_maxrss as u64 * 1024
        } else {
            0
        }
    };
    Verification {
        code,
        details,
        elapsed_seconds: start.elapsed().as_secs_f64(),
        cpu_seconds: Some(cpu_time() - cpu),
        process_peak_rss_bytes: Some(rss),
    }
}

/// A dedicated, resource-limited trusted verifier process, not a model capability.
pub fn verify_isolated(task: &Path, after: &Path) -> Verification {
    use std::{
        process::{Command, Stdio},
        thread,
        time::Duration,
    };
    let start = Instant::now();
    let result = (|| -> Result<Verification> {
        let mut child = Command::new("/usr/bin/prlimit")
            .args([
                "--cpu=30:30",
                "--as=536870912:536870912",
                "--fsize=1048576:1048576",
                "--nofile=64:64",
                "--core=0:0",
                "--",
            ])
            .arg(std::env::current_exe()?)
            .arg("verify-internal")
            .arg(task)
            .arg(after)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child.stdout.take().ok_or("missing verifier stdout")?;
        let reader = thread::spawn(move || crate::executor::drain(stdout, 65536));
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if start.elapsed() > Duration::from_secs(60) {
                child.kill()?;
                break child.wait()?;
            }
            thread::sleep(Duration::from_millis(5));
        };
        let (bytes, raw) = reader.join().map_err(|_| "verifier reader panicked")??;
        if !status.success() || bytes > 65536 {
            return Err("verifier worker failed, timed out, or exceeded output limit".into());
        }
        let value: Value = serde_json::from_slice(&raw)?;
        let code = match value["code"].as_str() {
            Some("PASS") => Code::Pass,
            Some("ORDER_ERROR") => Code::OrderError,
            Some("PRESERVATION_ERROR") => Code::PreservationError,
            Some("CONTENT_ERROR") => Code::ContentError,
            Some("FORMAT_ERROR") => Code::FormatError,
            Some("SAFETY_ERROR") => Code::SafetyError,
            Some("VERIFIER_ERROR") => Code::VerifierError,
            _ => return Err("invalid verifier worker code".into()),
        };
        let mut details = value["details"].clone();
        details["worker_verification_elapsed_seconds"] = value["elapsed_seconds"].clone();
        Ok(Verification {
            code,
            details,
            elapsed_seconds: start.elapsed().as_secs_f64(),
            cpu_seconds: value["cpu_seconds"].as_f64(),
            process_peak_rss_bytes: value["process_peak_rss_bytes"].as_u64(),
        })
    })();
    result.unwrap_or_else(|error| Verification {
        code: Code::VerifierError,
        details: json!({"error":error.to_string(),"worker_metrics_unavailable":true}),
        elapsed_seconds: start.elapsed().as_secs_f64(),
        cpu_seconds: None,
        process_peak_rss_bytes: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn valid_but_wrong_artifacts_are_not_certified() {
        let root = std::env::temp_dir().join(format!("feedback-verifier-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let input = root.join("input");
        fs::create_dir(&input).unwrap();
        let after = root.join("after");
        fs::create_dir(&after).unwrap();
        let csv = "id,value,secondary,payload\na,2,0,x\nb,1,0,y\nc,2,0,z\n";
        fs::write(input.join("input.csv"), csv).unwrap();
        fs::write(after.join("input.csv"), csv).unwrap();
        fs::write(root.join("manifest.json"),serde_json::to_vec(&json!({"family":"sorting","variant":0,"expected_outcome":"success","input_hashes":snapshot(&input).unwrap()})).unwrap()).unwrap();
        fs::write(after.join("output.csv"), csv).unwrap();
        assert_eq!(verify(&root, &after).code, Code::OrderError);
        fs::write(
            after.join("output.csv"),
            "id,value,secondary,payload\nb,1,0,y\na,2,0,x\nc,2,0,z\n",
        )
        .unwrap();
        let good = verify(&root, &after);
        assert_eq!(good.code, Code::Pass);
        assert!(good.cpu_seconds.unwrap() >= 0.0);
        assert!(good.elapsed_seconds >= 0.0);
        fs::write(after.join("input.csv"), "changed").unwrap();
        assert_eq!(verify(&root, &after).code, Code::PreservationError);
        fs::remove_dir_all(root).unwrap();
    }
}
