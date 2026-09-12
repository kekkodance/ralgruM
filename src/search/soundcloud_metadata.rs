use serde_json::Value;

const FALLBACK_ARTIST_SUBTITLE: &str = "SoundCloud artist";

pub(crate) fn artist_subtitle(value: &Value) -> String {
    let Some(followers) = value.get("followers_count").and_then(nonnegative_count) else {
        return FALLBACK_ARTIST_SUBTITLE.to_owned();
    };

    format!(
        "{} follower{}",
        format_grouped(followers),
        if followers == 1 { "" } else { "s" }
    )
}

fn nonnegative_count(value: &Value) -> Option<u64> {
    match value {
        Value::Number(value) => value.as_u64(),
        Value::String(value) => value.trim().parse().ok(),
        _ => None,
    }
}

fn format_grouped(value: u64) -> String {
    let digits = value.to_string();
    let first = digits.len() % 3;
    let mut output = String::with_capacity(digits.len() + digits.len() / 3);
    if first != 0 {
        output.push_str(&digits[..first]);
    }
    for (index, chunk) in digits.as_bytes()[first..].chunks(3).enumerate() {
        if first != 0 || index > 0 {
            output.push(',');
        }
        output.push_str(std::str::from_utf8(chunk).unwrap_or_default());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::artist_subtitle;
    use serde_json::json;

    #[test]
    fn formats_integer_follower_counts() {
        assert_eq!(
            artist_subtitle(&json!({"followers_count": 42})),
            "42 followers"
        );
    }

    #[test]
    fn formats_decimal_string_follower_counts() {
        assert_eq!(
            artist_subtitle(&json!({"followers_count": "42"})),
            "42 followers"
        );
    }

    #[test]
    fn formats_zero_followers() {
        assert_eq!(
            artist_subtitle(&json!({"followers_count": 0})),
            "0 followers"
        );
    }

    #[test]
    fn formats_one_follower_with_singular_noun() {
        assert_eq!(
            artist_subtitle(&json!({"followers_count": 1})),
            "1 follower"
        );
    }

    #[test]
    fn groups_large_follower_counts() {
        assert_eq!(
            artist_subtitle(&json!({"followers_count": "12345"})),
            "12,345 followers"
        );
    }

    #[test]
    fn invalid_follower_counts_use_the_artist_fallback() {
        for value in [
            json!({}),
            json!({"followers_count": null}),
            json!({"followers_count": -1}),
            json!({"followers_count": "-1"}),
            json!({"followers_count": "not-a-count"}),
            json!({"followers_count": 1.5}),
        ] {
            assert_eq!(artist_subtitle(&value), "SoundCloud artist");
        }
    }
}
