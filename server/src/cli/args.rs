//! Argument helpers shared by the CLI and the API schema's CLI bindings
//! (compiled into `deck-server` too, which has no CLI).

pub(crate) fn parse_token_assignment(raw: &str) -> Result<(String, Option<String>), String> {
    let Some((key, value)) = raw.split_once('=') else {
        return Err("token must use NAME=VALUE".into());
    };
    if key.is_empty() {
        return Err("token name must not be empty".into());
    }
    Ok((key.to_string(), Some(value.to_string())))
}

pub(crate) fn parse_env_assignment(raw: &str) -> Result<(String, String), String> {
    let Some((key, value)) = raw.split_once('=') else {
        return Err("env must use KEY=VALUE".into());
    };
    if key.is_empty() {
        return Err("env key must not be empty".into());
    }
    if key.contains('\0') || value.contains('\0') {
        return Err("env must not contain NUL bytes".into());
    }
    Ok((key.to_string(), value.to_string()))
}

pub(crate) fn normalize_workspace_id(value: &str) -> String {
    value.to_string()
}

pub(crate) fn normalize_tab_id(value: &str) -> String {
    value.to_string()
}

pub(crate) fn normalize_pane_id(value: &str) -> String {
    value.to_string()
}

pub(crate) fn parse_u32_flag(flag: &str, value: &str) -> std::io::Result<u32> {
    value
        .parse::<u32>()
        .map_err(|_| std::io::Error::other(format!("invalid value for {flag}: {value}")))
}

pub(crate) fn parse_u64_flag(flag: &str, value: &str) -> std::io::Result<u64> {
    value
        .parse::<u64>()
        .map_err(|_| std::io::Error::other(format!("invalid value for {flag}: {value}")))
}
