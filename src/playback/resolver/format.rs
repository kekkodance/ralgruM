use super::*;

pub(super) fn validate_soundcloud_transcoding_url(url: &reqwest::Url) -> Result<(), String> {
    if url.scheme() == "https" && url.host_str() == Some("api-v2.soundcloud.com") {
        Ok(())
    } else {
        Err("SoundCloud returned an unexpected transcoding URL".into())
    }
}

pub(super) const MAX_SOUNDCLOUD_TRACK_AUTHORIZATION_LENGTH: usize = 512;

pub(super) fn soundcloud_track_authorization(track: &Value) -> Option<&str> {
    let raw = track.get("track_authorization").and_then(Value::as_str)?;
    if raw.len() > MAX_SOUNDCLOUD_TRACK_AUTHORIZATION_LENGTH
        || !raw.is_ascii()
        || raw.chars().any(char::is_control)
    {
        return None;
    }
    let value = raw.trim();
    (!value.is_empty()).then_some(value)
}

pub(super) fn append_soundcloud_transcoding_query(
    url: &mut reqwest::Url,
    track_authorization: Option<&str>,
) {
    let preserved = url
        .query_pairs()
        .filter(|(key, _)| key != "client_id" && key != "track_authorization")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    url.set_query(None);
    let mut query = url.query_pairs_mut();
    for (key, value) in preserved {
        query.append_pair(&key, &value);
    }
    query.append_pair("client_id", SOUNDCLOUD_CLIENT_ID);
    if let Some(track_authorization) = track_authorization {
        query.append_pair("track_authorization", track_authorization);
    }
}

pub(super) fn validate_soundcloud_stream_url(url: &reqwest::Url) -> Result<(), String> {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let allowed = host == "api-v2.soundcloud.com"
        || host == "media.soundcloud.com"
        || host == "sndcdn.com"
        || host.ends_with(".sndcdn.com");
    if url.scheme() == "https" && allowed {
        Ok(())
    } else {
        Err("SoundCloud returned an unexpected stream URL".into())
    }
}

// Logical SoundCloud identity and Murglar/Deezer media transport are intentionally separate.
pub(super) fn validate_media_response_url(
    url: &reqwest::Url,
    is_soundcloud: bool,
) -> Result<(), String> {
    if is_soundcloud {
        validate_soundcloud_stream_url(url)
    } else {
        Ok(())
    }
}

pub(super) fn soundcloud_transcodings(transcodings: &[Value]) -> impl Iterator<Item = &Value> {
    transcodings.iter().filter(|item| {
        let protocol = item
            .pointer("/format/protocol")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        protocol == "progressive"
            && !is_encrypted_soundcloud_protocol(&protocol)
            && item.get("url").and_then(Value::as_str).is_some()
    })
}

pub(super) fn soundcloud_playback_transcodings(transcodings: &[Value]) -> Vec<&Value> {
    let mut hls = transcodings
        .iter()
        .filter(|item| is_soundcloud_hls_transcoding(item))
        .collect::<Vec<_>>();
    hls.sort_by_key(|item| Reverse(soundcloud_transcoding_bitrate(item).unwrap_or_default()));
    hls.extend(soundcloud_transcodings(transcodings));
    hls
}

pub(super) fn is_soundcloud_hls_transcoding(value: &Value) -> bool {
    let protocol = value
        .pointer("/format/protocol")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let mime_type = value
        .pointer("/format/mime_type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let preset = value
        .get("preset")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    protocol == "hls"
        && mime_type == "audio/mp4"
        && preset.starts_with("aac_")
        && soundcloud_transcoding_bitrate(value).is_some()
        && value.get("url").and_then(Value::as_str).is_some()
}

pub(super) fn is_encrypted_soundcloud_protocol(protocol: &str) -> bool {
    ["encrypted", "cbc", "ctr"]
        .iter()
        .any(|term| protocol.contains(term))
}

pub(super) fn transcoding_format(value: &Value) -> AudioFormat {
    let hint = format!(
        "{} {}",
        value
            .pointer("/format/mime_type")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        value
            .get("preset")
            .and_then(Value::as_str)
            .unwrap_or_default()
    );
    infer_soundcloud_format(
        &json!({"filename": hint}),
        &reqwest::Url::parse("https://cf-media.sndcdn.com/stream").unwrap(),
    )
}

pub(super) fn soundcloud_format_name(value: &Value, format: AudioFormat) -> String {
    let hint = format!(
        "{} {}",
        value
            .pointer("/format/mime_type")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        value
            .get("preset")
            .and_then(Value::as_str)
            .unwrap_or_default()
    )
    .to_ascii_lowercase();
    if format == AudioFormat::M4a {
        format.label().to_owned()
    } else if hint.contains("aac") {
        "AAC".into()
    } else if hint.contains("opus") {
        "Opus".into()
    } else if hint.contains("ogg") || hint.contains("vorbis") {
        "OGG".into()
    } else if hint.contains("mp3") || hint.contains("mpeg") {
        "MP3".into()
    } else {
        format.label().to_owned()
    }
}

pub(super) fn soundcloud_transcoding_bitrate(value: &Value) -> Option<u32> {
    value_bitrate(value)
}

pub(super) fn value_bitrate(value: &Value) -> Option<u32> {
    [
        value.get("bitrate"),
        value.get("quality"),
        value.get("preset"),
        value.get("format"),
        value.get("filename"),
    ]
    .into_iter()
    .flatten()
    .filter_map(|value| match value {
        Value::String(value) => declared_bitrate(value),
        Value::Number(value) => value
            .as_u64()
            .filter(|value| (16..=10_000).contains(value))
            .and_then(|value| u32::try_from(value).ok()),
        _ => None,
    })
    .next()
}

pub(super) fn declared_bitrate(value: &str) -> Option<u32> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|part| part.parse::<u32>().ok())
        .rfind(|value| (16..=10_000).contains(value))
}

pub(super) fn soundcloud_original_bitrate(value: &Value, format: AudioFormat) -> Option<u32> {
    let bitrate = value_bitrate(value)?;
    match format {
        AudioFormat::Mp3
        | AudioFormat::OggVorbis
        | AudioFormat::OggOpus
        | AudioFormat::Aac
        | AudioFormat::M4a => (16..=640).contains(&bitrate).then_some(bitrate),
        AudioFormat::Flac | AudioFormat::Wav | AudioFormat::Aiff => Some(bitrate),
    }
}

pub(super) fn choose_soundcloud_original_format(
    sniffed_format: Option<AudioFormat>,
    header_format: Option<AudioFormat>,
    provider_format: Option<AudioFormat>,
) -> Option<AudioFormat> {
    sniffed_format.or(header_format).or(provider_format)
}

pub(super) fn soundcloud_original_head_can_fallback(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::FORBIDDEN
            | StatusCode::NOT_FOUND
            | StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::NOT_IMPLEMENTED
    )
}

pub(super) fn infer_soundcloud_original_format(
    value: &Value,
    url: &reqwest::Url,
) -> Option<AudioFormat> {
    infer_soundcloud_provider_format(value).or_else(|| soundcloud_format_from_url_path(url))
}

pub(super) fn infer_soundcloud_provider_format(value: &Value) -> Option<AudioFormat> {
    ["original_format", "format", "mime_type", "filename"]
        .into_iter()
        .find_map(|key| value.get(key).and_then(soundcloud_format_from_value))
}

pub(super) fn soundcloud_format_from_value(value: &Value) -> Option<AudioFormat> {
    match value {
        Value::String(value) => soundcloud_format_from_hint(value),
        Value::Object(_) => [
            "original_format",
            "mime_type",
            "filename",
            "format",
            "extension",
        ]
        .into_iter()
        .find_map(|key| value.get(key).and_then(soundcloud_format_from_value)),
        Value::Array(values) => values.iter().find_map(soundcloud_format_from_value),
        _ => None,
    }
}

pub(super) fn soundcloud_format_from_hint(value: &str) -> Option<AudioFormat> {
    let value = value.trim().trim_matches('"').trim_matches('\'');
    if value.is_empty() {
        return None;
    }
    if let Some(format) = soundcloud_format_from_mime(value.split(';').next()?.trim()) {
        return Some(format);
    }
    let token = value.to_ascii_lowercase();
    if let Some((_, extension)) = token.rsplit_once('.')
        && let Some(format) = soundcloud_format_from_extension(extension)
    {
        return Some(format);
    }
    if token.contains('/') || token.chars().any(char::is_whitespace) {
        return None;
    }
    match token.as_str() {
        "flac" => Some(AudioFormat::Flac),
        "wav" | "wave" => Some(AudioFormat::Wav),
        "aiff" | "aif" | "aifc" => Some(AudioFormat::Aiff),
        "mp3" | "mpeg" => Some(AudioFormat::Mp3),
        "ogg" | "vorbis" => Some(AudioFormat::OggVorbis),
        "opus" => Some(AudioFormat::OggOpus),
        "aac" | "adts" => Some(AudioFormat::Aac),
        "m4a" | "mp4" => Some(AudioFormat::M4a),
        _ if token.strip_prefix("mp3_").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        }) =>
        {
            Some(AudioFormat::Mp3)
        }
        _ => token
            .rsplit_once('.')
            .and_then(|(_, extension)| soundcloud_format_from_extension(extension)),
    }
}

pub(super) fn soundcloud_format_from_mime(value: &str) -> Option<AudioFormat> {
    match value.trim().to_ascii_lowercase().as_str() {
        "audio/flac" | "audio/x-flac" | "application/flac" => Some(AudioFormat::Flac),
        "audio/wav" | "audio/wave" | "audio/x-wav" | "audio/vnd.wave" => Some(AudioFormat::Wav),
        "audio/aiff" | "audio/x-aiff" | "audio/aif" | "audio/x-aif" => Some(AudioFormat::Aiff),
        "audio/mpeg" | "audio/mp3" => Some(AudioFormat::Mp3),
        "audio/ogg" | "application/ogg" => Some(AudioFormat::OggVorbis),
        "audio/opus" => Some(AudioFormat::OggOpus),
        "audio/aac" | "audio/aacp" | "audio/x-aac" => Some(AudioFormat::Aac),
        "audio/mp4" | "video/mp4" | "audio/x-m4a" | "application/mp4" => Some(AudioFormat::M4a),
        _ => None,
    }
}

pub(super) fn soundcloud_format_from_extension(value: &str) -> Option<AudioFormat> {
    match value.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "flac" => Some(AudioFormat::Flac),
        "wav" | "wave" => Some(AudioFormat::Wav),
        "aiff" | "aif" | "aifc" => Some(AudioFormat::Aiff),
        "mp3" => Some(AudioFormat::Mp3),
        "ogg" | "oga" => Some(AudioFormat::OggVorbis),
        "opus" => Some(AudioFormat::OggOpus),
        "aac" => Some(AudioFormat::Aac),
        "m4a" | "mp4" => Some(AudioFormat::M4a),
        _ => None,
    }
}

pub(super) fn soundcloud_format_from_url_path(url: &reqwest::Url) -> Option<AudioFormat> {
    let filename = url.path().trim_end_matches('/').rsplit('/').next()?;
    let extension = filename.rsplit_once('.')?.1;
    soundcloud_format_from_extension(extension)
}

pub(super) fn soundcloud_format_from_media_headers(
    headers: &header::HeaderMap,
    url: &reqwest::Url,
) -> Option<AudioFormat> {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| soundcloud_format_from_mime(value.split(';').next()?.trim()))
        .or_else(|| {
            headers
                .get(header::CONTENT_DISPOSITION)
                .and_then(|value| value.to_str().ok())
                .and_then(soundcloud_content_disposition_filename)
                .and_then(|value| soundcloud_format_from_filename(&value))
        })
        .or_else(|| soundcloud_format_from_url_path(url))
}

pub(super) fn soundcloud_format_from_filename(value: &str) -> Option<AudioFormat> {
    let filename = value.trim().trim_matches('"').rsplit(['/', '\\']).next()?;
    let extension = filename.rsplit_once('.')?.1;
    soundcloud_format_from_extension(extension)
}

pub(super) fn soundcloud_content_disposition_filename(value: &str) -> Option<String> {
    let mut parameters = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in value.chars() {
        if character == '"' && !escaped {
            quoted = !quoted;
        }
        if character == ';' && !quoted {
            parameters.push(std::mem::take(&mut current));
        } else {
            current.push(character);
        }
        escaped = character == '\\' && !escaped;
        if character != '\\' {
            escaped = false;
        }
    }
    parameters.push(current);

    for name in ["filename*", "filename"] {
        for parameter in &parameters {
            let Some((key, raw_value)) = parameter.split_once('=') else {
                continue;
            };
            if !key.trim().eq_ignore_ascii_case(name) {
                continue;
            }
            let filename = raw_value.trim().trim_matches('"');
            let filename = filename
                .split_once("''")
                .map_or(filename, |(_, filename)| filename);
            return Some(percent_decode_filename(filename));
        }
    }
    None
}

pub(super) fn percent_decode_filename(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let (Some(high), Some(low)) = (bytes.get(index + 1), bytes.get(index + 2))
            && let (Some(high), Some(low)) = (hex_digit(*high), hex_digit(*low))
        {
            decoded.push(high << 4 | low);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

pub(super) fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

pub(super) fn sniff_soundcloud_original_format(bytes: &[u8]) -> Option<AudioFormat> {
    if bytes.starts_with(b"fLaC") {
        return Some(AudioFormat::Flac);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE" {
        return Some(AudioFormat::Wav);
    }
    if bytes.len() >= 12
        && bytes.starts_with(b"FORM")
        && (&bytes[8..12] == b"AIFF" || &bytes[8..12] == b"AIFC")
    {
        return Some(AudioFormat::Aiff);
    }
    if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
        return Some(AudioFormat::M4a);
    }
    if bytes.starts_with(b"OggS") {
        if bytes
            .windows(b"OpusHead".len())
            .any(|window| window == b"OpusHead")
        {
            return Some(AudioFormat::OggOpus);
        }
        if bytes
            .windows(b"vorbis".len())
            .any(|window| window == b"vorbis")
        {
            return Some(AudioFormat::OggVorbis);
        }
    }
    if bytes.starts_with(b"ID3") || looks_like_aac_adts(bytes) {
        return if bytes.starts_with(b"ID3") {
            Some(AudioFormat::Mp3)
        } else {
            Some(AudioFormat::Aac)
        };
    }
    looks_like_mp3(bytes).then_some(AudioFormat::Mp3)
}

pub(super) fn looks_like_aac_adts(bytes: &[u8]) -> bool {
    bytes
        .windows(2)
        .any(|window| window[0] == 0xff && (window[1] & 0xf6) == 0xf0)
}

pub(super) async fn read_bounded_response(
    mut response: Response,
    max_bytes: usize,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(max_bytes);
    while bytes.len() < max_bytes {
        let chunk = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                return Err("Playback request cancelled".into());
            }
            result = response.chunk() => result.map_err(request_error)?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        let remaining = max_bytes - bytes.len();
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    Ok(bytes)
}

pub(super) fn infer_soundcloud_format(value: &Value, url: &reqwest::Url) -> AudioFormat {
    let hint = format!(
        "{} {url}",
        value
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or_default()
    )
    .to_ascii_lowercase();
    if hint.contains("flac") {
        AudioFormat::Flac
    } else if hint.contains("aiff") || hint.contains("aif") {
        AudioFormat::Aiff
    } else if hint.contains("wav") {
        AudioFormat::Wav
    } else if hint.contains("opus") {
        AudioFormat::OggOpus
    } else if hint.contains("ogg") || hint.contains("vorbis") {
        AudioFormat::OggVorbis
    } else if hint.contains("m4a") || hint.contains("audio/mp4") {
        AudioFormat::M4a
    } else if hint.contains("aac") {
        AudioFormat::Aac
    } else {
        AudioFormat::Mp3
    }
}

pub(super) fn audio_format(value: &str) -> Option<AudioFormat> {
    let value = value.to_ascii_uppercase();
    if value.contains("FLAC") {
        Some(AudioFormat::Flac)
    } else if value.contains("MP3") || value.contains("MPEG") {
        Some(AudioFormat::Mp3)
    } else if value.contains("M4A") || value.contains("MP4") {
        Some(AudioFormat::M4a)
    } else if value.contains("AAC") {
        Some(AudioFormat::Aac)
    } else if value.contains("OPUS") {
        Some(AudioFormat::OggOpus)
    } else if value.contains("OGG") || value.contains("VORBIS") {
        Some(AudioFormat::OggVorbis)
    } else if value.contains("WAV") {
        Some(AudioFormat::Wav)
    } else if value.contains("AIFF") || value.contains("AIFC") {
        Some(AudioFormat::Aiff)
    } else {
        None
    }
}

pub(super) fn looks_like_mp3(bytes: &[u8]) -> bool {
    bytes.starts_with(b"ID3")
        || bytes.windows(2).take(4096).any(|window| {
            window[0] == 0xff && (window[1] & 0xe0) == 0xe0 && (window[1] & 0x06) != 0
        })
}

pub(super) async fn validate_audio_output(
    path: &std::path::Path,
    format: AudioFormat,
) -> Result<(), String> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|_| "The resolved audio output could not be inspected".to_string())?;
    if metadata.len() == 0 {
        crate::diagnostics::event("ERROR", "audio output failure length=0");
        return Err("The resolved audio output was empty".into());
    }
    if format == AudioFormat::Flac {
        let mut file = File::open(path)
            .await
            .map_err(|_| "The resolved FLAC output could not be inspected".to_string())?;
        let mut header = [0; 4];
        if file.read_exact(&mut header).await.is_err() {
            crate::diagnostics::event(
                "ERROR",
                format!("audio output failure length={}", metadata.len()),
            );
            return Err("The resolved FLAC output was truncated".into());
        }
        if &header != b"fLaC" {
            crate::diagnostics::event(
                "ERROR",
                format!("audio output failure length={}", metadata.len()),
            );
            return Err("The resolved FLAC output did not start with fLaC".into());
        }
    }
    Ok(())
}

pub(super) fn validate_progressive_prefix(
    path: &std::path::Path,
    format: AudioFormat,
) -> Result<(), String> {
    if format != AudioFormat::Flac {
        return Ok(());
    }
    let mut file = std::fs::File::open(path)
        .map_err(|_| "The resolved FLAC output could not be inspected".to_string())?;
    let mut header = [0; 4];
    file.read_exact(&mut header)
        .map_err(|_| "The resolved FLAC output was truncated".to_string())?;
    if &header != b"fLaC" {
        return Err("The resolved FLAC output did not start with fLaC".into());
    }
    Ok(())
}

pub(super) fn normalize_release_date(value: &str) -> Option<String> {
    let date = value.trim().split(['T', ' ']).next()?;
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .map(|date| date.format("%Y-%m-%d").to_string())
}
