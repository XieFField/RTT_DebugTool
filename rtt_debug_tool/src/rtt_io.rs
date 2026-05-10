//! probe-rs RTT 通道读写。
//!
//! 连接调试器 → 定位 RTT up1/down0 → 后台线程读取遥测 + 发送下行命令。

use std::sync::{Arc, RwLock, mpsc};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};

use super::watch_state::WatchState;

// ═══════════════════════════════════════════════════════════
// RttClient
// ═══════════════════════════════════════════════════════════

/// RTT 通信客户端。
///
/// 构造时自动连接 probe、attach 芯片、定位 RTT 通道、启动后台线程。
pub struct RttClient {
    /// 下行写命令通道 (用户 → 后台线程)
    cmd_tx: mpsc::Sender<String>,
    /// 后台线程退出信号
    stop_tx: Option<mpsc::Sender<()>>,
    /// 共享观测状态, 后台线程写入, UI 线程读取
    pub state: Arc<RwLock<WatchState>>,
}

impl RttClient {
    /// 连接到调试器并启动 RTT 通信。
    ///
    /// # 参数
    ///
    /// - `chip`: 目标芯片名, 如 `"STM32H723ZG"`
    /// - `up_ch`: 上行遥测通道号 (默认 1)
    /// - `down_ch`: 下行命令通道号 (默认 0)
    pub fn connect(chip: &str, up_ch: usize, down_ch: usize) -> Result<Self> {
        let chip = chip.to_string();
        let (cmd_tx, cmd_rx) = mpsc::channel::<String>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let state = Arc::new(RwLock::new(WatchState::new()));
        let state_clone = Arc::clone(&state);

        thread::spawn(move || {
            if let Err(e) = rtt_thread(&chip, up_ch, down_ch, cmd_rx, stop_rx, state_clone) {
                eprintln!("[RTT] 后台线程错误: {:?}", e);
            }
        });

        Ok(Self {
            cmd_tx,
            stop_tx: Some(stop_tx),
            state,
        })
    }

    /// 发送下行写命令到 MCU。
    ///
    /// 格式: `set <path> <value>\n`
    pub fn send_cmd(&self, path: &str, value: &str) {
        let frame = WatchState::encode_write_cmd(path, value);
        let _ = self.cmd_tx.send(frame);
    }
}

impl Drop for RttClient {
    fn drop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
    }
}

// ═══════════════════════════════════════════════════════════
// 后台 RTT 线程
// ═══════════════════════════════════════════════════════════

fn rtt_thread(
    chip: &str,
    up_ch: usize,
    down_ch: usize,
    cmd_rx: mpsc::Receiver<String>,
    stop_rx: mpsc::Receiver<()>,
    state: Arc<RwLock<WatchState>>,
) -> Result<()> {
    use probe_rs::probe::list::Lister;
    use probe_rs::rtt::Rtt;
    use probe_rs::Permissions;

    // ── 1. 枚举调试器 ──
    let lister = Lister::new();
    let probes = lister.list_all();
    let probe = probes
        .into_iter()
        .next()
        .context("未找到调试器")?
        .open()
        .context("无法打开调试器")?;

    // ── 2. attach 芯片 ──
    let mut session = probe
        .attach(chip, Permissions::default())
        .with_context(|| format!("无法 attach 到 {}", chip))?;

    // ── 3. Core 0 ──
    let mut core = session.core(0).context("无法访问 Core 0")?;

    // ── 4. 初始化 RTT, 验证通道存在 ──
    let mut rtt = Rtt::attach(&mut core).context("RTT 初始化失败")?;

    // 验证通道存在 (但不持有引用, 避免同时借用 up + down)
    if rtt.up_channel(up_ch).is_none() {
        anyhow::bail!("未找到 RTT up ch {}", up_ch);
    }
    if rtt.down_channel(down_ch).is_none() {
        anyhow::bail!("未找到 RTT down ch {}", down_ch);
    }

    eprintln!("[RTT] 已连接: chip={}, up_ch={}, down_ch={}", chip, up_ch, down_ch);

    // ── 5. 主循环 ──
    let mut line_buf = String::new();
    let mut read_buf = vec![0u8; 1024];

    loop {
        if stop_rx.try_recv().is_ok() {
            eprintln!("[RTT] 退出");
            break;
        }

        // 上行: 读遥测 (每次循环重新获取通道引用, 借出完即释放)
        if let Some(up) = rtt.up_channel(up_ch) {
            match up.read(&mut core, &mut read_buf) {
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
                Ok(_) => {}
                Err(e) => {
                    eprintln!("[RTT] 读错误: {:?}", e);
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }

        // 下行: 发写命令
        while let Ok(cmd) = cmd_rx.try_recv() {
            if let Some(down) = rtt.down_channel(down_ch) {
                if let Err(e) = down.write(&mut core, cmd.as_bytes()) {
                    eprintln!("[RTT] 写错误: {:?}", e);
                }
            }
        }

        thread::sleep(Duration::from_millis(1));
    }

    Ok(())
}
