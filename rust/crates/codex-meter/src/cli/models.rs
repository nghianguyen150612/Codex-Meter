use super::args::{CliError, ModelsArguments};
use crate::codex_app_server::{CodexAppError, CodexAppServer, ModelCatalog};
use serde::Serialize;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Serialize)]
struct ModelsJson<'a> {
    schema_version: u32,
    generated_at: String,
    source: &'static str,
    catalog: &'a ModelCatalog,
}

pub(super) fn run(arguments: ModelsArguments) -> Result<String, CliError> {
    let client = CodexAppServer::connect().map_err(server_error)?;
    let catalog = client.list_models().map_err(server_error)?;
    if arguments.json {
        return serde_json::to_string(&ModelsJson {
            schema_version: 1,
            generated_at: now_utc(),
            source: "local_codex_app_server",
            catalog: &catalog,
        })
        .map(|json| format!("{json}\n"))
        .map_err(|_| CliError {
            code: "json_encoding_failed",
            message: "could not encode the model catalog as JSON".into(),
        });
    }
    if catalog.models.is_empty() {
        return Ok(
            "Codex Meter — Models\n\nThe current App Server catalog contains no model entries.\n"
                .to_owned(),
        );
    }
    let mut output = String::from(
        "Codex Meter — Models (live App Server catalog)\n\nMODEL                     | DISPLAY NAME                | DEFAULT | VISIBILITY | REASONING EFFORTS                  | INPUT\n",
    );
    for model in &catalog.models {
        let efforts = model
            .supported_reasoning_efforts
            .iter()
            .map(|effort| safe_cell(&effort.id, 16))
            .collect::<Vec<_>>()
            .join(",");
        let modalities = model
            .input_modalities
            .as_ref()
            .map(|values| values.join(","))
            .unwrap_or_else(|| "unavailable".to_owned());
        output.push_str(&format!(
            "{:<25} | {:<28} | {:<7} | {:<10} | {:<34} | {}\n",
            safe_cell(&model.id, 25),
            safe_cell(&model.display_name, 28),
            if model.is_default { "yes" } else { "no" },
            if model.hidden { "hidden" } else { "visible" },
            safe_cell(&efforts, 34),
            safe_cell(&modalities, 24),
        ));
    }
    Ok(output)
}

fn safe_cell(value: &str, limit: usize) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(limit)
        .collect()
}

fn now_utc() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unavailable".to_owned())
}

fn server_error(error: CodexAppError) -> CliError {
    CliError {
        code: error.code(),
        message: error.to_string(),
    }
}
