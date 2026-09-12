pub(crate) fn playlist_image_url(id: &str) -> String {
    let id = id.trim();
    if id.is_empty() || id.len() > 32 || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return String::new();
    }
    format!("https://api.deezer.com/playlist/{id}/image")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_image_url_requires_a_numeric_id() {
        assert_eq!(
            playlist_image_url("13743145521"),
            "https://api.deezer.com/playlist/13743145521/image"
        );
        assert!(playlist_image_url("13743145521/cover").is_empty());
        assert!(playlist_image_url("").is_empty());
    }
}
