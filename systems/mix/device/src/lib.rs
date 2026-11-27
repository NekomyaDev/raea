//! The mixer's output: the system's default device, with Firewheel's processor pulled from cpal's callback, followed
//! when the system's output changes (headphones in, AirPods away, a display's speakers) or cpal says the stream broke
//! (11-26, Shane: "sometimes i dont hear any sound": the stream stayed on the device it opened on, gone or not the one
//! being listened to).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use audioadapter_buffers::direct::InterleavedSlice;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
pub use cpal::Stream;
use firewheel::backend::BackendProcessInfo;
use firewheel::node::StreamStatus;
use firewheel::{ActivateInfo, FirewheelContext};

/// The output a mixer plays on: the device's name (empty while there's none), the rate and block size it asks for,
/// and whether cpal said the stream broke.
pub struct Output {
    pub name: String,
    rate: u32,
    block: u32,
    lost: Arc<AtomicBool>,
    looked: Instant,
}

impl Output {
    /// The default device's stream, and the rate it runs at (`rate` when the device takes it).
    pub fn open(cx: &mut FirewheelContext, rate: u32, block: u32) -> Result<(Output, Stream, u32), String> {
        let lost = Arc::new(AtomicBool::new(false));
        let (stream, got, name) = start(cx, rate, block, lost.clone())?;
        Ok((Output { name, rate, block, lost, looked: Instant::now() }, stream, got))
    }

    /// Whether the stream should move: at once after cpal said it broke, else when the system's default output, looked
    /// at once a second, is another device.
    pub fn moved(&mut self) -> bool {
        let lost = self.lost.load(Ordering::Relaxed);
        if !lost && self.looked.elapsed() < Duration::from_secs(1) {
            return false;
        }
        self.looked = Instant::now();
        let now = cpal::default_host().default_output_device().and_then(|d| d.name().ok()).unwrap_or_default();
        lost || (!now.is_empty() && now != self.name)
    }

    /// The stream started again on the default device: `old` is let go first, which hands the graph back (its
    /// processor dropped). The new stream and its rate, or no device to play on (looked for again in a second).
    pub fn reopen(&mut self, cx: &mut FirewheelContext, old: Option<Stream>) -> Result<(Stream, u32), String> {
        drop(old);
        let t0 = Instant::now();
        while cx.is_active() && t0.elapsed() < Duration::from_millis(500) {
            let _ = cx.update();
            std::thread::sleep(Duration::from_millis(1));
        }
        self.lost.store(false, Ordering::Relaxed);
        match start(cx, self.rate, self.block, self.lost.clone()) {
            Ok((stream, rate, name)) => {
                eprintln!("raea mixer: sound now on {name} (was {})", if self.name.is_empty() { "nothing" } else { &self.name });
                self.name = name;
                Ok((stream, rate))
            }
            Err(e) => {
                if !self.name.is_empty() {
                    eprintln!("raea mixer: no sound: {e}");
                }
                self.name.clear();
                Err(e)
            }
        }
    }
}

/// The default output device, at `want` Hz when it takes it, `block` frames a callback when it takes that: Firewheel's
/// processor pulled from cpal's callback, as the bounce pulls it into a buffer (stereo, spread over the device's
/// channels).
fn start(cx: &mut FirewheelContext, want: u32, block: u32, lost: Arc<AtomicBool>) -> Result<(Stream, u32, String), String> {
    let host = cpal::default_host();
    let dev = host.default_output_device().ok_or("no audio output device")?;
    let name = dev.name().unwrap_or_default();
    let def = dev.default_output_config().map_err(|e| format!("audio output: {e}"))?;
    let channels = def.channels();
    let takes_48k = dev.supported_output_configs().map(|mut c| c.any(|r| r.channels() == channels && r.sample_format() == cpal::SampleFormat::F32 && r.min_sample_rate().0 <= want && r.max_sample_rate().0 >= want)).unwrap_or(false);
    let rate = if takes_48k { want } else { def.sample_rate().0 };
    let max_block = 8192usize;
    let mut proc = cx
        .activate(ActivateInfo { sample_rate: std::num::NonZeroU32::new(rate).unwrap(), max_block_frames: std::num::NonZeroU32::new(max_block as u32).unwrap(), num_stream_in_channels: 0, num_stream_out_channels: 2, input_to_output_latency_seconds: 0.0 })
        .map_err(|e| format!("the mixer's graph: {e:?}"))?;
    let ch = channels as usize;
    let began = std::time::Instant::now();
    let mut stereo = vec![0f32; max_block * 2];
    let empty: [f32; 0] = [];
    let mut callback = move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
        let ts = info.timestamp();
        let delay = ts.playback.duration_since(&ts.callback);
        let frames = out.len() / ch.max(1);
        let mut done = 0;
        while done < frames {
            let k = (frames - done).min(max_block);
            let pi = BackendProcessInfo { frames: k, process_timestamp: Some(std::time::Instant::now()), duration_since_stream_start: began.elapsed(), input_stream_status: StreamStatus::empty(), output_stream_status: StreamStatus::empty(), dropped_frames: 0, process_to_playback_delay: delay };
            if let (Ok(input), Ok(mut output)) = (InterleavedSlice::new(&empty[..], 0, 0), InterleavedSlice::new_mut(&mut stereo[..k * 2], 2, k)) {
                proc.process(&input, &mut output, pi);
            }
            for i in 0..k {
                let (l, r) = (stereo[2 * i], stereo[2 * i + 1]);
                let o = &mut out[(done + i) * ch..(done + i + 1) * ch];
                match ch {
                    1 => o[0] = 0.5 * (l + r),
                    _ => {
                        o[0] = l;
                        o[1] = r;
                        o[2..].fill(0.0);
                    }
                }
            }
            done += k;
        }
    };
    // the block size when the device's range takes it, else its own
    let fits = dev.supported_output_configs().map(|mut c| c.any(|r| r.channels() == channels && matches!(r.buffer_size(), cpal::SupportedBufferSize::Range { min, max } if *min <= block && block <= *max))).unwrap_or(false);
    let config = cpal::StreamConfig { channels, sample_rate: cpal::SampleRate(rate), buffer_size: if fits { cpal::BufferSize::Fixed(block) } else { cpal::BufferSize::Default } };
    let mut said = raea_io_trace::once::said_once::<cpal::StreamError>("raea mixer");
    let broke = move |e: cpal::StreamError| {
        lost.store(true, Ordering::Relaxed);
        said(e)
    };
    let stream = dev.build_output_stream(&config, move |o: &mut [f32], i: &cpal::OutputCallbackInfo| callback(o, i), broke, None).map_err(|e| format!("audio stream: {e}"))?;
    stream.play().map_err(|e| format!("audio stream: {e}"))?;
    Ok((stream, rate, name))
}
