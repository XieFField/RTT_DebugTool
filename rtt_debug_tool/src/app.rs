//! egui App 主循环。
//!
//! 菜单栏 + Watch Tree 主区域 + 启停开关 + 状态栏。

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use egui::{Color32, Context, RichText, TopBottomPanel, CentralPanel, ScrollArea};

use super::rtt_io::{RttClient, ProbeInfo, list_probes};
use super::uart_io::{UartClient, list_ports};
use super::watch_state::WatchState;
use super::watch_widget;

// ═══════════════════════════════════════════════════════════
// 连接模式
// ═══════════════════════════════════════════════════════════

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode { Swd, Uart }

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self { Mode::Swd => write!(f, "SWD"), Mode::Uart => write!(f, "UART") }
    }
}

// ═══════════════════════════════════════════════════════════
// RttWatchApp
// ═══════════════════════════════════════════════════════════

pub struct RttWatchApp {
    // ── 连接 ──
    mode: Mode,
    client_rtt: Option<RttClient>,
    client_uart: Option<UartClient>,
    state: Arc<RwLock<WatchState>>,
    frozen: WatchState,
    running: bool,

    // ── UI ──
    search_filter: String,
    collapsed: HashSet<String>,
    editing: Option<(String, String)>,
    error_msg: Option<String>,

    // ── SWD 参数 ──
    chip: String,
    chip_name: String,
    up_ch: usize,
    down_ch: usize,
    speed_khz: u32,
    probes: Vec<ProbeInfo>,
    probe_index: usize,

    // ── UART 参数 ──
    com_ports: Vec<String>,
    com_port: String,
    com_index: usize,
    baud_rate: u32,
}

impl RttWatchApp {
    pub fn new(chip: String, up_ch: usize, down_ch: usize, speed_khz: u32) -> Self {
        let state = Arc::new(RwLock::new(WatchState::new()));
        let probes = list_probes();
        let com_ports = list_ports();
        Self {
            mode: Mode::Swd,
            client_rtt: None,
            client_uart: None,
            state,
            frozen: WatchState::new(),
            running: false,
            search_filter: String::new(),
            collapsed: HashSet::new(),
            editing: None,
            error_msg: None,
            chip_name: chip.clone(),
            chip,
            up_ch, down_ch, speed_khz,
            probes, probe_index: 0,
            com_ports, com_port: String::new(), com_index: 0, baud_rate: 115200,
        }
    }

    fn connect(&mut self) {
        self.disconnect(); // 先断开旧连接
        match self.mode {
            Mode::Swd => {
                if self.probes.is_empty() {
                    self.error_msg = Some("No probe detected".into());
                    return;
                }
                self.error_msg = Some("Connecting...".into());
                match RttClient::connect(self.probe_index, self.speed_khz, &self.chip, self.up_ch, self.down_ch) {
                    Ok(c) => {
                        self.state = Arc::clone(&c.state);
                        self.client_rtt = Some(c); self.error_msg = None; self.running = true;
                    }
                    Err(e) => self.error_msg = Some(format!("Connect failed: {:?}", e)),
                }
            }
            Mode::Uart => {
                if self.com_port.is_empty() {
                    self.error_msg = Some("No COM port selected".into());
                    return;
                }
                self.error_msg = Some("Connecting...".into());
                match UartClient::connect(&self.com_port, self.baud_rate) {
                    Ok(c) => {
                        self.state = Arc::clone(&c.state);
                        self.client_uart = Some(c); self.error_msg = None; self.running = true;
                    }
                    Err(e) => self.error_msg = Some(format!("Connect failed: {:?}", e)),
                }
            }
        }
    }

    fn disconnect(&mut self) {
        self.client_rtt = None;
        self.client_uart = None;
        self.running = false;
        self.frozen = WatchState::new();
        self.editing = None;
    }

    fn active_snapshot(&self) -> WatchState {
        if self.running { self.state.read().unwrap().clone() }
        else { self.frozen.clone() }
    }

    fn commit_edit(&mut self, path: &str, value: &str) {
        eprintln!("[EDIT] set {} = {}", path, value);
        match self.mode {
            Mode::Swd => { if let Some(ref c) = self.client_rtt { c.send_cmd(path, value); } }
            Mode::Uart => { if let Some(ref c) = self.client_uart { c.send_cmd(path, value); } }
        }
    }

    fn toggle_running(&mut self) {
        if self.running {
            if let Ok(ws) = self.state.read() { self.frozen = ws.clone(); }
            self.running = false;
            self.editing = None;
        } else { self.running = true; }
    }

    fn refresh_probes(&mut self) { self.probes = list_probes(); }
    fn refresh_ports(&mut self) { self.com_ports = list_ports(); }
}

impl eframe::App for RttWatchApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let snapshot = self.active_snapshot();

        // ════ 菜单栏 ════
        TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Connect").clicked() { self.connect(); ui.close_menu(); }
                    if ui.button("Disconnect").clicked() { self.disconnect(); ui.close_menu(); }
                    ui.separator();
                    if ui.button("Exit").clicked() { std::process::exit(0); }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Expand All").clicked() { self.collapsed.clear(); ui.close_menu(); }
                });
            });
        });

        // ════ 错误提示 ════
        let mut dismiss_err = false;
        if let Some(err) = &self.error_msg {
            TopBottomPanel::top("error_bar").show(ctx, |ui| {
                ui.label(RichText::new(err).color(Color32::RED));
                if ui.button("X").clicked() { dismiss_err = true; }
            });
        }
        if dismiss_err { self.error_msg = None; }

        // ════ 主区域 ════
        CentralPanel::default().show(ctx, |ui| {
            // ── Row 0: 模式选择 ──
            ui.horizontal(|ui| {
                ui.label("Mode:");
                if ui.selectable_value(&mut self.mode, Mode::Swd, "SWD (Probe)").clicked()
                || ui.selectable_value(&mut self.mode, Mode::Uart, "UART (COM)").clicked() {
                    self.disconnect();
                }

                ui.separator();

                // 模式相关参数
                match self.mode {
                    Mode::Swd => {
                        ui.label("Probe:");
                        let txt = if self.probes.is_empty() { "(none)".into() }
                            else if self.probe_index < self.probes.len() {
                                format!("[{}] {}", self.probe_index, self.probes[self.probe_index].name)
                            } else { format!("idx {}", self.probe_index) };
                        let old_probe = self.probe_index;
                        egui::ComboBox::from_id_salt("probe").selected_text(&txt).show_ui(ui, |ui| {
                            for (i, p) in self.probes.iter().enumerate() {
                                ui.selectable_value(&mut self.probe_index, i, format!("[{}] {} S/N:{}", i, p.name, p.serial));
                            }
                        });
                        if self.probe_index != old_probe { self.disconnect(); }
                        if ui.button("R").clicked() { self.refresh_probes(); }
                        ui.label("SWD:");
                        ui.add(egui::DragValue::new(&mut self.speed_khz).clamp_range(100..=50000).suffix(" kHz").speed(100));
                    }
                    Mode::Uart => {
                        ui.label("COM:");
                        if self.com_ports.is_empty() {
                            ui.label("(none)");
                        } else {
                            let old_com = self.com_index;
                            egui::ComboBox::from_id_salt("com").selected_text(&self.com_port).show_ui(ui, |ui| {
                                for (i, p) in self.com_ports.iter().enumerate() {
                                    if ui.selectable_label(i == self.com_index, p).clicked() {
                                        self.com_index = i; self.com_port = p.clone();
                                    }
                                }
                            });
                            if self.com_index != old_com { self.disconnect(); }
                        }
                        if ui.button("R").clicked() { self.refresh_ports(); }
                        ui.label("Baud:");
                        ui.add(egui::DragValue::new(&mut self.baud_rate).clamp_range(1200..=4000000).speed(100));
                    }
                }
            });

            // ── Row 1: 启停 ──
            ui.horizontal(|ui| {
                let (txt, col) = if self.running {
                    ("[X] Stop", Color32::from_rgb(200, 60, 60))
                } else {
                    ("[>] Start", Color32::from_rgb(60, 180, 60))
                };
                if ui.add(egui::Button::new(RichText::new(txt).color(Color32::WHITE))
                    .fill(col).min_size(egui::Vec2::new(100.0, 24.0))).clicked()
                {
                    if self.client_rtt.is_some() || self.client_uart.is_some() { self.toggle_running(); }
                    else { self.connect(); }
                }

                ui.separator();

                let avail = ui.available_width();
                ui.add_sized(
                    egui::Vec2::new((avail - 80.0).max(120.0), 0.0),
                    egui::TextEdit::singleline(&mut self.search_filter).hint_text("filter..."),
                );
                if ui.button("R").clicked() { ctx.request_repaint(); }
                let clr_btn = egui::Button::new(RichText::new("Clear").color(Color32::WHITE))
                    .fill(Color32::from_rgb(60, 150, 60))
                    .min_size(egui::Vec2::new(50.0, 0.0));
                if ui.add(clr_btn).clicked() {
                    self.disconnect();
                    self.frozen = WatchState::new();
                    *self.state.write().unwrap() = WatchState::new();
                }
            });
            ui.separator();

            // ── Watch 表格 ──
            watch_widget::render_header(ui);
            ScrollArea::vertical().show(ui, |ui| {
                let f = &self.search_filter;
                let filtered: Vec<_> = if f.is_empty() { snapshot.roots.iter().collect() }
                    else { snapshot.roots.iter().filter(|r| r.path.contains(f.as_str())).collect() };
                if filtered.is_empty() && !snapshot.roots.is_empty() {
                    ui.label(RichText::new("(no match)").color(Color32::from_gray(150)));
                }
                let mut edit_result = None; let mut edit_path = None;
                for root in &filtered {
                    if let Some(val) = watch_widget::render_tree(
                        ui, root, 0, &mut self.collapsed, &mut self.editing, self.running,
                    ) {
                        if let Some((p, _)) = &self.editing { edit_path = Some(p.clone()); edit_result = Some(val); }
                    }
                }
                if let Some(p) = edit_path { if let Some(v) = edit_result {
                    if v.is_empty() { self.editing = None; } else { self.commit_edit(&p, &v); self.editing = None; }
                }}
            });

            let any = self.client_rtt.is_some() || self.client_uart.is_some();
            if !any {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("Not connected").color(Color32::from_gray(150)).size(24.0));
                });
            }
        });

        // ════ 状态栏 ════
        TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let s = if self.running { RichText::new("[RUN]").color(Color32::from_rgb(100, 200, 100)) }
                    else if self.client_rtt.is_some() || self.client_uart.is_some() { RichText::new("[PAUSE]").color(Color32::from_rgb(255, 200, 100)) }
                    else { RichText::new("[OFF]").color(Color32::from_gray(150)) };
                ui.label(s);
                ui.separator();
                ui.label(format!("{}:{} | {} vars",
                    self.mode,
                    match self.mode { Mode::Swd => self.chip_name.as_str(), Mode::Uart => &self.com_port },
                    snapshot.roots.len(),
                ));
            });
        });

        // Enter: commit edit, Esc: cancel
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            if let Some((path, buf)) = self.editing.take() {
                self.commit_edit(&path, &buf);
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.editing = None;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::R)) { ctx.request_repaint(); }
        if self.running { ctx.request_repaint_after(std::time::Duration::from_millis(50)); }
    }
}
