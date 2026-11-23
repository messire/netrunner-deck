//! `herdr pane` argument checks used by the pane bindings (shared with
//! `deck-server`, which compiles the bindings for the API schema).

use clap::ArgMatches;

use super::bindings;
use crate::api::schema::panes::{PaneMetadataProjection, PaneMoveProjection, PaneSwapProjection};
use crate::api::schema::{Method, PaneDirection, PaneMoveDestination};
use crate::cli::projection::ParseError;

pub(super) fn last_index(matches: &ArgMatches, names: &[&str]) -> Option<usize> {
    names
        .iter()
        .filter_map(|name| {
            if matches.value_source(name) == Some(clap::parser::ValueSource::CommandLine) {
                matches
                    .indices_of(name)
                    .and_then(|mut indices| indices.next_back())
            } else {
                None
            }
        })
        .max()
}

pub(super) fn caller_target(
    matches: &ArgMatches,
    pane: Option<String>,
    caller: Option<&str>,
    default_caller: bool,
    require_current_caller: bool,
) -> Result<Option<String>, ParseError> {
    let current = last_index(matches, &["current"]);
    let mut explicit = last_index(matches, &["pane"]);
    if matches.try_get_one::<String>("pane_id").is_ok() {
        explicit = explicit.max(last_index(matches, &["pane_id"]));
    }
    if current.is_some() && current > explicit {
        let caller = caller.map(crate::cli::normalize_pane_id);
        if require_current_caller && caller.is_none() {
            return Err(ParseError::Usage("--current requires HERDR_PANE_ID".into()));
        }
        return Ok(caller);
    }
    if current.is_none() && explicit.is_none() && default_caller {
        return Ok(caller.map(crate::cli::normalize_pane_id));
    }
    Ok(pane)
}

pub(super) fn direction(value: &str) -> Result<PaneDirection, ParseError> {
    bindings::parse_choice(bindings::DIRECTIONS, "pane direction", value)
}

pub(super) fn finite(name: &str, value: &str) -> Result<f32, ParseError> {
    value
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| ParseError::Usage(format!("invalid {name}: {value}")))
}

pub(super) fn source(value: String) -> Result<String, ParseError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Err(ParseError::Usage("missing required --source".into()));
    }
    Ok(value)
}

pub(super) fn swap(matches: &ArgMatches) -> Result<Method, ParseError> {
    let mut params = PaneSwapProjection::from_matches(matches)?;
    let directional = params.direction.is_some();
    let explicit = params.source_pane_id.is_some() || params.target_pane_id.is_some();
    if !((directional && !explicit)
        || (!directional
            && explicit
            && params.pane_id.is_none()
            && params.source_pane_id.is_some()
            && params.target_pane_id.is_some()))
    {
        return Err(ParseError::Usage(
            "provide a direction or both source and target panes".into(),
        ));
    }
    params.source_pane_id = params
        .source_pane_id
        .map(|value| crate::cli::normalize_pane_id(&value));
    params.target_pane_id = params
        .target_pane_id
        .map(|value| crate::cli::normalize_pane_id(&value));
    Ok(Method::PaneSwap(params))
}

pub(super) fn move_pane(matches: &ArgMatches) -> Result<Method, ParseError> {
    let mut params = PaneMoveProjection::from_matches(matches)?;
    let text = |name| {
        matches
            .get_many::<String>(name)
            .and_then(|mut values| values.next_back())
            .cloned()
    };
    let ratio = text("ratio")
        .map(|value| finite("ratio", &value))
        .transpose()?;
    params.destination = if let Some(tab) = text("tab") {
        let split =
            text("split").ok_or_else(|| ParseError::Usage("missing required --split".into()))?;
        PaneMoveDestination::Tab {
            tab_id: crate::cli::normalize_tab_id(&tab),
            target_pane_id: text("target-pane").map(|value| crate::cli::normalize_pane_id(&value)),
            split: bindings::parse_choice(bindings::SPLITS, "split direction", &split)?,
            ratio,
        }
    } else if matches.get_flag("new-tab") {
        PaneMoveDestination::NewTab {
            workspace_id: text("workspace").map(|value| crate::cli::normalize_workspace_id(&value)),
            label: text("label"),
        }
    } else {
        PaneMoveDestination::NewWorkspace {
            label: text("label"),
            tab_label: text("tab-label"),
        }
    };
    Ok(Method::PaneMove(params))
}

pub(super) fn metadata(matches: &ArgMatches) -> Result<Method, ParseError> {
    let mut params = PaneMetadataProjection::from_matches(matches)?;
    params.source = source(params.source)?;
    if params
        .applies_to_source
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(ParseError::Usage(
            "missing value for --applies-to-source".into(),
        ));
    }
    if params.title.is_none()
        && params.display_agent.is_none()
        && params.state_labels.is_empty()
        && params.tokens.is_empty()
        && !params.clear_title
        && !params.clear_display_agent
        && !params.clear_state_labels
    {
        return Err(ParseError::Usage(
            "missing metadata field to set or clear".into(),
        ));
    }
    Ok(Method::PaneReportMetadata(params))
}
