const REDACTED: &str = "[redacted]";
const AUTH_SCHEMES: &[&str] = &["bearer", "oauth", "basic", "token", "apikey"];
const SENSITIVE_KEYS: &[&str] = &[
    "accesstoken",
    "authtoken",
    "refreshtoken",
    "password",
    "secret",
    "apikey",
    "token",
    "clientsecret",
    "signature",
    "sig",
    "oauthtoken",
    "arl",
    "session",
    "sessionid",
    "sid",
];

pub(super) fn redact_sensitive_message(message: &str) -> String {
    let mut redacted = String::with_capacity(message.len());
    let mut index = 0;
    while index < message.len() {
        if let Some(end) = redact_url(message, index, &mut redacted) {
            index = end;
            continue;
        }

        if let Some((value_start, value_end)) = assignment_redaction_range(message, index)
            .or_else(|| authorization_redaction_range(message, index))
        {
            redacted.push_str(&message[index..value_start]);
            redacted.push_str(REDACTED);
            index = value_end;
            continue;
        }

        let character = message[index..]
            .chars()
            .next()
            .expect("index always points to a character boundary");
        redacted.push(character);
        index += character.len_utf8();
    }
    redacted
}

fn assignment_redaction_range(message: &str, start: usize) -> Option<(usize, usize)> {
    if !is_boundary_before(message, start) {
        return None;
    }
    let bytes = message.as_bytes();
    let quote = bytes
        .get(start)
        .copied()
        .filter(|byte| matches!(byte, b'"' | b'\''));
    let key_start = start + usize::from(quote.is_some());
    let key_end = identifier_end(message, key_start);
    let key = &message[key_start..key_end];
    let cookie = normalized_eq(key, "cookie") || normalized_eq(key, "setcookie");
    let authorization =
        normalized_eq(key, "authorization") || normalized_eq(key, "proxyauthorization");
    if !cookie && !authorization && !is_sensitive_key(key) {
        return None;
    }
    let mut separator = key_end;
    if let Some(quote) = quote {
        if bytes.get(separator) != Some(&quote) {
            return None;
        }
        separator += 1;
    }
    separator = skip_horizontal_whitespace(message, separator);
    if quote == Some(b'"') {
        let json_separator = skip_json_whitespace(message, separator);
        if bytes.get(json_separator) == Some(&b':') {
            separator = json_separator;
        }
    }
    if !matches!(bytes.get(separator), Some(b'=' | b':')) {
        return None;
    }
    let value_start = if quote == Some(b'"') && bytes.get(separator) == Some(&b':') {
        skip_json_whitespace(message, separator + 1)
    } else {
        skip_horizontal_whitespace(message, separator + 1)
    };
    if matches!(bytes.get(value_start), Some(b'"' | b'\'')) {
        return nonempty_value_range(message, value_start);
    }
    if cookie || authorization {
        let quoted_header = is_inside_double_quoted_string(message, start);
        if authorization && !quoted_header {
            if let Some(scheme_end) = auth_scheme_end(message, value_start, AUTH_SCHEMES) {
                return authorization_value_range(message, auth_value_start(message, scheme_end));
            }
        }
        let end = header_value_end(message, value_start, quoted_header);
        return (value_start < end).then_some((value_start, end));
    }
    nonempty_value_range(message, value_start)
}

fn header_value_end(message: &str, start: usize, quoted_header: bool) -> usize {
    let mut index = start;
    while index < message.len() {
        let character = message[index..]
            .chars()
            .next()
            .expect("index always points to a character boundary");
        if matches!(character, '\r' | '\n') {
            break;
        }
        if character == '\\' {
            // Escaped quotes belong to the header, not its enclosing JSON string.
            index += 1;
            if let Some(escaped) = message[index..].chars().next() {
                if !matches!(escaped, '\r' | '\n') {
                    index += escaped.len_utf8();
                }
            }
            continue;
        }
        if quoted_header {
            if character == '"' {
                break;
            }
        } else if matches!(character, '"' | '\'') {
            if message[start..index].trim_end().ends_with('=') {
                let (_, end) = sensitive_value_range(message, index);
                index = end + usize::from(end < message.len());
                continue;
            }
            break;
        }
        index += character.len_utf8();
    }
    index
}

fn is_inside_double_quoted_string(message: &str, end: usize) -> bool {
    let mut quoted = false;
    let mut bytes = message[..end].bytes();
    while let Some(byte) = bytes.next() {
        match byte {
            b'\\' => {
                bytes.next();
            }
            b'"' => quoted = !quoted,
            _ => {}
        }
    }
    quoted
}

fn authorization_redaction_range(message: &str, start: usize) -> Option<(usize, usize)> {
    if !is_boundary_before(message, start) {
        return None;
    }
    // Token and API-key need an authorization or assignment context. In particular,
    // an ordinary sentence such as "token expired" must not lose its next word.
    let scheme_end = auth_scheme_end(message, start, &AUTH_SCHEMES[..3])?;
    authorization_value_range(message, auth_value_start(message, scheme_end))
}

fn auth_scheme_end(message: &str, start: usize, schemes: &[&str]) -> Option<usize> {
    let end = identifier_end(message, start);
    if !schemes
        .iter()
        .any(|scheme| normalized_eq(&message[start..end], scheme))
        || !message
            .as_bytes()
            .get(end)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b':' | b'='))
    {
        return None;
    }
    Some(end)
}

fn auth_value_start(message: &str, start: usize) -> usize {
    start
        + message.as_bytes()[start..]
            .iter()
            .take_while(|byte| matches!(byte, b' ' | b'\t' | b':' | b'='))
            .count()
}

fn authorization_value_range(message: &str, start: usize) -> Option<(usize, usize)> {
    // Token authentication may spell its credential as token="...". Preserve the
    // parameter and its quotes while consuming escaped or whitespace-rich values.
    let parameter_end = identifier_end(message, start);
    let separator = skip_horizontal_whitespace(message, parameter_end);
    if is_sensitive_key(&message[start..parameter_end])
        && message.as_bytes().get(separator) == Some(&b'=')
    {
        if let Some(range) =
            nonempty_value_range(message, skip_horizontal_whitespace(message, separator + 1))
        {
            return Some(range);
        }
    }
    nonempty_value_range(message, start)
}

fn nonempty_value_range(message: &str, start: usize) -> Option<(usize, usize)> {
    let (start, end) = sensitive_value_range(message, start);
    (start < end).then_some((start, end))
}

fn sensitive_value_range(message: &str, start: usize) -> (usize, usize) {
    let bytes = message.as_bytes();
    if let Some(quote @ (b'"' | b'\'')) = bytes.get(start).copied() {
        let mut index = start + 1;
        while index < bytes.len() {
            match bytes[index] {
                b'\\' => {
                    index += 1;
                    if let Some(character) = message[index..].chars().next() {
                        index += character.len_utf8();
                    }
                }
                byte if byte == quote => return (start + 1, index),
                _ => {
                    index += message[index..]
                        .chars()
                        .next()
                        .expect("index always points to a character boundary")
                        .len_utf8();
                }
            }
        }
        return (start + 1, message.len());
    }

    let end = message[start..]
        .find(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '&' | ';' | '#' | ',' | ']' | ')' | '}' | '>' | '"' | '\''
                )
        })
        .map_or(message.len(), |offset| start + offset);
    (start, end)
}

fn redact_url(message: &str, start: usize, output: &mut String) -> Option<usize> {
    if !is_boundary_before(message, start) {
        return None;
    }
    let scheme_len = if starts_with_ascii_case(message, start, "https://") {
        8
    } else if starts_with_ascii_case(message, start, "http://") {
        7
    } else {
        return None;
    };
    let end = url_end(message, start, start + scheme_len);
    let url = &message[start..end];
    // All URL queries and fragments can carry provider-specific credentials.
    let base_end = url.find(['?', '#']).unwrap_or(url.len());
    let base = &url[..base_end];
    let authority_end = base[scheme_len..]
        .find('/')
        .map_or(base.len(), |offset| scheme_len + offset);
    output.push_str(&base[..scheme_len]);
    let authority = &base[scheme_len..authority_end];
    if let Some(userinfo_end) = authority.rfind('@') {
        output.push_str(REDACTED);
        output.push_str(&authority[userinfo_end..]);
    } else {
        output.push_str(authority);
    }

    let mut redact_next_segment = false;
    for segment in base[authority_end..].split('/').skip(1) {
        output.push('/');
        if segment.is_empty() {
            continue;
        }
        if redact_next_segment {
            output.push_str(REDACTED);
            redact_next_segment = false;
        } else if let Some(separator) = segment
            .find(['=', ':'])
            .filter(|&separator| is_sensitive_url_key(&segment[..separator]))
        {
            output.push_str(&segment[..separator + 1]);
            output.push_str(REDACTED);
        } else {
            output.push_str(segment);
            redact_next_segment = is_sensitive_url_path_label(segment);
        }
    }
    Some(end)
}

fn url_end(message: &str, url_start: usize, authority_start: usize) -> usize {
    // Userinfo permits URI sub-delimiters that otherwise look like prose. Look
    // for its terminator only inside this authority, before splitting on them.
    let authority_end = message[authority_start..]
        .find(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '/' | '?' | '#' | '"' | '<' | '>' | '[' | ']' | '{' | '}' | '\\'
                )
        })
        .map_or(message.len(), |offset| authority_start + offset);
    let mut index = message[authority_start..authority_end]
        .rfind('@')
        .map_or(authority_start, |offset| authority_start + offset + 1);
    let wrapped = message[..url_start].ends_with('(');
    let mut ipv6 = false;
    let mut in_authority = true;
    let mut query = false;
    let mut sensitive_segment = false;
    let mut segment_start = index;
    let mut parentheses = 0usize;
    while index < message.len() {
        let character = message[index..]
            .chars()
            .next()
            .expect("index always points to a character boundary");
        if character.is_whitespace()
            || matches!(character, '"' | '<' | '>' | '{' | '}')
            || (character == ']' && !ipv6)
            || (matches!(character, '\'' | '(' | ')' | ',') && !query && !sensitive_segment)
        {
            break;
        }
        if character == ')' && parentheses == 0 && wrapped {
            let remainder = message[index + 1..].trim_start_matches([',', '.', ';', '!']);
            if remainder
                .chars()
                .next()
                .is_none_or(|next| next.is_whitespace() || matches!(next, '"' | '\'' | ']' | '}'))
            {
                break;
            }
        }
        if character == '(' {
            parentheses += 1;
        } else if character == ')' {
            parentheses = parentheses.saturating_sub(1);
        }
        if matches!(character, '?' | '#') {
            query = true;
        } else if !query {
            if character == '/' {
                if in_authority {
                    in_authority = false;
                } else if index > segment_start {
                    sensitive_segment = !sensitive_segment
                        && is_sensitive_url_path_label(&message[segment_start..index]);
                }
                segment_start = index + 1;
            } else if !in_authority && matches!(character, '=' | ':') {
                sensitive_segment |= is_sensitive_url_key(&message[segment_start..index]);
            }
        }
        if character == '[' {
            ipv6 = true;
        } else if character == ']' {
            ipv6 = false;
        } else if character == '\\' {
            // Keep escaped string content inside the URL span rather than exposing
            // the rest of a signed query after an escaped quote.
            index += 1;
            if let Some(escaped) = message[index..].chars().next() {
                index += escaped.len_utf8();
            }
            continue;
        }
        index += character.len_utf8();
    }
    index
}

fn is_sensitive_url_path_label(segment: &str) -> bool {
    is_sensitive_url_key(segment)
        || AUTH_SCHEMES
            .iter()
            .any(|scheme| url_key_eq(segment, scheme))
        || url_key_eq(segment, "authorization")
}

fn is_sensitive_key(key: &str) -> bool {
    SENSITIVE_KEYS
        .iter()
        .any(|candidate| normalized_eq(key, candidate))
}

fn is_sensitive_url_key(key: &str) -> bool {
    SENSITIVE_KEYS
        .iter()
        .any(|candidate| url_key_eq(key, candidate))
}

fn url_key_eq(value: &str, expected: &str) -> bool {
    let mut bytes = value.bytes();
    std::iter::from_fn(|| {
        let byte = bytes.next()?;
        if byte != b'%' {
            return Some(byte);
        }
        let high = bytes.next().and_then(|byte| (byte as char).to_digit(16));
        let low = bytes.next().and_then(|byte| (byte as char).to_digit(16));
        Some(match (high, low) {
            (Some(high), Some(low)) => (high * 16 + low) as u8,
            _ => b'%',
        })
    })
    .filter(|byte| !matches!(byte, b'_' | b'-'))
    .map(|byte| byte.to_ascii_lowercase())
    .eq(expected.bytes())
}

fn normalized_eq(value: &str, expected: &str) -> bool {
    value
        .bytes()
        .filter(|byte| !matches!(byte, b'_' | b'-'))
        .map(|byte| byte.to_ascii_lowercase())
        .eq(expected.bytes())
}

fn identifier_end(message: &str, start: usize) -> usize {
    start
        + message.as_bytes()[start..]
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            .count()
}

fn skip_horizontal_whitespace(message: &str, start: usize) -> usize {
    start
        + message[start..]
            .chars()
            .take_while(|character| character.is_whitespace() && !matches!(character, '\r' | '\n'))
            .map(char::len_utf8)
            .sum::<usize>()
}

fn skip_json_whitespace(message: &str, start: usize) -> usize {
    start
        + message.as_bytes()[start..]
            .iter()
            .take_while(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
            .count()
}

fn starts_with_ascii_case(message: &str, start: usize, needle: &str) -> bool {
    message
        .get(start..start.saturating_add(needle.len()))
        .is_some_and(|value| value.eq_ignore_ascii_case(needle))
}

fn is_boundary_before(message: &str, index: usize) -> bool {
    message[..index]
        .chars()
        .next_back()
        .is_none_or(|character| !character.is_alphanumeric() && !matches!(character, '_' | '-'))
}

#[cfg(test)]
mod tests;
