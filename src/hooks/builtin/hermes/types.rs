use serde::{Deserialize, Serialize};

/// A single fact extracted by the Hermes LLM analysis pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedFact {
    pub key: String,
    pub content: String,
    pub category: String,
    pub confidence: f64,
}

/// Action type returned by the consolidation LLM pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsolidationActionKind {
    Keep,
    Forget,
    Merge,
    Update,
}

/// A single consolidation action recommended by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationAction {
    pub action: ConsolidationActionKind,
    pub keys: Vec<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracted_fact_roundtrip() {
        let fact = ExtractedFact {
            key: "preferred_language".into(),
            content: "User prefers Rust".into(),
            category: "preference".into(),
            confidence: 0.9,
        };
        let json = serde_json::to_string(&fact).unwrap();
        let parsed: ExtractedFact = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.key, "preferred_language");
        assert!((parsed.confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_extraction_array() {
        let json = r#"[
            {"key": "name", "content": "User is Alice", "category": "knowledge", "confidence": 1.0},
            {"key": "goal", "content": "Building a CLI tool", "category": "goal", "confidence": 0.7}
        ]"#;
        let facts: Vec<ExtractedFact> = serde_json::from_str(json).unwrap();
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].key, "name");
        assert_eq!(facts[1].category, "goal");
    }

    #[test]
    fn malformed_json_returns_error() {
        let bad = r#"{"key": "oops"}"#; // not an array
        let result: Result<Vec<ExtractedFact>, _> = serde_json::from_str(bad);
        assert!(result.is_err());
    }

    #[test]
    fn missing_field_returns_error() {
        let json = r#"[{"key": "x", "content": "y"}]"#; // missing category + confidence
        let result: Result<Vec<ExtractedFact>, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn consolidation_action_roundtrip() {
        let action = ConsolidationAction {
            action: ConsolidationActionKind::Merge,
            keys: vec!["key_a".into(), "key_b".into()],
            content: Some("merged content".into()),
            confidence: Some(0.9),
        };
        let json = serde_json::to_string(&action).unwrap();
        let parsed: ConsolidationAction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.action, ConsolidationActionKind::Merge);
        assert_eq!(parsed.keys.len(), 2);
        assert_eq!(parsed.content.as_deref(), Some("merged content"));
    }

    #[test]
    fn consolidation_action_array_all_kinds() {
        let json = r#"[
            {"action": "keep", "keys": ["k1"]},
            {"action": "forget", "keys": ["k2"]},
            {"action": "merge", "keys": ["k3", "k4"], "content": "merged", "confidence": 0.85},
            {"action": "update", "keys": ["k5"], "content": "new text", "confidence": 0.7}
        ]"#;
        let actions: Vec<ConsolidationAction> = serde_json::from_str(json).unwrap();
        assert_eq!(actions.len(), 4);
        assert_eq!(actions[0].action, ConsolidationActionKind::Keep);
        assert_eq!(actions[1].action, ConsolidationActionKind::Forget);
        assert_eq!(actions[2].action, ConsolidationActionKind::Merge);
        assert_eq!(actions[3].action, ConsolidationActionKind::Update);
        // Optional fields default to None when absent.
        assert!(actions[0].content.is_none());
        assert!(actions[0].confidence.is_none());
    }

    #[test]
    fn consolidation_action_invalid_kind() {
        let json = r#"[{"action": "destroy", "keys": ["k1"]}]"#;
        let result: Result<Vec<ConsolidationAction>, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
