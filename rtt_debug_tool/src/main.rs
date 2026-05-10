//! RTT Debug Tool — 宿主机侧。
//!
//! 通过调试器连接目标板, 实时接收 RTT 遥测数据并以 Watch 窗口展示。

use clap::Parser;
use rtt_debug_tool::rtt_io::RttClient;
use rtt_debug_tool::watch_state::VarInfo;

/// RTT Debug Tool — 嵌入式实时调试工具
#[derive(Parser, Debug)]
#[command(name = "rtt-debug-tool", version = "0.1.0")]
struct Args {
    /// 目标芯片型号
    #[arg(short, long, default_value = "STM32H723ZG")]
    chip: String,

    /// RTT 上行通道号 (MCU → PC)
    #[arg(long, default_value = "1")]
    up_ch: usize,

    /// RTT 下行通道号 (PC → MCU)
    #[arg(long, default_value = "0")]
    down_ch: usize,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    println!("RTT Debug Tool v0.1.0");
    println!("  芯片: {}", args.chip);
    println!("  RTT up={} down={}", args.up_ch, args.down_ch);

    let client = RttClient::connect(&args.chip, args.up_ch, args.down_ch)?;
    println!("已连接, 等待遥测数据...\n");

    // 终端模式: 每秒打印当前遥测树
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let state = client.state.read().unwrap();
        if state.roots.is_empty() {
            println!("  (暂无遥测数据)");
        } else {
            for root in &state.roots {
                print_tree(root, 0);
            }
        }
        println!();
    }
}

fn print_tree(node: &VarInfo, depth: usize) {
    let indent = "  ".repeat(depth);
    if node.is_struct {
        println!("{}▾ {} [Struct]", indent, node.name);
        for child in &node.children {
            print_tree(child, depth + 1);
        }
    } else {
        println!("{}  {} = {} ({})", indent, node.name, node.value, node.access);
    }
}
