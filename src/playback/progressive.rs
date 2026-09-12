use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    pin::Pin,
    sync::{Arc, Condvar, Mutex, mpsc},
    task::{Context, Poll},
    time::Duration,
};

use tokio::{
    fs::File as TokioFile,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
};
use tokio_util::sync::CancellationToken;

use super::resolver::AudioFormat;

const EMPTY_READ_WAIT: Duration = Duration::from_millis(1);

/// The initial reader and backing file produced by a timeline-aware seek.
///
/// Timeline sessions fetch only enough data to construct a decoder, then keep
/// appending later fragments through the same progressive reader.
pub(crate) struct TimelineSeekStartup {
    pub(crate) reader: ProgressiveReader,
    pub(crate) file: tempfile::NamedTempFile,
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
        })
    }

    fn reset(&self, total: Option<u64>) {
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

/// Cloneable completion state used by the decoder to upgrade a fragmented
/// MP4 stream to a fully seekable file once the downloader has finished.
#[derive(Clone)]
pub(crate) struct ProgressiveCompletion {
    shared: Arc<Shared>,
}

impl ProgressiveCompletion {
    pub(crate) fn is_complete(&self) -> bool {
        self.shared
            .state
            .lock()
            .ok()
            .is_some_and(|state| matches!(state.terminal.as_ref(), Some(TerminalState::Complete)))
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

/// A temporary file whose reader blocks at the downloaded frontier instead
/// of returning bytes that have not been written yet.
pub(crate) struct ProgressiveFile {
    file: tempfile::NamedTempFile,
    shared: Arc<Shared>,
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
        })
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
                    let (next, _) = self
                        .shared
                        .wake
                        .wait_timeout(state, EMPTY_READ_WAIT)
                        .map_err(|_| {
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
        assert!(!writer.shared.state.lock().unwrap().terminal.is_some());
        block_on(writer.finish()).unwrap();
        waiter.join().unwrap();
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
