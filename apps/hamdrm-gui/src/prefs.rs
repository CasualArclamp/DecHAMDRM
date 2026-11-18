//! Settings kept between runs: `%APPDATA%\DecHAMDRM\prefs.txt` (key=value lines).

use hamdrm_engine::{Bandwidth, Mode, PttConfig, Qam, VoiceKind};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Prefs {
    pub call: String,
    pub mode: Mode,
    pub bandwidth: Bandwidth,
    pub qam: Qam,
    pub protection: u8,
    pub long: bool,
    pub rs: u8,
    pub lead_in: usize,
    pub dc_freq: f64,
    pub level: f64,
    pub input: String,
    pub output: String,
    pub ptt: PttConfig,
    pub auto_bsr: bool,
    /// Fetch hybrid pictures from the sender's FTP server without asking.
    pub hybrid_fetch: bool,
    pub out_dir: PathBuf,
    pub mic: String,
    pub speaker: String,
    pub mic_gain: f32,
    pub voice: VoiceKind,
    pub jp2_quality: u32,
    pub jp2_side: u32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            call: "NOCALL".into(),
            mode: Mode::B,
            bandwidth: Bandwidth::Wide,
            qam: Qam::Qam16,
            protection: 0,
            long: false,
            rs: 0,
            lead_in: 24,
            dc_freq: 350.0,
            level: 0.2,
            input: String::new(),
            output: String::new(),
            // VOX: nothing is keyed until the user chooses a method.
            ptt: PttConfig::default(),
            auto_bsr: false,
            hybrid_fetch: false,
            out_dir: default_out_dir(),
            mic: String::new(),
            speaker: String::new(),
            mic_gain: 1.0,
            voice: VoiceKind::Neural,
            jp2_quality: 40,
            jp2_side: 480,
        }
    }
}

fn dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("HOME"))
        .map(|d| PathBuf::from(d).join("DecHAMDRM"))
}

fn default_out_dir() -> PathBuf {
    dir()
        .map(|d| d.join("Rx Files"))
        .unwrap_or_else(|| PathBuf::from("Rx Files"))
}

impl Prefs {
    pub fn load() -> Self {
        let mut p = Self::default();
        let Some(text) = dir().and_then(|d| std::fs::read_to_string(d.join("prefs.txt")).ok())
        else {
            return p;
        };
        let map: HashMap<String, String> = text
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect();
        let get = |k: &str| map.get(k).map(String::as_str);
        if let Some(v) = get("call") {
            p.call = v.into();
        }
        p.mode = match get("mode") {
            Some("A") => Mode::A,
            Some("E") => Mode::E,
            _ => Mode::B,
        };
        p.bandwidth = if get("bandwidth") == Some("narrow") {
            Bandwidth::Narrow
        } else {
            Bandwidth::Wide
        };
        p.qam = match get("qam") {
            Some("4") => Qam::Qam4,
            Some("64") => Qam::Qam64,
            _ => Qam::Qam16,
        };
        let num = |k: &str| get(k).and_then(|v| v.parse::<f64>().ok());
        p.protection = num("protection").map_or(0, |v| v as u8).min(1);
        p.long = get("long") == Some("1");
        p.rs = num("rs").map_or(0, |v| v as u8).min(4);
        p.lead_in = num("lead_in").map_or(24, |v| v as usize).min(24);
        p.dc_freq = num("dc_freq").unwrap_or(350.0).clamp(200.0, 600.0);
        p.level = num("level").unwrap_or(0.2).clamp(0.02, 0.5);
        p.input = get("input").unwrap_or_default().into();
        p.output = get("output").unwrap_or_default().into();
        p.auto_bsr = get("auto_bsr") == Some("1");
        p.hybrid_fetch = get("hybrid_fetch") == Some("1");
        if let Some(v) = get("out_dir").filter(|v| !v.is_empty()) {
            p.out_dir = PathBuf::from(v);
        }
        p.mic = get("mic").unwrap_or_default().into();
        p.speaker = get("speaker").unwrap_or_default().into();
        p.mic_gain = num("mic_gain").unwrap_or(1.0).clamp(0.1, 8.0) as f32;
        p.voice = if get("voice") == Some("lpc10") {
            VoiceKind::Lpc10
        } else {
            VoiceKind::Neural
        };
        p.jp2_quality = num("jp2_quality").map_or(40, |v| v as u32).clamp(1, 100);
        p.jp2_side = num("jp2_side").map_or(480, |v| v as u32).clamp(96, 1280);
        p.ptt = PttConfig::from_lookup(|k| map.get(k).map(String::as_str));
        p
    }

    pub fn save(&self) {
        let Some(d) = dir() else { return };
        let _ = std::fs::create_dir_all(&d);
        let mut s = String::new();
        let mut kv = |k: &str, v: String| s.push_str(&format!("{k}={v}\n"));
        kv("call", self.call.clone());
        kv("mode", self.mode.to_string());
        kv(
            "bandwidth",
            if self.bandwidth == Bandwidth::Narrow {
                "narrow"
            } else {
                "wide"
            }
            .into(),
        );
        kv(
            "qam",
            match self.qam {
                Qam::Qam4 => "4",
                Qam::Qam16 => "16",
                Qam::Qam64 => "64",
            }
            .into(),
        );
        kv("protection", self.protection.to_string());
        kv("long", u8::from(self.long).to_string());
        kv("rs", self.rs.to_string());
        kv("lead_in", self.lead_in.to_string());
        kv("dc_freq", self.dc_freq.to_string());
        kv("level", self.level.to_string());
        kv("input", self.input.clone());
        kv("output", self.output.clone());
        kv("auto_bsr", u8::from(self.auto_bsr).to_string());
        kv("hybrid_fetch", u8::from(self.hybrid_fetch).to_string());
        kv("out_dir", self.out_dir.display().to_string());
        kv("mic", self.mic.clone());
        kv("speaker", self.speaker.clone());
        kv("mic_gain", self.mic_gain.to_string());
        kv(
            "voice",
            if self.voice == VoiceKind::Lpc10 {
                "lpc10"
            } else {
                "neural"
            }
            .into(),
        );
        kv("jp2_quality", self.jp2_quality.to_string());
        kv("jp2_side", self.jp2_side.to_string());
        for (k, v) in self.ptt.to_pairs() {
            kv(k, v);
        }
        let _ = std::fs::write(d.join("prefs.txt"), s);
    }
}
