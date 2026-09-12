const MAX_SMART_MIX_TITLE_CHARS: usize = 200;

pub(crate) const CANONICAL_SMART_MIX_TITLE: &str = "Mix";

/// Return a trimmed, provider-cased SmartMix title when it is safe to display.
pub(crate) fn specific_smart_mix_title(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_SMART_MIX_TITLE_CHARS
        || value.chars().any(char::is_control)
        || is_generic_smart_mix_title(value)
    {
        None
    } else {
        Some(value)
    }
}

/// Use this for generated/provider titles whose trailing date is presentation metadata.
pub(crate) fn generated_smart_mix_title(value: &str) -> Option<String> {
    let value = value.trim();
    let title = value
        .rsplit_once(" - ")
        .filter(|(_, suffix)| numeric_date_suffix(suffix))
        .map_or(value, |(title, _)| title.trim());
    specific_smart_mix_title(title).map(str::to_owned)
}

fn is_generic_smart_mix_title(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "mix" | "flow" | "daily mix" | "daily" | "daily n"
    ) {
        return true;
    }
    let Some(suffix) = lower.strip_prefix("daily") else {
        return false;
    };
    let suffix = suffix.trim();
    if suffix.is_empty() || suffix == "n" {
        return true;
    }
    let mut has_digit = false;
    let numeric_or_date = suffix.chars().all(|character| {
        if character.is_ascii_digit() {
            has_digit = true;
            true
        } else {
            character.is_whitespace() || matches!(character, '/' | '.' | '-' | '_')
        }
    });
    has_digit && numeric_or_date
}

fn numeric_date_suffix(value: &str) -> bool {
    let parts = value.trim().split(['/', '.', '-']).collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.len() <= 4 && part.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_specific_titles_without_changing_provider_casing() {
        assert_eq!(
            specific_smart_mix_title("  Electro DANCE  "),
            Some("Electro DANCE")
        );
        assert_eq!(specific_smart_mix_title("R&B Vibes"), Some("R&B Vibes"));
    }

    #[test]
    fn rejects_generic_mix_flow_and_daily_variants() {
        for title in [
            "Mix",
            "FLOW",
            "Daily Mix",
            "daily",
            "Daily N",
            "Daily 1",
            "Daily 1 - 08/09/26",
            "Daily - 9/8/26",
            "Daily 2/3",
        ] {
            assert_eq!(specific_smart_mix_title(title), None, "{title}");
        }
        assert_eq!(specific_smart_mix_title("Daily Drive"), Some("Daily Drive"));
    }

    #[test]
    fn validates_length_and_control_characters() {
        assert_eq!(specific_smart_mix_title("Title\nwith break"), None);
        assert_eq!(specific_smart_mix_title(&"x".repeat(201)), None);
    }

    #[test]
    fn generated_titles_strip_only_numeric_date_suffixes() {
        assert_eq!(
            generated_smart_mix_title("Electro Dance - 9/8/26").as_deref(),
            Some("Electro Dance")
        );
        assert_eq!(
            generated_smart_mix_title("R&B Vibes - 08.09.2026").as_deref(),
            Some("R&B Vibes")
        );
        assert_eq!(
            generated_smart_mix_title("Release Radar - Deluxe").as_deref(),
            Some("Release Radar - Deluxe")
        );
        assert_eq!(generated_smart_mix_title("Daily 1 - 08/09/26"), None);
    }
}
