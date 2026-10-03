//! Bounded native /v1/decision and Ollama /v1/systemone clients. No retries, tools, or DB
//! mutations. Every result is validated before a caller may apply any decision.
use super::decision_profiles::{DecisionBackend, DecisionProfile};
use anyhow::{ensure, Context as _};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct Decision {
    pub label: String,
    pub probability: f64,
    pub confidence: Option<f64>,
    pub usage: Option<DecisionUsage>,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct DecisionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub fn validate_results(
    profile: &DecisionProfile,
    data: &Value,
    count: usize,
) -> anyhow::Result<Vec<Decision>> {
    let results = data["results"]
        .as_array()
        .context("Decision results missing")?;
    ensure!(
        results.len() == count,
        "Decision result count does not match contexts"
    );
    let choices = profile.choices()?;
    results
        .iter()
        .map(|result| {
            let label = result["decision"][&profile.state_field]
                .as_str()
                .context("Decision label missing")?;
            ensure!(
                choices.contains(&label),
                "Decision returned an undeclared choice"
            );
            let field = &result["fields"][&profile.state_field];
            ensure!(
                field["value"].as_str() == Some(label),
                "Decision field and result disagree"
            );
            let probability = field["probability"]
                .as_f64()
                .context("Decision probability missing")?;
            ensure!(
                probability.is_finite() && (0.0..=1.0).contains(&probability),
                "Invalid Decision probability"
            );
            Ok(Decision {
                label: label.into(),
                probability,
                confidence: None,
                usage: None,
            })
        })
        .collect()
}

/// System One's selected-choice probability drives the existing threshold.
/// Its distinct confidence metric is diagnostic and never substitutes for it.
pub fn validate_systemone(profile: &DecisionProfile, data: &Value) -> anyhow::Result<Decision> {
    ensure!(
        data["model"].as_str() == Some(profile.model.as_str()),
        "System One response model differs from request"
    );
    let answers = data["answers"]
        .as_object()
        .context("System One answers missing")?;
    ensure!(
        answers.len() == 1,
        "System One answer count differs from questions"
    );
    let answer = &data["answers"][&profile.state_field];
    ensure!(
        answer["type"] == "choice",
        "System One answer must be a choice"
    );
    let choices = profile.choices()?;
    let label = answer["choice"]
        .as_str()
        .context("System One choice missing")?;
    ensure!(
        choices.contains(&label),
        "System One returned an undeclared choice"
    );
    let probabilities = answer["probabilities"]
        .as_object()
        .context("System One probabilities missing")?;
    ensure!(
        probabilities.len() == choices.len(),
        "System One must score every declared choice exactly once"
    );
    let mut sum = 0.0;
    let mut maximum = 0.0_f64;
    for choice in choices {
        let value = probabilities
            .get(choice)
            .and_then(Value::as_f64)
            .context("System One choice probability missing")?;
        ensure!(
            value.is_finite() && (0.0..=1.0).contains(&value),
            "Invalid System One probability"
        );
        sum += value;
        maximum = maximum.max(value);
    }
    ensure!(
        (sum - 1.0).abs() <= 0.002,
        "System One probabilities do not sum to one"
    );
    let probability = probabilities[label]
        .as_f64()
        .context("System One selected probability missing")?;
    ensure!(
        probability >= maximum,
        "System One selected choice is not a maximum"
    );
    let confidence = answer["confidence"]
        .as_f64()
        .context("System One confidence missing")?;
    ensure!(
        confidence.is_finite() && (0.0..=1.0).contains(&confidence),
        "Invalid System One confidence"
    );
    let usage = match (
        data["usage"]["input_tokens"].as_u64(),
        data["usage"]["output_tokens"].as_u64(),
    ) {
        (Some(input_tokens), Some(output_tokens)) => Some(DecisionUsage {
            input_tokens,
            output_tokens,
        }),
        _ => None,
    };
    Ok(Decision {
        label: label.into(),
        probability,
        confidence: Some(confidence),
        usage,
    })
}

async fn post(client: &reqwest::Client, endpoint: &str, body: &Value) -> anyhow::Result<Value> {
    let mut response = client
        .post(endpoint)
        .json(body)
        .send()
        .await
        .context("Decision transport failed")?;
    ensure!(
        response.status().is_success(),
        "Decision HTTP {}",
        response.status().as_u16()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 1024 * 1024,
            "Decision response exceeds 1 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).context("Invalid Decision JSON")
}

pub async fn decide(
    profile: &DecisionProfile,
    contexts: Vec<String>,
    cancel: &CancellationToken,
) -> anyhow::Result<Vec<Decision>> {
    profile.validate()?;
    ensure!(!cancel.is_cancelled(), "Decision cancelled");
    ensure!(
        !contexts.is_empty() && contexts.len() <= 256,
        "Decision expects 1–256 contexts"
    );
    ensure!(
        contexts
            .iter()
            .all(|s| s.chars().count() <= profile.max_context_chars),
        "Decision context exceeds configured bound"
    );
    let count = contexts.len();
    let body = json!({"model":profile.model,"instructions":profile.instructions,"contexts":contexts,"schema":profile.schema,"mode":profile.mode,"cache_prompt":profile.cache_prompt});
    ensure!(
        body.to_string().len() <= 1024 * 1024,
        "Decision request exceeds 1 MiB"
    );
    // No credentials in profiles; no redirect may forward task data elsewhere.
    let client = crate::branding::client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let request = async {
        if profile.backend == DecisionBackend::Native {
            return validate_results(
                profile,
                &post(&client, &profile.endpoint, &body).await?,
                count,
            );
        }
        let mut results = Vec::with_capacity(count);
        for context in contexts {
            ensure!(
                !context.trim().is_empty(),
                "System One state must not be empty"
            );
            let criteria = if profile.criteria.is_empty() {
                &profile.state_map
            } else {
                &profile.criteria
            };
            let body = json!({"model":profile.model,"state":context,"questions":{
                (profile.state_field.clone()):{"type":"choice","instructions":profile.instructions,"criteria":criteria}
            }});
            ensure!(
                body.to_string().len() <= 64 * 1024,
                "System One text request exceeds 64 KiB"
            );
            results.push(validate_systemone(
                profile,
                &post(&client, &profile.endpoint, &body).await?,
            )?);
        }
        Ok(results)
    };
    tokio::select! {
        biased;
        _=cancel.cancelled()=>anyhow::bail!("Decision cancelled"),
        result=tokio::time::timeout(std::time::Duration::from_millis(profile.timeout_ms),request)=>result.context("Decision timed out")?,
    }
}
