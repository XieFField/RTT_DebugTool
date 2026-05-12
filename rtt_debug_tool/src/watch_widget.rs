//! Watch Tree 树形控件。
//!
//! 渲染变量树: 折叠/展开、值单元格点击编辑、类型颜色标识。

use std::collections::HashSet;
use egui::{Color32, RichText, Ui, Vec2};

use super::watch_state::{VarInfo, Access, WatchState};

/// 树形行渲染。
///
/// - `var`: 当前节点
/// - `depth`: 缩进级别 (0 = 根)
/// - `state`: 当前观测状态 (只读)
/// - `collapsed`: 折叠路径集合
/// - `editing`: 当前编辑状态 `Some((path, buffer))`
/// - `running`: 是否正在接收 (false → 冻结, 不可编辑)
///
/// 返回: 用户是否提交了编辑 `Some(new_value)` 或取消 `Some("")` 或 `None`
pub fn render_tree(
    ui: &mut Ui,
    var: &VarInfo,
    depth: usize,
    collapsed: &mut HashSet<String>,
    editing: &mut Option<(String, String)>,
    running: bool,
) -> Option<String> {
    let mut result = None;
    let indent = depth as f32 * 18.0;

    if var.is_struct {
        // ── 结构体 / 分组 ──
        let is_collapsed = collapsed.contains(&var.path);
        let arrow = if is_collapsed { "▶" } else { "▼" };

        ui.horizontal(|ui| {
            ui.add_space(indent);
            if ui.selectable_label(false, RichText::new(arrow).size(10.0)).clicked() {
                if is_collapsed {
                    collapsed.remove(&var.path);
                } else {
                    collapsed.insert(var.path.clone());
                }
            }
            ui.label(RichText::new(&var.name).strong());
            ui.label(RichText::new("[Struct]").color(Color32::from_gray(140)));
        });

        // 递归子节点
        if !is_collapsed {
            for child in &var.children {
                let r = render_tree(ui, child, depth + 1, collapsed, editing, running);
                if r.is_some() { result = r; }
            }
        }
    } else {
        // ── 叶子节点 ──
        ui.horizontal(|ui| {
            ui.add_space(indent);

            // 路径名
            ui.label(RichText::new(&var.name).color(type_color(&var.type_name)));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // 权限标签
                ui.label(access_label(var.access));

                // 值单元格
                let is_editing = editing.as_ref().map_or(false, |(p, _)| p == &var.path);

                if is_editing && running {
                    if let Some((_, buf)) = editing {
                        let resp = ui.add(
                            egui::TextEdit::singleline(buf)
                                .desired_width(90.0)
                                .font(egui::TextStyle::Body)
                            // 关闭 Enter 自动提交, 改用手动提交
                            .hint_text("edit then click away")
                        );
                        if resp.lost_focus() {
                            result = Some(buf.clone());
                        }
                    }
                } 
                else {
                    // 显示模式
                    let enabled = running && var.access == Access::ReadWrite;
                    let val_color = if enabled {
                        value_type_color(&var.type_name)
                    } 
                    else {
                        Color32::from_gray(120) // 冻结/只读时灰
                    };
                    let btn = egui::Button::new(RichText::new(&var.value).color(val_color))
                        .fill(Color32::from_gray(35))
                        .min_size(Vec2::new(80.0, 18.0));

                    let resp = ui.add_enabled(enabled, btn);
                    if resp.clicked() {
                        *editing = Some((var.path.clone(), var.value.clone()));
                    }
                }
            });
        });
    }

    result
}

/// 访问标签 (RO / RW)
fn access_label(access: Access) -> RichText {
    match access {
        Access::ReadOnly  => RichText::new("RO").color(Color32::from_gray(130)),
        Access::ReadWrite => RichText::new("RW").color(Color32::from_rgb(100, 200, 100)),
    }
}

/// 类型颜色
fn type_color(type_name: &str) -> Color32 {
    match type_name {
        "f32" | "f64"                            => Color32::WHITE,
        "i8" | "i16" | "i32" | "i64"             => Color32::from_rgb(100, 200, 255),
        "u8" | "u16" | "u32" | "u64"             => Color32::from_rgb(100, 220, 255),
        "bool"                                   => Color32::from_rgb(255, 220, 100),
        _ if type_name.starts_with("Str")        => Color32::from_rgb(255, 150, 200),
        _                                        => Color32::from_rgb(255, 180, 100), // struct/unknown → 橙色
    }
}

/// 值颜色 (与类型颜色一致)
fn value_type_color(type_name: &str) -> Color32 {
    type_color(type_name)
}

/// 渲染表头
pub fn render_header(ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Variable").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new("Value").strong());
        });
    });
    ui.separator();
}

/// 渲染搜索栏 (最小宽度 120px)
pub fn render_search(ui: &mut Ui, filter: &mut String) {
    ui.horizontal(|ui| {
        ui.label("(.)"); // magnifier icon placeholder
        ui.add(
            egui::TextEdit::singleline(filter)
                .hint_text("filter...")
                .min_size(egui::vec2(120.0, 0.0))
        );
        if !filter.is_empty() && ui.button("x").clicked() {
            filter.clear();
        }
    });
}
