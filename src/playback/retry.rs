use std::{
    future::Future,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{RequestBuilder, Response, StatusCode, header::HeaderMap};
use tokio_util::sync::CancellationToken;

const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(150);
pub(crate) const MAX_RETRIES: usize = 2;
pub(crate) const MAX_RETRY_DELAY: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestClass {
    Provider,
    Media,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RetryClass {
    None,
    Transient,
    ExpiredMedia,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RetryBudget {
    retries: usize,
    source_refreshes: usize,
}

impl RetryBudget {
    pub(crate) fn take_retry(&mut self) -> bool {
        if self.retries >= MAX_RETRIES {
            return false;
        }
        self.retries += 1;
        true
    }

    pub(crate) fn take_source_refresh(&mut self) -> bool {
        if self.source_refreshes != 0 {
            return false;
        }
        self.source_refreshes = 1;
        true
    }

    #[cfg(test)]
    fn retries(&self) -> usize {
        self.retries
    }
}

pub(crate) fn classify_status(status: StatusCode, class: RequestClass) -> RetryClass {
    if is_transient_status(status) {
        RetryClass::Transient
    } else if class == RequestClass::Media && is_expired_media_status(status) {
        RetryClass::ExpiredMedia
    } else {
        RetryClass::None
    }
}

pub(crate) fn is_transient_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_EARLY | StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
}

pub(crate) fn is_expired_media_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND | StatusCode::GONE
    )
}

pub(crate) fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds).min(MAX_RETRY_DELAY));
    }
    let date = parse_http_date(value)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let seconds = date.saturating_sub(now as i64);
    Some(Duration::from_secs(seconds.max(0) as u64).min(MAX_RETRY_DELAY))
}

fn parse_http_date(value: &str) -> Option<i64> {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    let (year, month, day, time) = match parts.as_slice() {
        [_, day, month, year, time, "GMT"] => (
            year.parse::<i64>().ok()?,
            month_number(month)?,
            day.parse::<u32>().ok()?,
            *time,
        ),
        [_, date, time, "GMT"] => {
            let mut date_parts = date.split('-');
            let day = date_parts.next()?.parse::<u32>().ok()?;
            let month = month_number(date_parts.next()?)?;
            let short_year = date_parts.next()?.parse::<i64>().ok()?;
            if date_parts.next().is_some() {
                return None;
            }
            let year = if short_year >= 50 {
                1900 + short_year
            } else {
                2000 + short_year
            };
            (year, month, day, *time)
        }
        [_, month, day, time, year] => (
            year.parse::<i64>().ok()?,
            month_number(month)?,
            day.parse::<u32>().ok()?,
            *time,
        ),
        _ => return None,
    };
    let (hour, minute, second) = parse_clock(time)?;
    let days = unix_days(year, month, day)?;
    Some(
        days.saturating_mul(86_400)
            .saturating_add(i64::from(hour) * 3_600)
            .saturating_add(i64::from(minute) * 60)
            .saturating_add(i64::from(second)),
    )
}

fn month_number(value: &str) -> Option<u32> {
    Some(match value.trim_end_matches(',') {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

fn parse_clock(value: &str) -> Option<(u32, u32, u32)> {
    let mut parts = value.split(':');
    let hour = parts.next()?.parse::<u32>().ok()?;
    let minute = parts.next()?.parse::<u32>().ok()?;
    let second = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some((hour, minute, second))
}

fn unix_days(year: i64, month: u32, day: u32) -> Option<i64> {
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => return None,
    };
    if day == 0 || day > max_day {
        return None;
    }
    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year / 400
    } else {
        (adjusted_year - 399) / 400
    };
    let year_of_era = adjusted_year - era * 400;
    let month_offset = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_offset + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub(crate) fn retry_delay(retry_index: usize, retry_after: Option<Duration>) -> Duration {
    let fallback = INITIAL_RETRY_DELAY
        .checked_mul(1_u32.checked_shl(retry_index.min(4) as u32).unwrap_or(16))
        .unwrap_or(MAX_RETRY_DELAY);
    retry_after.unwrap_or(fallback).min(MAX_RETRY_DELAY)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TooManyRequestsPolicy {
    Retry,
    Return,
}

pub(crate) async fn send_with_retry<F>(
    stage: &'static str,
    class: RequestClass,
    cancellation: &CancellationToken,
    request: F,
) -> Result<Response, String>
where
    F: FnMut() -> RequestBuilder,
{
    send_with_retry_with_gate(
        stage,
        class,
        cancellation,
        TooManyRequestsPolicy::Retry,
        || async { Ok::<(), String>(()) },
        request,
    )
    .await
}

pub(crate) async fn send_with_retry_with_gate<F, G, GateFuture>(
    stage: &'static str,
    class: RequestClass,
    cancellation: &CancellationToken,
    too_many_requests: TooManyRequestsPolicy,
    mut gate: G,
    request: F,
) -> Result<Response, String>
where
    F: FnMut() -> RequestBuilder,
    G: FnMut() -> GateFuture,
    GateFuture: Future<Output = Result<(), String>>,
{
    send_with_retry_core(
        stage,
        class,
        cancellation,
        too_many_requests,
        &mut gate,
        request,
    )
    .await
}

async fn send_with_retry_core<F, G, GateFuture>(
    stage: &'static str,
    class: RequestClass,
    cancellation: &CancellationToken,
    too_many_requests: TooManyRequestsPolicy,
    gate: &mut G,
    mut request: F,
) -> Result<Response, String>
where
    F: FnMut() -> RequestBuilder,
    G: FnMut() -> GateFuture,
    GateFuture: Future<Output = Result<(), String>>,
{
    let mut budget = RetryBudget::default();
    loop {
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        let request = request();
        gate().await?;
        let result = tokio::select! {
            _ = cancellation.cancelled() => return Err("Playback request cancelled".into()),
            result = request.send() => result,
        };
        let response = match result {
            Ok(response) => response,
            Err(error) if is_retryable_error(&error) && budget.take_retry() => {
                let delay = retry_delay(budget.retries.saturating_sub(1), None);
                let error_kind = request_error_kind(&error);
                crate::diagnostics::event(
                    "WARN",
                    format!(
                        "playback provider stage={stage} transport={error_kind} retry=true attempt={} delay_ms={}",
                        budget.retries,
                        delay.as_millis()
                    ),
                );
                tokio::select! {
                    _ = cancellation.cancelled() => return Err("Playback request cancelled".into()),
                    _ = tokio::time::sleep(delay) => {}
                }
                continue;
            }
            Err(error) => {
                let error_kind = request_error_kind(&error);
                crate::diagnostics::event(
                    "WARN",
                    format!("playback provider stage={stage} transport={error_kind} retry=false"),
                );
                return Err(request_error(stage, &error));
            }
        };
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let retry_class = classify_status_for_policy(status, class, too_many_requests);
        if retry_class != RetryClass::Transient || !budget.take_retry() {
            crate::diagnostics::event(
                "WARN",
                format!(
                    "playback provider stage={stage} status={status} retry=false class={retry_class:?}"
                ),
            );
            return Ok(response);
        }
        let delay = retry_delay(
            budget.retries.saturating_sub(1),
            retry_after(response.headers()),
        );
        crate::diagnostics::event(
            "WARN",
            format!(
                "playback provider stage={stage} status={status} retry=true attempt={} delay_ms={} class={retry_class:?}",
                budget.retries,
                delay.as_millis()
            ),
        );
        tokio::select! {
            _ = cancellation.cancelled() => return Err("Playback request cancelled".into()),
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

fn classify_status_for_policy(
    status: StatusCode,
    class: RequestClass,
    too_many_requests: TooManyRequestsPolicy,
) -> RetryClass {
    if too_many_requests == TooManyRequestsPolicy::Return && status == StatusCode::TOO_MANY_REQUESTS
    {
        RetryClass::None
    } else {
        classify_status(status, class)
    }
}

fn is_retryable_error(error: &reqwest::Error) -> bool {
    !error.is_builder()
        && !error.is_body()
        && !error.is_decode()
        && !error.is_redirect()
        && !error.is_status()
        && (error.is_timeout() || error.is_connect() || error.is_request())
}

fn request_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_request() {
        "request"
    } else if error.is_builder() {
        "builder"
    } else if error.is_body() {
        "body"
    } else if error.is_decode() {
        "decode"
    } else if error.is_redirect() {
        "redirect"
    } else if error.is_status() {
        "status"
    } else {
        "other"
    }
}

fn request_error(stage: &str, error: &reqwest::Error) -> String {
    let reason = match request_error_kind(error) {
        "timeout" => "timed out",
        "builder" => "could not be prepared",
        "body" => "could not send its body",
        _ => "could not be reached",
    };
    format!("The {stage} playback request {reason}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::{Shutdown, TcpListener, TcpStream},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
    };

    fn read_request_headers(stream: &mut TcpStream) {
        const HEADER_END: &[u8] = b"\r\n\r\n";
        const MAX_HEADER_BYTES: usize = 16 * 1024;

        let mut request = Vec::with_capacity(1024);
        let mut buffer = [0_u8; 1024];
        while !request
            .windows(HEADER_END.len())
            .any(|window| window == HEADER_END)
        {
            let bytes_read = stream.read(&mut buffer).unwrap();
            assert!(
                bytes_read > 0,
                "client closed before completing request headers"
            );
            request.extend_from_slice(&buffer[..bytes_read]);
            assert!(
                request.len() <= MAX_HEADER_BYTES,
                "test request headers exceeded {MAX_HEADER_BYTES} bytes"
            );
        }
    }

    fn status_server(
        statuses: Vec<StatusCode>,
    ) -> (String, Arc<AtomicUsize>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let server_attempts = Arc::clone(&attempts);
        let server = thread::spawn(move || {
            for status in statuses {
                let (mut stream, _) = listener.accept().unwrap();
                read_request_headers(&mut stream);
                server_attempts.fetch_add(1, Ordering::SeqCst);
                let reason = status.canonical_reason().unwrap_or("Test Status");
                write!(
                    stream,
                    "HTTP/1.1 {} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    status.as_u16()
                )
                .unwrap();
                stream.flush().unwrap();
                stream.shutdown(Shutdown::Write).unwrap();
            }
        });
        (format!("http://{address}/"), attempts, server)
    }

    #[test]
    fn transient_statuses_are_limited_to_provider_retry_conditions() {
        for status in [
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::TOO_EARLY,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            assert_eq!(
                classify_status(status, RequestClass::Provider),
                RetryClass::Transient
            );
        }
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
        ] {
            assert_eq!(
                classify_status(status, RequestClass::Provider),
                RetryClass::None
            );
        }
    }

    #[test]
    fn media_expiry_is_distinguished_from_session_rejection() {
        assert_eq!(
            classify_status(StatusCode::FORBIDDEN, RequestClass::Media),
            RetryClass::ExpiredMedia
        );
        assert_eq!(
            classify_status(StatusCode::FORBIDDEN, RequestClass::Provider),
            RetryClass::None
        );
        assert_eq!(
            classify_status(StatusCode::SERVICE_UNAVAILABLE, RequestClass::Media),
            RetryClass::Transient
        );
    }

    #[test]
    fn return_policy_does_not_retry_too_many_requests() {
        assert_eq!(
            classify_status_for_policy(
                StatusCode::TOO_MANY_REQUESTS,
                RequestClass::Provider,
                TooManyRequestsPolicy::Return,
            ),
            RetryClass::None
        );
        assert_eq!(
            classify_status_for_policy(
                StatusCode::TOO_MANY_REQUESTS,
                RequestClass::Provider,
                TooManyRequestsPolicy::Retry,
            ),
            RetryClass::Transient
        );
        assert_eq!(
            classify_status_for_policy(
                StatusCode::TOO_MANY_REQUESTS,
                RequestClass::Media,
                TooManyRequestsPolicy::Return,
            ),
            RetryClass::None
        );
        assert_eq!(
            classify_status_for_policy(
                StatusCode::TOO_MANY_REQUESTS,
                RequestClass::Media,
                TooManyRequestsPolicy::Retry,
            ),
            RetryClass::Transient
        );
    }

    #[tokio::test]
    async fn custom_gate_runs_before_every_server_retry_attempt() {
        let (url, server_attempts, server) = status_server(vec![
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::OK,
        ]);
        let cancellation = CancellationToken::new();
        let gated_attempts = AtomicUsize::new(0);
        let client = reqwest::Client::new();

        let response = send_with_retry_with_gate(
            "gated.test",
            RequestClass::Provider,
            &cancellation,
            TooManyRequestsPolicy::Return,
            || async {
                gated_attempts.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            || client.get(&url),
        )
        .await
        .unwrap();

        server.join().unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(gated_attempts.load(Ordering::SeqCst), 3);
        assert_eq!(server_attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn return_policy_returns_429_after_one_gated_attempt() {
        let (url, server_attempts, server) = status_server(vec![StatusCode::TOO_MANY_REQUESTS]);
        let cancellation = CancellationToken::new();
        let gated_attempts = AtomicUsize::new(0);
        let client = reqwest::Client::new();

        let response = send_with_retry_with_gate(
            "gated.test",
            RequestClass::Provider,
            &cancellation,
            TooManyRequestsPolicy::Return,
            || async {
                gated_attempts.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            || client.get(&url),
        )
        .await
        .unwrap();

        server.join().unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(gated_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(server_attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancellation_stops_before_the_custom_gate() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let gated_attempts = AtomicUsize::new(0);
        let client = reqwest::Client::new();
        let result = send_with_retry_with_gate(
            "cancelled.test",
            RequestClass::Provider,
            &cancellation,
            TooManyRequestsPolicy::Return,
            || async {
                gated_attempts.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            || client.get("http://127.0.0.1:1/"),
        )
        .await;

        assert_eq!(result.unwrap_err(), "Playback request cancelled");
        assert_eq!(gated_attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn standard_media_requests_use_the_ungated_retry_path() {
        let (url, server_attempts, server) = status_server(vec![StatusCode::OK]);
        let cancellation = CancellationToken::new();
        let client = reqwest::Client::new();

        let response = send_with_retry("media.test", RequestClass::Media, &cancellation, || {
            client.get(&url)
        })
        .await
        .unwrap();

        server.join().unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(server_attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn retry_after_and_exponential_delays_are_capped() {
        let mut headers = HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "3".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(MAX_RETRY_DELAY));
        assert_eq!(retry_delay(0, retry_after(&headers)), MAX_RETRY_DELAY);
        assert_eq!(retry_delay(0, None), INITIAL_RETRY_DELAY);
        assert_eq!(retry_delay(20, None), MAX_RETRY_DELAY);
        headers.insert(reqwest::header::RETRY_AFTER, "invalid".parse().unwrap());
        assert_eq!(retry_after(&headers), None);
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after(&headers), Some(Duration::ZERO));
        assert_eq!(
            parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT"),
            Some(1_445_412_480)
        );
        assert_eq!(
            parse_http_date("Wednesday, 21-Oct-15 07:28:00 GMT"),
            Some(1_445_412_480)
        );
        assert_eq!(
            parse_http_date("Wed Oct 21 07:28:00 2015"),
            Some(1_445_412_480)
        );
        assert!(parse_http_date("not a date").is_none());
    }

    #[test]
    fn retry_and_source_refresh_budgets_are_bounded() {
        let mut budget = RetryBudget::default();
        assert!(budget.take_source_refresh());
        assert!(!budget.take_source_refresh());
        for _ in 0..MAX_RETRIES {
            assert!(budget.take_retry());
        }
        assert!(!budget.take_retry());
        assert_eq!(budget.retries(), MAX_RETRIES);
    }
}
