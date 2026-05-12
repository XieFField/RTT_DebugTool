
// #![windows_subsystem = "windows"]  // 调试期间注释, 否则 eprintln! 无输出
use clap::Parser;
use rtt_debug_tool::app::RttWatchApp;

#[derive(Parser, Debug)]
#[command(name = "rtt-debug-tool", version = "0.1.0")]
struct Args {
    #[arg(short, long, default_value = "STM32H723ZG")]
    chip: String,

    #[arg(long, default_value = "1")]
    up_ch: usize,

    #[arg(long, default_value = "0")]
    down_ch: usize,

    /// 探针 SWD 时钟速度 (kHz), 默认 5000
    #[arg(long, default_value = "5000")]
    speed: u32,
}

fn main() {


    let args = Args::parse();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([500.0, 300.0])
            .with_title("RTT Debug Tool"),
        ..Default::default()
    };

    eframe::run_native(
        "RTT Debug Tool",
        native_options,
        Box::new(|cc| {
            setup_chinese_fonts(&cc.egui_ctx);
            Ok(Box::new(RttWatchApp::new(
                args.chip.clone(),
                args.up_ch,
                args.down_ch,
                args.speed,
            )))
        }),
    ).expect("eframe 启动失败");
}

/// 注入系统中文等宽字体, 解决 egui 默认字体无法渲染中文的问题。
fn setup_chinese_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions, FontFamily};

    let mut fonts = FontDefinitions::default();

    // Windows 中文字体候选 (按优先级)
    let chinese_font_names = [
        "Microsoft YaHei",  // 微软雅黑
        "SimHei",            // 黑体
        "SimSun",            // 宋体
        "Noto Sans CJK SC",  // Linux
        "PingFang SC",       // macOS
    ];

    // 尝试加载第一个可用的系统字体
    for name in &chinese_font_names {
        if let Some(data) = try_load_system_font(name) {
            fonts
                .font_data
                .insert(name.to_string(), FontData::from_owned(data).into());
            // 将中文字体插入 Proportional 和 Monospace 的最前面
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, name.to_string());
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .insert(1, name.to_string()); // 保留默认等宽在前
            break;
        }
    }

    ctx.set_fonts(fonts);
}

#[cfg(target_os = "windows")]
fn try_load_system_font(name: &str) -> Option<Vec<u8>> {
    use std::fs;
    // Windows 字体目录
    let fonts_dir = std::path::Path::new("C:/Windows/Fonts");
    // 尝试常见扩展名
    for ext in &["ttf", "ttc", "otf"] {
        let path = fonts_dir.join(format!("{}.{}", name, ext));
        if let Ok(data) = fs::read(&path) {
            return Some(data);
        }
    }
    // 某些字体名与文件名不同, 尝试 msyh (微软雅黑简写)
    let aliases: &[(&str, &[&str])] = &[
        ("Microsoft YaHei", &["msyh.ttf", "msyh.ttc"]),
        ("SimHei", &["simhei.ttf"]),
        ("SimSun", &["simsun.ttf", "simsun.ttc"]),
    ];
    for (alias_name, files) in aliases {
        if name == *alias_name {
            for file in *files {
                let path = fonts_dir.join(file);
                if let Ok(data) = fs::read(&path) {
                    return Some(data);
                }
            }
        }
    }
    None
}

#[cfg(not(target_os = "windows"))]
fn try_load_system_font(_name: &str) -> Option<Vec<u8>> {
    // 额，其实没试过在其他系统上运行这个工具，先返回 None 吧
    None
}
