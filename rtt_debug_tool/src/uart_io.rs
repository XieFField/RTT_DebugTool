//! 串口通信层 — 通过 COM 口替代 SWD/RTT 传输 Watch 数据。
//!
//! 适用于需要同时使用调试器断点调试的场景 (串口不占用 SWD)。

use std::io::{Read, Write};
use std::sync::{Arc, RwLock, mpsc};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};

use super::watch_state::WatchState;

// ═══════════════════════════════════════════════════════════
// UartClient
// ═══════════════════════════════════════════════════════════

/// 串口通信客户端。
pub struct UartClient {
    cmd_tx: mpsc::Sender<String>,
    stop_tx: Option<mpsc::Sender<()>>,
    /// 共享观测状态, 后台线程写入, UI 线程读取
    pub state: Arc<RwLock<WatchState>>,
}

impl UartClient {
    /// 连接串口并启动后台读取线程。
    ///
    /// # 参数
    ///
    /// - `port_name`: COM 口名称, 如 `"COM3"`, `"/dev/ttyUSB0"`
    /// - `baud_rate`: 波特率, 如 115200
    pub fn connect(port_name: &str, baud_rate: u32) -> Result<Self> {
        let port_name = port_name.to_string();
        let (cmd_tx, cmd_rx) = mpsc::channel::<String>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let state = Arc::new(RwLock::new(WatchState::new()));
        let state_clone = Arc::clone(&state);

        thread::spawn(move || {
            if let Err(e) = uart_thread(&port_name, baud_rate, cmd_rx, stop_rx, state_clone) {
                eprintln!("[UART] 后台线程错误: {:?}", e);
            }
        });

        Ok(Self {
            cmd_tx,
            stop_tx: Some(stop_tx),
            state,
        })
    }

    /// 发送下行写命令到 MCU。
    pub fn send_cmd(&self, path: &str, value: &str) {
        let frame = WatchState::encode_write_cmd(path, value);
        let _ = self.cmd_tx.send(frame);
    }
}

impl Drop for UartClient {
    fn drop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
    }
}

// ═══════════════════════════════════════════════════════════
// 后台串口线程
// ═══════════════════════════════════════════════════════════

fn uart_thread(
    port_name: &str,
    baud_rate: u32,
    cmd_rx: mpsc::Receiver<String>,
    stop_rx: mpsc::Receiver<()>,
    state: Arc<RwLock<WatchState>>,
) -> Result<()> {
    // ── 1. 打开串口 ──
    let mut port = serialport::new(port_name, baud_rate)
        .timeout(Duration::from_millis(10))
        .open()
        .with_context(|| format!("无法打开串口 {}", port_name))?;

    eprintln!("[UART] 已连接: {} @ {} baud", port_name, baud_rate);

    // ── 2. 主循环 ──
    let mut line_buf = String::new();
    let mut read_buf = vec![0u8; 1024];

    loop {
        if stop_rx.try_recv().is_ok() {
            eprintln!("[UART] 退出");
            break;
        }

        // 上行: 读遥测
        match port.read(&mut read_buf) {
            Ok(n) if n > 0 => {
                let text = String::from_utf8_lossy(&read_buf[..n]);
                line_buf.push_str(&text);
                while let Some(pos) = line_buf.find('\n') {
                    let line = line_buf[..pos].to_string();
                    line_buf = line_buf[pos + 1..].to_string();
                    if let Ok(mut ws) = state.write() {
                        ws.handle_line(&line);
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => {
                eprintln!("[UART] 读错误: {:?}", e);
                thread::sleep(Duration::from_millis(10));
            }
            _ => {}
        }

        // 下行: 发写命令
        while let Ok(cmd) = cmd_rx.try_recv() {
            if let Err(e) = port.write_all(cmd.as_bytes()) {
                eprintln!("[UART] 写错误: {:?}", e);
            }
        }

        thread::sleep(Duration::from_millis(1));
    }

    Ok(())
}

/// 枚举系统可用串口列表
pub fn list_ports() -> Vec<String> {
    serialport::available_ports()
        .map(|ports| ports.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default()
}
