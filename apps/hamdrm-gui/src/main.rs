#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! DecHAMDRM's window, in DecSAT's colours.
//!
//! Left: the receiver (sync lights, figures, constellation, the object being received
//! with its segment map and a BSR button). Middle: spectrum and waterfall of the audio,
//! and the picture being viewed. Right: transmit settings and queue, BSR requests,
//! received files and the log.
//!
//! `dechamdrm-gui [FILE.wav [--speed X]] [--out-dir DIR] [--screenshot OUT.png --after SECS]`

mod chantab;
mod plots;
mod prefs;
mod rxcard;
mod theme;
mod transcode;

use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Frame, Margin, RichText, Sense, Stroke,
    TextureHandle, TextureOptions, Ui, pos2, vec2,
};
use hamdrm_engine::decdsss_rig::{self, Method, Model};
use hamdrm_engine::{
    Bandwidth, Command, Engine, Input, Interleave, Layout, Mode, MscConfig, Options, PttConfig,
    Qam, ReceiverConfig, RxState, TxSettings, View, VoiceKind,
};
use prefs::Prefs;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use theme::*;

struct Shot {
    path: PathBuf,
    after: f64,
    requested: Option<Instant>,
}

#[derive(PartialEq, Clone, Copy)]
enum CentreTab {
    Picture,
    Channel,
}

/// The picture transcoder window.
struct Jp2Window {
    name: String,
    source: std::sync::Arc<image::DynamicImage>,
    source_tex: Option<(TextureHandle, [usize; 2])>,
    /// Pixels waiting to become `source_tex` (textures need the egui context).
    source_rgba: Option<(Vec<u8>, [usize; 2])>,
    settings: transcode::Settings,
    result: Option<transcode::Encoded>,
    result_tex: Option<TextureHandle>,
    error: Option<String>,
}

struct App {
    engine: Engine,
    view: View,
    prefs: Prefs,
    /// Prefs are saved only on interactive runs.
    save_prefs: bool,
    queue: Vec<PathBuf>,
    wf_tex: Option<TextureHandle>,
    wf_pixels: Vec<Color32>,
    wf_seen: u64,
    wf_floor: f64,
    palette: Vec<Color32>,
    fading_tex: Option<TextureHandle>,
    centre_tab: CentreTab,
    voice_on: bool,
    voice_since: Option<Instant>,
    downloading: Option<std::thread::JoinHandle<Result<PathBuf, String>>>,
    images: HashMap<PathBuf, Option<(TextureHandle, [usize; 2])>>,
    viewing: Option<PathBuf>,
    files_seen: usize,
    id_text: String,
    tuning: bool,
    show_settings: bool,
    jp2: Option<Jp2Window>,
    transcoder: transcode::Transcoder,
    inputs: Vec<String>,
    outputs: Vec<String>,
    ports: Vec<String>,
    started: Instant,
    shot: Option<Shot>,
}

const WF_COLS: usize = 1200;
const WF_ROWS: usize = 300;

impl App {
    fn settings(&self) -> TxSettings {
        let p = &self.prefs;
        TxSettings {
            msc: MscConfig {
                layout: Layout::new(p.mode, p.bandwidth),
                qam: p.qam,
                protection: p.protection,
                interleave: if p.long {
                    Interleave::Long
                } else {
                    Interleave::Short
                },
            },
            call: p.call.clone(),
            rs: p.rs,
            lead_in: p.lead_in,
            dc_freq: p.dc_freq,
            rms: p.level,
        }
    }

    /// After pressing Talk the engine needs a moment to start the job.
    fn voice_grace(&mut self) -> bool {
        match self.voice_since {
            Some(t) => t.elapsed().as_secs_f64() > 1.5,
            None => {
                self.voice_since = Some(Instant::now());
                false
            }
        }
    }

    fn update_waterfall(&mut self, ctx: &egui::Context) {
        let new = self.view.waterfall_count.saturating_sub(self.wf_seen) as usize;
        self.wf_seen = self.view.waterfall_count;
        if self.wf_pixels.is_empty() {
            self.wf_pixels = vec![Color32::BLACK; WF_COLS * WF_ROWS];
        }
        let rows = self.view.waterfall.len();
        let take = new.min(rows).min(WF_ROWS);
        if take == 0 && self.wf_tex.is_some() {
            return;
        }
        // Scroll down by `take` rows, newest at the top.
        self.wf_pixels
            .copy_within(0..(WF_ROWS - take) * WF_COLS, take * WF_COLS);
        let bin = self.view.waterfall_bin_hz.max(1.0);
        // Auto level: the noise floor (20th percentile of the newest row) tracks slowly.
        if let Some(row) = self.view.waterfall.back() {
            let mut v: Vec<f32> = row.iter().copied().filter(|d| d.is_finite()).collect();
            if !v.is_empty() {
                v.sort_by(f32::total_cmp);
                // Not more than 40 dB under the strongest bin: a recording with digital
                // silence around the signal would otherwise paint everything at full scale.
                let floor = f64::from(v[v.len() / 5]).max(f64::from(v[v.len() - 1]) - 40.0);
                self.wf_floor = if self.wf_floor < -200.0 {
                    floor
                } else {
                    self.wf_floor + 0.05 * (floor - self.wf_floor)
                };
            }
        }
        for i in 0..take {
            let row = &self.view.waterfall[rows - 1 - i];
            for x in 0..WF_COLS {
                // Strongest bin under this pixel, so narrow lines are never skipped.
                let last = row.len().saturating_sub(1);
                let b0 = ((x as f64 / WF_COLS as f64 * 3500.0 / bin) as usize).min(last);
                let b1 = (((x + 1) as f64 / WF_COLS as f64 * 3500.0 / bin) as usize)
                    .clamp(b0 + 1, last + 1);
                let db = row
                    .get(b0..b1)
                    .and_then(|s| s.iter().copied().reduce(f32::max))
                    .unwrap_or(-140.0);
                let t = ((f64::from(db) - self.wf_floor + 4.0) / 45.0).clamp(0.0, 1.0);
                self.wf_pixels[i * WF_COLS + x] = self.palette[(t * 255.0) as usize];
            }
        }
        let img = ColorImage::new([WF_COLS, WF_ROWS], self.wf_pixels.clone());
        match &mut self.wf_tex {
            Some(t) => t.set(img, TextureOptions::LINEAR),
            None => self.wf_tex = Some(ctx.load_texture("waterfall", img, TextureOptions::LINEAR)),
        }
    }

    fn image(
        &mut self,
        ctx: &egui::Context,
        path: &std::path::Path,
    ) -> Option<(TextureHandle, [usize; 2])> {
        self.images
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                let img = transcode::open_picture(path)?.to_rgba8();
                let size = [img.width() as usize, img.height() as usize];
                let ci = ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                Some((
                    ctx.load_texture(path.display().to_string(), ci, TextureOptions::LINEAR),
                    size,
                ))
            })
            .clone()
    }

    fn screenshot(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.shot else { return };
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            match image::save_buffer(
                &shot.path,
                &rgba,
                image.size[0] as u32,
                image.size[1] as u32,
                image::ColorType::Rgba8,
            ) {
                Ok(()) => eprintln!("screenshot saved to {}", shot.path.display()),
                Err(e) => eprintln!("cannot save {}: {e}", shot.path.display()),
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if shot.requested.is_none() && self.started.elapsed().as_secs_f64() >= shot.after {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            shot.requested = Some(Instant::now());
        } else if shot.requested.is_some_and(|t| t.elapsed().as_secs() > 3) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    // ---------------------------------------------------------------- top bar
    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("DecHAMDRM").strong().color(WHITE).size(16.0));
            ui.add_space(12.0);
            let (txt, col) = if self.view.tx.is_some() {
                ("TX", BAD)
            } else {
                match self.view.status.state {
                    RxState::Acquisition => ("RX · searching", DIM),
                    RxState::Tracking => ("RX · tracking", WARN),
                    RxState::Locked => ("RX · locked", GOOD),
                }
            };
            pill(ui, txt, col);
            if let Some(h) = &self.view.heard {
                ui.label(RichText::new(format!("heard {h}")).color(AMBER).strong());
            }
            ui.add_space(8.0);
            level_meter(ui, self.view.input_db);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Settings").clicked() {
                    self.show_settings = !self.show_settings;
                    if self.show_settings {
                        self.refresh_devices();
                    }
                }
                let ptt = match &self.view.ptt {
                    Some(s) if self.prefs.ptt.keys() => {
                        format!(
                            "PTT {} {}",
                            self.prefs.ptt.method.label(),
                            if s.keyed { "· keyed" } else { "" }
                        )
                    }
                    _ => "PTT: VOX".into(),
                };
                ui.label(RichText::new(ptt).color(DIM));
                ui.label(
                    RichText::new(format!(
                        "in: {}  out: {}",
                        short(&self.view.input_name),
                        short(&self.view.output_name)
                    ))
                    .color(FAINT),
                );
            });
        });
        if let Some(e) = &self.view.input_error {
            ui.label(RichText::new(e).color(BAD));
        }
    }

    // ---------------------------------------------------------------- receiver
    fn receiver(&mut self, ui: &mut Ui) {
        let st = self.view.status.clone();
        rxcard::show(
            ui,
            &st,
            &rxcard::Extra {
                cer: self.view.msc_cer,
                crc_errors: self.view.crc_errors,
                voice: self.view.voice.as_ref().map(|v| (v.good, v.bad)),
                heard: st.label.as_deref(),
            },
        );
        ui.add_space(6.0);
        self.constellations(ui);
        if let Some(v) = &self.view.voice {
            ui.add_space(6.0);
            heading(ui, "Voice");
            ui.label(
                RichText::new(format!(
                    "{}  {}  good {} / bad {}",
                    v.codec,
                    if v.active { "▶" } else { "·" },
                    v.good,
                    v.bad
                ))
                .color(if v.active { GOOD } else { DIM }),
            );
        }
        ui.add_space(6.0);
        heading(ui, "Receiving");
        let mut objs = self.view.objects.clone();
        objs.sort_by_key(|o| std::cmp::Reverse(o.received));
        if objs.is_empty() {
            ui.label(RichText::new("nothing yet").color(FAINT));
        }
        for o in objs.iter().take(3) {
            let name = o
                .name
                .clone()
                .unwrap_or_else(|| format!("transport {}", o.tid));
            let total = o.total.map_or("?".into(), |t| t.to_string());
            ui.label(
                RichText::new(format!(
                    "{name}  {}/{total}{}",
                    o.received,
                    if o.done { "  ✔" } else { "" }
                ))
                .color(TEXT),
            );
            segment_strip(ui, &o.segments, o.done);
            if !o.done
                && o.total.is_some()
                && ui.small_button("Send BSR (ask for missing)").clicked()
            {
                self.engine.send(Command::SendBsr(o.tid, self.settings()));
            }
            ui.add_space(4.0);
        }
    }

    /// FAC (4-QAM) and MSC constellations side by side, the receiver's rolling window
    /// of the latest cells, redrawn every display frame.
    fn constellations(&mut self, ui: &mut Ui) {
        let side = ((ui.available_width() - 6.0) / 2.0).min(150.0);
        let msc_range = match self.view.status.msc.map(|m| m.qam) {
            Some(Qam::Qam4) => 1.2,
            _ => 1.4,
        };
        ui.horizontal(|ui| {
            plots::constellation(ui, "FAC", &self.view.chain.fac, side, 1.2);
            plots::constellation(ui, "MSC", &self.view.chain.msc, side, msc_range);
        });
    }

    // ---------------------------------------------------------------- band
    fn band(&mut self, ui: &mut Ui, height: f32) {
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(w, height), Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 2.0, WELL);
        let spec_h = 110.0f32;
        let spec = egui::Rect::from_min_size(rect.min, vec2(w, spec_h));
        let x_of = |hz: f64| rect.left() + (hz / 3500.0) as f32 * w;
        // Signal band
        if let Some((a, b)) = self.view.signal_band_hz {
            p.rect_filled(
                egui::Rect::from_x_y_ranges(x_of(a)..=x_of(b), rect.y_range()),
                0.0,
                Color32::from_rgba_unmultiplied(80, 200, 255, 18),
            );
        }
        // Grid
        for k in 0..=7 {
            let hz = k as f64 * 500.0;
            let x = x_of(hz);
            p.line_segment(
                [pos2(x, spec.top()), pos2(x, spec.bottom())],
                Stroke::new(1.0, GRID),
            );
            p.text(
                pos2(x + 2.0, spec.bottom() - 2.0),
                Align2::LEFT_BOTTOM,
                format!("{hz:.0}"),
                FontId::proportional(10.0),
                FAINT,
            );
        }
        // Spectrum trace
        let s = &self.view.spectrum_db;
        if !s.is_empty() {
            let max = s.iter().cloned().fold(f64::MIN, f64::max);
            let pts: Vec<egui::Pos2> = s
                .iter()
                .enumerate()
                .filter(|(i, _)| (*i as f64 * self.view.spectrum_bin_hz) <= 3500.0)
                .map(|(i, &db)| {
                    let y = spec.top()
                        + 6.0
                        + ((max - db) / 60.0).clamp(0.0, 1.0) as f32 * (spec_h - 18.0);
                    pos2(x_of(i as f64 * self.view.spectrum_bin_hz), y)
                })
                .collect();
            p.add(egui::Shape::line(pts, Stroke::new(1.2, TRACE)));
        }
        if let Some(dc) = self.view.dc_hz {
            let x = x_of(dc);
            p.line_segment(
                [pos2(x, spec.top()), pos2(x, spec.bottom())],
                Stroke::new(1.0, VFO),
            );
            p.text(
                pos2(x + 3.0, spec.top() + 2.0),
                Align2::LEFT_TOP,
                format!("DC {dc:.0}"),
                FontId::proportional(10.0),
                VFO,
            );
        }
        // Waterfall
        if let Some(t) = &self.wf_tex {
            let wf = egui::Rect::from_min_max(pos2(rect.left(), spec.bottom()), rect.max);
            p.image(
                t.id(),
                wf,
                egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }

    fn viewer(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let Some(path) = self.viewing.clone() else {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("Received pictures appear here").color(FAINT))
            });
            return;
        };
        let info = self.view.files.iter().find(|f| f.path == path).cloned();
        if let Some(f) = &info {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&f.name).strong().color(WHITE));
                ui.label(RichText::new(format!("{} bytes", f.bytes)).color(DIM));
                if let Some(fr) = &f.from {
                    ui.label(RichText::new(format!("from {fr}")).color(AMBER));
                }
                if let Some(rs) = &f.rs {
                    ui.label(RichText::new(rs).color(DIM));
                }
                if ui.small_button("Open folder").clicked() {
                    let _ = open_path(path.parent().unwrap_or(&path));
                }
            });
        }
        match self.image(ctx, &path) {
            Some((tex, size)) => {
                let avail = ui.available_size();
                let s = (avail.x / size[0] as f32)
                    .min(avail.y / size[1] as f32)
                    .min(4.0);
                let sz = vec2(size[0] as f32 * s, size[1] as f32 * s);
                ui.centered_and_justified(|ui| ui.add(egui::Image::new((tex.id(), sz))));
            }
            None => {
                ui.label(
                    RichText::new("not a picture this build can show (saved to disk)").color(DIM),
                );
            }
        }
    }

    // ---------------------------------------------------------------- transmit
    fn transmit(&mut self, ui: &mut Ui) {
        heading(ui, "Transmit");
        let busy = self.view.tx.is_some();
        egui::Grid::new("txset")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("Callsign");
                ui.add(
                    egui::TextEdit::singleline(&mut self.prefs.call)
                        .char_limit(9)
                        .desired_width(110.0),
                );
                ui.end_row();
                ui.label("Mode");
                ui.horizontal(|ui| {
                    for m in Mode::ALL {
                        ui.selectable_value(&mut self.prefs.mode, m, m.to_string());
                    }
                });
                ui.end_row();
                ui.label("Bandwidth");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.prefs.bandwidth, Bandwidth::Narrow, "2.3 kHz");
                    ui.selectable_value(&mut self.prefs.bandwidth, Bandwidth::Wide, "2.5 kHz");
                });
                ui.end_row();
                ui.label("QAM");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.prefs.qam, Qam::Qam4, "4");
                    ui.selectable_value(&mut self.prefs.qam, Qam::Qam16, "16");
                    ui.selectable_value(&mut self.prefs.qam, Qam::Qam64, "64");
                });
                ui.end_row();
                ui.label("Protection");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.prefs.protection, 0, "normal");
                    ui.selectable_value(&mut self.prefs.protection, 1, "low");
                    if self.prefs.qam == Qam::Qam4 && self.prefs.protection == 0 {
                        ui.label(RichText::new("non-compliant").color(WARN).size(10.5))
                            .on_hover_text(
                                "HamDRM has a single 4-QAM code, rate 3/5: that is 4-QAM low                                  here. 4-QAM normal is DecHAMDRM's own rate 1/2 (as 16- and                                  64-QAM normal are): more robust, but EasyPal, EasyDRF and                                  QSSTV cannot receive it. Pick low to work with them.",
                            );
                    }
                    ui.checkbox(&mut self.prefs.long, "long interleave");
                });
                ui.end_row();
                ui.label("Reed–Solomon");
                ui.horizontal(|ui| {
                    for (v, t) in [(0u8, "off"), (1, "1"), (2, "2"), (3, "3"), (4, "4")] {
                        ui.selectable_value(&mut self.prefs.rs, v, t);
                    }
                });
                ui.end_row();
                ui.label("Lead-in");
                ui.add(egui::Slider::new(&mut self.prefs.lead_in, 0..=24).suffix(" seg"));
                ui.end_row();
            });
        let s = self.settings();
        let bytes = hamdrm_engine::frame_bytes(&s.msc);
        ui.label(
            RichText::new(format!(
                "{} bytes / 400 ms  ≈ {:.0} bit/s",
                bytes,
                (bytes - 14) as f64 * 8.0 / 0.4
            ))
            .color(DIM),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Add files…").clicked()
                && let Some(f) = rfd::FileDialog::new().pick_files()
            {
                self.queue.extend(f);
            }
            if ui.button("Picture → JP2…").clicked()
                && let Some(f) = rfd::FileDialog::new()
                    .add_filter(
                        "Pictures",
                        &["jpg", "jpeg", "png", "bmp", "gif", "webp", "jp2", "j2k"],
                    )
                    .pick_file()
            {
                self.open_transcoder(&f);
            }
            if ui.button("Clear").clicked() {
                self.queue.clear();
            }
        });
        for q in &self.queue {
            let size = std::fs::metadata(q).map(|m| m.len()).unwrap_or(0);
            let secs = hamdrm_engine::estimate_seconds(&s, size as usize);
            ui.label(
                RichText::new(format!("{}  {} B  ~{secs:.0} s", file_label(q), size)).color(TEXT),
            );
        }
        if self.queue.is_empty() {
            ui.label(RichText::new("drop files here or add them").color(FAINT));
        }
        ui.horizontal(|ui| {
            let send = ui.add_enabled(
                !busy && !self.queue.is_empty(),
                egui::Button::new(RichText::new("Send").strong()),
            );
            if send.clicked() {
                self.engine.send(Command::SendFiles(
                    std::mem::take(&mut self.queue),
                    s.clone(),
                ));
            }
            if ui.add_enabled(busy, egui::Button::new("Stop")).clicked() {
                self.engine.send(Command::StopTx);
                self.tuning = false;
            }
            if ui.toggle_value(&mut self.tuning, "Tune").changed() {
                self.engine.send(Command::Tune(self.tuning, s.clone()));
            }
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.id_text)
                    .desired_width(120.0)
                    .hint_text("waterfall text"),
            );
            if ui.add_enabled(!busy, egui::Button::new("ID")).clicked() {
                let t = if self.id_text.trim().is_empty() {
                    self.prefs.call.clone()
                } else {
                    self.id_text.clone()
                };
                self.engine.send(Command::SendWaterfall(t, s.clone()));
            }
        });
        if let Some(t) = &self.view.tx {
            ui.label(RichText::new(&t.what).color(BAD));
            if t.total_s > 0.0 {
                ui.add(
                    egui::ProgressBar::new((t.done_s / t.total_s) as f32)
                        .text(format!("{:.0} / {:.0} s", t.done_s, t.total_s)),
                );
            }
        }
        // Voice
        ui.add_space(6.0);
        heading(ui, "Voice");
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.prefs.voice,
                VoiceKind::Neural,
                "Neural (DecHAMDRM)",
            );
            ui.selectable_value(&mut self.prefs.voice, VoiceKind::Lpc10, "LPC-10 (EasyDRF)");
        });
        let neural_missing = self.prefs.voice == VoiceKind::Neural && !self.view.neural_ready;
        if neural_missing {
            ui.label(RichText::new("neural voice weights not downloaded (Settings)").color(WARN));
        }
        let voice_busy = busy && !self.voice_on;
        let talk = ui.add_enabled(
            !voice_busy && !neural_missing,
            egui::Button::new(
                RichText::new(if self.voice_on {
                    "■ Stop talking"
                } else {
                    "● Talk"
                })
                .strong(),
            )
            .selected(self.voice_on),
        );
        if talk.clicked() {
            self.voice_on = !self.voice_on;
            self.voice_since = None;
            self.engine
                .send(Command::Voice(self.voice_on, s.clone(), self.prefs.voice));
        }
        // BSR requests
        if !self.view.bsrs.is_empty() {
            ui.add_space(6.0);
            heading(ui, "Repeat requests");
            for b in self
                .view
                .bsrs
                .iter()
                .rev()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
            {
                ui.horizontal(|ui| {
                    let who = b.from.clone().unwrap_or_else(|| "?".into());
                    match &b.ours {
                        Some(n) => {
                            ui.label(
                                RichText::new(format!(
                                    "{who}: {} seg of {n}",
                                    b.request.missing.len()
                                ))
                                .color(TEXT),
                            );
                            if ui.add_enabled(!busy, egui::Button::new("Repair")).clicked() {
                                self.engine
                                    .send(Command::SendRepair(b.request.clone(), self.settings()));
                            }
                        }
                        None => {
                            ui.label(
                                RichText::new(format!(
                                    "{who}: {} seg, not ours",
                                    b.request.missing.len()
                                ))
                                .color(FAINT),
                            );
                        }
                    }
                });
            }
        }
    }

    fn gallery(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        heading(ui, "Received");
        let files = self.view.files.clone();
        if files.is_empty() {
            ui.label(RichText::new("no files yet").color(FAINT));
        }
        egui::ScrollArea::vertical()
            .id_salt("gallery")
            .max_height(260.0)
            .show(ui, |ui| {
                for f in files.iter().rev() {
                    let selected = self.viewing.as_ref() == Some(&f.path);
                    let resp = ui
                        .horizontal(|ui| {
                            if let Some((tex, size)) = self.image(ctx, &f.path) {
                                let s = 40.0 / size[0].max(size[1]) as f32;
                                ui.add(egui::Image::new((
                                    tex.id(),
                                    vec2(size[0] as f32 * s, size[1] as f32 * s),
                                )));
                            } else {
                                ui.add_sized(
                                    [40.0, 40.0],
                                    egui::Label::new(RichText::new("file").color(FAINT)),
                                );
                            }
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&f.name).color(if selected {
                                    WHITE
                                } else {
                                    TEXT
                                }));
                                ui.label(
                                    RichText::new(format!(
                                        "{}  {} B",
                                        f.from.as_deref().unwrap_or(""),
                                        f.bytes
                                    ))
                                    .color(DIM)
                                    .size(11.0),
                                );
                            });
                        })
                        .response
                        .interact(Sense::click());
                    if resp.clicked() {
                        self.viewing = Some(f.path.clone());
                    }
                }
            });
    }

    fn log(&self, ui: &mut Ui) {
        heading(ui, "Log");
        egui::ScrollArea::vertical()
            .id_salt("log")
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for l in &self.view.log {
                    ui.label(RichText::new(l).color(DIM).size(11.0).monospace());
                }
            });
    }

    // ---------------------------------------------------------------- settings
    fn refresh_devices(&mut self) {
        use hamdrm_engine::decdsss_audio::{list_input_devices, list_output_devices};
        self.inputs = list_input_devices()
            .map(|v| v.into_iter().map(|d| d.name).collect())
            .unwrap_or_default();
        self.outputs = list_output_devices()
            .map(|v| v.into_iter().map(|d| d.name).collect())
            .unwrap_or_default();
        self.ports = decdsss_rig::ports().into_iter().map(|p| p.name).collect();
    }

    // ---------------------------------------------------------------- JPEG 2000
    fn open_transcoder(&mut self, path: &std::path::Path) {
        let Some(img) = transcode::open_picture(path) else {
            eprintln!("cannot read {}", path.display());
            return;
        };
        let src = std::sync::Arc::new(img);
        let rgba = src.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let settings = transcode::Settings {
            quality: self.prefs.jp2_quality,
            max_side: self.prefs.jp2_side,
        };
        self.transcoder.request(src.clone(), settings);
        self.jp2 = Some(Jp2Window {
            name: path
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "picture".into()),
            source: src,
            source_tex: None,
            source_rgba: Some((rgba.into_raw(), size)),
            settings,
            result: None,
            result_tex: None,
            error: None,
        });
    }

    fn jp2_window(&mut self, ctx: &egui::Context) {
        // Collect the newest encode even while the window is being closed.
        let polled = self.transcoder.poll();
        let Some(w) = &mut self.jp2 else { return };
        match polled {
            Some(Ok(e)) => {
                let img = ColorImage::from_rgba_unmultiplied(e.size, &e.rgba);
                match &mut w.result_tex {
                    Some(t) => t.set(img, TextureOptions::LINEAR),
                    None => {
                        w.result_tex =
                            Some(ctx.load_texture("jp2-result", img, TextureOptions::LINEAR))
                    }
                }
                w.error = None;
                w.result = Some(e);
            }
            Some(Err(e)) => w.error = Some(e),
            None => {}
        }
        if let Some((rgba, size)) = w.source_rgba.take() {
            let img = ColorImage::from_rgba_unmultiplied(size, &rgba);
            w.source_tex = Some((
                ctx.load_texture("jp2-source", img, TextureOptions::LINEAR),
                size,
            ));
        }
        let tx_settings = {
            let p = &self.prefs;
            TxSettings {
                msc: MscConfig {
                    layout: Layout::new(p.mode, p.bandwidth),
                    qam: p.qam,
                    protection: p.protection,
                    interleave: if p.long {
                        Interleave::Long
                    } else {
                        Interleave::Short
                    },
                },
                call: p.call.clone(),
                rs: p.rs,
                lead_in: p.lead_in,
                dc_freq: p.dc_freq,
                rms: p.level,
            }
        };
        let mut open = true;
        let mut add = false;
        let before = w.settings;
        egui::Window::new("Picture → JPEG 2000")
            .open(&mut open)
            .default_size([900.0, 560.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Quality");
                    ui.add(egui::Slider::new(&mut w.settings.quality, 1..=100));
                    ui.add_space(12.0);
                    ui.label("Longest side");
                    ui.add(
                        egui::Slider::new(&mut w.settings.max_side, 96..=1280)
                            .logarithmic(true)
                            .suffix(" px"),
                    );
                });
                ui.horizontal(|ui| {
                    match &w.result {
                        Some(r) => {
                            let secs = hamdrm_engine::estimate_seconds(&tx_settings, r.jp2.len());
                            ui.label(
                                RichText::new(format!(
                                    "{} × {}   {} bytes   ≈ {:.0} s on air ({})   encoded in {} ms",
                                    r.size[0],
                                    r.size[1],
                                    r.jp2.len(),
                                    secs,
                                    tx_settings.msc,
                                    r.millis
                                ))
                                .color(WHITE)
                                .strong(),
                            );
                        }
                        None => {
                            ui.label(RichText::new("encoding…").color(DIM));
                        }
                    }
                    if self.transcoder.busy() {
                        ui.spinner();
                    }
                    if let Some(e) = &w.error {
                        ui.label(RichText::new(e).color(BAD));
                    }
                });
                ui.horizontal(|ui| {
                    let ready = w.result.as_ref().is_some_and(|r| r.settings == w.settings)
                        && !self.transcoder.busy();
                    if ui
                        .add_enabled(
                            ready,
                            egui::Button::new(RichText::new("Add to send queue").strong()),
                        )
                        .clicked()
                    {
                        add = true;
                    }
                    ui.label(
                        RichText::new("left: original   right: what the other station will see")
                            .color(DIM),
                    );
                });
                ui.separator();
                let avail = ui.available_size();
                let half = vec2((avail.x - 10.0) / 2.0, avail.y.max(200.0));
                ui.horizontal(|ui| {
                    let show = |ui: &mut Ui, tex: Option<(&TextureHandle, [usize; 2])>| {
                        let (rect, _) = ui.allocate_exact_size(half, Sense::hover());
                        ui.painter().rect_filled(rect, 3.0, WELL);
                        if let Some((t, size)) = tex {
                            let sc = (half.x / size[0] as f32).min(half.y / size[1] as f32);
                            let sz = vec2(size[0] as f32 * sc, size[1] as f32 * sc);
                            ui.painter().image(
                                t.id(),
                                egui::Rect::from_center_size(rect.center(), sz),
                                egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }
                    };
                    show(ui, w.source_tex.as_ref().map(|(t, s)| (t, *s)));
                    show(
                        ui,
                        w.result_tex
                            .as_ref()
                            .zip(w.result.as_ref())
                            .map(|(t, r)| (t, r.size)),
                    );
                });
            });
        if w.settings != before {
            self.transcoder.request(w.source.clone(), w.settings);
            self.prefs.jp2_quality = w.settings.quality;
            self.prefs.jp2_side = w.settings.max_side;
        }
        if add && let Some(r) = &w.result {
            let dir = transcode::tx_dir();
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join(format!("{}.jp2", w.name));
            match std::fs::write(&path, &r.jp2) {
                Ok(()) => {
                    self.queue.push(path);
                    open = false;
                }
                Err(e) => w.error = Some(e.to_string()),
            }
        }
        if !open {
            self.jp2 = None;
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        let mut apply = false;
        // Sound devices take effect as soon as they are picked; the rest on Apply or
        // when the window closes.
        let before = (
            self.prefs.mic.clone(),
            self.prefs.speaker.clone(),
            self.prefs.mic_gain,
            self.prefs.input.clone(),
            self.prefs.output.clone(),
        );
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                egui::Grid::new("set")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Receive from");
                        combo(
                            ui,
                            "in",
                            &mut self.prefs.input,
                            &self.inputs,
                            "system default",
                        );
                        ui.end_row();
                        ui.label("Transmit to");
                        combo(
                            ui,
                            "out",
                            &mut self.prefs.output,
                            &self.outputs,
                            "system default",
                        );
                        ui.end_row();
                        ui.label("TX level");
                        ui.add(egui::Slider::new(&mut self.prefs.level, 0.02..=0.5).text("RMS"));
                        ui.end_row();
                        ui.label("Carrier 0 at");
                        ui.add(
                            egui::Slider::new(&mut self.prefs.dc_freq, 200.0..=600.0).suffix(" Hz"),
                        );
                        ui.end_row();
                        ui.label("PTT");
                        egui::ComboBox::from_id_salt("pttm")
                            .selected_text(self.prefs.ptt.method.label())
                            .show_ui(ui, |ui| {
                                for m in Method::ALL {
                                    ui.selectable_value(&mut self.prefs.ptt.method, m, m.label());
                                }
                            });
                        ui.end_row();
                        ui.label("CAT");
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.prefs.ptt.cat, "on");
                            egui::ComboBox::from_id_salt("rig")
                                .selected_text(self.prefs.ptt.rig.model.name())
                                .show_ui(ui, |ui| {
                                    for m in Model::ALL {
                                        if ui
                                            .selectable_label(
                                                self.prefs.ptt.rig.model == m,
                                                m.label(),
                                            )
                                            .clicked()
                                        {
                                            self.prefs.ptt.rig = decdsss_rig::Settings::new(
                                                m,
                                                &self.prefs.ptt.rig.port,
                                            );
                                        }
                                    }
                                });
                        });
                        ui.end_row();
                        ui.label("CAT port");
                        combo(
                            ui,
                            "catport",
                            &mut self.prefs.ptt.rig.port,
                            &self.ports,
                            "—",
                        );
                        ui.end_row();
                        ui.label("DTR/RTS port");
                        combo(
                            ui,
                            "pttport",
                            &mut self.prefs.ptt.ptt_port,
                            &self.ports,
                            "CAT port",
                        );
                        ui.end_row();
                        ui.label("Lead / tail");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut self.prefs.ptt.lead_ms)
                                    .range(0..=2000)
                                    .suffix(" ms"),
                            );
                            ui.add(
                                egui::DragValue::new(&mut self.prefs.ptt.tail_ms)
                                    .range(0..=2000)
                                    .suffix(" ms"),
                            );
                        });
                        ui.end_row();
                        ui.label("Microphone");
                        combo(
                            ui,
                            "mic",
                            &mut self.prefs.mic,
                            &self.inputs,
                            "system default",
                        );
                        ui.end_row();
                        ui.label("Mic gain");
                        ui.add(
                            egui::Slider::new(&mut self.prefs.mic_gain, 0.1..=8.0)
                                .logarithmic(true),
                        );
                        ui.end_row();
                        ui.label("Voice to");
                        combo(
                            ui,
                            "spk",
                            &mut self.prefs.speaker,
                            &self.outputs,
                            "system default",
                        );
                        ui.end_row();
                        ui.label("Neural voice");
                        ui.horizontal(|ui| {
                            if self.view.neural_ready {
                                ui.label(RichText::new("weights present").color(GOOD));
                            } else if self.downloading.is_some() {
                                ui.spinner();
                                ui.label("downloading 385 MB…");
                            } else if ui
                                .button("Download weights (385 MB, kyutai/mimi)")
                                .clicked()
                            {
                                self.downloading = Some(std::thread::spawn(
                                    hamdrm_engine::download_neural_weights,
                                ));
                            }
                        });
                        ui.end_row();
                        ui.label("BSR");
                        ui.checkbox(
                            &mut self.prefs.auto_bsr,
                            "answer repeat requests for my files automatically",
                        );
                        ui.end_row();
                        ui.label("Received files");
                        ui.label(self.prefs.out_dir.display().to_string());
                        ui.end_row();
                    });
                if let Err(e) = self.prefs.ptt.check() {
                    ui.label(RichText::new(e).color(WARN));
                }
                if ui.button("Apply").clicked() {
                    apply = true;
                }
            });
        // Closing the window with its X applies too (it used to drop the changes).
        apply |= self.show_settings && !open;
        self.show_settings = open;
        let voice_changed = before.0 != self.prefs.mic
            || before.1 != self.prefs.speaker
            || before.2 != self.prefs.mic_gain;
        let input_changed = before.3 != self.prefs.input;
        let output_changed = before.4 != self.prefs.output;
        if apply || input_changed {
            self.engine
                .send(Command::SetInput(Input::Sound(none_if_empty(
                    &self.prefs.input,
                ))));
        }
        if apply || output_changed {
            self.engine
                .send(Command::SetOutput(none_if_empty(&self.prefs.output)));
        }
        if apply {
            self.engine.send(Command::SetPtt(self.prefs.ptt.clone()));
        }
        if apply || voice_changed {
            self.engine.send(Command::SetAudioDevices {
                mic: none_if_empty(&self.prefs.mic),
                speaker: none_if_empty(&self.prefs.speaker),
                mic_gain: self.prefs.mic_gain,
            });
        }
        if (apply || voice_changed || input_changed || output_changed) && self.save_prefs {
            self.prefs.save();
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.view = self.engine.with_view(|v| v.clone());
        if self.view.files.len() > self.files_seen {
            self.files_seen = self.view.files.len();
            self.viewing = self.view.files.last().map(|f| f.path.clone());
        }
        self.update_waterfall(ctx);
        if self.downloading.as_ref().is_some_and(|h| h.is_finished())
            && let Some(h) = self.downloading.take()
        {
            match h.join() {
                Ok(Ok(p)) => eprintln!("weights saved to {}", p.display()),
                Ok(Err(e)) => eprintln!("download failed: {e}"),
                Err(_) => {}
            }
        }
        // A voice transmission the engine ended (or refused) releases the button.
        if self.voice_on
            && self
                .view
                .tx
                .as_ref()
                .is_none_or(|t| !t.what.starts_with("voice"))
            && self.voice_grace()
        {
            self.voice_on = false;
        }
        self.screenshot(ctx);
        // Every display frame: the constellations follow the signal symbol by symbol.
        ctx.request_repaint();
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Drag and drop to the transmit queue.
        let dropped: Vec<PathBuf> = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        self.queue.extend(dropped);
        let plain = |fill: Color32, m: i8| Frame::new().fill(fill).inner_margin(Margin::same(m));
        egui::Panel::top("top")
            .frame(plain(PANEL, 6))
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::left("rx")
            .frame(plain(PANEL, 8))
            .resizable(false)
            .default_size(290.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("rxs")
                    .show(ui, |ui| self.receiver(ui));
            });
        egui::Panel::right("tx")
            .frame(plain(PANEL, 8))
            .resizable(false)
            .default_size(330.0)
            .show(ui, |ui| {
                self.transmit(ui);
                ui.separator();
                self.gallery(ui, &ctx);
                ui.separator();
                self.log(ui);
            });
        egui::CentralPanel::default()
            .frame(plain(BG, 6))
            .show(ui, |ui| {
                let h = (ui.available_height() * 0.45).max(220.0);
                self.band(ui, h);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.centre_tab, CentreTab::Picture, "Picture");
                    ui.selectable_value(&mut self.centre_tab, CentreTab::Channel, "Channel");
                });
                match self.centre_tab {
                    CentreTab::Picture => self.viewer(ui, &ctx),
                    CentreTab::Channel => chantab::show(
                        ui,
                        &self.view.chain,
                        &self.view.status,
                        self.view
                            .status
                            .msc
                            .as_ref()
                            .map(|c| rxcard::needed_snr(c).0),
                        &mut self.fading_tex,
                        &self.palette,
                    ),
                }
            });
        if self.show_settings {
            self.settings_window(&ctx);
        }
        self.jp2_window(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.save_prefs {
            self.prefs.save();
        }
    }
}

// ---------------------------------------------------------------- widgets
fn heading(ui: &mut Ui, t: &str) {
    ui.label(RichText::new(t).strong().color(WHITE).size(14.0));
}

fn pill(ui: &mut Ui, t: &str, c: Color32) {
    Frame::new()
        .fill(PANEL2)
        .corner_radius(8.0)
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(t).color(c).strong());
        });
}

fn level_meter(ui: &mut Ui, db: f64) {
    let (r, _) = ui.allocate_exact_size(vec2(120.0, 10.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, 2.0, WELL_BAR);
    let t = ((db + 60.0) / 60.0).clamp(0.0, 1.0) as f32;
    let col = if db > -3.0 {
        BAD
    } else if db > -10.0 {
        WARN
    } else {
        GOOD
    };
    p.rect_filled(
        egui::Rect::from_min_size(r.min, vec2(r.width() * t, r.height())),
        2.0,
        col,
    );
    ui.label(RichText::new(format!("{db:.0} dBFS")).color(DIM).size(11.0));
}

fn segment_strip(ui: &mut Ui, segs: &[bool], done: bool) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 12.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, 2.0, WELL_BAR);
    if segs.is_empty() {
        return;
    }
    let dx = w / segs.len() as f32;
    // Gaps below the newest received segment are lost; beyond it, still to come.
    // (The last segment is sent early, with the lead-in: it does not count.)
    let newest = segs[..segs.len() - 1].iter().rposition(|&s| s).unwrap_or(0);
    for (i, &s) in segs.iter().enumerate() {
        let c = match (s, done) {
            (true, true) => GREEN_STRIP,
            (true, false) => GOOD,
            (false, _) if i < newest => BAD,
            (false, _) => PANEL2,
        };
        let x = r.left() + i as f32 * dx;
        p.rect_filled(
            egui::Rect::from_min_size(pos2(x, r.top()), vec2(dx.max(1.0), r.height())),
            0.0,
            c,
        );
    }
}

fn combo(ui: &mut Ui, id: &str, value: &mut String, options: &[String], default: &str) {
    let shown = if value.is_empty() {
        default.to_string()
    } else {
        value.clone()
    };
    egui::ComboBox::from_id_salt(id)
        .width(230.0)
        .selected_text(short(&shown))
        .show_ui(ui, |ui| {
            ui.selectable_value(value, String::new(), default);
            for o in options {
                ui.selectable_value(value, o.clone(), o);
            }
        });
}

fn short(s: &str) -> String {
    if s.chars().count() > 34 {
        format!("{}…", s.chars().take(33).collect::<String>())
    } else {
        s.to_string()
    }
}

fn file_label(p: &std::path::Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn none_if_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn open_path(p: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(p)
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open")
            .arg(p)
            .spawn()
            .map(|_| ())
    }
}

fn main() -> eframe::Result {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("dechamdrm-gui {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let mut args = std::env::args().skip(1);
    let mut start_channel = false;
    let mut jp2_arg: Option<PathBuf> = None;
    let mut jp2_quality: Option<u32> = None;
    let (mut tx_qam, mut tx_prot): (Option<String>, Option<u8>) = (None, None);
    let (mut file, mut speed, mut out_dir, mut shot_path, mut after) =
        (None, 1.0, None, None, 10.0);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--speed" => speed = args.next().and_then(|v| v.parse().ok()).unwrap_or(1.0),
            "--out-dir" => out_dir = args.next().map(PathBuf::from),
            "--screenshot" => shot_path = args.next().map(PathBuf::from),
            "--tab" => start_channel = args.next().as_deref() == Some("channel"),
            "--jp2" => jp2_arg = args.next().map(PathBuf::from),
            "--jp2-quality" => jp2_quality = args.next().and_then(|v| v.parse().ok()),
            "--after" => after = args.next().and_then(|v| v.parse().ok()).unwrap_or(10.0),
            // Transmit settings for a screenshot (unattended runs use default settings).
            "--qam" => tx_qam = args.next(),
            "--prot" => tx_prot = args.next().and_then(|v| v.parse().ok()),
            _ => file = Some(PathBuf::from(a)),
        }
    }
    let unattended = shot_path.is_some() || out_dir.is_some();
    let mut prefs = if unattended {
        Prefs::default()
    } else {
        Prefs::load()
    };
    if let Some(d) = &out_dir {
        prefs.out_dir = d.clone();
    }
    match tx_qam.as_deref() {
        Some("4") => prefs.qam = Qam::Qam4,
        Some("16") => prefs.qam = Qam::Qam16,
        Some("64") => prefs.qam = Qam::Qam64,
        _ => {}
    }
    if let Some(p) = tx_prot {
        prefs.protection = p.min(1);
    }
    let input = match &file {
        Some(p) => Input::File {
            path: p.clone(),
            speed,
        },
        // Unattended runs without a file listen to nothing.
        None if unattended => Input::None,
        None => Input::Sound(none_if_empty(&prefs.input)),
    };
    let opts = Options {
        input,
        // Unattended runs never play anything.
        output: if unattended {
            Some(String::new())
        } else {
            none_if_empty(&prefs.output)
        },
        ptt: if unattended {
            PttConfig::default()
        } else {
            prefs.ptt.clone()
        },
        out_dir: prefs.out_dir.clone(),
        rx: ReceiverConfig::default(),
        auto_bsr: prefs.auto_bsr,
        mic: none_if_empty(&prefs.mic),
        speaker: if unattended {
            Some(String::new())
        } else {
            none_if_empty(&prefs.speaker)
        },
        mic_gain: prefs.mic_gain,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1500.0, 900.0])
            .with_min_inner_size([1100.0, 650.0])
            .with_title("DecHAMDRM")
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "DecHAMDRM",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            let mut app = App {
                engine: Engine::start(opts),
                view: View::default(),
                prefs,
                save_prefs: !unattended,
                queue: Vec::new(),
                wf_tex: None,
                wf_pixels: Vec::new(),
                wf_seen: 0,
                wf_floor: -1000.0,
                palette: palette(),
                fading_tex: None,
                centre_tab: if start_channel {
                    CentreTab::Channel
                } else {
                    CentreTab::Picture
                },
                voice_on: false,
                voice_since: None,
                downloading: None,
                images: HashMap::new(),
                viewing: None,
                files_seen: 0,
                id_text: String::new(),
                tuning: false,
                show_settings: false,
                jp2: None,
                transcoder: transcode::Transcoder::new(),
                inputs: Vec::new(),
                outputs: Vec::new(),
                ports: Vec::new(),
                started: Instant::now(),
                shot: shot_path.map(|path| Shot {
                    path,
                    after,
                    requested: None,
                }),
            };
            if let Some(q) = jp2_quality {
                app.prefs.jp2_quality = q.clamp(1, 100);
            }
            if let Some(p) = &jp2_arg {
                app.open_transcoder(p);
            }
            Ok(Box::new(app))
        }),
    )
}
