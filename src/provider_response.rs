use futures::StreamExt as _;
use reqwest::Response;
use serde::de::DeserializeOwned;

pub(crate) const MAX_PROVIDER_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderResponseError {
    TooLarge,
    Read,
    InvalidJson,
}

impl std::fmt::Display for ProviderResponseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::TooLarge => "provider response exceeded the size limit",
            Self::Read => "provider response could not be read",
            Self::InvalidJson => "provider response contained invalid JSON",
        };
        formatter.write_str(message)
    }
}

pub(crate) async fn json<T: DeserializeOwned>(
    response: Response,
) -> Result<T, ProviderResponseError> {
    let bytes = bytes(response, MAX_PROVIDER_RESPONSE_BYTES).await?;
    serde_json::from_slice(&bytes).map_err(|_| ProviderResponseError::InvalidJson)
}

pub(crate) async fn bytes(
    response: Response,
    maximum: usize,
) -> Result<Vec<u8>, ProviderResponseError> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(ProviderResponseError::TooLarge);
    }

    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or_default()
        .min(maximum);
    let mut output = Vec::with_capacity(capacity);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ProviderResponseError::Read)?;
        append_limited(&mut output, &chunk, maximum)?;
    }
    Ok(output)
}

fn append_limited(
    output: &mut Vec<u8>,
    chunk: &[u8],
    maximum: usize,
) -> Result<(), ProviderResponseError> {
    if chunk.len() > maximum.saturating_sub(output.len()) {
        return Err(ProviderResponseError::TooLarge);
    }
    output.extend_from_slice(chunk);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_append_accepts_the_exact_limit() {
        let mut output = b"abc".to_vec();
        assert_eq!(append_limited(&mut output, b"def", 6), Ok(()));
        assert_eq!(output, b"abcdef");
    }

    #[test]
    fn bounded_append_rejects_overflow_without_mutating_output() {
        let mut output = b"abc".to_vec();
        assert_eq!(
            append_limited(&mut output, b"defg", 6),
            Err(ProviderResponseError::TooLarge)
        );
        assert_eq!(output, b"abc");
    }
}
