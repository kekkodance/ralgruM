use futures::StreamExt as _;

/// Reads a provider response while enforcing a hard byte budget, so an
/// unexpected or hostile body cannot exhaust memory.
pub(crate) async fn read_response_limited(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(format!("response body exceeded the {limit}-byte limit"));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream
        .next()
        .await
        .transpose()
        .map_err(|error| format!("response body read failed: {error}"))?
    {
        if chunk.len() > limit.saturating_sub(body.len()) {
            return Err(format!("response body exceeded the {limit}-byte limit"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
