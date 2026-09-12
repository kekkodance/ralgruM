use super::super::format_line;
use super::redact_sensitive_message;

fn formatted_message(message: &str) -> String {
    let line = format_line("timestamp", 42, "ThreadId(7)", "worker", "INFO", message);
    line.split_once(" message=")
        .expect("formatted diagnostic message")
        .1
        .strip_suffix('\n')
        .expect("one terminating newline")
        .to_owned()
}

#[test]
fn authorization_token_and_api_key_schemes_hide_unicode_credentials() {
    let message = concat!(
        "前文 Authorization: tOkEn 秘密-token-哨兵\n",
        "API 呼び出し Authorization: API-key 秘密-key-哨兵\n",
        "Proxy-Authorization: ApiKey 秘密-proxy-哨兵\n",
        "Authorization = Bearer 秘密-bearer-哨兵\n",
        "Basic 秘密-basic-哨兵 OAuth 秘密-oauth-哨兵 後文"
    );
    assert_eq!(
        formatted_message(message),
        concat!(
            "前文 Authorization: tOkEn [redacted]\\n",
            "API 呼び出し Authorization: API-key [redacted]\\n",
            "Proxy-Authorization: ApiKey [redacted]\\n",
            "Authorization = Bearer [redacted]\\n",
            "Basic [redacted] OAuth [redacted] 後文"
        )
    );
}

#[test]
fn token_parameters_hide_quoted_secrets_without_exposing_basic_padding() {
    assert_eq!(
        formatted_message(concat!(
            "Authorization: Token token=\"秘密 token\\\"suffix\", scope=read\n",
            "Authorization: API-Key api_key='秘密 api key'\n",
            "Basic c2VudGluZWwtcGFzc3dvcmQ= 後"
        )),
        concat!(
            "Authorization: Token token=\"[redacted]\", scope=read\\n",
            "Authorization: API-Key api_key='[redacted]'\\n",
            "Basic [redacted] 後"
        )
    );
}

#[test]
fn credential_assignments_hide_refresh_password_secret_and_api_key() {
    let message = concat!(
        "café refresh_token=秘密-refresh-哨兵; password='秘密 password 哨兵'; ",
        "secret=秘密-secret-哨兵, api_key=秘密-api-哨兵 ",
        "accessToken=秘密-access-哨兵 authToken=秘密-auth-哨兵 終わり"
    );
    assert_eq!(
        formatted_message(message),
        concat!(
            "café refresh_token=[redacted]; password='[redacted]'; ",
            "secret=[redacted], api_key=[redacted] ",
            "accessToken=[redacted] authToken=[redacted] 終わり"
        )
    );
}

#[test]
fn unicode_assignment_spacing_preserves_surrounding_text() {
    assert_eq!(
        formatted_message("前 password\u{a0}=\u{2003}'秘密 哨兵' 後"),
        "前 password\u{a0}=\u{2003}'[redacted]' 後"
    );
}

#[test]
fn json_credentials_preserve_quotes_escapes_and_unrelated_unicode_fields() {
    let message = r#"{"before":"café 日本語","cookie":"unknown=秘密-cookie; theme=dark","authorization":"Token 秘密-auth","accessToken":"秘密-access","authToken":"秘密-token","refresh_token":"秘密-refresh","password":"秘密-escaped\"-suffix\\","secret":"秘密-secret","api_key":"秘密-api","after":"é 音楽"}"#;
    let expected = r#"{"before":"café 日本語","cookie":"[redacted]","authorization":"[redacted]","accessToken":"[redacted]","authToken":"[redacted]","refresh_token":"[redacted]","password":"[redacted]","secret":"[redacted]","api_key":"[redacted]","after":"é 音楽"}"#;
    let redacted = formatted_message(message);
    assert_eq!(redacted, expected);
    let decoded: serde_json::Value = serde_json::from_str(&redacted).unwrap();
    assert_eq!(decoded["after"], "é 音楽");
}

#[test]
fn multiline_json_credential_whitespace_preserves_following_fields() {
    for message in [
        "{\"password\":\n\"sentinel-password\",\"after\":\"ok\"}",
        "{\"cookie\"\r\n:\n\"unfamiliar=sentinel-cookie; theme=theme-sentinel\",\"after\":\"ok\"}",
        "{\"authorization\":\t\n\"Bearer sentinel-auth\",\"after\":\"ok\"}",
    ] {
        serde_json::from_str::<serde_json::Value>(message).expect("valid multiline JSON input");
        let redacted = formatted_message(message);
        assert!(!redacted.contains("sentinel"));
        // The sink escapes record newlines after redaction; restore this fixture's JSON whitespace.
        let decoded: serde_json::Value =
            serde_json::from_str(&redacted.replace("\\n", "\n")).unwrap();
        assert_eq!(decoded["after"], "ok");
    }
}

#[test]
fn quoted_secrets_consume_escaped_quotes_but_not_following_fields() {
    let message =
        r#"前 password="秘密\"still-secret\\" status=503 authToken='秘密\'more-secret' 後"#;
    assert_eq!(
        formatted_message(message),
        r#"前 password="[redacted]" status=503 authToken='[redacted]' 後"#
    );
    assert_eq!(
        formatted_message("前 password=\"秘密\\終"),
        "前 password=\"[redacted]"
    );
}

#[test]
fn cookie_headers_hide_unknown_names_without_eating_the_next_line() {
    assert_eq!(
        formatted_message(concat!(
            "status=200 Cookie: unfamiliar=秘密-cookie; theme=dark\r\n",
            "Set-Cookie: unfamiliar=秘密-set-cookie; Path=/; HttpOnly\n",
            "続行 status=503"
        )),
        "status=200 Cookie: [redacted]\\n\\nSet-Cookie: [redacted]\\n続行 status=503"
    );
}

#[test]
fn embedded_cookie_header_preserves_json_delimiters() {
    let redacted = formatted_message(
        r#"{"message":"Cookie: session=秘密-cookie; theme=dark","after":"音楽"}"#,
    );
    assert_eq!(
        redacted,
        r#"{"message":"Cookie: [redacted]","after":"音楽"}"#
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&redacted).unwrap()["after"],
        "音楽"
    );
    assert_eq!(
        formatted_message(r#"Cookie: session="秘密\" still-secret"; other=秘密-other"#),
        "Cookie: [redacted]"
    );
}

#[test]
fn escaped_quoted_cookie_in_json_hides_every_value_and_preserves_fields() {
    let redacted = formatted_message(
        r#"{"message":"Cookie: unfamiliar=\"sentinel-cookie\"; theme=theme-sentinel","after":"ok"}"#,
    );
    assert!(!redacted.contains("sentinel-cookie"));
    assert!(!redacted.contains("theme-sentinel"));
    let decoded: serde_json::Value = serde_json::from_str(&redacted).unwrap();
    assert!(decoded["message"].as_str().unwrap().starts_with("Cookie:"));
    assert_eq!(decoded["after"], "ok");
}

#[test]
fn escaped_quoted_authorization_in_json_preserves_surrounding_unicode() {
    let redacted = formatted_message(
        r#"{"before":"café 日本語","message":"Authorization: \"Bearer 秘密-auth\"","after":"音楽"}"#,
    );
    assert!(!redacted.contains("秘密-auth"));
    let decoded: serde_json::Value = serde_json::from_str(&redacted).unwrap();
    assert!(
        decoded["message"]
            .as_str()
            .unwrap()
            .starts_with("Authorization:")
    );
    assert_eq!(decoded["before"], "café 日本語");
    assert_eq!(decoded["after"], "音楽");
}

#[test]
fn escaped_quoted_bearer_in_json_hides_whitespace_separated_suffix() {
    let redacted = formatted_message(
        r#"{"message":"Authorization: Bearer \"sentinel-left 秘密-right\"","after":"ok"}"#,
    );
    assert!(!redacted.contains("sentinel-left"));
    assert!(!redacted.contains("秘密-right"));
    let decoded: serde_json::Value = serde_json::from_str(&redacted).unwrap();
    assert!(
        decoded["message"]
            .as_str()
            .unwrap()
            .starts_with("Authorization:")
    );
    assert_eq!(decoded["after"], "ok");
}

#[test]
fn signed_urls_hide_queries_fragments_and_userinfo_preserving_hosts() {
    assert_eq!(
        formatted_message(concat!(
            "前 (HTTPS://秘密-user:秘密-pass@media.example.test:443/音楽?signature=秘密-query#秘密-fragment), ",
            "http://秘密-user@media.example.test/ä#秘密-fragment ",
            "https://media.example.test/ä?token=秘密-query 後"
        )),
        concat!(
            "前 (HTTPS://[redacted]@media.example.test:443/音楽), ",
            "http://[redacted]@media.example.test/ä ",
            "https://media.example.test/ä 後"
        )
    );
}

#[test]
fn url_userinfo_subdelimiters_hide_credentials_and_preserve_authority() {
    for (username, password) in [
        ("sentinel,user", "秘密,pass"),
        ("sentinel(user)", "秘密(pass)"),
        ("sentinel'user", "秘密'pass"),
    ] {
        let redacted = formatted_message(&format!(
            "前 (https://{username}:{password}@media.example.test:443/音楽), 後"
        ));
        assert!(!redacted.contains("sentinel"));
        assert!(!redacted.contains("秘密"));
        assert!(redacted.contains("https://[redacted]@media.example.test:443/音楽"));
        assert!(redacted.starts_with("前 ("));
        assert!(redacted.ends_with("), 後"));
    }
}

#[test]
fn sensitive_url_subdelimiters_do_not_expose_credential_suffixes() {
    for sensitive_value in [
        "sentinel-left,秘密-right",
        "sentinel-left'秘密-right",
        "sentinel-left(秘密-right)",
        "sentinel-left)秘密-right",
    ] {
        for (suffix, expected_url) in [
            (
                format!("/token/{sensitive_value}"),
                "https://media.example.test/token/[redacted]",
            ),
            (
                format!("/音楽?sig={sensitive_value}"),
                "https://media.example.test/音楽",
            ),
        ] {
            let redacted = formatted_message(&format!(
                r#"{{"url":"https://media.example.test{suffix}","after":"café 音楽"}}"#
            ));
            assert!(!redacted.contains("sentinel-left"));
            assert!(!redacted.contains("秘密-right"));
            let decoded: serde_json::Value = serde_json::from_str(&redacted).unwrap();
            assert_eq!(decoded["url"], expected_url);
            assert_eq!(decoded["after"], "café 音楽");
        }
    }
}

#[test]
fn ordinary_urls_preserve_prose_punctuation_and_enclosing_json() {
    let redacted = formatted_message(
        "前 https://media.example.test, status=503 (https://audio.example.test/音楽), 後 reader@example.test",
    );
    assert!(redacted.starts_with("前 https://media.example.test, status=503 ("));
    assert!(redacted.ends_with("https://audio.example.test/音楽), 後 reader@example.test"));

    let redacted = formatted_message(
        r#"{"url":"https://media.example.test","after":"café 音楽 reader@example.test"}"#,
    );
    let decoded: serde_json::Value = serde_json::from_str(&redacted).unwrap();
    assert_eq!(decoded["url"], "https://media.example.test");
    assert_eq!(decoded["after"], "café 音楽 reader@example.test");
}

#[test]
fn credential_labelled_url_paths_hide_only_sensitive_segments() {
    assert_eq!(
        formatted_message(concat!(
            "前 https://media.example.test/accessToken/秘密-access/音楽/",
            "API-key/秘密-api/token=秘密-token/password:秘密-password/",
            "bEaReR/秘密-bearer/終 後"
        )),
        concat!(
            "前 https://media.example.test/accessToken/[redacted]/音楽/",
            "API-key/[redacted]/token=[redacted]/password:[redacted]/",
            "bEaReR/[redacted]/終 後"
        )
    );
}

#[test]
fn percent_encoded_url_labels_do_not_reencode_unrelated_paths() {
    assert_eq!(
        formatted_message(concat!(
            "URL=https://media.example.test/%74oKeN/秘密-token/",
            "api%5Fkey/秘密-api/%73ecret=秘密-secret/%E9%9F%B3?other=秘密-query"
        )),
        concat!(
            "URL=https://media.example.test/%74oKeN/[redacted]/",
            "api%5Fkey/[redacted]/%73ecret=[redacted]/%E9%9F%B3"
        )
    );
}

#[test]
fn json_signed_urls_keep_escaped_content_inside_the_sensitive_span() {
    let message = r#"{"url":"https://秘密-user:秘密-pass@[::1]:443/音?sig=秘密\"still-secret\\","after":"café"}"#;
    let redacted = formatted_message(message);
    assert_eq!(
        redacted,
        r#"{"url":"https://[redacted]@[::1]:443/音","after":"café"}"#
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&redacted).unwrap()["after"],
        "café"
    );
}

#[test]
fn ordinary_token_password_and_unicode_diagnostics_remain_unchanged() {
    for message in [
        "provider=public status=503 retry=2 category=media café 日本語",
        "The token expired but the password remains unchanged. API-key rotation is available.",
        "A secret is not a password assignment. accessToken refresh completed.",
        "Token expired. API-key rotation is available. The password was rejected.",
        "tokenization=ready passwordless=true my_token=counter authTokenCount=3",
        "étoken=counter 日本password=counter",
        r#"{"message":"token expired; password was not supplied","note":"café 音楽"}"#,
        "https://media.example.test/音楽/%E9%9F%B3/token-guide/password-reset.mp3",
    ] {
        assert_eq!(formatted_message(message), message);
    }
}

#[test]
fn legacy_credential_aliases_and_authorization_delimiters_remain_protected() {
    let message = concat!(
        "前 access-token=秘密-access client_secret=秘密-client signature=秘密-signature ",
        "sig=秘密-sig oauth_token=秘密-oauth arl=秘密-arl session=秘密-session ",
        "session_id=秘密-id sid=秘密-sid Bearer:秘密-bearer Basic=秘密-basic 後"
    );
    let redacted = redact_sensitive_message(message);
    assert!(!redacted.contains("秘密"));
    assert_eq!(
        redacted,
        concat!(
            "前 access-token=[redacted] client_secret=[redacted] signature=[redacted] ",
            "sig=[redacted] oauth_token=[redacted] arl=[redacted] session=[redacted] ",
            "session_id=[redacted] sid=[redacted] Bearer:[redacted] Basic=[redacted] 後"
        )
    );
}
