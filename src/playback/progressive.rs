use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    pin::Pin,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

use tokio::{
    fs::File as TokioFile,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
};
use tokio_util::sync::CancellationToken;

use super::resolver::AudioFormat;

/// The initial reader and backing file produced by a timeline-aware seek.
///
/// Timeline sessions fetch only enough data to construct a decoder, then keep
/// appending later fragments through the same progressive reader.
pub(crate) struct TimelineSeekStartup {
    pub(crate) reader: ProgressiveReader,
    pub(crate) file: tempfile::NamedTempFile,
    /// Discard the engine must apply before playing this startup. A session
    /// sets it when the fetched fragment starts before the seek target and
    /// the exact offset only becomes known once the fetch has landed; the
    /// request's own intra-segment offset is used when this stays `None`.
    pub(crate) intra_segment_offset: Option<Duration>,
}

/// Transport-neutral request details needed to decode a timeline seek.
pub(crate) struct TimelineSeekRequest {
    pub(crate) format: AudioFormat,
    pub(crate) intra_segment_offset: Duration,
    pub(crate) cancellation: CancellationToken,
    pub(crate) startup: mpsc::Receiver<Result<TimelineSeekStartup, String>>,
}

/// A resolved media session that can produce a progressive decoder suffix for
/// any timeline position.
pub(crate) trait TimelineSeekSession: Send + Sync {
    fn request(&self, position: Duration) -> Result<TimelineSeekRequest, String>;

    /// Whether the active progressive prefix already contains enough data
    /// to rebuild a decoder at this position without fetching a suffix.
    fn can_seek_from_front(&self, _position: Duration) -> bool {
        false
    }

    /// A landed suffix whose flushed bytes cover the position. A local seek
    /// can rebuild from it while its writer continues appending bytes.
    fn landed_suffix(&self, _position: Duration) -> Option<LandedSuffixSource> {
        None
    }

    /// Live state of the latest suffix this session fetched, so the
    /// buffering indicator can track what will actually play while a seek
    /// is landing. Sessions whose suffix cannot be tracked report nothing
    fn suffix_state(&self) -> Option<TimelineSuffixState> {
        None
    }
}

/// Live progress of a timeline seek suffix, mapped onto the track timeline.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TimelineSuffixState {
    /// Whether the suffix is still waiting to land in the engine. While it
    /// is, the front download is no longer what plays next.
    pub(crate) pending: bool,
    /// Timeline position the suffix starts at.
    pub(crate) base: Duration,
    /// Suffix bytes already staged in the seek buffer.
    pub(crate) written: u64,
    /// Total bytes the suffix will span.
    pub(crate) total: u64,
}

/// A suffix offered as a local seek source for its flushed span.
#[derive(Clone)]
pub(crate) struct LandedSuffixSource {
    /// Path of the suffix buffer file.
    pub(crate) path: std::path::PathBuf,
    /// Completion of the growing suffix buffer.
    pub(crate) completion: ProgressiveCompletion,
    /// Timeline position the suffix starts at.
    pub(crate) base: Duration,
}

/// Pauses a track's front download while a timeline seek suffix is fetched.
///
/// A fresh track keeps downloading its front buffer while the user seeks.
/// The suffix fetch and that download share the same connection, so the
/// bulk stream starves the small range requests the seek needs and playback
/// resumes only after the front buffer completes. Holding this gate makes
/// the front writer stall between chunks, giving the suffix the whole
/// connection until its fetch task ends.
#[derive(Clone)]
pub(crate) struct DownloadPauseGate {
    core: Arc<PauseCore>,
}

struct PauseCore {
    state: std::sync::Mutex<PauseState>,
}

struct PauseState {
    holders: usize,
    wakers: Vec<Waker>,
}

impl PauseCore {
    /// Parks a write while the gate is held, registering the caller's
    /// waker so the final release can resume it.
    fn park_write(&self, waker: Waker) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.holders == 0 {
            return false;
        }
        if !state
            .wakers
            .iter()
            .any(|registered| registered.will_wake(&waker))
        {
            state.wakers.push(waker);
        }
        true
    }

    fn release(&self) {
        let wakers = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.holders = state.holders.saturating_sub(1);
            if state.holders == 0 {
                std::mem::take(&mut state.wakers)
            } else {
                Vec::new()
            }
        };
        for waker in wakers {
            waker.wake();
        }
    }
}

impl DownloadPauseGate {
    pub(crate) fn new() -> Self {
        Self {
            core: Arc::new(PauseCore {
                state: std::sync::Mutex::new(PauseState {
                    holders: 0,
                    wakers: Vec::new(),
                }),
            }),
        }
    }

    /// Holds the gate until the returned guard is dropped. Seek workers
    /// keep one alive for the whole suffix fetch, so cancellation and
    /// failure release the front download through the normal drop path.
    pub(crate) fn hold(&self) -> DownloadPauseGuard {
        let mut state = self
            .core
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.holders += 1;
        drop(state);
        DownloadPauseGuard {
            core: self.core.clone(),
        }
    }
}

pub(crate) struct DownloadPauseGuard {
    core: Arc<PauseCore>,
}

impl Drop for DownloadPauseGuard {
    fn drop(&mut self) {
        self.core.release();
    }
}

#[derive(Clone, Debug)]
enum TerminalState {
    Complete,
    Failed(String),
    Cancelled,
}

struct SharedState {
    written: u64,
    total: Option<u64>,
    terminal: Option<TerminalState>,
    startup_ready: bool,
    pending_seek: Option<Duration>,
}

#[derive(Clone, Copy)]
struct ProgressiveMetadata {
    format: AudioFormat,
    declared_bitrate: Option<u32>,
}

struct Shared {
    state: Mutex<SharedState>,
    metadata: Mutex<ProgressiveMetadata>,
    wake: Condvar,
    read_frontier: AtomicU64,
}

impl Shared {
    fn new(format: AudioFormat, total: Option<u64>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(SharedState {
                written: 0,
                total,
                terminal: None,
                startup_ready: false,
                pending_seek: None,
            }),
            metadata: Mutex::new(ProgressiveMetadata {
                format,
                declared_bitrate: None,
            }),
            wake: Condvar::new(),
            read_frontier: AtomicU64::new(0),
        })
    }

    fn reset(&self, total: Option<u64>) {
        self.read_frontier.store(0, Ordering::Release);
        if let Ok(mut state) = self.state.lock() {
            state.written = 0;
            state.total = total;
            state.terminal = None;
            state.startup_ready = false;
            state.pending_seek = None;
            self.wake.notify_all();
        }
    }

    fn commit(&self, amount: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.written = state.written.saturating_add(amount);
            self.wake.notify_all();
        }
    }

    fn set_total(&self, total: Option<u64>) {
        if let Ok(mut state) = self.state.lock() {
            state.total = total;
            self.wake.notify_all();
        }
    }

    fn set_metadata(&self, format: AudioFormat, declared_bitrate: Option<u32>) {
        if let Ok(mut metadata) = self.metadata.lock() {
            metadata.format = format;
            metadata.declared_bitrate = declared_bitrate;
        }
        self.wake.notify_all();
    }

    fn mark_startup_ready(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.startup_ready = true;
            self.wake.notify_all();
        }
    }

    fn metadata(&self) -> Option<ProgressiveMetadata> {
        self.metadata.lock().ok().map(|metadata| *metadata)
    }

    fn complete(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.terminal = match state.total {
                Some(total) if total != state.written => Some(TerminalState::Failed(
                    "The audio provider returned an incomplete playback stream".into(),
                )),
                Some(_) => Some(TerminalState::Complete),
                None => {
                    state.total = Some(state.written);
                    Some(TerminalState::Complete)
                }
            };
            self.wake.notify_all();
        }
    }

    fn fail(&self, error: impl Into<String>) {
        if let Ok(mut state) = self.state.lock() {
            state.terminal = Some(TerminalState::Failed(error.into()));
            self.wake.notify_all();
        }
    }

    fn cancel(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.terminal = Some(TerminalState::Cancelled);
            self.wake.notify_all();
        }
    }
}

/// Cloneable completion state used by the engine to swap a live progressive
/// stream for a fully seekable reload once the downloader has finished.
#[derive(Clone)]
pub(crate) struct ProgressiveCompletion {
    shared: Arc<Shared>,
}

impl ProgressiveCompletion {
    pub(crate) fn read_frontier(&self) -> u64 {
        self.shared.read_frontier.load(Ordering::Acquire)
    }
    pub(crate) fn is_complete(&self) -> bool {
        self.shared
            .state
            .lock()
            .ok()
            .is_some_and(|state| matches!(state.terminal.as_ref(), Some(TerminalState::Complete)))
    }

    /// A failed or cancelled writer cannot extend a partially landed buffer.
    pub(crate) fn is_usable(&self) -> bool {
        self.shared.state.lock().ok().is_some_and(|state| {
            state.terminal.is_none() || matches!(state.terminal, Some(TerminalState::Complete))
        })
    }

    /// Number of bytes that have been flushed into the progressive file.
    /// Range-seek sessions use this frontier to reuse bytes already fetched
    /// by the front download.
    pub(crate) fn written(&self) -> u64 {
        self.shared
            .state
            .lock()
            .ok()
            .map_or(0, |state| state.written)
    }

    /// Opens another reader over the same growing progressive file. The new
    /// reader shares the writer's completion state, so reaching the current
    /// frontier waits for more bytes instead of treating it as end of file.
    pub(crate) fn open_reader(&self, path: &std::path::Path) -> io::Result<ProgressiveReader> {
        Ok(ProgressiveReader {
            file: File::open(path)?,
            shared: self.shared.clone(),
            cursor: 0,
        })
    }

    /// Completion state for a buffer that finished downloading before the
    /// engine took ownership of it, such as a standby source resolved in
    /// the background. Seeks on it take the completed-buffer paths right
    /// away instead of waiting on a writer that no longer exists.
    pub(crate) fn for_completed_buffer(format: AudioFormat) -> Self {
        let shared = Shared::new(format, None);
        shared.complete();
        Self { shared }
    }

    pub(crate) fn request_seek(&self, position: Duration) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.pending_seek = Some(position);
            self.shared.wake.notify_all();
        }
    }

    pub(crate) fn take_pending_seek(&self) -> Option<Duration> {
        self.shared
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.pending_seek.take())
    }

    pub(crate) fn clear_pending_seek(&self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.pending_seek = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn pending_seek(&self) -> Option<Duration> {
        self.shared
            .state
            .lock()
            .ok()
            .and_then(|state| state.pending_seek)
    }
}

pub(crate) struct ProgressiveFile {
    file: tempfile::NamedTempFile,
    shared: Arc<Shared>,
    pause: Option<DownloadPauseGate>,
}

impl ProgressiveFile {
    pub(crate) fn new(format: AudioFormat, total: Option<u64>) -> io::Result<Self> {
        let file = tempfile::Builder::new()
            .prefix("ralgrum-progressive-")
            .suffix(&format!(".{}", format.extension()))
            .tempfile()?;
        Ok(Self {
            file,
            shared: Shared::new(format, total),
            pause: None,
        })
    }

    /// Attaches a pause gate to the writer this file produces. While the
    /// gate is held by a timeline seek, the front download stalls between
    /// chunks so the seek suffix gets the whole connection.
    pub(crate) fn set_pause_gate(&mut self, pause: DownloadPauseGate) {
        self.pause = Some(pause);
    }

    pub(crate) fn reader(&self) -> io::Result<ProgressiveReader> {
        Ok(ProgressiveReader {
            file: self.file.reopen()?,
            shared: self.shared.clone(),
            cursor: 0,
        })
    }

    pub(crate) fn writer(&self) -> io::Result<ProgressiveWriter> {
        Ok(ProgressiveWriter {
            file: TokioFile::from_std(self.file.reopen()?),
            shared: self.shared.clone(),
            pending: 0,
            pause: self.pause.clone(),
        })
    }

    #[cfg(test)]
    pub(crate) fn wait_until_ready(&self, minimum: u64) -> Result<(), String> {
        wait_until_ready(&self.shared, minimum)
    }

    pub(crate) fn metadata(&self) -> Option<(AudioFormat, Option<u32>)> {
        self.shared
            .metadata()
            .map(|metadata| (metadata.format, metadata.declared_bitrate))
    }

    pub(crate) fn into_file(self) -> tempfile::NamedTempFile {
        self.file
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        self.file.path()
    }
}

pub(crate) fn startup_bytes(format: AudioFormat, total: Option<u64>) -> u64 {
    if format == AudioFormat::M4a {
        // MP4 metadata may live at the end, so wait for the complete file.
        return total.unwrap_or(u64::MAX);
    }
    let minimum = match format {
        AudioFormat::Wav => 44,
        AudioFormat::Aiff => 64,
        AudioFormat::Mp3 | AudioFormat::Aac => 16 * 1024,
        AudioFormat::Flac => 8 * 1024,
        AudioFormat::OggVorbis | AudioFormat::OggOpus => 32 * 1024,
        AudioFormat::M4a => unreachable!(),
    };
    total.map_or(minimum, |total| total.min(minimum))
}

pub(crate) struct ProgressiveWriter {
    file: TokioFile,
    shared: Arc<Shared>,
    pending: u64,
    /// While the attached pause gate is held, writes stall here instead of
    /// reaching the file, throttling the front download while a timeline
    /// seek fetches its suffix.
    pause: Option<DownloadPauseGate>,
}

impl ProgressiveWriter {
    pub(crate) async fn reset(&mut self, total: Option<u64>) -> io::Result<()> {
        self.file.set_len(0).await?;
        self.file.seek(SeekFrom::Start(0)).await?;
        self.pending = 0;
        self.shared.reset(total);
        Ok(())
    }

    pub(crate) fn set_total(&self, total: Option<u64>) {
        self.shared.set_total(total);
    }

    pub(crate) fn set_metadata(&self, format: AudioFormat, declared_bitrate: Option<u32>) {
        self.shared.set_metadata(format, declared_bitrate);
    }

    pub(crate) fn mark_startup_ready(&self) {
        self.shared.mark_startup_ready();
    }

    pub(crate) fn total(&self) -> Option<u64> {
        self.shared.state.lock().ok().and_then(|state| state.total)
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.shared
            .state
            .lock()
            .ok()
            .is_some_and(|state| matches!(state.terminal, Some(TerminalState::Complete)))
    }

    pub(crate) async fn read_range(&self, offset: u64, length: usize) -> io::Result<Vec<u8>> {
        let mut file = self.file.try_clone().await?;
        file.seek(SeekFrom::Start(offset)).await?;
        let mut bytes = vec![0; length];
        file.read_exact(&mut bytes).await?;
        Ok(bytes)
    }

    pub(crate) async fn finish(&mut self) -> io::Result<()> {
        self.file.flush().await?;
        self.commit_pending();
        self.shared.complete();
        Ok(())
    }

    pub(crate) fn fail(&self, error: impl Into<String>) {
        self.shared.fail(error);
    }

    pub(crate) fn cancel(&self) {
        self.shared.cancel();
    }

    fn commit_pending(&mut self) {
        if self.pending != 0 {
            self.shared.commit(self.pending);
            self.pending = 0;
        }
    }
}

impl AsyncWrite for ProgressiveWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if let Some(pause) = self.pause.as_ref()
            && pause.core.park_write(cx.waker().clone())
        {
            // A held gate parks the write without touching the file, so
            // the caller's next chunk is not even requested until the
            // seek suffix finishes with the connection.
            return Poll::Pending;
        }
        let result = Pin::new(&mut self.file).poll_write(cx, buffer);
        if let Poll::Ready(Ok(written)) = result {
            self.pending = self.pending.saturating_add(written as u64);
        }
        result
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result = Pin::new(&mut self.file).poll_flush(cx);
        if matches!(result, Poll::Ready(Ok(()))) {
            self.commit_pending();
        }
        result
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result = Pin::new(&mut self.file).poll_shutdown(cx);
        if matches!(result, Poll::Ready(Ok(()))) {
            self.commit_pending();
        }
        result
    }
}

pub(crate) struct ProgressiveReader {
    file: File,
    shared: Arc<Shared>,
    cursor: u64,
}

impl ProgressiveReader {
    pub(crate) fn wait_until_ready(&self, minimum: u64) -> Result<(), String> {
        wait_until_ready(&self.shared, minimum)
    }

    pub(crate) fn wait_until_startup_ready(&self) -> Result<(), String> {
        wait_until_startup_ready(&self.shared)
    }

    pub(crate) fn completion(&self) -> ProgressiveCompletion {
        ProgressiveCompletion {
            shared: self.shared.clone(),
        }
    }

    pub(crate) fn total(&self) -> Option<u64> {
        self.shared.state.lock().ok().and_then(|state| state.total)
    }

    pub(crate) fn written(&self) -> u64 {
        self.shared
            .state
            .lock()
            .ok()
            .map_or(0, |state| state.written)
    }
}

fn wait_until_ready(shared: &Arc<Shared>, minimum: u64) -> Result<(), String> {
    let mut state = shared
        .state
        .lock()
        .map_err(|_| "The progressive playback state was poisoned".to_string())?;
    loop {
        if state.written >= minimum {
            return Ok(());
        }
        match state.terminal.clone() {
            Some(TerminalState::Complete) if state.written > 0 => return Ok(()),
            Some(TerminalState::Complete) => {
                return Err("The resolved audio output was empty".into());
            }
            Some(TerminalState::Failed(error)) => return Err(error.clone()),
            Some(TerminalState::Cancelled) => return Err("Playback request cancelled".into()),
            None => {
                state = shared
                    .wake
                    .wait(state)
                    .map_err(|_| "The progressive playback state was poisoned".to_string())?;
            }
        }
    }
}

fn wait_until_startup_ready(shared: &Arc<Shared>) -> Result<(), String> {
    let mut state = shared
        .state
        .lock()
        .map_err(|_| "The progressive playback state was poisoned".to_string())?;
    loop {
        if state.startup_ready {
            return Ok(());
        }
        match state.terminal.clone() {
            Some(TerminalState::Complete) if state.written > 0 => return Ok(()),
            Some(TerminalState::Complete) => {
                return Err("The resolved audio output was empty".into());
            }
            Some(TerminalState::Failed(error)) => return Err(error.clone()),
            Some(TerminalState::Cancelled) => {
                return Err("Playback request cancelled".into());
            }
            None => {
                state = shared
                    .wake
                    .wait(state)
                    .map_err(|_| "The progressive playback state was poisoned".to_string())?;
            }
        }
    }
}

impl Read for ProgressiveReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        self.shared.read_frontier.fetch_max(
            self.cursor.saturating_add(buffer.len() as u64),
            Ordering::AcqRel,
        );
        loop {
            let state = self
                .shared
                .state
                .lock()
                .map_err(|_| io::Error::other("The progressive playback state was poisoned"))?;
            if self.cursor < state.written {
                let available = (state.written - self.cursor).min(buffer.len() as u64) as usize;
                drop(state);
                self.file.seek(SeekFrom::Start(self.cursor))?;
                let read = self.file.read(&mut buffer[..available])?;
                if read > 0 {
                    self.cursor = self.cursor.saturating_add(read as u64);
                    return Ok(read);
                }
                std::thread::yield_now();
                continue;
            }
            match state.terminal.clone() {
                Some(TerminalState::Complete) => return Ok(0),
                Some(TerminalState::Failed(error)) => return Err(io::Error::other(error.clone())),
                Some(TerminalState::Cancelled) => {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "Playback request cancelled",
                    ));
                }
                None => {
                    let next = self.shared.wake.wait(state).map_err(|_| {
                        io::Error::other("The progressive playback state was poisoned")
                    })?;
                    drop(next);
                }
            }
        }
    }
}

impl Seek for ProgressiveReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let target = match position {
            SeekFrom::Start(offset) => offset,
            SeekFrom::Current(offset) => checked_seek(self.cursor, offset)?,
            SeekFrom::End(offset) => {
                let total = self.total().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "The progressive stream length is not known",
                    )
                })?;
                checked_seek(total, offset)?
            }
        };
        self.cursor = target;
        Ok(target)
    }
}

fn checked_seek(base: u64, offset: i64) -> io::Result<u64> {
    if offset >= 0 {
        base.checked_add(offset as u64)
    } else {
        base.checked_sub(offset.unsigned_abs())
    }
    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "The seek position was invalid"))
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        io::{Read, Seek, SeekFrom},
        sync::mpsc,
        thread,
    };

    use super::*;

    fn buffer(total: Option<u64>) -> (ProgressiveFile, ProgressiveWriter) {
        let file = ProgressiveFile::new(AudioFormat::Mp3, total).unwrap();
        let writer = file.writer().unwrap();
        (file, writer)
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        tokio::runtime::Runtime::new().unwrap().block_on(future)
    }

    #[test]
    fn early_readiness_waits_for_a_bounded_prefix() {
        let (file, mut writer) = buffer(Some(128));
        let (started_tx, started_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let waiter = thread::spawn(move || {
            started_tx.send(()).unwrap();
            file.wait_until_ready(8).unwrap();
            ready_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(ready_rx.try_recv().is_err());
        block_on(writer.write_all(&[1; 8])).unwrap();
        block_on(writer.flush()).unwrap();
        ready_rx.recv().unwrap();
        waiter.join().unwrap();
    }

    #[test]
    fn startup_ready_can_be_signalled_before_download_completion() {
        let (file, mut writer) = buffer(None);
        let (started_tx, started_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let waiter = thread::spawn(move || {
            started_tx.send(()).unwrap();
            file.reader().unwrap().wait_until_startup_ready().unwrap();
            ready_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(ready_rx.try_recv().is_err());
        block_on(writer.write_all(b"init-and-first-fragment")).unwrap();
        block_on(writer.flush()).unwrap();
        writer.mark_startup_ready();
        ready_rx.recv().unwrap();
        assert!(writer.shared.state.lock().unwrap().terminal.is_none());
        block_on(writer.finish()).unwrap();
        waiter.join().unwrap();
    }

    /// A held pause gate must park front-download writes without touching
    /// the buffer, and release them exactly when the last holder drops.
    /// This is what gives a timeline seek suffix the whole connection
    /// instead of competing with the front stream it replaces.
    #[tokio::test]
    async fn pause_gate_parks_writes_until_the_last_holder_drops() {
        let mut file = ProgressiveFile::new(AudioFormat::Mp3, Some(16)).unwrap();
        let gate = DownloadPauseGate::new();
        file.set_pause_gate(gate.clone());
        let mut writer = file.writer().unwrap();

        let first = gate.hold();
        let second = gate.hold();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                writer.write_all(&[1; 8])
            )
            .await
            .is_err(),
            "a held gate must park the front write"
        );

        drop(first);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                writer.write_all(&[1; 8])
            )
            .await
            .is_err(),
            "the gate must stay held while any holder remains"
        );

        drop(second);
        tokio::time::timeout(std::time::Duration::from_secs(1), writer.write_all(&[1; 8]))
            .await
            .expect("the front write must resume once the gate is released")
            .unwrap();
        writer.flush().await.unwrap();
        assert_eq!(writer.shared.state.lock().unwrap().written, 8);
    }

    #[test]
    fn writer_completion_distinguishes_exact_and_truncated_totals() {
        let (_, mut complete) = buffer(Some(4));
        block_on(complete.write_all(b"data")).unwrap();
        block_on(complete.finish()).unwrap();
        assert!(complete.is_complete());
        assert_eq!(complete.total(), Some(4));

        let (_, mut truncated) = buffer(Some(4));
        block_on(truncated.write_all(b"no")).unwrap();
        block_on(truncated.finish()).unwrap();
        assert!(!truncated.is_complete());
        assert_eq!(truncated.total(), Some(4));
    }

    #[test]
    fn m4a_startup_is_bounded_to_full_download() {
        assert_eq!(startup_bytes(AudioFormat::M4a, Some(123)), 123);
        assert_eq!(startup_bytes(AudioFormat::M4a, None), u64::MAX);
    }

    #[test]
    fn read_blocks_and_wakes_at_the_download_frontier() {
        let (file, mut writer) = buffer(Some(16));
        let mut reader = file.reader().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let read = thread::spawn(move || {
            started_tx.send(()).unwrap();
            let mut bytes = [0; 4];
            reader.read_exact(&mut bytes).unwrap();
            result_tx.send(bytes).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(result_rx.try_recv().is_err());
        block_on(writer.write_all(b"wake")).unwrap();
        block_on(writer.flush()).unwrap();
        assert_eq!(result_rx.recv().unwrap(), *b"wake");
        read.join().unwrap();
    }

    #[test]
    fn seek_uses_declared_length_and_reads_only_downloaded_bytes() {
        let (file, mut writer) = buffer(Some(6));
        block_on(writer.write_all(b"abcdef")).unwrap();
        block_on(writer.finish()).unwrap();
        let mut reader = file.reader().unwrap();
        assert_eq!(reader.seek(SeekFrom::End(-2)).unwrap(), 4);
        let mut bytes = [0; 2];
        reader.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"ef");
    }

    #[test]
    fn seek_can_wait_for_a_later_downloaded_range() {
        let (file, mut writer) = buffer(Some(8));
        block_on(writer.write_all(b"abcd")).unwrap();
        block_on(writer.flush()).unwrap();
        let mut reader = file.reader().unwrap();
        assert_eq!(reader.seek(SeekFrom::Start(6)).unwrap(), 6);
        let (started_tx, started_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let read = thread::spawn(move || {
            started_tx.send(()).unwrap();
            let mut bytes = [0; 2];
            reader.read_exact(&mut bytes).unwrap();
            result_tx.send(bytes).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(result_rx.try_recv().is_err());
        block_on(writer.write_all(b"efgh")).unwrap();
        block_on(writer.finish()).unwrap();
        assert_eq!(result_rx.recv().unwrap(), *b"gh");
        read.join().unwrap();
    }

    #[test]
    fn completion_opens_an_independent_reader_on_the_same_growing_file() {
        let (file, mut writer) = buffer(Some(8));
        let original = file.reader().unwrap();
        let completion = original.completion();
        block_on(writer.write_all(b"abcd")).unwrap();
        block_on(writer.flush()).unwrap();

        let mut reopened = completion.open_reader(file.path()).unwrap();
        reopened.seek(SeekFrom::Start(2)).unwrap();
        let mut prefix = [0; 2];
        reopened.read_exact(&mut prefix).unwrap();
        assert_eq!(&prefix, b"cd");

        let (result_tx, result_rx) = mpsc::channel();
        let read = thread::spawn(move || {
            let mut tail = [0; 4];
            reopened.read_exact(&mut tail).unwrap();
            result_tx.send(tail).unwrap();
        });
        assert!(result_rx.try_recv().is_err());
        block_on(writer.write_all(b"efgh")).unwrap();
        block_on(writer.finish()).unwrap();
        assert_eq!(result_rx.recv().unwrap(), *b"efgh");
        read.join().unwrap();
    }

    #[test]
    fn completion_reports_truncation_without_exposing_unwritten_bytes() {
        let (file, mut writer) = buffer(Some(8));
        block_on(writer.write_all(b"only")).unwrap();
        block_on(writer.finish()).unwrap();
        let mut reader = file.reader().unwrap();
        let mut bytes = [0; 8];
        assert_eq!(reader.read(&mut bytes).unwrap(), 4);
        assert_eq!(
            reader.read(&mut bytes).unwrap_err().kind(),
            io::ErrorKind::Other
        );
    }

    #[test]
    fn failure_and_cancellation_wake_blocked_readers() {
        let (file, writer) = buffer(Some(16));
        let mut reader = file.reader().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let read = thread::spawn(move || {
            started_tx.send(()).unwrap();
            let mut bytes = [0; 1];
            result_tx
                .send(reader.read(&mut bytes).unwrap_err().kind())
                .unwrap();
        });
        started_rx.recv().unwrap();
        writer.fail("network failed");
        assert_eq!(result_rx.recv().unwrap(), io::ErrorKind::Other);
        read.join().unwrap();

        let (file, writer) = buffer(Some(16));
        let mut reader = file.reader().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let read = thread::spawn(move || {
            started_tx.send(()).unwrap();
            let mut bytes = [0; 1];
            result_tx
                .send(reader.read(&mut bytes).unwrap_err().kind())
                .unwrap();
        });
        started_rx.recv().unwrap();
        writer.cancel();
        assert_eq!(result_rx.recv().unwrap(), io::ErrorKind::Interrupted);
        read.join().unwrap();
    }
}
