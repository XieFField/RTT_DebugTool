//! # 后台遥测任务与通信协议
//!
//! 通过 RTT 双工通道实现 MCU ↔ 宿主机通信:
//!
//! - **上行 (up ch1)**: MCU 周期发送观测值 → 宿主机刷新 UI
//! - **下行 (down ch0)**: 宿主机发送写命令 → MCU 修改变量
//!
//! ## 协议格式
//!
//! ### 上行 (遥测 + 反馈)
//!
//! ```text
//! arm.pitch.rpm=1000.5\n       ← 遥测数据
//! arm.voltage=24.0\n
//! OK arm.pitch.rpm=1100.0\n     ← 写成功反馈
//! ERR bad_path: not found\n     ← 写失败反馈
//! ```
//!
//! ### 下行 (写命令)
//!
//! ```text
//! set arm.pitch.rpm 1100.0\n    ← 格式: set + 空格 + path + 空格 + value + \n
//! ```
//!
//! ## 通道分配
//!
//! | 通道 | 方向 | 用途 |
//! |------|------|------|
//! | up ch0 | MCU → Host | `rprintln!` 日志 |
//! | up ch1 | MCU → Host | Watch 遥测 + 反馈 |
//! | down ch0 | Host → MCU | Watch 写命令 |
//!
//! ## 频率关系
//!
//! 宿主机刷新频率 = MCU 遥测频率 × 75% (向上取整)。
//! 默认: MCU 40Hz (25ms), Host 30Hz。
//! 上限: MCU 1000Hz (1ms)。

use core::fmt::Write;
use embassy_time::{Duration, Timer};
use heapless::String;
use rtt_target::{UpChannel, DownChannel, ChannelMode};
use embassy_stm32::usart::Uart;
use embassy_stm32::mode::Async;
use embassy_futures::select::{select, Either};
use crate::watch_table;

// ═══════════════════════════════════════════════════════════
// 缓冲区常量
// ═══════════════════════════════════════════════════════════

/// 上行本地累积缓冲大小 (4096 字节)。
///
/// 遥测数据先序列化到此缓冲, 再分批 `write()` 到 RTT 通道,
/// 防止单次写入超过 RTT 缓冲区容量。
const UP_BUF_SIZE: usize = 4096;

/// 下行单次接收缓冲大小 (128 字节)。
const DOWN_BUF_SIZE: usize = 128;

/// 下行行缓冲大小 (一行命令最大 128 字节)。
const DOWN_LINE_SIZE: usize = 128;

// ═══════════════════════════════════════════════════════════
// WatchConfig
// ═══════════════════════════════════════════════════════════

/// 遥测配置参数。
///
/// 用户只需设定 MCU 侧频率, 宿主机频率和周期自动推算。
///
/// # 自动推算规则
///
/// - `period_ms = ceil(1000 / mcu_freq_hz)`, 非整数毫秒向上取整
/// - `host_freq_hz = ceil(mcu_freq_hz × 75%)`
///
/// # 示例
///
/// ```
/// use rtt_debug_tool_mcu::watch_task::WatchConfig;
///
/// // 默认 40Hz
/// let cfg = WatchConfig::default();
/// assert_eq!(cfg.mcu_freq_hz, 40);
/// assert_eq!(cfg.host_freq_hz, 30);
/// assert_eq!(cfg.period_ms, 25);
///
/// // 自定义 100Hz
/// let cfg = WatchConfig::from_freq(100);
/// assert_eq!(cfg.mcu_freq_hz, 100);
/// assert_eq!(cfg.host_freq_hz, 75);
/// assert_eq!(cfg.period_ms, 10);
/// ```
#[derive(Clone, Copy)]
pub struct WatchConfig {
    /// MCU 侧遥测频率 (Hz), 范围 `1..=1000`
    pub mcu_freq_hz: u16,

    /// 宿主机侧刷新频率 (Hz), 由 `mcu_freq_hz × 75%` 自动推算
    pub host_freq_hz: u16,

    /// 遥测周期 (ms), `ceil(1000 / mcu_freq_hz)`
    pub period_ms: u64,

    /// 每次遥测最多遍历的条目数 (默认 64)
    pub max_entries: usize,
}

impl WatchConfig {
    /// 默认配置。
    ///
    /// - MCU: 40Hz (周期 25ms)
    /// - Host: 30Hz
    /// - 最多 64 条目
    pub const fn default() -> Self {
        Self {
            mcu_freq_hz:  40,
            host_freq_hz: 30,
            period_ms:    25,
            max_entries:  64,
        }
    }

    /// 从 MCU 频率创建配置。
    ///
    /// # 参数
    ///
    /// - `freq`: MCU 遥测频率 (Hz), `1..=1000`, 越界自动钳位
    ///
    /// # 返回
    ///
    /// 自动推算 `host_freq_hz` 和 `period_ms` 的配置
    pub const fn from_freq(freq: u16) -> Self {
        let mcu = if freq < 1 {
            1
        } else if freq > 1000 {
            1000
        } else {
            freq
        };
        let period = (1000u32 + mcu as u32 - 1) / mcu as u32;
        let host = ((mcu as u32 * 3 + 3) / 4) as u16;

        Self {
            mcu_freq_hz:  mcu,
            host_freq_hz: host,
            period_ms:    period as u64,
            max_entries:  64,
        }
    }
}

// ═══════════════════════════════════════════════════════════
// watch_config! 宏
// ═══════════════════════════════════════════════════════════

/// 快捷创建 [`WatchConfig`]。
///
/// 用户只需设定 MCU 频率, host 频率和周期自动推算。
///
/// # 用法
///
/// ```
/// use rtt_debug_tool_mcu::watch_config;
///
/// // 默认 (40Hz MCU, 30Hz host, 64 条目)
/// let cfg = watch_config!();
///
/// // 自定义频率 100Hz → host 75Hz, 周期 10ms
/// let cfg = watch_config!(freq: 100);
///
/// // 自定义频率 + 条目上限
/// let cfg = watch_config!(freq: 50, entries: 32);
/// ```
#[macro_export]
macro_rules! watch_config {
    () => {
        $crate::watch_task::WatchConfig::default()
    };
    (freq: $freq:literal) => {
        $crate::watch_task::WatchConfig::from_freq($freq)
    };
    (freq: $freq:literal, entries: $entries:literal) => {{
        let mut c = $crate::watch_task::WatchConfig::from_freq($freq);
        c.max_entries = $entries;
        c
    }};
}

// ═══════════════════════════════════════════════════════════
// debug_watch_task
// ═══════════════════════════════════════════════════════════

/// RTT Watch 后台任务。
///
/// 两个职责交替循环:
///
/// 1. **上行遥测**: 遍历 [`WATCH_TABLE`], 序列化每个条目的当前值
///    为 `"path=value\n"`, 分批写入 RTT up 通道
/// 2. **下行命令**: 非阻塞轮询 RTT down 通道, 解析 `"set path value\n"`,
///    写入目标变量, 反馈 `"OK"` / `"ERR"` 到 up 通道
///
/// # 参数
///
/// - `up_ch`: RTT up channel 1 — 遥测 + 反馈
/// - `down_ch`: RTT down channel 0 — 收写命令
/// - `config`: 遥测频率 / 条目数配置
///
/// # 启动方式
///
/// ```ignore
/// // 1. 初始化 RTT 多通道
/// let channels = rtt_init! {
///     up:   { 0: { size: 1024, name: "Terminal" }
///             1: { size: 1024, name: "Watch" } }
///     down: { 0: { size: 128,  name: "Command" } }
/// };
/// rtt_target::set_print_channel(channels.up.0);  // rprintln! 走 ch0
///
/// // 2. 启动任务
/// spawner.must_spawn(debug_watch_task(
///     channels.up.1,
///     channels.down.0,
///     watch_config!(),
/// ));
/// ```
///
/// # 上行缓冲
///
/// 遥测数据先序列化到本地 4096B 缓冲, 再分批 `write()` 到 RTT。
/// 若单条数据超出缓冲剩余空间, 跳过本条, 下个周期重发。
/// RTT 通道使用 `NoBlockTrim` 模式: 能写多少写多少, 不阻塞 CPU。
#[embassy_executor::task]
pub async fn debug_watch_task(
    mut up_ch: UpChannel,
    mut down_ch: DownChannel,
    config: WatchConfig,
) -> ! {
    let period = Duration::from_millis(config.period_ms);

    let mut up_buf: [u8; UP_BUF_SIZE] = [0u8; UP_BUF_SIZE];
    let mut down_buf: [u8; DOWN_BUF_SIZE] = [0u8; DOWN_BUF_SIZE];
    let mut down_line: String<DOWN_LINE_SIZE> = String::new();

    // NoBlockTrim: 写多少算多少, 不丢数据, 不阻塞
    up_ch.set_mode(ChannelMode::NoBlockTrim);

    loop {
        // ── 1. 上行遥测 ──
        let mut up_len: usize = 0;

        watch_table::with_table(|table| {
            let n = table.len().min(config.max_entries as usize);
            for i in 0..n
            {
                // 缓冲剩余不足 64 字节时停止 (避免拼接中途溢出)
                if up_len + 64 > up_buf.len() { break; }

                if let Some(entry) = table.get(i)
                {
                    if let Some(val) = (entry.read_fn)(entry.ptr, entry.field_idx)
                    {
                        let path_bytes = entry.path.as_bytes();
                        let val_bytes  = val.as_bytes();
                        let needed = path_bytes.len() + 1 + val_bytes.len() + 1;

                        if up_len + needed > up_buf.len() { continue; }

                        up_buf[up_len..up_len + path_bytes.len()]
                            .copy_from_slice(path_bytes);
                        up_len += path_bytes.len();

                        up_buf[up_len] = b'=';
                        up_len += 1;

                        up_buf[up_len..up_len + val_bytes.len()]
                            .copy_from_slice(val_bytes);
                        up_len += val_bytes.len();

                        up_buf[up_len] = b'\n';
                        up_len += 1;
                    }
                }
            }
        });

        // 分批写入 RTT
        if up_len > 0 {
            let mut offset: usize = 0;
            while offset < up_len {
                let n = up_ch.write(&up_buf[offset..up_len]);
                if n == 0 { break; }
                offset += n;
            }
        }

        // ── 2. 下行命令 ──
        let n = down_ch.read(&mut down_buf);
        if n > 0
        {
            for &byte in &down_buf[..n]
            {
                if byte == b'\n' {
                    handle_cmd(&down_line, &mut up_ch);
                    down_line.clear();
                } else if down_line.len() < down_line.capacity() {
                    let _ = down_line.push(byte as char);
                }
            }
        }

        // ── 3. 等待 ──
        Timer::after(period).await;
    }
}

// ═══════════════════════════════════════════════════════════
// 下行命令处理
// ═══════════════════════════════════════════════════════════

/// 解析并执行一行下行命令, 结果回写到 up 通道。
///
/// # 命令格式
///
/// ```text
/// set <path> <value>\n
/// ```
///
/// - path: 观测路径 (如 `"arm.pitch.rpm"`)
/// - value: 新值字符串 (会被 [`WatchValue::watch_write`] 解析)
///
/// # 反馈格式
///
/// ```text
/// OK arm.pitch.rpm=1100.0\n        ← 写入成功
/// ERR arm.pitch.rpm: readonly\n    ← 只读变量
/// ERR arm.pitch.rpm: not found\n   ← 路径不存在
/// ERR arm.pitch.rpm: parse error\n ← 值解析失败
/// ```
fn handle_cmd(line: &str, up_ch: &mut UpChannel) {
    let line = line.trim();
    if line.is_empty() { return; }

    let Some(rest) = line.strip_prefix("set ") else { return; };

    // path 和 value 以最后一个空格分界
    let Some(sep) = rest.rfind(' ') else {
        let _ = write!(up_ch, "ERR parse: expected 'set path value'\n");
        return;
    };

    let path  = &rest[..sep];
    let value = rest[sep + 1..].trim();

    if path.is_empty() || value.is_empty() {
        let _ = write!(up_ch, "ERR parse: empty path or value\n");
        return;
    }

    match watch_table::apply_write(path, value) {
        Ok(()) => {
            let _ = write!(up_ch, "OK {}={}\n", path, value);
        }
        Err(reason) => {
            let _ = write!(up_ch, "ERR {}: {}\n", path, reason);
        }
    }
}

// ═══════════════════════════════════════════════════════════
// UART 传输版本 — Uart<'static, Async> 具体类型, 可用 embassy task 宏
// ═══════════════════════════════════════════════════════════
// TX 走 async Write (DMA), RX 走 nb read (DMA 后台收, CPU 只读状态寄存器)
// nb 不是阻塞轮询 — DMA 硬件在后台填充 RX FIFO, read() 只检查是否有新数据

/// RTT Watch 后台任务 — UART 串口传输版本。
///
/// 接口和 [`debug_watch_task`] 完全一致: `spawner.must_spawn(debug_watch_task_uart(uart, config))`
///
/// # 工作原理
///
/// - **TX**: `uart.write()` → async DMA, 不阻塞 CPU
/// - **RX**: `uart.read()` → nb 轮询, DMA 在后台填充缓冲区,
///   每次迭代只读一个状态寄存器, 无数据立即返回 `WouldBlock`
/// - 发送完上行后, 在 timer 周期内持续读下行命令, timer 到期发下一轮遥测
///
/// # 启动示例
///
/// ```ignore
/// let uart = Uart::new(p.USART1, p.PB7, p.PB6, p.DMA1_CH0, p.DMA1_CH1, Irqs, config);
/// spawner.must_spawn(debug_watch_task_uart(uart, watch_config!()));
/// ```
#[embassy_executor::task]
pub async fn debug_watch_task_uart(
    mut uart: Uart<'static, Async>,
    config: WatchConfig,
) -> ! {
    let period = Duration::from_millis(config.period_ms);
    let mut up_buf: [u8; UP_BUF_SIZE] = [0u8; UP_BUF_SIZE];
    let mut down_buf: [u8; DOWN_BUF_SIZE] = [0u8; DOWN_BUF_SIZE];
    let mut down_line: String<DOWN_LINE_SIZE> = String::new();

    loop {
        // ── 1. 上行: 序列化遥测 → async DMA TX ──
        let mut up_len: usize = 0;
        watch_table::with_table(|table| {
            let n = table.len().min(config.max_entries as usize);
            for i in 0..n {
                if up_len + 64 > up_buf.len() { break; }
                if let Some(entry) = table.get(i) {
                    if let Some(val) = (entry.read_fn)(entry.ptr, entry.field_idx) {
                        let pb = entry.path.as_bytes();
                        let vb = val.as_bytes();
                        let needed = pb.len() + 1 + vb.len() + 1;
                        if up_len + needed > up_buf.len() { continue; }
                        up_buf[up_len..up_len + pb.len()].copy_from_slice(pb);
                        up_len += pb.len();
                        up_buf[up_len] = b'=';
                        up_len += 1;
                        up_buf[up_len..up_len + vb.len()].copy_from_slice(vb);
                        up_len += vb.len();
                        up_buf[up_len] = b'\n';
                        up_len += 1;
                    }
                }
            }
        });

        if up_len > 0 {
            let _ = uart.write(&up_buf[..up_len]).await;
        }

        // ── 2. select: timer 到点 vs DMA 收到下行命令 ──
        // Uart 自带 async fn read(&mut self, &mut [u8]) — DMA 驱动, 不占 CPU
        let tick = Timer::after(period);
        let rx_fut = uart.read(&mut down_buf);

        match select(tick, rx_fut).await {
            Either::First(_) => {
                // timer 到期 → 下一轮上行
            }
            Either::Second(rx_result) => {
                if let Ok(()) = rx_result {
                    // DMA 读成功 — 可能读到 0..n 字节
                    for &byte in down_buf.iter() {
                        if byte == 0 { break; } // 未填充部分跳过
                        if byte == b'\n' {
                            handle_cmd_uart_line(&down_line, &mut uart).await;
                            down_line.clear();
                        } else if down_line.len() < down_line.capacity() {
                            let _ = down_line.push(byte as char);
                        }
                    }
                }
                // 收到命令后不等 timer, 直接开始下一轮上行
            }
        }
    }
}

/// 处理一行下行命令, 回写 OK/ERR
async fn handle_cmd_uart_line(line: &str, uart: &mut Uart<'static, Async>) {
    let line = line.trim();
    if line.is_empty() { return; }

    let Some(rest) = line.strip_prefix("set ") else { return; };
    let Some(sep) = rest.rfind(' ') else {
        let _ = uart.write(b"ERR parse: expected 'set path value'\n").await;
        return;
    };

    let path  = &rest[..sep];
    let value = rest[sep + 1..].trim();
    if path.is_empty() || value.is_empty() {
        let _ = uart.write(b"ERR parse: empty path or value\n").await;
        return;
    }

    match watch_table::apply_write(path, value) {
        Ok(()) => {
            let mut resp: String<128> = String::new();
            let _ = core::write!(resp, "OK {}={}\n", path, value);
            let _ = uart.write(resp.as_bytes()).await;
        }
        Err(reason) => {
            let mut resp: String<128> = String::new();
            let _ = core::write!(resp, "ERR {}: {}\n", path, reason);
            let _ = uart.write(resp.as_bytes()).await;
        }
    }
}

// 注: 因为 embassy task 宏不接受泛型, 用户需写一行具体类型包装 task。
// 详见 examples/rtt_debugtool_uart_demo.rs

