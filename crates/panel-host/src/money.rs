//! Money as administrators and providers write it: three-letter currency codes and plain
//! decimal amounts. What an amount is (a price, a budget, a ledger entry) and its limits
//! stay with its owner; how it is spelled is checked here.

/// Whether `value` is a currency code: three upper-case ASCII letters, such as `CNY`.
pub fn currency(value: &str) -> bool {
    value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_uppercase())
}

/// The whole and fraction digits of a plain decimal amount `whole[.fraction]`: the whole
/// part is one or more ASCII digits, the fraction at most `fraction_digits` ASCII digits
/// and may be empty (`12.` is twelve). No sign, separators or exponent.
pub fn decimal(value: &str, fraction_digits: usize) -> Option<(&str, &str)> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    (!whole.is_empty()
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && fraction.len() <= fraction_digits
        && fraction.bytes().all(|byte| byte.is_ascii_digit()))
    .then_some((whole, fraction))
}

#[cfg(test)]
mod tests {
    use super::{currency, decimal};

    #[test]
    fn codes_and_amounts_are_spelled_plainly() {
        assert!(currency("CNY"));
        for invalid in ["cny", "CN", "CNYY", "C1Y", ""] {
            assert!(!currency(invalid), "{invalid}");
        }
        assert_eq!(decimal("12.50", 2), Some(("12", "50")));
        assert_eq!(decimal("12", 2), Some(("12", "")));
        assert_eq!(decimal("12.", 2), Some(("12", "")));
        assert_eq!(decimal("0.123456", 6), Some(("0", "123456")));
        for invalid in [".5", "1.234", "-1", "+1", "1,000", "1e3", "1.2.3", " 1", ""] {
            assert_eq!(decimal(invalid, 2), None, "{invalid}");
        }
    }
}
