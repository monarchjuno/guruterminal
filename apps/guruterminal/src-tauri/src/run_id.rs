use crate::app::CommandError;

/// Run IDs become app-private path components and broker endpoint stems.
/// Keep this stricter than general durable identifiers: one ASCII component,
/// with no dot, colon, separator, or platform-specific alternate syntax.
pub(crate) fn validate_run_id(value: &str, label: &str) -> Result<String, CommandError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(CommandError::invalid(format!("{label} run ID is invalid")));
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_one_safe_component_and_rejects_path_syntax() {
        assert_eq!(
            validate_run_id("chat-ui_A-123", "Chat").unwrap(),
            "chat-ui_A-123"
        );
        for unsafe_id in ["", ".", "..", "a.b", "a:b", "a/b", "a\\b", "é"] {
            assert!(validate_run_id(unsafe_id, "Chat").is_err(), "{unsafe_id}");
        }
    }
}
