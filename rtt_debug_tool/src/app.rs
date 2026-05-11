//! egui App 主循环。
//!
//! 菜单栏 + Watch Tree 主区域 + 启停开关 + 状态栏。

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use egui::{Color32, Context, RichText, TopBottomPanel, CentralPanel, ScrollArea};

use super::rtt_io::{RttClient, ProbeInfo, list_probes};
use super::watch_state::WatchState;
use super::watch_widget;

/// Watch 工具主应用。
pub struct RttWatchApp {
    /// RTT 客户端
    client: Option<RttClient>,

    /// 共享状态
    state: Arc<RwLock<WatchState>>,

    /// 冻结快照
    frozen: WatchState,

    /// 接收开关
    running: bool,

    // ── UI 状态 ──
    search_filter: String,
    collapsed: HashSet<String>,
    editing: Option<(String, String)>,
    error_msg: Option<String>,

    // ── 连接参数 ──
    chip: String,
    chip_name: String,
    up_ch: usize,
    down_ch: usize,
    speed_khz: u32,

    // ── 调试器选择 ──
    probes: Vec<ProbeInfo>,
    probe_index: usize,
}

impl RttWatchApp {
    pub fn new(chip: String, up_ch: usize, down_ch: usize, speed_khz: u32) -> Self {
        let state = Arc::new(RwLock::new(WatchState::new()));
        let probes = list_probes();
        Self {
            client: None,
            state,
            frozen: WatchState::new(),
            running: false,
            search_filter: String::new(),
            collapsed: HashSet::new(),
            editing: None,
            error_msg: None,
            chip_name: chip.clone(),
            chip,
            up_ch,
            down_ch,
            speed_khz,
            probes,
            probe_index: 0,
        }
    }

    fn connect(&mut self) {
        if self.probes.is_empty() {
            self.error_msg = Some("No probe detected".into());
            return;
        }
        self.error_msg = Some("Connecting... (scanning RAM for RTT, may take ~30s)".into());
        match RttClient::connect(self.probe_index, self.speed_khz, &self.chip, self.up_ch, self.down_ch) {
            Ok(client) => {
                self.state = Arc::clone(&client.state);
                self.client = Some(client);
                self.error_msg = None;
                self.running = true;
            }
            Err(e) => {
                self.error_msg = Some(format!("Connect failed: {:?}", e));
            }
        }
    }

    fn disconnect(&mut self) {
        self.client = None;
        self.running = false;
        self.frozen = WatchState::new();
        self.editing = None;
    }

    fn active_snapshot(&self) -> WatchState {
        if self.running 
        {
            self.state.read().unwrap().clone()
        } 
        else 
        {
            self.frozen.clone()
        }
    }

    fn commit_edit(&mut self, path: &str, value: &str) {
        if let Some(ref client) = self.client 
        {
            client.send_cmd(path, value);
        }
    }

    fn toggle_running(&mut self) {
        if self.running 
        {
            if let Ok(ws) = self.state.read() 
            {
                self.frozen = ws.clone();
            }
            self.running = false;
            self.editing = None;
        } 
        else 
        {
            self.running = true;
        }
    }

    fn refresh_probes(&mut self) 
    {
        self.probes = list_probes();
    }
}

impl eframe::App for RttWatchApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let snapshot = self.active_snapshot();

        // ════ 菜单栏 ════
        TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("文件", |ui| {
                    if ui.button("连接").clicked() { self.connect(); ui.close_menu(); }
                    if ui.button("断开").clicked() { self.disconnect(); ui.close_menu(); }
                    ui.separator();
                    if ui.button("退出").clicked() { std::process::exit(0); }
                });
                ui.menu_button("视图", |ui| {
                    if ui.button("展开全部").clicked() { self.collapsed.clear(); ui.close_menu(); }
                });
            });
        });

        // ════ 错误提示 ════
        let mut dismiss_err = false;
        if let Some(err) = &self.error_msg {
            TopBottomPanel::top("error_bar").show(ctx, |ui| {
                ui.label(RichText::new(err).color(Color32::RED));
                if ui.button("关闭").clicked() { dismiss_err = true; }
            });
        }
        if dismiss_err { self.error_msg = None; }

        // ════ 主区域 ════
        CentralPanel::default().show(ctx, |ui| {
            // ── Row 1: 探头选择 ──
            ui.horizontal(|ui| {
                ui.label("Probe:");
                let probe_text = if self.probes.is_empty() {
                    "(none)".to_string()
                } else if self.probe_index < self.probes.len() {
                    format!("[{}] {}", self.probe_index, self.probes[self.probe_index].name)
                } else {
                    format!("idx {} out of range", self.probe_index)
                };
                egui::ComboBox::from_id_salt("probe_select")
                    .selected_text(&probe_text)
                    .show_ui(ui, |ui| {
                        for (i, p) in self.probes.iter().enumerate() {
                            let label = format!("[{}] {} (S/N:{})", i, p.name, p.serial);
                            ui.selectable_value(&mut self.probe_index, i, &label);
                        }
                    });
                if ui.button("R").on_hover_text("refresh probe list").clicked() {
                    self.refresh_probes();
                }

                ui.separator();

                // SWD 速度
                ui.label("SWD:");
                ui.add(
                    egui::DragValue::new(&mut self.speed_khz)
                        .clamp_range(100..=50000)
                        .suffix(" kHz")
                        .speed(100),
                );

                ui.separator();

                // 启停开关
                let (btn_text, btn_color) = if self.running {
                    ("[X] Stop", Color32::from_rgb(200, 60, 60))
                } else {
                    ("[▶] Start", Color32::from_rgb(60, 180, 60))
                };
                if ui.add(
                    egui::Button::new(RichText::new(btn_text).color(Color32::WHITE))
                        .fill(btn_color).min_size(egui::Vec2::new(100.0, 24.0))
                ).clicked() {
                    if self.client.is_some() { self.toggle_running(); }
                    else { self.connect(); }
                }
            });

            // ── Row 2: 搜索栏 (横向拉伸) + 刷新 ──
            ui.horizontal(|ui| {
                let available = ui.available_width();
                ui.add_sized(
                    egui::Vec2::new((available - 50.0).max(120.0), 0.0),
                    egui::TextEdit::singleline(&mut self.search_filter)
                        .hint_text("filter..."),
                );
                if ui.button("R").on_hover_text("manual refresh").clicked() {
                    ctx.request_repaint();
                }
            });
            ui.separator();

            // ── Watch 表格 ──
            watch_widget::render_header(ui);

            ScrollArea::vertical().show(ui, |ui| {
                let filter = &self.search_filter;
                let filtered: Vec<&super::watch_state::VarInfo> = if filter.is_empty() {
                    snapshot.roots.iter().collect()
                } 
                else 
                {
                    snapshot.roots.iter()
                        .filter(|r| r.path.contains(filter.as_str()))
                        .collect()
                };

                if filtered.is_empty() && !snapshot.roots.is_empty() {
                    ui.label(RichText::new("(无匹配)").color(Color32::from_gray(150)));
                }

                let mut edit_result: Option<String> = None;
                let mut edit_path: Option<String> = None;

                for root in &filtered {
                    if let Some(val) = watch_widget::render_tree(
                        ui, root, 0, &mut self.collapsed, &mut self.editing, self.running,
                    ) {
                        if let Some((path, _)) = &self.editing {
                            edit_path = Some(path.clone());
                            edit_result = Some(val);
                        }
                    }
                }

                if let Some(path) = edit_path {
                    if let Some(val) = edit_result {
                        if val.is_empty() {
                            self.editing = None;
                        } else {
                            self.commit_edit(&path, &val);
                            self.editing = None;
                        }
                    }
                }
            });

            // 未连接提示
            if self.client.is_none() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    if self.probes.is_empty() 
                    {
                        ui.label(RichText::new("未检测到调试器，请连接 DAP-Link 后点击 🔄 刷新").color(Color32::from_gray(150)));
                    } 
                    else 
                    {
                        ui.label(format!("已检测到 {} 个调试器, 请选择后点击 ▶ 开始接收", self.probes.len()));
                    }
                });
            }
        });

        // ════ 状态栏 ════
        TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let status = if self.running {
                    RichText::new("[RUN]").color(Color32::from_rgb(100, 200, 100))
                } else if self.client.is_some() {
                    RichText::new("[PAUSE]").color(Color32::from_rgb(255, 200, 100))
                } else {
                    RichText::new("[OFF]").color(Color32::from_gray(150))
                };
                ui.label(status);
                ui.separator();
                ui.label(format!("probe#{} | chip={} | {} vars",
                    self.probe_index,
                    self.chip_name,
                    snapshot.roots.len(),
                ));
            });
        });

        // ════ 快捷键 ════
        if ctx.input(|i| i.key_pressed(egui::Key::R)) { ctx.request_repaint(); }

        // ════ 定时刷新 ════
        if self.running {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}
