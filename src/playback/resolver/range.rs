use super::*;

pub(super) fn content_range_total(headers: &header::HeaderMap) -> Option<u64> {
    let value = headers.get(header::CONTENT_RANGE)?.to_str().ok()?;
    let total = value.rsplit_once('/')?.1.trim();
    let total = total.parse::<u64>().ok()?;
    (total > 0).then_some(total)
}

pub(super) fn validate_download_range_response(
    status: StatusCode,
    headers: &header::HeaderMap,
    expected_start: u64,
    expected_end: u64,
    expected_total: u64,
) -> Result<(), String> {
    if status != StatusCode::PARTIAL_CONTENT {
        return Err("The audio provider did not honor the byte range request".into());
    }

    let value = headers
        .get(header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| "The audio provider omitted the content range".to_string())?;
    let (unit, value) = value
        .trim()
        .split_once(' ')
        .ok_or_else(|| "The audio provider returned an invalid content range".to_string())?;
    let (range, total) = value
        .trim()
        .split_once('/')
        .ok_or_else(|| "The audio provider returned an invalid content range".to_string())?;
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| "The audio provider returned an invalid content range".to_string())?;
    let start = start
        .parse::<u64>()
        .map_err(|_| "The audio provider returned an invalid content range".to_string())?;
    let end = end
        .parse::<u64>()
        .map_err(|_| "The audio provider returned an invalid content range".to_string())?;
    let total = total
        .parse::<u64>()
        .map_err(|_| "The audio provider returned an invalid content range".to_string())?;

    if !unit.eq_ignore_ascii_case("bytes")
        || start != expected_start
        || end != expected_end
        || total != expected_total
    {
        return Err("The audio provider returned an unexpected content range".into());
    }

    Ok(())
}

pub(super) fn cacheable_size(size: u64, max_bytes: u64) -> Option<u64> {
    (size > 0).then_some(size).filter(|size| *size <= max_bytes)
}

pub(super) fn prefetch_range(size: u64) -> Option<(u64, u64)> {
    (size > 0).then(|| (0, size.min(BLOCK_SIZE) - 1))
}

pub(super) fn inline_range(
    bytes: &[u8],
    start: u64,
    end: u64,
) -> Result<Vec<u8>, PlaybackDownloadError> {
    let start = usize::try_from(start)
        .map_err(|_| PlaybackDownloadError::message("The inline source range was invalid"))?;
    let end = usize::try_from(end)
        .map_err(|_| PlaybackDownloadError::message("The inline source range was invalid"))?;
    bytes
        .get(start..=end)
        .map(|bytes| bytes.to_vec())
        .ok_or_else(|| PlaybackDownloadError::message("The inline source range was unavailable"))
}

pub(super) fn ranged_body(
    status: StatusCode,
    bytes: &[u8],
    start: u64,
    end: u64,
) -> Result<Vec<u8>, String> {
    let expected = (end - start + 1) as usize;
    let offset = if status == StatusCode::PARTIAL_CONTENT {
        0
    } else {
        start as usize
    };
    bytes
        .get(offset..offset + expected)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| "The audio provider returned a truncated range".into())
}

pub(super) async fn read_response_range(
    response: Response,
    start: u64,
    end: u64,
) -> Result<(StatusCode, Option<u64>, Vec<u8>), PlaybackDownloadError> {
    let status = response.status();
    let content_length = response.content_length();
    let expected = (end - start + 1) as usize;
    let mut skip = if status != StatusCode::PARTIAL_CONTENT {
        start
    } else {
        0
    };
    let mut bytes = Vec::with_capacity(expected);
    let mut stream = response.bytes_stream();
    while bytes.len() < expected {
        let Some(chunk) = stream.next().await else {
            break;
        };
        let chunk = chunk.map_err(request_error)?;
        let skipped = skip.min(chunk.len() as u64) as usize;
        skip -= skipped as u64;
        let chunk = &chunk[skipped..];
        let take = (expected - bytes.len()).min(chunk.len());
        bytes.extend_from_slice(&chunk[..take]);
    }
    if skip != 0 || bytes.len() != expected {
        return Err("The audio provider returned a truncated range".into());
    }
    Ok((status, content_length, bytes))
}

pub(super) fn aligned_range(start: u64, end: u64, total: u64) -> (u64, u64) {
    let aligned_start = start / STRIPE_SIZE as u64 * STRIPE_SIZE as u64;
    let aligned_end =
        (((end + 1).div_ceil(STRIPE_SIZE as u64) * STRIPE_SIZE as u64) - 1).min(total - 1);
    (aligned_start, aligned_end)
}

pub(super) fn trim_range(
    bytes: Vec<u8>,
    start: u64,
    end: u64,
    aligned_start: u64,
) -> Result<Vec<u8>, String> {
    let skip = (start - aligned_start) as usize;
    let length = (end - start + 1) as usize;
    bytes
        .get(skip..skip + length)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| "The audio provider returned a truncated aligned range".into())
}
