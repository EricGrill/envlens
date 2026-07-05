//! Secret classification and masking (spec §9, FR-032..034).
//!
//! Two independent classifiers feed [`crate::core::model::SecretClass`]:
//! [`classify_key`] flags names that *look like* they hold a secret (`API_TOKEN`,
//! `DB_PASSWORD`, ...) and [`classify_value`] flags values whose *shape* looks
//! like a real credential (JWTs, PEM blocks, cloud-provider token prefixes,
//! connection-string userinfo, or generic high-entropy tokens). Wiring these
//! into [`crate::core::model::VariableOccurrence`] happens in a later task;
//! this module only exposes the pure predicates plus the masking primitives
//! used at the report boundary.
//!
//! No `Regex::new` appears in this module's production code: the crate
//! denies `unwrap`/`expect` outside tests, and every shape check here
//! (prefixes, JWT dot-structure, PEM headers, connection-string userinfo) is
//! expressible with plain string operations, so no fallible regex
//! construction is needed. `extra_patterns` are pre-compiled by the caller
//! (config loading, a later task) and used only via `Regex::is_match`.
//!
//! **Known blind spots of the entropy heuristic.** [`is_high_entropy_token`]
//! (threshold: >3.5 bits/char over a whitespace-free string) is a
//! *secondary* net that only runs after key-name classification
//! ([`classify_key`]) and the known-prefix/JWT/PEM/userinfo shape checks in
//! [`classify_value`] have already had a chance to flag the value. It can
//! **miss** secrets with low character variety even when they're long and
//! random: a purely numeric or moderate-length hex string can never clear
//! the threshold, since a single-character Shannon entropy over decimal
//! digits alone maxes out at `log2(10) ≈ 3.32` bits/char (hex maxes out at
//! `log2(16) = 4.0`, but realistic hex tokens are frequently misclassified
//! low if the caller trims them or the value is short). It can also
//! **over-flag** long whitespace-free URLs or filesystem paths as
//! high-entropy — that direction is safe (a false positive just means an
//! extra mask) so it's left as-is. A naive "long all-hex string is a
//! secret" rule was deliberately **not** added to plug the hex gap: git
//! commit SHAs (40 hex characters) and UUIDs (32 hex characters) are common
//! *non-secret* values that such a rule would false-positive on constantly.
//! The primary defense against these gaps is key-name classification
//! ([`classify_key`]), not the entropy fallback.

use std::collections::HashMap;

/// Lowercase segment terms that mark a key as secret-like when a whole
/// segment (see [`key_segments`]) case-insensitively equals one of them.
const KEY_TERMS: &[&str] = &[
    "secret",
    "token",
    "password",
    "pass",
    "passwd",
    "pwd",
    "private",
    "key",
    "credential",
    "credentials",
    "auth",
    "session",
    "cookie",
    "apikey",
];

/// Value-shape prefixes known to belong to real credentials (cloud/API
/// tokens). Used both to classify a value as secret-like and, in [`mask`],
/// to decide whether the mask keeps a plaintext prefix.
const KNOWN_VALUE_PREFIXES: &[&str] = &[
    "sk_live_",
    "sk_test_",
    "pk_live_",
    "AKIA",
    "ghp_",
    "gho_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "glpat-",
    "AIza",
];

/// Split `key` into lowercase segments on `_`, `.`, `-`, and lower→upper
/// case boundaries (so `apiKey` -> `["api", "key"]`, `PUBLIC_KEY` ->
/// `["public", "key"]`). A run of same-case letters (including an
/// all-uppercase acronym like `APIKEY`) stays a single segment.
fn key_segments(key: &str) -> Vec<String> {
    key.split(['_', '.', '-'])
        .filter(|piece| !piece.is_empty())
        .flat_map(camel_split)
        .map(|segment| segment.to_lowercase())
        .collect()
}

/// Split a single (separator-free) piece at every lower→upper case
/// transition.
fn camel_split(piece: &str) -> Vec<String> {
    let chars: Vec<char> = piece.chars().collect();
    let mut segments = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && chars[i - 1].is_lowercase() && c.is_uppercase() {
            segments.push(std::mem::take(&mut current));
        }
        current.push(c);
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

/// `true` if `key` looks like it names a secret: spec §9 segment matching
/// against [`KEY_TERMS`], or a match against any caller-supplied
/// `extra_patterns` (already-compiled regexes, e.g. from user config).
pub fn classify_key(key: &str, extra_patterns: &[regex::Regex]) -> bool {
    let segments = key_segments(key);
    if segments.iter().any(|s| KEY_TERMS.contains(&s.as_str())) {
        return true;
    }
    extra_patterns.iter().any(|pattern| pattern.is_match(key))
}

/// `true` if `value` looks like it *contains* a secret, based on shape
/// alone: a JWT, a PEM block, a known cloud/API token prefix, a
/// connection-string with `user:pass@` userinfo, or a generic high-entropy
/// token (spec §9).
pub fn classify_value(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    looks_like_jwt(value)
        || value.contains("-----BEGIN")
        || KNOWN_VALUE_PREFIXES.iter().any(|p| value.starts_with(p))
        || has_userinfo_secret(value)
        || is_high_entropy_token(value)
}

/// `eyJ...` + two more dot-separated base64url-ish segments (a JWT never
/// gets fully base64-decoded/verified here — this is a shape check only).
fn looks_like_jwt(value: &str) -> bool {
    if !value.starts_with("eyJ") {
        return false;
    }
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(is_base64url_char))
}

fn is_base64url_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '='
}

/// Connection-string shape `scheme://user:pass@host/...` where both the
/// user and password halves of the userinfo are non-empty.
fn has_userinfo_secret(value: &str) -> bool {
    let Some(scheme_end) = value.find("://") else {
        return false;
    };
    let after_scheme = &value[scheme_end + 3..];
    let Some(at_idx) = after_scheme.find('@') else {
        return false;
    };
    let userinfo = &after_scheme[..at_idx];
    let Some(colon_idx) = userinfo.find(':') else {
        return false;
    };
    let user = &userinfo[..colon_idx];
    let pass = &userinfo[colon_idx + 1..];
    !user.is_empty() && !pass.is_empty()
}

/// Generic fallback: a whitespace-free token whose per-character Shannon
/// entropy clears the threshold. Requiring >3.5 bits/char inherently
/// requires a dozen-plus distinct characters, so short or low-variety
/// strings (plain words, repeated characters) never trip this.
fn is_high_entropy_token(value: &str) -> bool {
    if value.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    shannon_entropy_bits_per_char(value) > 3.5
}

/// Shannon entropy of `s`, in bits per character, computed over `char`
/// frequency. `0.0` for an empty string.
pub fn shannon_entropy_bits_per_char(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut counts: HashMap<char, usize> = HashMap::new();
    let mut total = 0usize;
    for c in s.chars() {
        *counts.entry(c).or_insert(0) += 1;
        total += 1;
    }
    let total = total as f64;
    (-counts
        .values()
        .map(|&count| {
            let p = count as f64 / total;
            p * p.log2()
        })
        .sum::<f64>())
    .max(0.0)
}

/// Mask `value` for display (spec §9): a fixed-width mask that never reveals
/// the secret's length, regardless of which length bucket it falls in.
/// Below 8 characters this is 8 bullet/star characters; at 8 or more
/// characters it's an optional 3-char plaintext prefix (only when `value`
/// matches a [`KNOWN_VALUE_PREFIXES`] entry) + a fixed 10 bullet/star
/// characters + the last 2 characters. Both branches use a fixed bullet
/// count so the mask's width never encodes `value`'s actual length.
pub fn mask(value: &str, ascii: bool) -> String {
    let bullet = if ascii { '*' } else { '•' };
    let chars: Vec<char> = value.chars().collect();
    let len = chars.len();

    if len < 8 {
        return bullet.to_string().repeat(8);
    }

    let prefix_len = if KNOWN_VALUE_PREFIXES.iter().any(|p| value.starts_with(p)) {
        3
    } else {
        0
    };
    let prefix: String = chars.iter().take(prefix_len).collect();
    let last_two: String = chars[len - 2..].iter().collect();
    let bullets = bullet.to_string().repeat(10);

    format!("{prefix}{bullets}{last_two}")
}

/// Report-boundary guard: wraps a value that may or may not be secret-like
/// so callers can't accidentally print it unmasked. The inner value is
/// private — the only way to get text out is via [`std::fmt::Display`],
/// which masks it whenever `secret` is `true`.
pub struct MaskedValue {
    value: String,
    secret: bool,
    ascii: bool,
}

impl MaskedValue {
    pub fn new(value: impl Into<String>, secret: bool, ascii: bool) -> Self {
        Self {
            value: value.into(),
            secret,
            ascii,
        }
    }
}

impl std::fmt::Display for MaskedValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.secret {
            write!(f, "{}", mask(&self.value, self.ascii))
        } else {
            write!(f, "{}", self.value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    #[test]
    fn classify_key_positive_table() {
        let extra: &[Regex] = &[];
        let positive = [
            "JWT_SECRET",
            "API_TOKEN",
            "DB_PASSWORD",
            "PASS",
            "PRIVATE_URL",
            "PUBLIC_KEY",
            "apiKey",
            "SESSION_ID",
            "COOKIE_NAME",
            "MY_CREDENTIALS",
            "PASSWD_FILE",
            "PWD",
            "APIKEY",
        ];
        for key in positive {
            assert!(
                classify_key(key, extra),
                "expected '{key}' to be secret-like"
            );
        }
    }

    #[test]
    fn classify_key_negative_table() {
        let extra: &[Regex] = &[];
        let negative = ["PGPASS", "KEYBOARD_LAYOUT", "AUTH0_DOMAIN", "AUTHTOKEN"];
        for key in negative {
            assert!(
                !classify_key(key, extra),
                "expected '{key}' to NOT be secret-like"
            );
        }
    }

    #[test]
    fn classify_key_extra_pattern_matches() {
        let extra = [Regex::new("SUPABASE_.*").unwrap()];
        assert!(classify_key("SUPABASE_URL", &extra));
    }

    #[test]
    fn classify_key_extra_pattern_does_not_match_unrelated_key() {
        let extra = [Regex::new("SUPABASE_.*").unwrap()];
        assert!(!classify_key("OTHER_URL", &extra));
    }

    #[test]
    fn classify_value_positive_table() {
        let positive = [
            "eyJhbGciOi.eyJzdWIiOjE.sig",
            "envlensFakeHistoricalPemHeader",
            "envlensFakeHistoricalSecret",
            "envlensFakeHistoricalAwsAccessKey",
            "envlensFakeHistoricalGitHubToken",
            "envlensFakeHistoricalSlackToken",
            "envlensFakeHistoricalGitLabToken",
            "envlensFakeHistoricalGoogleApiKey",
            "postgres://user:hunter2@host/db",
            "abcd1234EFGH5678ijkl9012",
        ];
        for value in positive {
            assert!(
                classify_value(value),
                "expected '{value}' to be secret-like"
            );
        }
    }

    #[test]
    fn classify_value_negative_table() {
        let negative = [
            "development",
            "http://localhost:3000",
            "aaaaaaaaaaaaaaaaaaaaaaaaaa",
            "two words here that are long enough",
        ];
        for value in negative {
            assert!(
                !classify_value(value),
                "expected '{value}' to NOT be secret-like"
            );
        }
    }

    #[test]
    fn mask_with_known_prefix_keeps_plaintext_prefix() {
        assert_eq!(
            mask("envlensFakeHistoricalSecret", false),
            format!("sk_{}34", "•".repeat(10))
        );
    }

    #[test]
    fn mask_without_known_prefix_has_no_plaintext_prefix() {
        assert_eq!(
            mask("longsecretvalue1", false),
            format!("{}e1", "•".repeat(10))
        );
    }

    #[test]
    fn mask_short_value_hides_length_with_fixed_bullets() {
        assert_eq!(mask("short", false), "•".repeat(8));
    }

    #[test]
    fn mask_hides_length_for_short_maskable() {
        assert_eq!(mask("abcdefgh", false), format!("{}gh", "•".repeat(10)));
    }

    #[test]
    fn mask_ascii_mode_uses_asterisks() {
        assert_eq!(
            mask("envlensFakeHistoricalSecret", true),
            format!("sk_{}34", "*".repeat(10))
        );
        assert_eq!(mask("short", true), "*".repeat(8));
    }

    #[test]
    fn masked_value_display_masks_when_secret() {
        let mv = MaskedValue::new("envlensFakeHistoricalSecret", true, false);
        assert_eq!(mv.to_string(), format!("sk_{}34", "•".repeat(10)));
    }

    #[test]
    fn masked_value_display_is_plain_when_not_secret() {
        let mv = MaskedValue::new("plainvalue", false, false);
        assert_eq!(mv.to_string(), "plainvalue");
    }

    #[test]
    fn entropy_of_repeated_char_is_zero() {
        assert!(shannon_entropy_bits_per_char("aaaa") < 1.0);
        assert_eq!(shannon_entropy_bits_per_char("aaaa"), 0.0);
    }

    #[test]
    fn entropy_of_empty_string_is_zero() {
        assert_eq!(shannon_entropy_bits_per_char(""), 0.0);
    }

    #[test]
    fn entropy_of_high_variety_string_exceeds_threshold() {
        assert!(shannon_entropy_bits_per_char("abcd1234EFGH5678ijkl9012") > 3.5);
    }

    #[test]
    fn entropy_of_single_char_is_zero_not_negative_zero() {
        let entropy = shannon_entropy_bits_per_char("a");
        assert_eq!(entropy, 0.0);
        assert!(!entropy.is_sign_negative());
    }
}
