use serde_json::json;

pub const MAX_CERTIFICATE_BYTES: usize = 96;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    Pass,
    OrderError,
    PreservationError,
    ContentError,
    FormatError,
    SafetyError,
    VerifierError,
}

impl Code {
    pub fn name(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::OrderError => "ORDER_ERROR",
            Self::PreservationError => "PRESERVATION_ERROR",
            Self::ContentError => "CONTENT_ERROR",
            Self::FormatError => "FORMAT_ERROR",
            Self::SafetyError => "SAFETY_ERROR",
            Self::VerifierError => "VERIFIER_ERROR",
        }
    }
}

/// No artifact or free-form diagnostic can enter the model-facing contract.
pub fn certificate(code: Code, row: Option<u64>) -> String {
    let mut value = json!({"ok":code == Code::Pass,"code":code.name()});
    if code != Code::Pass
        && let Some(row) = row
    {
        value["row"] = json!(row);
    }
    let encoded = value.to_string();
    if encoded.len() <= MAX_CERTIFICATE_BYTES {
        encoded
    } else {
        json!({"ok":false,"code":"VERIFIER_ERROR"}).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_valid_json_for_every_code() {
        for code in [
            Code::Pass,
            Code::OrderError,
            Code::PreservationError,
            Code::ContentError,
            Code::FormatError,
            Code::SafetyError,
            Code::VerifierError,
        ] {
            for row in [None, Some(0), Some(u64::MAX)] {
                let text = certificate(code, row);
                assert!(text.len() <= MAX_CERTIFICATE_BYTES);
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(value["ok"], code == Code::Pass);
                assert_eq!(value["code"], code.name());
                assert!(value.as_object().unwrap().len() <= 3);
            }
        }
    }
}
