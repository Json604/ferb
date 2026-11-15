//! The sound one app plays, captured with ScreenCaptureKit into a WAV file in FERB's workspace (audio.py, L26,
//! HAND-401): a display filter including only that app's exact process (by bundle id and pid, never a near match);
//! FERB's own sound excluded; the microphone off; a 2x2 picture the stream needs, ignored. Mono float32 samples are
//! written as they arrive, each buffer at its presentation time (silence written as zeros). Blocking: Hearing runs
//! `start` on its own thread and `stop` off the event loop. A start that answers late still gets its stream stopped.

use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass};
use objc2_core_graphics::CGMainDisplayID;
use objc2_core_media::{
    CMAudioFormatDescriptionGetStreamBasicDescription, CMClock, CMSampleBuffer, CMTime,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCRunningApplication, SCStream, SCStreamConfiguration, SCStreamOutput,
    SCStreamOutputType, SCWindow,
};
use serde_json::{json, Map, Value};

use super::capture::{error_of, shareable};
use crate::error::{HandError, Result};
use crate::listen::{Capture, Running};
use crate::shots::TIMEOUT_S;
use crate::wav::{WavWriter, SAMPLE_RATE};

const FLOAT: u32 = 1; // kAudioFormatFlagIsFloat
const NON_INTERLEAVED: u32 = 32; // kAudioFormatFlagIsNonInterleaved

/// What the sink writes into, shared with the capture handle.
pub struct SinkState {
    writer: WavWriter,
    error: Mutex<Option<String>>,
    reference: Mutex<f64>,
    started: Instant,
    started_at: Mutex<f64>,
    clock: Arc<dyn Fn() -> f64 + Send + Sync>,
}

impl SinkState {
    fn on_buffer(&self, buf: &CMSampleBuffer) {
        let got = (|| -> std::result::Result<(), String> {
            // SAFETY: a sample buffer ScreenCaptureKit handed us for the call's duration.
            let desc = unsafe { buf.format_description() }.ok_or("no format description")?;
            // SAFETY: an audio format description we hold; NULL for a non-audio one, checked below.
            let asbd = unsafe { CMAudioFormatDescriptionGetStreamBasicDescription(&desc) };
            if asbd.is_null() {
                return Err("not an audio format".into());
            }
            // SAFETY: a non-null pointer into the format description, which we hold.
            let asbd = unsafe { *asbd };
            // SAFETY: the sample buffer's own block buffer.
            let block = unsafe { buf.data_buffer() }.ok_or("no data buffer")?;
            // SAFETY: the block buffer's own length.
            let len = unsafe { block.data_length() };
            let mut data = vec![0u8; len];
            if len > 0 {
                // SAFETY: `data` holds `len` bytes; the block buffer has at least that many.
                let status = unsafe {
                    block.copy_data_bytes(
                        0,
                        len,
                        std::ptr::NonNull::new(data.as_mut_ptr().cast()).expect("non-null"),
                    )
                };
                if status != 0 {
                    return Err(format!("CMBlockBufferCopyDataBytes {status}"));
                }
            }
            if asbd.mFormatFlags & FLOAT == 0 || asbd.mBitsPerChannel != 32 {
                return Err(format!(
                    "unexpected audio format (flags {}, {} bits)",
                    asbd.mFormatFlags, asbd.mBitsPerChannel
                ));
            }
            let channels = asbd.mChannelsPerFrame.max(1) as usize;
            if channels > 1 {
                let values: Vec<f32> = data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_ne_bytes(*c))
                    .collect();
                let first: Vec<f32> = if asbd.mFormatFlags & NON_INTERLEAVED != 0 {
                    values[..values.len() / channels].to_vec()
                } else {
                    values.iter().step_by(channels).copied().collect()
                };
                data = first.iter().flat_map(|v| v.to_ne_bytes()).collect();
            }
            // SAFETY: the buffer's presentation time.
            let pts = unsafe { buf.presentation_time_stamp().seconds() };
            let reference = *self.reference.lock().unwrap_or_else(|p| p.into_inner());
            let since = (self.clock)() - *self.started_at.lock().unwrap_or_else(|p| p.into_inner());
            let at = (pts - reference).max(0.0).min(since.max(0.0));
            self.writer
                .write(&data, asbd.mSampleRate as u32, Some(at))
                .map_err(|e| e.to_string())
        })();
        if let Err(e) = got {
            let mut err = self.error.lock().unwrap_or_else(|p| p.into_inner());
            if err.is_none() {
                *err = Some(format!("RuntimeError: {e}"));
            }
        }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the sink has no Drop.
    #[unsafe(super(NSObject))]
    #[ivars = Arc<SinkState>]
    struct AudioSink;

    unsafe impl NSObjectProtocol for AudioSink {}

    unsafe impl SCStreamOutput for AudioSink {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn stream_did_output(&self, _stream: &SCStream, buffer: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind == SCStreamOutputType::Audio {
                objc2::rc::autoreleasepool(|_| self.ivars().on_buffer(buffer));
            }
        }
    }
);

impl AudioSink {
    fn new(state: Arc<SinkState>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(state);
        // SAFETY: NSObject's designated initializer on a freshly allocated, ivar-initialized instance.
        unsafe { msg_send![super(this), init] }
    }
}

struct Sent<T>(T);
// SAFETY: the stream and sink are retained Objective-C objects; ScreenCaptureKit's stream is thread-safe to start and
// stop from any thread, and the sink only touches its own Arc'd, locked state.
unsafe impl<T> Send for Sent<T> {}
// SAFETY: as above: shared references only call thread-safe stop and read locked state.
unsafe impl<T> Sync for Sent<T> {}

fn done<F: FnOnce(&block2::DynBlock<dyn Fn(*mut NSError)>)>(start: F) -> Result<()> {
    let slot: Arc<(Mutex<Option<Option<HandError>>>, Condvar)> =
        Arc::new((Mutex::new(None), Condvar::new()));
    let s2 = slot.clone();
    let handler = RcBlock::new(move |error: *mut NSError| {
        // SAFETY: a valid NSError or null for the call's duration.
        let e = unsafe { Retained::retain(error) }.map(|e| error_of(&e));
        let (m, cv) = &*s2;
        *m.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
        cv.notify_all();
    });
    start(&handler);
    let (m, cv) = &*slot;
    let g = m.lock().unwrap_or_else(|p| p.into_inner());
    let (mut g, _) = cv
        .wait_timeout_while(g, Duration::from_secs_f64(TIMEOUT_S), |v| v.is_none())
        .unwrap_or_else(|p| p.into_inner());
    match g.take() {
        None => Err(HandError::Timeout(
            "the screen capture didn't answer".into(),
        )),
        Some(Some(e)) => Err(e),
        Some(None) => Ok(()),
    }
}

/// One running app-audio stream into one file.
pub struct AudioCapture {
    stream: Sent<Retained<SCStream>>,
    _sink: Sent<Retained<AudioSink>>,
    state: Arc<SinkState>,
}

impl Running for AudioCapture {
    fn started(&self) -> Option<f64> {
        Some(
            *self
                .state
                .started_at
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
        )
    }

    fn stop(&self) -> Map<String, Value> {
        let stream = &self.stream.0;
        // SAFETY: stopping a stream we started; the handler block is copied by ScreenCaptureKit.
        let stopped = done(|h| unsafe { stream.stopCaptureWithCompletionHandler(Some(h)) })
            .err()
            .map(|e| format!("{}: {e}", e.class()));
        let facts = self.state.writer.close();
        let mut out = json!({"frames": facts.frames, "sample_rate": facts.sample_rate, "audio_s": facts.audio_s, "peak": facts.peak}).as_object().cloned().unwrap_or_default();
        let error = self
            .state
            .error
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .or(stopped);
        if let Some(e) = error {
            out.insert("error".into(), json!(e));
        }
        out
    }
}

/// Starts app-audio capture.
pub struct AppAudio {
    pub clock: Arc<dyn Fn() -> f64 + Send + Sync>,
}

impl Capture for AppAudio {
    fn start(&self, bundle_id: &str, pid: i32, path: &Path) -> Result<Arc<dyn Running>> {
        objc2::rc::autoreleasepool(|_| {
            let content = shareable()?;
            // SAFETY: reading the content's lists.
            let apps = unsafe { content.applications() }.to_vec();
            let app: Option<Retained<SCRunningApplication>> =
                apps.into_iter().find(|a| unsafe { a.processID() } == pid && unsafe { a.bundleIdentifier() }.to_string() == bundle_id);
            let Some(app) = app else {
                return Err(HandError::Refused(format!(
                    "{bundle_id} (pid {pid}) isn't one screen capture can hear right now"
                )));
            };
            // SAFETY: reading the content's displays.
            let displays = unsafe { content.displays() }.to_vec();
            if displays.is_empty() {
                return Err(HandError::Refused(
                    "screen capture lists no display to attach the app's sound to".into(),
                ));
            }
            let main = CGMainDisplayID();
            let display = displays
                .iter()
                .find(|d| unsafe { d.displayID() } == main)
                .unwrap_or(&displays[0])
                .clone();
            let no_windows: Retained<NSArray<SCWindow>> = NSArray::new();
            // SAFETY: a display filter including exactly that app.
            let filter = unsafe {
                SCContentFilter::initWithDisplay_includingApplications_exceptingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::from_retained_slice(&[app]),
                    &no_windows,
                )
            };
            // SAFETY: plain configuration setters.
            let config = unsafe { SCStreamConfiguration::new() };
            // SAFETY: plain configuration setters.
            unsafe {
                config.setCapturesAudio(true);
                config.setExcludesCurrentProcessAudio(true);
                config.setCaptureMicrophone(false);
                config.setSampleRate(SAMPLE_RATE as isize);
                config.setChannelCount(1);
                config.setWidth(2);
                config.setHeight(2);
                config.setMinimumFrameInterval(CMTime::new(1, 1));
                config.setShowsCursor(false);
            }
            let writer = WavWriter::create(path).map_err(|e| HandError::Other(e.to_string()))?;
            let state = Arc::new(SinkState {
                writer,
                error: Mutex::new(None),
                reference: Mutex::new(0.0),
                started: Instant::now(),
                started_at: Mutex::new((self.clock)()),
                clock: self.clock.clone(),
            });
            let _ = state.started;
            // SAFETY: a stream from a filter and configuration made above, no delegate.
            let stream = unsafe {
                SCStream::initWithFilter_configuration_delegate(
                    SCStream::alloc(),
                    &filter,
                    &config,
                    None,
                )
            };
            let sink = AudioSink::new(state.clone());
            let proto: &ProtocolObject<dyn SCStreamOutput> = ProtocolObject::from_ref(&*sink);
            let started = (|| -> Result<()> {
                for kind in [SCStreamOutputType::Audio, SCStreamOutputType::Screen] {
                    // SAFETY: the sink stays alive (held by the capture) as long as the stream outputs to it.
                    unsafe {
                        stream.addStreamOutput_type_sampleHandlerQueue_error(proto, kind, None)
                    }
                    .map_err(|e| {
                        HandError::Refused(format!(
                            "screen capture refused the audio output: {}",
                            e.localizedDescription()
                        ))
                    })?;
                }
                // SAFETY: the host clock's time now: no buffer can be presented before capture starts.
                *state.reference.lock().unwrap_or_else(|p| p.into_inner()) =
                    unsafe { CMClock::host_time_clock().time().seconds() };
                *state.started_at.lock().unwrap_or_else(|p| p.into_inner()) = (self.clock)();
                // SAFETY: starting the stream; the handler block is copied by ScreenCaptureKit.
                done(|h| unsafe { stream.startCaptureWithCompletionHandler(Some(h)) })
            })();
            if let Err(e) = started {
                if matches!(e, HandError::Timeout(_)) {
                    // M3: a start answering late must not leave a stream nobody stops (best effort, not waited on)
                    // SAFETY: stopping the stream we asked to start; no handler.
                    unsafe { stream.stopCaptureWithCompletionHandler(None) };
                }
                state.writer.close();
                let _ = std::fs::remove_file(path);
                return Err(e);
            }
            Ok(Arc::new(AudioCapture {
                stream: Sent(stream),
                _sink: Sent(sink),
                state,
            }) as Arc<dyn Running>)
        })
    }
}
