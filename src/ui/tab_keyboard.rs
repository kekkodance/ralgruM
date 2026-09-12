pub(crate) fn next_tab_index(key: &str, current: usize, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }

    match key {
        "left" => Some((current + len - 1) % len),
        "right" => Some((current + 1) % len),
        "home" => Some(0),
        "end" => Some(len - 1),
        _ => None,
    }
}

pub(crate) fn is_activation_key(key: &str) -> bool {
    matches!(key, "enter" | "space")
}

#[cfg(test)]
mod tests {
    use super::{is_activation_key, next_tab_index};

    #[test]
    fn keyboard_navigation_wraps_and_jumps_to_bounds() {
        assert_eq!(next_tab_index("left", 0, 3), Some(2));
        assert_eq!(next_tab_index("right", 2, 3), Some(0));
        assert_eq!(next_tab_index("home", 2, 3), Some(0));
        assert_eq!(next_tab_index("end", 0, 3), Some(2));
    }

    #[test]
    fn unknown_keys_and_empty_lists_are_ignored() {
        assert_eq!(next_tab_index("up", 1, 3), None);
        assert_eq!(next_tab_index("right", 0, 0), None);
    }

    #[test]
    fn activation_keys_are_enter_and_space_only() {
        assert!(is_activation_key("enter"));
        assert!(is_activation_key("space"));
        assert!(!is_activation_key("right"));
    }
}
