//! Types the JSON bodies exchanged with the TypeSafe API.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value as JsonValue;

/// Preserves a present value, including JSON null, separately from absence.
fn deserialize_present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Sends one state and a map of named decisions to System One.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SystemOneRequest {
    /// The structured state being evaluated.
    pub(crate) state: JsonValue,
    /// The requested model name or alias.
    pub(crate) model: String,
    /// Named questions whose keys also identify returned answers.
    pub(crate) questions: BTreeMap<String, Question>,
}

/// Describes one typed decision with instructions and criteria.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum Question {
    /// Asks for the probability that the stated condition is true.
    Noul {
        /// Optional instructions, preserving explicit null.
        #[serde(
            default,
            deserialize_with = "deserialize_present",
            skip_serializing_if = "Option::is_none"
        )]
        instructions: Option<JsonValue>,
        /// Optional true/false descriptions, preserving explicit null.
        #[serde(
            default,
            deserialize_with = "deserialize_present",
            skip_serializing_if = "Option::is_none"
        )]
        criteria: Option<NoulCriteria>,
    },
    /// Selects one of the named options.
    Choice {
        /// Optional instructions, preserving explicit null.
        #[serde(
            default,
            deserialize_with = "deserialize_present",
            skip_serializing_if = "Option::is_none"
        )]
        instructions: Option<JsonValue>,
        /// Named choices and their optional structured descriptions.
        criteria: BTreeMap<String, JsonValue>,
    },
    /// Estimates an expected ordinal score over ordered levels.
    Score {
        /// Optional instructions, preserving explicit null.
        #[serde(
            default,
            deserialize_with = "deserialize_present",
            skip_serializing_if = "Option::is_none"
        )]
        instructions: Option<JsonValue>,
        /// Ordered level descriptions, beginning at zero.
        criteria: Vec<JsonValue>,
    },
}

/// Distinguishes explicit null criteria from a true/false description record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum NoulCriteria {
    /// Sends an explicit JSON null for the criteria field.
    Null,
    /// Sends independently optional true and false descriptions.
    Descriptions(NoulDescriptions),
}

/// Holds only the two valid Noul criterion names and their structured values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoulDescriptions {
    /// Description of the positive case, preserving explicit null.
    #[serde(
        rename = "true",
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) yes: Option<JsonValue>,
    /// Description of the negative case, preserving explicit null.
    #[serde(
        rename = "false",
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) no: Option<JsonValue>,
}

/// Returns answers under the submitted names with usage metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SystemOneResponse {
    /// The resolved model used by the service.
    pub(crate) model: String,
    /// One answer per submitted question.
    pub(crate) answers: BTreeMap<String, Answer>,
    /// Token usage for this HTTP evaluation.
    pub(crate) usage: Usage,
}

/// Carries one tagged answer without changing server-reported confidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum Answer {
    /// Reports the probability of true.
    Noul {
        /// Probability of the positive decision.
        noul: f64,
    },
    /// Reports the selected option and its probability distribution.
    Choice {
        /// Selected option name.
        choice: String,
        /// Server-reported confidence.
        confidence: f64,
        /// Probabilities by option name.
        probabilities: BTreeMap<String, f64>,
    },
    /// Reports the expected ordinal level and its distribution.
    Score {
        /// Fractional expected score.
        score: f64,
        /// Server-reported confidence.
        confidence: f64,
        /// Human-readable level descriptions keyed by ordinal level.
        legend: BTreeMap<String, JsonValue>,
        /// Probabilities keyed by ordinal level.
        probabilities: BTreeMap<String, f64>,
    },
}

/// Counts input and output tokens for a completed evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Usage {
    /// Tokens counted in the request.
    pub(crate) input_tokens: u64,
    /// Tokens counted in the response.
    pub(crate) output_tokens: u64,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value as JsonValue, json};

    use super::{SystemOneRequest, SystemOneResponse};

    /// Ensures a mixed-answer envelope retains its wire shape.
    #[test]
    fn parses_mixed_answers() {
        let wire = json!({
            "model": "jev-latest",
            "answers": {
                "spam": {"type": "noul", "noul": 0.982},
                "kind": {"type": "choice", "choice": "spam", "confidence": 0.91,
                    "probabilities": {"normal": 0.09, "spam": 0.91}},
                "urgency": {"type": "score", "score": 2.4, "confidence": 0.81,
                    "legend": {"0": "none", "1": "later", "2": "today", "3": "now"},
                    "probabilities": {"0": 0.0, "1": 0.1, "2": 0.4, "3": 0.5}}
            },
            "usage": {"input_tokens": 731, "output_tokens": 18}
        });
        let response: SystemOneResponse = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(response).unwrap(), wire);
    }

    /// Ensures an omitted field remains absent while explicit null remains present.
    #[test]
    fn request_round_trip_preserves_optional_presence() {
        let wire = json!({
            "state": {"message": "hello"},
            "model": "jev-latest",
            "questions": {
                "missing": {"type": "noul"},
                "nulls": {"type": "noul", "instructions": null, "criteria": null},
                "one_sided": {"type": "noul", "criteria": {"true": null}},
                "two_sided": {"type": "noul", "criteria": {"true": {"weight": 2}, "false": ["ordinary", false]}},
                "choice": {"type": "choice", "instructions": ["sort", {"priority": 1}],
                    "criteria": {"yes": null, "no": {"reason": false}}},
                "score": {"type": "score", "criteria": ["low", {"weight": 2}]}
            }
        });
        let request: SystemOneRequest = serde_json::from_value(wire.clone()).unwrap();
        let round_trip: JsonValue = serde_json::to_value(request).unwrap();
        assert_eq!(round_trip, wire);
    }

    /// Rejects unknown Noul criterion keys at deserialization rather than erasing them.
    #[test]
    fn rejects_unknown_noul_criteria_fields() {
        let invalid = json!({
            "type": "noul",
            "criteria": {"true": "spam", "other": "not in schema"}
        });
        assert!(serde_json::from_value::<super::Question>(invalid).is_err());
    }
}
