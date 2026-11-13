//! Checks for short texts: names, labels, notes and reasons that administrators enter, and
//! the short values that providers, Agents and imported documents report. One set of rules
//! ([`Text`]): a length limit in bytes or characters, whether the text may be empty or
//! blank, and whether line feeds and tabs are allowed; any other control character is
//! refused. Short texts are checked here only; each caller keeps its own message.

use crate::error::{ApiError, ApiResult};

/// The rules a short text is checked against.
#[derive(Clone, Copy, Debug)]
pub struct Text {
    maximum: usize,
    unit: Unit,
    presence: Presence,
    multiline: bool,
}

#[derive(Clone, Copy, Debug)]
enum Unit {
    Bytes,
    Chars,
}

#[derive(Clone, Copy, Debug)]
enum Presence {
    Optional,
    NotEmpty,
    NotBlank,
}

impl Text {
    /// At most `maximum` bytes; may be empty.
    pub const fn bytes(maximum: usize) -> Self {
        Self {
            maximum,
            unit: Unit::Bytes,
            presence: Presence::Optional,
            multiline: false,
        }
    }

    /// Any length, for a caller that limits the length in a unit of its own; may be empty.
    pub const fn unbounded() -> Self {
        Self::bytes(usize::MAX)
    }

    /// At most `maximum` characters; may be empty.
    pub const fn chars(maximum: usize) -> Self {
        Self {
            unit: Unit::Chars,
            ..Self::bytes(maximum)
        }
    }

    /// The empty text is refused.
    pub const fn not_empty(self) -> Self {
        Self {
            presence: Presence::NotEmpty,
            ..self
        }
    }

    /// A text of nothing but whitespace is refused, the empty text included.
    pub const fn not_blank(self) -> Self {
        Self {
            presence: Presence::NotBlank,
            ..self
        }
    }

    /// Line feeds and tabs are allowed.
    pub const fn multiline(self) -> Self {
        Self {
            multiline: true,
            ..self
        }
    }

    /// Whether `value`, as it is, follows the rules.
    pub fn accepts(self, value: &str) -> bool {
        let length = match self.unit {
            Unit::Bytes => value.len(),
            Unit::Chars => value.chars().count(),
        };
        let present = match self.presence {
            Presence::Optional => true,
            Presence::NotEmpty => !value.is_empty(),
            Presence::NotBlank => !value.trim().is_empty(),
        };
        let controls = if self.multiline {
            value
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        } else {
            has_control(value)
        };
        present && length <= self.maximum && !controls
    }

    /// `value` without surrounding whitespace, if that follows the rules.
    pub fn trimmed(self, value: &str) -> Option<&str> {
        let value = value.trim();
        self.accepts(value).then_some(value)
    }
}

/// Whether `value` contains a control character, line feeds and tabs included.
pub fn has_control(value: &str) -> bool {
    value.chars().any(char::is_control)
}

/// `value` without its control characters, cut to `maximum` characters: a reported name
/// kept for display.
pub fn printable(value: &str, maximum: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(maximum)
        .collect()
}

/// The trimmed text, at most `maximum` bytes and without control characters.
pub fn bounded(value: &str, maximum: usize) -> ApiResult<String> {
    Text::bytes(maximum)
        .trimmed(value)
        .map(str::to_owned)
        .ok_or_else(|| ApiError::BadRequest("文本长度超限或包含控制字符".into()))
}

/// A label that is not blank, at most `limit` characters and without control characters.
pub fn label(value: &str, limit: usize) -> ApiResult<()> {
    if Text::chars(limit).not_blank().accepts(value) {
        Ok(())
    } else {
        Err(ApiError::BadRequest("文字为空、过长或包含控制字符".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::{Text, bounded, label, printable};

    #[test]
    fn texts_are_trimmed_and_bounded() {
        assert_eq!(bounded("  维护说明 ", 64).unwrap(), "维护说明");
        assert!(bounded("a\nb", 64).is_err());
        assert!(bounded(&"a".repeat(9), 8).is_err());
        assert!(label("  ", 8).is_err());
        assert!(label("名称", 2).is_ok());
        assert!(label("名称过长", 2).is_err());
    }

    #[test]
    fn rules_count_presence_length_and_line_breaks() {
        // Two characters, six bytes.
        assert!(Text::chars(2).accepts("名称"));
        assert!(!Text::bytes(2).accepts("名称"));
        assert!(Text::bytes(6).accepts("名称"));
        assert!(Text::bytes(4).accepts(""));
        assert!(!Text::bytes(4).not_empty().accepts(""));
        assert!(Text::bytes(4).not_empty().accepts("  "));
        assert!(!Text::bytes(4).not_blank().accepts("  "));
        assert!(!Text::bytes(8).accepts("a\nb"));
        assert!(Text::bytes(8).multiline().accepts("a\nb\tc"));
        assert!(!Text::bytes(8).multiline().accepts("a\rb"));
        assert!(!Text::bytes(8).multiline().accepts("a\u{7f}"));
        assert_eq!(Text::chars(4).not_empty().trimmed("  ab  "), Some("ab"));
        assert_eq!(Text::chars(4).not_empty().trimmed("   "), None);
        assert_eq!(printable("a\u{0}b\nc", 2), "ab");
    }
}
