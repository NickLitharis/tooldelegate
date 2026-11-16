use crate::{executor::Execution, verifier::Verification};

#[derive(Clone, Copy)]
pub enum Mode {
    RawTool,
    RawVerifier,
    Certificate,
}

/// Bound bytes before they enter model context; exact tokenization is adapter work.
pub fn render(
    mode: Mode,
    execution: &Execution,
    verification: Option<&Verification>,
    cap: usize,
) -> Result<Vec<u8>, &'static str> {
    let mut output = match mode {
        Mode::RawTool => {
            let mut bytes = format!(
                "exit={:?} timeout={} resource_limited={}\nstdout:\n",
                execution.status, execution.timed_out, execution.resource_limited
            )
            .into_bytes();
            bytes.extend_from_slice(&execution.stdout);
            bytes.extend_from_slice(b"\nstderr:\n");
            bytes.extend_from_slice(&execution.stderr);
            bytes
        }
        Mode::RawVerifier => verification
            .ok_or("missing verification")?
            .raw()
            .into_bytes(),
        Mode::Certificate => {
            let bytes = verification
                .ok_or("missing verification")?
                .certificate()
                .into_bytes();
            if bytes.len() > cap {
                return Err("feedback cap smaller than certificate");
            }
            return Ok(bytes);
        }
    };
    output.truncate(cap);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certificates::Code;
    #[test]
    fn contract_never_leaks_success_stdout() {
        let run = Execution {
            status: Some(0),
            timed_out: false,
            resource_limited: false,
            stdout_bytes: 6,
            stderr_bytes: 0,
            stdout: b"SECRET".to_vec(),
            stderr: vec![],
            elapsed: std::time::Duration::ZERO,
            before: Default::default(),
            after: Default::default(),
            after_error: None,
        };
        let verification = Verification {
            code: Code::Pass,
            details: serde_json::json!({"private":"DETAIL"}),
            elapsed_seconds: 0.0,
            cpu_seconds: Some(0.0),
            process_peak_rss_bytes: Some(0),
        };
        let cert = render(Mode::Certificate, &run, Some(&verification), 96).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&cert).unwrap(),
            serde_json::json!({"ok":true,"code":"PASS"})
        );
        assert!(render(Mode::Certificate, &run, Some(&verification), 1).is_err());
        assert_eq!(render(Mode::RawTool, &run, None, 8).unwrap().len(), 8);
        assert_eq!(
            render(Mode::RawVerifier, &run, Some(&verification), 8)
                .unwrap()
                .len(),
            8
        );
        assert!(render(Mode::RawVerifier, &run, None, 8).is_err());
    }
}
