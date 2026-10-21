use super::client::CodexAppServer;
use super::errors::CodexAppError;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;

const MODEL_PAGE_SIZE: u32 = 100;
const MAX_CATALOG_ENTRIES: usize = 10_000;
const MAX_MODEL_TEXT_BYTES: usize = 1024;
const MAX_EFFORT_DESCRIPTION_BYTES: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReasoningEffortInfo {
    pub id: String,
    pub description: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub is_default: bool,
    pub hidden: bool,
    pub supported_reasoning_efforts: Vec<ReasoningEffortInfo>,
    pub default_reasoning_effort: String,
    pub input_modalities: Option<Vec<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModelCatalog {
    /// The catalog is fetched from the current App Server connection.
    pub availability: &'static str,
    pub models: Vec<ModelInfo>,
}

impl CodexAppServer {
    /// Fetches the live model catalog, including hidden entries, without
    /// inferring account access from model identifiers or source files.
    pub fn list_models(&self) -> Result<ModelCatalog, CodexAppError> {
        let mut models = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen_cursors = HashSet::new();
        loop {
            let mut params = serde_json::Map::new();
            params.insert("limit".into(), Value::from(MODEL_PAGE_SIZE));
            params.insert("includeHidden".into(), Value::Bool(true));
            if let Some(cursor) = cursor.as_ref() {
                params.insert("cursor".into(), Value::String(cursor.clone()));
            }
            let response = self.call("model/list", Value::Object(params))?;
            let object = response
                .as_object()
                .ok_or(CodexAppError::ProtocolVersionMismatch)?;
            let data = object
                .get("data")
                .and_then(Value::as_array)
                .ok_or(CodexAppError::ProtocolVersionMismatch)?;
            for entry in data {
                models.push(parse_model(entry)?);
                if models.len() > MAX_CATALOG_ENTRIES {
                    return Err(CodexAppError::ResponseLimitExceeded);
                }
            }
            cursor = optional_cursor(object.get("nextCursor"))?;
            if let Some(cursor) = cursor.as_ref() {
                if !seen_cursors.insert(cursor.clone()) {
                    return Err(CodexAppError::ProtocolVersionMismatch);
                }
            } else {
                break;
            }
        }
        Ok(ModelCatalog {
            availability: "live",
            models,
        })
    }
}

fn parse_model(value: &Value) -> Result<ModelInfo, CodexAppError> {
    let object = value
        .as_object()
        .ok_or(CodexAppError::ProtocolVersionMismatch)?;
    let required_string = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= MAX_MODEL_TEXT_BYTES)
            .map(str::to_owned)
            .ok_or(CodexAppError::ProtocolVersionMismatch)
    };
    let required_bool = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_bool)
            .ok_or(CodexAppError::ProtocolVersionMismatch)
    };
    let efforts = object
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .ok_or(CodexAppError::ProtocolVersionMismatch)?
        .iter()
        .map(|effort| {
            let effort = effort
                .as_object()
                .ok_or(CodexAppError::ProtocolVersionMismatch)?;
            Ok(ReasoningEffortInfo {
                id: effort
                    .get("reasoningEffort")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty() && value.len() <= 128)
                    .ok_or(CodexAppError::ProtocolVersionMismatch)?
                    .to_owned(),
                description: effort
                    .get("description")
                    .and_then(Value::as_str)
                    .filter(|value| value.len() <= MAX_EFFORT_DESCRIPTION_BYTES)
                    .ok_or(CodexAppError::ProtocolVersionMismatch)?
                    .to_owned(),
            })
        })
        .collect::<Result<Vec<_>, CodexAppError>>()?;
    let input_modalities = match object.get("inputModalities") {
        None | Some(Value::Null) => None,
        Some(Value::Array(values)) => Some(
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .filter(|value| value.len() <= 64)
                        .map(str::to_owned)
                        .ok_or(CodexAppError::ProtocolVersionMismatch)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Some(_) => return Err(CodexAppError::ProtocolVersionMismatch),
    };
    Ok(ModelInfo {
        id: required_string("id")?,
        display_name: required_string("displayName")?,
        is_default: required_bool("isDefault")?,
        hidden: required_bool("hidden")?,
        supported_reasoning_efforts: efforts,
        default_reasoning_effort: required_string("defaultReasoningEffort")?,
        input_modalities,
    })
}

pub(super) fn optional_cursor(value: Option<&Value>) -> Result<Option<String>, CodexAppError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.is_empty() && value.len() <= 4096 => {
            Ok(Some(value.clone()))
        }
        Some(Value::String(_)) => Err(CodexAppError::ResponseLimitExceeded),
        Some(_) => Err(CodexAppError::ProtocolVersionMismatch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_app_server::client::CodexAppServer;

    #[test]
    fn live_model_catalog_pages_hidden_models_and_reasoning_options() {
        let catalog = CodexAppServer::connect_test_server("models")
            .unwrap()
            .list_models()
            .unwrap();
        assert_eq!(catalog.availability, "live");
        assert_eq!(catalog.models.len(), 2);
        assert_eq!(catalog.models[0].id, "model-visible");
        assert!(catalog.models[0].is_default);
        assert!(!catalog.models[0].hidden);
        assert_eq!(catalog.models[0].default_reasoning_effort, "medium");
        assert_eq!(
            catalog.models[0].input_modalities.clone().unwrap(),
            vec!["text".to_owned(), "image".to_owned()]
        );
        assert_eq!(catalog.models[0].supported_reasoning_efforts[0].id, "low");
        assert_eq!(catalog.models[1].input_modalities, None);
        assert!(catalog.models[1].hidden);
        assert!(!catalog.models[1].is_default);
    }

    #[test]
    fn unsupported_model_discovery_is_not_reported_as_an_empty_catalog() {
        let result = CodexAppServer::connect_test_server("unsupported")
            .unwrap()
            .list_models();
        assert_eq!(
            result.unwrap_err(),
            CodexAppError::UnsupportedMethod("model/list")
        );
    }

    #[test]
    fn empty_catalog_and_missing_optional_capabilities_are_distinct_and_safe() {
        let empty = CodexAppServer::connect_test_server("empty_models")
            .unwrap()
            .list_models()
            .unwrap();
        assert_eq!(empty.availability, "live");
        assert!(empty.models.is_empty());

        let optional = CodexAppServer::connect_test_server("models_optional")
            .unwrap()
            .list_models()
            .unwrap();
        assert_eq!(optional.models.len(), 1);
        assert_eq!(optional.models[0].id, "model-minimal");
        assert!(optional.models[0].supported_reasoning_efforts.is_empty());
        assert_eq!(optional.models[0].input_modalities, None);
    }

    #[test]
    fn model_shapes_are_validated_and_catalog_text_is_bounded() {
        assert_eq!(
            parse_model(&serde_json::json!({"id":"x"})).unwrap_err(),
            CodexAppError::ProtocolVersionMismatch
        );
        let mut value = serde_json::json!({
            "id":"x",
            "model":"x",
            "displayName":"x",
            "hidden":false,
            "isDefault":false,
            "supportedReasoningEfforts":[],
            "defaultReasoningEffort":"medium"
        });
        value["displayName"] = Value::String("x".repeat(MAX_MODEL_TEXT_BYTES + 1));
        assert_eq!(
            parse_model(&value).unwrap_err(),
            CodexAppError::ProtocolVersionMismatch
        );
    }
}
