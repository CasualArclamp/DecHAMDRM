//! The receiver card, laid out like DecSAT's DVB-S2 card (`decsat-gui/src/side_panel.rs`,
//! `dvb_card` / `dvb_chain`): a title bar in the lock colour, the constellation and
//! protection big with the link margin beside it, an SNR bar ticked at the SNR the mode
//! needs, then the receive chain stage by stage with a health dot each — sync, channel,
//! FAC, the MSC's FEC, and the data packets.

use crate::theme::*;
use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Ui, pos2, vec2};
use hamdrm_core::mlc::MlcParams;
use hamdrm_core::params::{Interleave, Mode, MscConfig, Qam};
use hamdrm_engine::{RxState, RxStatus};

/// 4-QAM rate 1/2 (ours): threshold in mode B 2.5 kHz, and whether it was measured.
const QAM4_HALF_SNR: f64 = 5.0;
const QAM4_HALF_MEASURED: bool = true;

/// SNR (in 2.5 kHz) at which 99 % of MSC frames decode over AWGN. Measured with
/// `awgn_threshold_sweep` (`docs/DESIGN.md`, milestone 3) except 64-QAM protection 1,
/// which is an estimate (`false`).
pub fn needed_snr(c: &MscConfig) -> (f64, bool) {
    let b = c.layout.mode == Mode::B;
    match (c.qam, c.protection) {
        // 4-QAM low is the HamDRM standard code (3/5); normal is our rate 1/2.
        (Qam::Qam4, 0) => (QAM4_HALF_SNR, QAM4_HALF_MEASURED && b),
        (Qam::Qam4, _) => (6.0, b),
        (Qam::Qam16, 0) => (if b { 10.0 } else { 9.5 }, true),
        (Qam::Qam16, _) => (11.5, b),
        (Qam::Qam64, 0) => (15.0, b),
        (Qam::Qam64, _) => (16.5, false),
    }
}

/// Overall code rate of the MSC's multilevel code: information bits per coded bit.
fn code_rate(c: &MscConfig) -> f64 {
    // The rate barely depends on the block length; any typical one will do.
    const CELLS: usize = 1200;
    let p = MlcParams::msc(c.qam, c.protection, CELLS);
    let info: usize = p.bits.iter().sum();
    info as f64 / (CELLS * 2 * p.bits.len()) as f64
}

/// A link margin's colour: red below 0 dB, amber within 1 dB, else green (DecSAT).
fn margin_colour(margin: Option<f64>) -> Color32 {
    match margin {
        Some(m) if m < 0.0 => BAD,
        Some(m) if m < 1.0 => WARN,
        _ => GOOD,
    }
}

/// One stage of the receive chain: its health dot, its name, what it says.
fn stage(ui: &mut Ui, health: Color32, name: &str, text: String) -> egui::Response {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(RichText::new("●").color(health));
        ui.label(RichText::new(name).strong().color(TEXT));
        ui.label(RichText::new(text).color(DIM));
    })
    .response
}

/// What the engine knows beyond [`RxStatus`].
pub struct Extra<'a> {
    /// Cell error rate before FEC, 0..1.
    pub cer: Option<f64>,
    pub crc_errors: u64,
    /// "good / bad" voice frames while a voice service is on.
    pub voice: Option<(u64, u64)>,
    pub heard: Option<&'a str>,
}

pub fn show(ui: &mut Ui, st: &RxStatus, x: &Extra) {
    let signal = st.dc_frequency_hz.is_some();
    let locked = st.state == RxState::Locked;
    let colour = if locked {
        GOOD
    } else if signal {
        GOOD.gamma_multiply(0.55)
    } else {
        FAINT
    };

    // ---- title bar: HamDRM · mode, and the bandwidth
    let title = match st.mode.filter(|_| signal) {
        Some(m) => format!("HamDRM · mode {m}"),
        None => "HamDRM".to_string(),
    };
    let bw = st
        .msc
        .map(|m| m.layout.bandwidth)
        .or(st.bandwidth)
        .filter(|_| signal);
    let bar = egui::Frame::new()
        .fill(colour.gamma_multiply(0.15))
        .corner_radius(CornerRadius {
            nw: 4,
            ne: 4,
            sw: 0,
            se: 0,
        })
        .inner_margin(egui::Margin {
            left: 8,
            right: 8,
            top: 3,
            bottom: 3,
        })
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(title)
                        .monospace()
                        .size(14.0)
                        .strong()
                        .color(colour),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (num, unit) = match bw {
                        Some(hamdrm_core::params::Bandwidth::Narrow) => ("2.3", "kHz wide"),
                        Some(hamdrm_core::params::Bandwidth::Wide) => ("2.5", "kHz wide"),
                        None => ("–", ""),
                    };
                    ui.label(RichText::new(unit).size(11.0).color(DIM));
                    ui.label(RichText::new(num).monospace().size(20.0).color(WHITE));
                });
            });
        })
        .response;
    ui.painter().hline(
        bar.rect.x_range(),
        bar.rect.bottom(),
        egui::Stroke::new(2.0, colour),
    );
    ui.add_space(4.0);

    // ---- the constellation and protection, big, and the margin
    let cfg = st.msc.filter(|_| signal);
    let need = cfg.as_ref().map(needed_snr);
    let margin = need
        .zip(st.snr_db)
        .filter(|_| locked)
        .map(|((n, _), s)| s - n);
    let mcol = margin_colour(margin);
    ui.horizontal(|ui| {
        let name = cfg.map(|c| {
            format!(
                "{} {}",
                c.qam,
                if c.protection == 0 { "normal" } else { "low" }
            )
        });
        ui.label(
            RichText::new(name.as_deref().unwrap_or("–"))
                .monospace()
                .size(24.0)
                .color(if name.is_some() { colour } else { FAINT }),
        );
        if let Some(m) = margin {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{m:+5.1} dB"))
                        .monospace()
                        .size(18.0)
                        .color(mcol),
                );
            });
        }
    })
    .response
    .on_hover_text(
        "The MSC constellation and protection level from the FAC. Margin: the measured \
         SNR less the SNR this mode needs for 99 % of frames over a steady channel. \
         Below 0 dB frames fail; within 1 dB a fade drops them.",
    );

    // ---- the SNR bar on a fixed 0–35 dB scale, ticked at the SNR needed
    let (lo, hi) = (0.0, 35.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
    let x_of =
        |db: f64| rect.left() + rect.width() * ((db - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(2), WELL);
    if let Some(s) = st
        .snr_db
        .filter(|_| signal && st.state != RxState::Acquisition)
    {
        let fill = egui::Rect::from_min_max(rect.min, pos2(x_of(s), rect.max.y));
        p.rect_filled(fill, CornerRadius::same(2), mcol);
    }
    if let Some((n, _)) = need {
        let x = x_of(n);
        p.rect_filled(
            egui::Rect::from_min_max(
                pos2(x - 1.0, rect.top() - 3.0),
                pos2(x + 1.0, rect.bottom() + 3.0),
            ),
            CornerRadius::ZERO,
            WHITE,
        );
    }
    ui.horizontal(|ui| {
        let left = match (st.snr_db, st.state, signal) {
            (_, _, false) => "searching…".to_string(),
            (Some(s), RxState::Locked | RxState::Tracking, _) => format!("SNR {s:.1} dB"),
            (_, RxState::Acquisition, _) => "acquiring…".to_string(),
            _ => "SNR not measured yet".to_string(),
        };
        ui.label(RichText::new(left).size(12.0).color(DIM));
        if let Some((n, measured)) = need {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "needs {}{n:.1} dB",
                        if measured { "" } else { "≈ " }
                    ))
                    .size(12.0)
                    .color(DIM),
                );
            });
        }
    });

    // ---- the receive chain
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.vertical(|ui| {
            // Sync: carrier found by the frequency pilots, then time and frame sync.
            let ppm = st.sro_hz / 48_000.0 * 1e6;
            match (st.dc_frequency_hz, st.state) {
                (None, _) => stage(ui, FAINT, "Sync", "searching for the pilots".into()),
                (Some(dc), RxState::Acquisition) => stage(
                    ui,
                    WARN,
                    "Sync",
                    format!("pilots at DC {dc:.1} Hz · finding frames"),
                ),
                (Some(dc), _) => stage(
                    ui,
                    GOOD,
                    "Sync",
                    format!("locked · DC {dc:.1} Hz · clock {ppm:+.0} ppm"),
                ),
            }
            .on_hover_text(
                "DC: the audio frequency of carrier 0 (350 Hz when tuned exactly), found \
                 from the three frequency pilots and then tracked. Clock: the other \
                 station's sample clock against this sound card's.",
            );

            // Channel
            if signal && st.state != RxState::Acquisition {
                let mer = st.mer_db.map_or("–".into(), |m| format!("{m:.1}"));
                let col = match (st.mer_db, need) {
                    (Some(m), Some((n, _))) if m < n => WARN,
                    _ => GOOD,
                };
                stage(
                    ui,
                    col,
                    "Channel",
                    format!(
                        "MER {mer} dB · Doppler {:.2} Hz · delay {:.2} ms",
                        st.doppler_hz, st.delay_ms
                    ),
                )
                .on_hover_text(
                    "MER: how far the received MSC cells sit from their ideal points. \
                     Doppler: how fast the channel changes. Delay: the spread of the echoes.",
                );
            } else {
                stage(ui, FAINT, "Channel", "–".into());
            }

            // FAC
            let facs = st.fac_ok + st.fac_bad;
            if facs == 0 {
                stage(ui, FAINT, "FAC", "none yet".into());
            } else {
                let pct = 100.0 * st.fac_ok as f64 / facs as f64;
                let col = if !locked {
                    WARN
                } else if pct > 95.0 {
                    GOOD
                } else {
                    WARN
                };
                let who = x.heard.map(|h| format!(" · {h}")).unwrap_or_default();
                stage(
                    ui,
                    col,
                    "FAC",
                    format!("{pct:.1} % good · {} failed{who}", st.fac_bad),
                )
                .on_hover_text(format!(
                    "{} of {} FAC blocks passed their CRC since the start. The FAC names \
                     the mode and the station (4-QAM, rate 3/5).",
                    st.fac_ok, facs
                ));
            }

            // The MSC's FEC
            match cfg {
                Some(c) => {
                    let frames = st.msc_ok + st.msc_doubtful;
                    let rate = code_rate(&c);
                    let cer = x.cer.map(|c| c * 100.0);
                    let col = match (cer, locked) {
                        (_, false) => WARN,
                        (Some(e), _) if e > 12.0 => BAD,
                        (Some(e), _) if e > 6.0 => WARN,
                        _ => GOOD,
                    };
                    let cer_text =
                        cer.map_or(String::new(), |e| format!(" · {e:.1} % cells fixed"));
                    let il = if c.interleave == Interleave::Long {
                        " · 2 s interleave"
                    } else {
                        ""
                    };
                    stage(
                        ui,
                        col,
                        "Viterbi MLC",
                        format!(
                            "rate {rate:.2}{cer_text} · {} of {frames} frames in sync{il}",
                            st.msc_ok
                        ),
                    )
                    .on_hover_text(
                        "The MSC's forward error correction: a multilevel code of K = 7 \
                         convolutional codes, one per bit level, decoded by Viterbi. Rate: \
                         data bits per transmitted bit. Cells fixed: the share of received \
                         cells that were wrong before the decoder corrected them (a few \
                         per cent near the limit, about half when the frame is lost). \
                         In sync: frames decoded with a good FAC and a full interleaver.",
                    );
                }
                None => {
                    stage(ui, FAINT, "Viterbi MLC", "waiting for the FAC".into());
                }
            }

            // What the MSC carries
            match x.voice {
                Some((good, bad)) => {
                    let col = if bad * 20 > good + bad { WARN } else { GOOD };
                    stage(ui, col, "Voice", format!("{good} frames good · {bad} bad"));
                }
                None if cfg.is_some() => {
                    let col = if x.crc_errors == 0 { GOOD } else { WARN };
                    stage(
                        ui,
                        col,
                        "Packets",
                        format!("{} with a bad CRC", x.crc_errors),
                    )
                    .on_hover_text(
                        "Each 400 ms frame carries one data packet with a CRC-16. Packets \
                         that fail it are dropped; Reed-Solomon or a BSR repeat fills the gap.",
                    );
                }
                None => {}
            }
        });
    });
}
