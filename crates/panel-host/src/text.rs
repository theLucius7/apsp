//! Checks for short texts that administrators enter: names, labels, notes and reasons.

use crate::error::{ApiError, ApiResult};

/// The trimmed text, at most `maximum` bytes and without control characters.
pub fn bounded(value: &str, maximum: usize) -> ApiResult<String> {
    let value = value.trim();
    if value.len() > maximum || value.chars().any(char::is_control) {
        return Err(ApiError::BadRequest("文本长度超限或包含控制字符".into()));
    }
    Ok(value.to_owned())
}

/// A label that is not blank, at most `limit` characters and without control characters.
pub fn label(value: &str, limit: usize) -> ApiResult<()> {
    if value.trim().is_empty()
        || value.chars().count() > limit
        || value.chars().any(char::is_control)
    {
        Err(ApiError::BadRequest("文字为空、过长或包含控制字符".into()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{bounded, label};

    #[test]
    fn texts_are_trimmed_and_bounded() {
        assert_eq!(bounded("  维护说明 ", 64).unwrap(), "维护说明");
        assert!(bounded("a\nb", 64).is_err());
        assert!(bounded(&"a".repeat(9), 8).is_err());
        assert!(label("  ", 8).is_err());
        assert!(label("名称", 2).is_ok());
        assert!(label("名称过长", 2).is_err());
    }
}
