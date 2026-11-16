//! Transmissions: audio generated a block at a time so long files need no big buffer.

use decdsss_audio::InputStream;
use hamdrm_core::fac::Service;
use hamdrm_core::params::{Bandwidth, DEFAULT_DC_FREQ, Layout, Mode, MscConfig, SAMPLE_RATE};
use hamdrm_core::tx::{Transmitter, TxConfig};
use hamdrm_core::wftext;
use hamdrm_data::bsr::{BSR_NAME, BsrRequest, repair_copies};
use hamdrm_data::mot::{MotEncoder, MotObject, segment_size};
use hamdrm_data::packet::PacketEncoder;
use hamdrm_data::send::{OutFile, SendConfig, frames, object_for};
use hamdrm_voice::{SAMPLES_PER_FRAME, VoiceCodec};
use std::f64::consts::PI;

/// How to transmit.
#[derive(Debug, Clone)]
pub struct TxSettings {
    pub msc: MscConfig,
    pub call: String,
    /// EasyPal/QSSTV RS level 0..=4.
    pub rs: u8,
    pub lead_in: usize,
    pub dc_freq: f64,
    /// RMS level, full scale 1.
    pub rms: f64,
}

impl Default for TxSettings {
    fn default() -> Self {
        Self {
            msc: MscConfig::default_picture(),
            call: "NOCALL".into(),
            rs: 0,
            lead_in: 24,
            dc_freq: DEFAULT_DC_FREQ,
            rms: 0.2,
        }
    }
}

const SILENCE: usize = 9600; // 200 ms before and after

enum Kind {
    Frames {
        tx: Box<Transmitter>,
        frames: Vec<Vec<u8>>,
        next: usize,
        flush: usize,
    },
    Audio(Vec<f32>),
    Tune {
        phase: [f64; 3],
        freqs: [f64; 3],
        rms: f64,
    },
    /// Live voice: 400 ms of microphone audio per MSC frame.
    Voice {
        tx: Box<Transmitter>,
        codec: Box<dyn VoiceCodec>,
        mic: Box<InputStream>,
        /// Interleaver flush frames still to send after the operator let go.
        stopping: Option<usize>,
        primed: bool,
        gain: f32,
    },
}

pub(crate) struct TxJob {
    kind: Kind,
    buf: Vec<f32>,
    pos: usize,
    produced: usize,
    total: usize,
    tail: usize,
    label: String,
}

impl TxJob {
    fn new(kind: Kind, total: usize) -> Self {
        Self {
            kind,
            buf: vec![0.0; SILENCE],
            pos: 0,
            produced: 0,
            total: total + 2 * SILENCE,
            tail: SILENCE,
            label: String::new(),
        }
    }

    pub fn with_label(mut self, l: String) -> Self {
        self.label = l;
        self
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    fn transmitter(s: &TxSettings) -> Transmitter {
        let mut c = TxConfig::new(s.msc, &s.call);
        c.dc_freq = s.dc_freq;
        c.rms = s.rms;
        Transmitter::new(c)
    }

    fn of_frames(s: &TxSettings, tx: Transmitter, frames: Vec<Vec<u8>>) -> Self {
        let flush = s.msc.interleave.depth() - 1;
        let total = (frames.len() + flush) * hamdrm_core::params::SAMPLES_PER_FRAME;
        Self::new(
            Kind::Frames {
                tx: Box::new(tx),
                frames,
                next: 0,
                flush,
            },
            total,
        )
    }

    pub fn files(files: &[(String, Vec<u8>)], s: &TxSettings) -> Self {
        let tx = Self::transmitter(s);
        let out: Vec<OutFile> = files
            .iter()
            .map(|(n, d)| OutFile {
                name: n.clone(),
                data: d.clone(),
            })
            .collect();
        let cfg = SendConfig {
            packet_len: tx.packet_len(),
            packet_id: 0,
            rs_level: s.rs,
            lead_in: s.lead_in,
        };
        let f = frames(&out, &cfg);
        Self::of_frames(s, tx, f)
    }

    /// A BSR is sent twice, plain (EasyDRF `Dialog.cpp:2504-2512`).
    pub fn bsr(req: &BsrRequest, s: &TxSettings) -> Self {
        let tx = Self::transmitter(s);
        let f = OutFile {
            name: BSR_NAME.into(),
            data: req.encode(true),
        };
        let cfg = SendConfig {
            packet_len: tx.packet_len(),
            packet_id: 0,
            rs_level: 0,
            lead_in: 0,
        };
        let mut fr = frames(std::slice::from_ref(&f), &cfg);
        fr.extend(frames(&[f], &cfg));
        Self::of_frames(s, tx, fr)
    }

    /// The requested segments of a file we sent, with EasyDRF's extra copies for
    /// short lists (`bsr.cpp:280-294`).
    pub fn repair(name: &str, data: &[u8], req: &BsrRequest, s: &TxSettings) -> Self {
        let tx = Self::transmitter(s);
        let obj: MotObject = object_for(
            &OutFile {
                name: name.into(),
                data: data.to_vec(),
            },
            s.rs,
        );
        let mut mot = MotEncoder::new(segment_size(tx.packet_len()));
        let mut pk = PacketEncoder::new(tx.packet_len(), 0);
        let mut fr = Vec::new();
        for _ in 0..repair_copies(1, req.missing.len()) {
            for g in mot.object(&obj, &req.missing) {
                fr.extend(pk.encode(&g));
            }
        }
        Self::of_frames(s, tx, fr)
    }

    pub fn waterfall(text: &str, s: &TxSettings) -> Self {
        let a = wftext::render(text, s.dc_freq, s.rms);
        let n = a.len();
        Self::new(Kind::Audio(a), n)
    }

    /// EasyDRF's TUNE: three carriers of mode B at k = 8, 24, 32 (`OFDM.cpp:49-55`).
    pub fn tune(s: &TxSettings) -> Self {
        let sp = Layout::new(Mode::B, Bandwidth::Wide).mode.carrier_spacing();
        let freqs = [8.0, 24.0, 32.0].map(|k| s.dc_freq + k * sp);
        Self::new(
            Kind::Tune {
                phase: [0.0; 3],
                freqs,
                rms: s.rms,
            },
            usize::MAX / 4,
        )
    }

    /// Live voice from `mic`, coded with `codec`; FAC service and extension byte as
    /// the codec needs.
    pub fn voice(
        s: &TxSettings,
        codec: Box<dyn VoiceCodec>,
        mic: InputStream,
        service: Service,
        ext: u8,
        gain: f32,
    ) -> Self {
        let mut tx = Self::transmitter(s);
        tx.set_service(service, ext);
        Self::new(
            Kind::Voice {
                tx: Box::new(tx),
                codec,
                mic: Box::new(mic),
                stopping: None,
                primed: false,
                gain,
            },
            usize::MAX / 4,
        )
    }

    pub fn is_voice(&self) -> bool {
        matches!(self.kind, Kind::Voice { .. })
    }

    /// Let go of the voice key: the interleaver is flushed, then the job ends.
    pub fn stop_voice(&mut self) {
        if let Kind::Voice { stopping, tx, .. } = &mut self.kind
            && stopping.is_none()
        {
            *stopping = Some(tx.config().msc.interleave.depth());
        }
    }

    pub fn is_tune(&self) -> bool {
        matches!(self.kind, Kind::Tune { .. })
    }

    pub fn total_s(&self) -> f64 {
        if self.is_tune() || self.is_voice() {
            0.0
        } else {
            self.total as f64 / f64::from(SAMPLE_RATE)
        }
    }

    pub fn done_s(&self) -> f64 {
        self.produced as f64 / f64::from(SAMPLE_RATE)
    }

    pub fn finished(&self) -> bool {
        self.pos >= self.buf.len() && self.tail == 0 && self.source_done()
    }

    fn source_done(&self) -> bool {
        match &self.kind {
            Kind::Frames {
                frames,
                next,
                flush,
                ..
            } => *next >= frames.len() + flush,
            Kind::Audio(a) => a.is_empty(),
            Kind::Tune { .. } => false,
            Kind::Voice { stopping, .. } => *stopping == Some(0),
        }
    }

    fn refill(&mut self) {
        self.buf.clear();
        self.pos = 0;
        match &mut self.kind {
            Kind::Frames {
                tx,
                frames,
                next,
                flush,
            } => {
                if *next < frames.len() {
                    tx.push_frame(&frames[*next], &mut self.buf);
                    *next += 1;
                } else if *next < frames.len() + *flush {
                    tx.push_frame(&[], &mut self.buf);
                    *next += 1;
                }
            }
            Kind::Audio(a) => {
                self.buf = std::mem::take(a);
            }
            Kind::Voice {
                tx,
                codec,
                mic,
                stopping,
                primed,
                gain,
            } => {
                let bits = tx.frame_bytes() * 8;
                let pcm = if !*primed {
                    // One frame of silence first, so the sound card always has a frame
                    // queued while the next 400 ms of speech is captured.
                    *primed = true;
                    Some(vec![0.0f32; SAMPLES_PER_FRAME])
                } else if let Some(left) = stopping {
                    if *left > 0 {
                        *left -= 1;
                        Some(vec![0.0f32; SAMPLES_PER_FRAME])
                    } else {
                        None
                    }
                } else if mic.available_frames() >= SAMPLES_PER_FRAME {
                    let mut x = mic.read(SAMPLES_PER_FRAME);
                    x.iter_mut()
                        .for_each(|v| *v = (*v * *gain).clamp(-1.0, 1.0));
                    Some(x)
                } else {
                    None
                };
                if let Some(pcm) = pcm {
                    let b = codec.encode(&pcm, bits);
                    let bytes: Vec<u8> = b
                        .chunks(8)
                        .map(|c| c.iter().fold(0u8, |a, &v| (a << 1) | (v & 1)))
                        .collect();
                    tx.push_frame(&bytes[..bits / 8], &mut self.buf);
                }
            }
            Kind::Tune { phase, freqs, rms } => {
                let g = *rms * (2.0f64 / 3.0).sqrt();
                for _ in 0..4800 {
                    let mut v = 0.0;
                    for (p, f) in phase.iter_mut().zip(freqs.iter()) {
                        v += p.sin();
                        *p = (*p + 2.0 * PI * f / f64::from(SAMPLE_RATE)) % (2.0 * PI);
                    }
                    self.buf.push((v * g) as f32);
                }
            }
        }
        if self.buf.is_empty() && self.tail > 0 && self.source_done() {
            self.buf = vec![0.0; self.tail];
            self.tail = 0;
        }
    }

    /// Up to `max` samples.
    pub fn next_block(&mut self, max: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(max);
        while out.len() < max {
            if self.pos >= self.buf.len() {
                if self.finished() {
                    break;
                }
                self.refill();
                if self.buf.is_empty() {
                    break;
                }
            }
            let n = (max - out.len()).min(self.buf.len() - self.pos);
            out.extend_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
        }
        self.produced += out.len();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_job_produces_frames_and_silence() {
        let s = TxSettings::default();
        let mut j = TxJob::files(&[("a.bin".into(), vec![1u8; 500])], &s);
        let total = j.total;
        let mut n = 0;
        while !j.finished() {
            let b = j.next_block(4800);
            assert!(!b.is_empty());
            n += b.len();
        }
        assert_eq!(n, total);
    }
}
