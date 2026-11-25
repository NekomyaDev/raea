//! Opus read back (libopus), as a Symphonia decoder: Symphonia's MP4 reader finds an Opus track and has no decoder for
//! it, so the MP4s this backend writes itself (H.264 + Opus) came back without their sound wherever no platform reader
//! stood in front (raea #242: on Windows `raea media probe` listed no audio and `raea media share` wrote a silent copy).
//! `codecs()` is Symphonia's registry with this one added; every decoder here is made from it.

use std::sync::{Mutex, OnceLock};

use symphonia::core::audio::{AsAudioBufferRef, AudioBuffer, AudioBufferRef, Channels, Signal, SignalSpec};
use symphonia::core::codecs::{CODEC_TYPE_OPUS, CodecDescriptor, CodecParameters, CodecRegistry, Decoder, DecoderOptions, FinalizeResult};
use symphonia::core::errors::{Result, decode_error, unsupported_error};
use symphonia::core::formats::Packet;
use symphonia::core::support_codec;

/// Opus always decodes at 48 kHz, and a packet holds at most 120 ms of it
const RATE: u32 = 48_000;
const MAX_FRAMES: usize = 5760;

pub struct OpusDecoder {
    params: CodecParameters,
    /// (behind a lock only because a Symphonia decoder must be Sync, which libopus's state isn't: one caller at a time)
    dec: Mutex<opus::Decoder>,
    channels: usize,
    pcm: Vec<f32>,
    buf: AudioBuffer<f32>,
}

/// Symphonia's own codecs and Opus.
pub fn codecs() -> &'static CodecRegistry {
    static ALL: OnceLock<CodecRegistry> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut r = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut r);
        r.register_all::<OpusDecoder>();
        r
    })
}

/// How many channels an Opus track has: its identification header's count (OpusHead, as an MP4's dOps box and an Ogg
/// stream's first packet both carry it: 8 bytes of magic, a version, the count), else what the container said.
fn channel_count(p: &CodecParameters) -> usize {
    let head = p.extra_data.as_deref().filter(|d| d.len() > 9 && d.starts_with(b"OpusHead")).map(|d| d[9] as usize);
    let said = p.channels.map(|c| c.count()).or(p.channel_layout.map(|l| l.into_channels().count()));
    head.or(said).unwrap_or(2)
}

impl Decoder for OpusDecoder {
    fn try_new(params: &CodecParameters, _: &DecoderOptions) -> Result<Self> {
        if params.codec != CODEC_TYPE_OPUS {
            return unsupported_error("opus: not an Opus track");
        }
        // mono and stereo (what raea writes, and nearly every Opus file): a surround stream is several coupled streams
        // under a mapping, which this isn't
        let (channels, layout, how) = match channel_count(params) {
            1 => (1, Channels::FRONT_LEFT, opus::Channels::Mono),
            2 => (2, Channels::FRONT_LEFT | Channels::FRONT_RIGHT, opus::Channels::Stereo),
            _ => return unsupported_error("opus: more than two channels"),
        };
        let dec = match opus::Decoder::new(RATE, how) {
            Ok(d) => d,
            Err(_) => return unsupported_error("opus: the decoder didn't start"),
        };
        let mut params = params.clone();
        params.with_sample_rate(RATE).with_channels(layout);
        Ok(OpusDecoder { params, dec: Mutex::new(dec), channels, pcm: vec![0.0; MAX_FRAMES * channels], buf: AudioBuffer::new(MAX_FRAMES as u64, SignalSpec::new(RATE, layout)) })
    }

    fn supported_codecs() -> &'static [CodecDescriptor] {
        &[support_codec!(CODEC_TYPE_OPUS, "opus", "Opus")]
    }

    fn reset(&mut self) {
        if let Ok(d) = self.dec.get_mut() {
            let _ = d.reset_state();
        }
    }

    fn codec_params(&self) -> &CodecParameters {
        &self.params
    }

    fn decode(&mut self, packet: &Packet) -> Result<AudioBufferRef<'_>> {
        self.buf.clear();
        let n = match self.dec.get_mut().map(|d| d.decode_float(packet.buf(), &mut self.pcm, false)) {
            Ok(Ok(n)) => n,
            _ => return decode_error("opus: a packet didn't decode"),
        };
        self.buf.render_reserved(Some(n));
        for c in 0..self.channels {
            for (o, i) in self.buf.chan_mut(c).iter_mut().zip(self.pcm.iter().skip(c).step_by(self.channels)) {
                *o = *i;
            }
        }
        Ok(self.buf.as_audio_buffer_ref())
    }

    fn finalize(&mut self) -> FinalizeResult {
        FinalizeResult::default()
    }

    fn last_decoded(&self) -> AudioBufferRef<'_> {
        self.buf.as_audio_buffer_ref()
    }
}
