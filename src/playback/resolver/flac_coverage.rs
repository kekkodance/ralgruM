use std::{
    fs::File,
    io::{self, Read as _, Seek as _, SeekFrom},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use super::range_seek::{FlacStreamInfo, parse_first_flac_frame};

const SCAN_CHUNK_BYTES: usize = 256 * 1024;
const FRAME_HEADER_OVERLAP: usize = 32;

#[derive(Clone)]
pub(super) struct FlacCoverage {
    verified_through_nanos: Arc<AtomicU64>,
    required_bytes: Arc<AtomicU64>,
}

impl FlacCoverage {
    pub(super) fn new() -> Self {
        Self {
            verified_through_nanos: Arc::new(AtomicU64::new(0)),
            required_bytes: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(super) fn contains(&self, position: Duration, written: u64) -> bool {
        let through = self.verified_through_nanos.load(Ordering::Acquire);
        position.as_nanos() < u128::from(through)
            && written >= self.required_bytes.load(Ordering::Acquire)
    }

    fn clear(&self) {
        self.required_bytes.store(u64::MAX, Ordering::Release);
        self.verified_through_nanos.store(0, Ordering::Release);
        self.required_bytes.store(0, Ordering::Release);
    }
}

/// A following frame header proves the preceding frame is completely staged.
/// The scan is incremental, including a short overlap for split headers.
pub(super) struct FlacScanner {
    info: FlacStreamInfo,
    coverage: FlacCoverage,
    audio_start: u64,
    next_offset: u64,
    tail: Vec<u8>,
    last_frame_offset: Option<u64>,
}

impl FlacScanner {
    pub(super) fn new(info: FlacStreamInfo, audio_start: u64, coverage: FlacCoverage) -> Self {
        Self {
            info,
            coverage,
            audio_start,
            next_offset: audio_start,
            tail: Vec::new(),
            last_frame_offset: None,
        }
    }

    pub(super) fn next_offset(&self) -> u64 {
        self.next_offset
    }

    pub(super) fn scan_file_to(&mut self, path: &Path, written: u64) -> io::Result<()> {
        if written < self.next_offset {
            self.coverage.clear();
            self.next_offset = self.audio_start;
            self.tail.clear();
            self.last_frame_offset = None;
        }
        if written <= self.next_offset {
            return Ok(());
        }
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(self.next_offset))?;
        let mut chunk = vec![0; SCAN_CHUNK_BYTES];
        while self.next_offset < written {
            let wanted = (written - self.next_offset).min(SCAN_CHUNK_BYTES as u64) as usize;
            let read = file.read(&mut chunk[..wanted])?;
            if read == 0 {
                break;
            }
            self.ingest(self.next_offset, &chunk[..read]);
        }
        Ok(())
    }

    pub(super) fn ingest(&mut self, offset: u64, bytes: &[u8]) {
        if offset != self.next_offset {
            self.tail.clear();
            self.last_frame_offset = None;
        }
        let base = offset.saturating_sub(self.tail.len() as u64);
        let mut combined = Vec::with_capacity(self.tail.len() + bytes.len());
        combined.extend_from_slice(&self.tail);
        combined.extend_from_slice(bytes);
        let mut cursor = 0;
        while cursor < combined.len() {
            let Some(frame) =
                parse_first_flac_frame(&combined[cursor..], &self.info, base + cursor as u64)
            else {
                break;
            };
            if self
                .last_frame_offset
                .is_none_or(|previous| frame.offset > previous)
            {
                if self.last_frame_offset.is_some() {
                    let nanos = frame.position.as_nanos().min(u128::from(u64::MAX)) as u64;
                    self.coverage.required_bytes.store(
                        frame.offset.saturating_add(FRAME_HEADER_OVERLAP as u64),
                        Ordering::Release,
                    );
                    self.coverage
                        .verified_through_nanos
                        .fetch_max(nanos, Ordering::Release);
                }
                self.last_frame_offset = Some(frame.offset);
            }
            cursor = (frame.offset - base) as usize + 1;
        }
        self.tail = combined[combined.len().saturating_sub(FRAME_HEADER_OVERLAP)..].to_vec();
        self.next_offset = offset.saturating_add(bytes.len() as u64);
    }
}
