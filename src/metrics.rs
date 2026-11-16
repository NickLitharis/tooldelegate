use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Pricing {
    pub currency: String,
    pub input_per_million: f64,
    pub cached_input_per_million: f64,
    pub output_per_million: f64,
    pub source: String,
}

#[derive(Default, Serialize)]
pub struct Metrics {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub feedback_input_tokens: Option<u64>,
    pub verification_input_tokens: Option<u64>,
    pub schema_input_tokens: Option<u64>,
    pub context_peak: u64,
    pub calls: u64,
    pub attempts: u64,
    pub retries: u64,
    pub tool_errors: u64,
    pub tool_output_bytes: u64,
    pub returned_feedback_bytes: u64,
    pub verifier_cpu_seconds: f64,
    pub verifier_elapsed_seconds: f64,
    pub verifier_metrics_complete: bool,
    pub last_verifier_ok: Option<bool>,
    pub retries_after_negative_verification: u64,
    pub end_to_end_seconds: f64,
    pub usage_complete: bool,
    pub monetary_cost: Option<f64>,
}

impl Metrics {
    pub fn start() -> Self {
        Self {
            cached_input_tokens: Some(0),
            feedback_input_tokens: Some(0),
            verification_input_tokens: Some(0),
            schema_input_tokens: Some(0),
            usage_complete: true,
            verifier_metrics_complete: true,
            ..Self::default()
        }
    }
    pub fn add(&mut self, usage: &crate::model_adapter::Usage) -> Result<(), &'static str> {
        if usage
            .cached_input_tokens
            .is_some_and(|n| n > usage.input_tokens)
            || usage
                .feedback_input_tokens
                .is_some_and(|n| n > usage.input_tokens)
            || usage
                .verification_input_tokens
                .is_some_and(|n| n > usage.input_tokens)
            || usage.schema_tokens.is_some_and(|n| n > usage.input_tokens)
            || usage
                .verification_input_tokens
                .zip(usage.feedback_input_tokens)
                .is_some_and(|(verification, all)| verification > all)
        {
            return Err("invalid token accounting");
        }
        self.input_tokens = self
            .input_tokens
            .checked_add(usage.input_tokens)
            .ok_or("token overflow")?;
        self.output_tokens = self
            .output_tokens
            .checked_add(usage.output_tokens)
            .ok_or("token overflow")?;
        let context = usage
            .input_tokens
            .checked_add(usage.output_tokens)
            .ok_or("token overflow")?;
        self.total_tokens = self
            .input_tokens
            .checked_add(self.output_tokens)
            .ok_or("token overflow")?;
        self.context_peak = self.context_peak.max(context);
        self.cached_input_tokens = self
            .cached_input_tokens
            .zip(usage.cached_input_tokens)
            .and_then(|(a, b)| a.checked_add(b));
        self.feedback_input_tokens = self
            .feedback_input_tokens
            .zip(usage.feedback_input_tokens)
            .and_then(|(a, b)| a.checked_add(b));
        self.verification_input_tokens = self
            .verification_input_tokens
            .zip(usage.verification_input_tokens)
            .and_then(|(a, b)| a.checked_add(b));
        self.schema_input_tokens = self
            .schema_input_tokens
            .zip(usage.schema_tokens)
            .and_then(|(a, b)| a.checked_add(b));
        Ok(())
    }
    pub fn price(&mut self, pricing: Option<&Pricing>) {
        self.monetary_cost = if self.usage_complete {
            pricing.zip(self.cached_input_tokens).map(|(p, cached)| {
                ((self.input_tokens - cached) as f64 * p.input_per_million
                    + cached as f64 * p.cached_input_per_million
                    + self.output_tokens as f64 * p.output_per_million)
                    / 1e6
            })
        } else {
            None
        }
        .filter(|cost| cost.is_finite());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_attribution_and_cache_counts_stay_unknown() {
        let mut metrics = Metrics::start();
        let mut usage = crate::model_adapter::Usage {
            input_tokens: 100,
            output_tokens: 20,
            cached_input_tokens: Some(10),
            schema_tokens: Some(8),
            feedback_input_tokens: Some(12),
            verification_input_tokens: Some(4),
            source: "synthetic_fixture".into(),
        };
        metrics.add(&usage).unwrap();
        assert_eq!(metrics.total_tokens, 120);
        assert_eq!(metrics.cached_input_tokens, Some(10));
        usage.cached_input_tokens = None;
        usage.feedback_input_tokens = None;
        usage.schema_tokens = None;
        usage.verification_input_tokens = None;
        metrics.add(&usage).unwrap();
        assert_eq!(metrics.total_tokens, 240);
        assert_eq!(metrics.cached_input_tokens, None);
        assert_eq!(metrics.schema_input_tokens, None);
        assert_eq!(metrics.verification_input_tokens, None);
        metrics.price(Some(&Pricing {
            currency: "TEST".into(),
            input_per_million: 1.0,
            cached_input_per_million: 0.0,
            output_per_million: 2.0,
            source: "synthetic fixture".into(),
        }));
        assert_eq!(metrics.monetary_cost, None);
        usage.cached_input_tokens = Some(101);
        assert!(metrics.add(&usage).is_err());
    }
}
