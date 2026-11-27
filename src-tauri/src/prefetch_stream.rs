use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

/// Fixed native frame size.
pub const FRAME_SIZE: usize = 8;
/// Capacity of the transport channel.
pub(crate) const QUEUE_FRAMES: usize = 4096;
const PLAYING_STALL_LIMIT: Duration = Duration::from_secs(10);

/// Phase constants for the stream state.
const READY: u8 = 0;
const INSTALLED: u8 = 1;
const REVOKED: u8 = 2;
const PACKET_REJECTED: u8 = 3;
const DECODE_FAILED: u8 = 4;
const DISCONNECTED: u8 = 5;
const STALLED: u8 = 6;
const DECODE_BUDGET_EXHAUSTED: u8 = 7;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StreamFailure {
    PacketBudget,
    DecodeBudget,
    Decode,
    Disconnected,
    Stall,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StreamRead {
    Sample(f32),
    Pending,
    End,
    Failed(StreamFailure),
}

/// Events sent from the producer to the consumer.
#[derive(Debug, Clone, Copy)]
pub enum StreamEvent {
    Frame([f32; FRAME_SIZE]),
    End,
    Failed(StreamFailure),
}

/// The producer side of the stream.
pub struct StreamWriter {
    tx: Sender<StreamEvent>,
    phase: Arc<AtomicU8>,
}

/// The consumer side of the stream.
#[derive(Debug)]
pub struct PreparedStream {
    rx: Receiver<StreamEvent>,
    phase: Arc<AtomicU8>,
    channels: u16,
    current_frame: [f32; FRAME_SIZE],
    current_idx: usize,
    terminal: Option<StreamRead>,
    _source_slot: Option<Arc<crate::player::PcmCaptureSlot>>,
    cancel: Option<Arc<AtomicBool>>,
    pending_since: Option<(u64, Instant)>,
}

pub(crate) fn read_native_frame(
    channels: u16,
    read: &mut impl FnMut() -> Result<Option<f32>, StreamFailure>,
) -> Result<Option<[f32; FRAME_SIZE]>, StreamFailure> {
    let count = channels as usize;
    if count == 0 || count > FRAME_SIZE {
        return Err(StreamFailure::Decode);
    }

    let mut frame = [0.0f32; FRAME_SIZE];

    for i in 0..count {
        match read() {
            Ok(Some(sample)) => {
                frame[i] = sample;
            }
            Ok(None) => {
                if i == 0 {
                    return Ok(None);
                } else {
                    return Err(StreamFailure::Decode);
                }
            }
            Err(e) => return Err(e),
        }
    }

    Ok(Some(frame))
}

impl StreamWriter {
    pub(crate) fn pump(self, channels: u16, mut read: impl FnMut() -> Result<Option<f32>, StreamFailure>) {
        loop {
            match read_native_frame(channels, &mut read) {
                Ok(Some(frame)) => {
                    if !self.write_frame(frame) {
                        return;
                    }
                }
                Ok(None) => {
                    self.end();
                    return;
                }
                Err(reason) => {
                    self.fail(reason);
                    return;
                }
            }
        }
    }

    /// Sends a frame to the transport. Returns false if the receiver is disconnected.
    pub fn write_frame(&self, frame: [f32; FRAME_SIZE]) -> bool {
        self.tx.send(StreamEvent::Frame(frame)).is_ok()
    }

    /// Sends an ordered End event.
    pub fn end(self) {
        let _ = self.tx.send(StreamEvent::End);
    }

    /// Sets the failure phase and sends the event.
    pub fn fail(self, reason: StreamFailure) {
        let next_phase = match reason {
            StreamFailure::PacketBudget => PACKET_REJECTED,
            StreamFailure::DecodeBudget => DECODE_BUDGET_EXHAUSTED,
            StreamFailure::Decode => DECODE_FAILED,
            StreamFailure::Disconnected => DISCONNECTED,
            StreamFailure::Stall => STALLED,
        };
        if let Err(INSTALLED) = self.phase.compare_exchange(
            READY,
            next_phase,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            let _ = self.tx.send(StreamEvent::Failed(reason));
        }
    }
}

impl PreparedStream {
    pub(crate) fn retain_source_slot(&mut self, slot: Arc<crate::player::PcmCaptureSlot>) {
        self._source_slot = Some(slot);
    }


    /// Atomically transition from READY to INSTALLED.
    pub(crate) fn try_install(&self) -> bool {
        self.phase.compare_exchange(READY, INSTALLED, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    }

    pub(crate) fn retain_cancel_flag(&mut self, flag: Option<Arc<AtomicBool>>) {
        self.cancel = flag;
    }

    fn signal_cancel(&self) {
        if let Some(flag) = &self.cancel {
            flag.store(true, Ordering::Release);
        }
    }

    /// Atomically transition from READY to REVOKED.
    pub(crate) fn revoke(&self) -> bool {
        let revoked = self.phase.compare_exchange(READY, REVOKED, Ordering::SeqCst, Ordering::SeqCst).is_ok();
        if revoked {
            self.signal_cancel();
        }
        revoked
    }

    /// Returns the typed failure if the phase is a failure state.
    pub(crate) fn preinstall_failure(&self) -> Option<StreamFailure> {
        match self.phase.load(Ordering::SeqCst) {
            PACKET_REJECTED => Some(StreamFailure::PacketBudget),
            DECODE_BUDGET_EXHAUSTED => Some(StreamFailure::DecodeBudget),
            DECODE_FAILED => Some(StreamFailure::Decode),
            DISCONNECTED => Some(StreamFailure::Disconnected),
            STALLED => Some(StreamFailure::Stall),
            _ => None,
        }
    }

    pub(crate) fn poll_playing_sample(
        &mut self,
        playback_epoch: u64,
        clock: impl FnOnce() -> Instant,
    ) -> StreamRead {
        let sample = self.poll_sample();
        if sample != StreamRead::Pending {
            self.pending_since = None;
            return sample;
        }

        let now = clock();
        let (_, start) = self.pending_since
            .filter(|(epoch, _)| *epoch == playback_epoch)
            .unwrap_or((playback_epoch, now));
        self.pending_since = Some((playback_epoch, start));
        if now.saturating_duration_since(start) < PLAYING_STALL_LIMIT {
            return StreamRead::Pending;
        }

        let failure = StreamRead::Failed(StreamFailure::Stall);
        self.terminal = Some(failure);
        self.signal_cancel();
        failure
    }

    /// Polls for the next sample. Returns a stable terminal result if available.
    pub fn poll_sample(&mut self) -> StreamRead {
        if let Some(terminal) = self.terminal {
            return terminal;
        }

        if self.current_idx == usize::from(self.channels) {
            match self.rx.try_recv() {
                Ok(StreamEvent::Frame(frame)) => {
                    self.current_frame = frame;
                    self.current_idx = 0;
                    let sample = self.current_frame[self.current_idx];
                    self.current_idx += 1;
                    StreamRead::Sample(sample)
                }
                Ok(StreamEvent::End) => {
                    self.terminal = Some(StreamRead::End);
                    StreamRead::End
                }
                Ok(StreamEvent::Failed(reason)) => {
                    self.terminal = Some(StreamRead::Failed(reason));
                    StreamRead::Failed(reason)
                }
                Err(TryRecvError::Empty) => StreamRead::Pending,
                Err(TryRecvError::Disconnected) => {
                    self.terminal = Some(StreamRead::Failed(StreamFailure::Disconnected));
                    StreamRead::Failed(StreamFailure::Disconnected)
                }
            }
        } else {
            let sample = self.current_frame[self.current_idx];
            self.current_idx += 1;
            StreamRead::Sample(sample)
        }
    }
}

impl Drop for PreparedStream {
    fn drop(&mut self) {
        self.signal_cancel();
    }
}

/// Factory function to initialize a stream with an initial frame.
pub fn prepared_stream(channels: u16, first: [f32; FRAME_SIZE]) -> Option<(StreamWriter, PreparedStream)> {
    if channels == 0 || channels > FRAME_SIZE as u16 {
        return None;
    }

    let (tx, rx) = bounded(QUEUE_FRAMES);
    let phase = Arc::new(AtomicU8::new(READY));

    // Send the first frame before publishing
    if tx.try_send(StreamEvent::Frame(first)).is_err() {
        return None;
    }

    let writer = StreamWriter {
        tx,
        phase: Arc::clone(&phase),
    };

    let reader = PreparedStream {
        rx,
        phase,
        channels,
        current_frame: [0.0; FRAME_SIZE],
        current_idx: usize::from(channels),
        terminal: None,
        _source_slot: None,
        cancel: None,
        pending_since: None,
    };

    Some((writer, reader))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playing_stall_counts_only_continuous_pending_in_the_same_playback_epoch() {
        let start = Instant::now();
        let (writer, mut source) = prepared_stream(1, [0.0; FRAME_SIZE]).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        source.retain_cancel_flag(Some(cancel.clone()));
        assert!(source.try_install());
        let at = |seconds| start + Duration::from_secs(seconds);
        assert_eq!(source.poll_playing_sample(1, || at(0)), StreamRead::Sample(0.0));
        assert_eq!(source.poll_playing_sample(1, || at(0)), StreamRead::Pending);
        assert_eq!(source.poll_playing_sample(1, || at(9)), StreamRead::Pending);

        assert_eq!(source.poll_playing_sample(2, || at(60)), StreamRead::Pending);
        assert!(writer.write_frame([0.25; FRAME_SIZE]));
        assert_eq!(source.poll_playing_sample(2, || at(69)), StreamRead::Sample(0.25));
        assert_eq!(source.poll_playing_sample(2, || at(70)), StreamRead::Pending);
        assert_eq!(source.poll_playing_sample(2, || at(79)), StreamRead::Pending);
        assert!(!cancel.load(Ordering::Acquire));
        assert_eq!(source.poll_playing_sample(2, || at(80)), StreamRead::Failed(StreamFailure::Stall));
        assert!(cancel.load(Ordering::Acquire));
        assert_eq!(source.poll_sample(), StreamRead::Failed(StreamFailure::Stall));
    }

    #[test]
    fn cancellation_reaches_revoked_and_dropped_sources() {
        let (_writer, mut pending) = prepared_stream(2, [0.0; FRAME_SIZE]).unwrap();
        let pending_flag = Arc::new(AtomicBool::new(false));
        pending.retain_cancel_flag(Some(pending_flag.clone()));
        assert!(pending.revoke());
        assert!(pending_flag.load(Ordering::Acquire));

        let (_writer, mut active) = prepared_stream(2, [0.0; FRAME_SIZE]).unwrap();
        let active_flag = Arc::new(AtomicBool::new(false));
        active.retain_cancel_flag(Some(active_flag.clone()));
        assert!(active.try_install());
        assert!(!active.revoke());
        assert!(!active_flag.load(Ordering::Acquire));
        drop(active);
        assert!(active_flag.load(Ordering::Acquire));
    }

    #[test]
    fn pump_rejects_an_incomplete_stereo_frame() {
        let first = [0.25, -0.25, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let (writer, mut source) = prepared_stream(2, first).unwrap();
        assert!(source.try_install());
        let mut input = [0.5, -0.5, 0.75].into_iter();
        writer.pump(2, || Ok(input.next()));
        for expected in [0.25, -0.25, 0.5, -0.5] {
            assert_eq!(source.poll_sample(), StreamRead::Sample(expected));
        }
        assert_eq!(source.poll_sample(), StreamRead::Failed(StreamFailure::Decode));
    }

    #[test]
    fn test_preserve_first_stereo_samples() {
        let first = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let (writer, mut reader) = prepared_stream(2, first).unwrap();
        
        // Should get first sample of first frame
        assert_eq!(reader.poll_sample(), StreamRead::Sample(1.0));
        // Should get second sample of first frame
        assert_eq!(reader.poll_sample(), StreamRead::Sample(2.0));
    }

    #[test]
    fn test_no_source_data_yields_pending() {
        let first = [0.0; 8];
        let (writer, mut reader) = prepared_stream(2, first).unwrap();
        
        // First two samples consumed
        reader.poll_sample();
        reader.poll_sample();
        
        // Next should be pending
        assert_eq!(reader.poll_sample(), StreamRead::Pending);
    }

    #[test]
    fn test_ordered_end_stays_sticky() {
        let first = [0.0; 8];
        let (writer, mut reader) = prepared_stream(1, first).unwrap();
        
        reader.poll_sample(); // Consume first
        writer.end();
        
        assert_eq!(reader.poll_sample(), StreamRead::End);
        assert_eq!(reader.poll_sample(), StreamRead::End); // Sticky
    }

    #[test]
    fn test_packet_rejection_before_installation() {
        let (writer, reader) = prepared_stream(2, [0.0; 8]).unwrap();
        writer.fail(StreamFailure::PacketBudget);
        assert_eq!(reader.preinstall_failure(), Some(StreamFailure::PacketBudget));
        assert!(!reader.try_install());
    }

    #[test]
    fn test_packet_rejection_after_installation() {
        let (writer, mut reader) = prepared_stream(1, [0.25; FRAME_SIZE]).unwrap();
        assert!(reader.try_install());
        writer.fail(StreamFailure::PacketBudget);
        assert_eq!(reader.poll_sample(), StreamRead::Sample(0.25));
        let failed = StreamRead::Failed(StreamFailure::PacketBudget);
        assert_eq!(reader.poll_sample(), failed);
        assert_eq!(reader.poll_sample(), failed);
    }

    #[test]
    fn test_buffer_disconnect_is_not_eof() {
        let (writer, mut reader) = prepared_stream(1, [0.25; FRAME_SIZE]).unwrap();
        assert!(reader.try_install());
        drop(writer);
        assert_eq!(reader.poll_sample(), StreamRead::Sample(0.25));
        let failed = StreamRead::Failed(StreamFailure::Disconnected);
        assert_eq!(reader.poll_sample(), failed);
        assert_eq!(reader.poll_sample(), failed);
    }

    #[test]
    fn test_revoke_prevents_installation() {
        let first = [0.0; 8];
        let (writer, reader) = prepared_stream(1, first).unwrap();
        
        assert!(reader.revoke());
        assert!(!reader.try_install());
    }
}