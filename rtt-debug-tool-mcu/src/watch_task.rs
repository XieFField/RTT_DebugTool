//! @file watch_task.rs
//! @brief 上行发送和下行接收，并在此规定协议
//! 上行协议：arm.target_height=500.000\n
//!           写命令反馈：OK arm.target_height=600.000\n  ERR bad_path: not found\n
//! 下行协议：set arm.target_height 600.0\n 格式：set + 空格 + path + 空格 + value + \n
//! 通道分配： up0 用于rprintln! up1 用于上行协议 down0 用于下行协议

use core::fmt::Write;
use embassy_time::{Duration, Timer};
use heapless::String;
use rtt_target::{UpChannel, DownChannel, ChannelMode};

use crate::watch_table;

// 上行缓冲区常量
/// 上行本地累积缓冲大小 
/// 序列化后先存这里, 再分批 write 到 RTT
const UP_BUF_SIZE: usize = 4096;

/// 下行单次接收缓冲大小
const DOWN_BUF_SIZE: usize = 128;

/// 下行行缓冲大小 一行命令最大长度
const DOWN_LINE_SIZE: usize = 128;

/// 遥测配置参数
#[derive(Clone, Copy)]
pub struct WatchConfig {
    /// MCU 侧遥测频率 (Hz), 1..=1000
    pub mcu_freq_hz: u16,

    /// 宿主机侧刷新频率 (Hz), 由 mcu_freq_hz × 75% 向上取整
    pub host_freq_hz: u16,

    /// 遥测周期 (ms), ceil(1000 / mcu_freq_hz)
    pub period_ms: u64,

    /// 每次遥测最多遍历的条目数
    pub max_entries: usize,
}

impl WatchConfig {
    /// - 默认配置: MCU 40Hz → host 30Hz, period 25ms, max 64 条目
    pub const fn default() -> Self {
        Self {
            mcu_freq_hz:  40,
            host_freq_hz: 30,
            period_ms:    25,
            max_entries:  64,
        }
    }

    /// ## 从 MCU 频率创建配置, 自动推算 host 频率和周期
    /// - freq 范围: 1..=1000 Hz, 越界自动钳位
    /// - period_ms = ceil(1000 / freq), 非整数毫秒向上取整
    /// - host_freq_hz = ceil(freq × 3 / 4), 即 MCU 频率的 75% 向上取整
    pub const fn from_freq(freq: u16) -> Self {
        let mcu = if freq < 1 {
            1
        } else if freq > 1000 {
            1000
        } else {
            freq
        };
        // ceil(1000 / mcu)
        let period = (1000u32 + mcu as u32 - 1) / mcu as u32;
        // ceil(mcu * 3 / 4)
        let host = ((mcu as u32 * 3 + 3) / 4) as u16;

        Self {
            mcu_freq_hz:  mcu,
            host_freq_hz: host,
            period_ms:    period as u64,
            max_entries:  64,
        }
    }
}


/// ## 快捷配置遥测参数
///
/// 用户只需设定 MCU 侧频率, host 频率和周期自动推算
///
/// ### 用法
/// ```ignore
/// // 使用默认值 40Hz MCU, 30Hz host
/// let cfg = watch_config!();
///
/// // 自定义频率
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

/// ## RTT Watch 后台任务
///
/// 两个职责交替循环:
/// 1. 上行遥测: 遍历注册表 → 序列化到本地缓冲 → 分批写入 RTT up 通道
/// 2. 下行命令: 非阻塞轮询 RTT down 通道 → 解析 "set path value\n" → 写入目标变量
///
/// # 参数
/// - `up_ch`:   RTT up channel 1, 用于发送遥测数据和命令反馈
/// - `down_ch`: RTT down channel 0, 用于接收写命令
/// - `config`:  遥测配置 (频率 / 条目数)
#[embassy_executor::task]
pub async fn debug_watch_task(
    mut up_ch: UpChannel,
    mut down_ch: DownChannel,
    config: WatchConfig,
) -> ! {
    let period = Duration::from_millis(config.period_ms);

    // 上行缓冲: 先在本地序列化, 再分批 write 到 RTT
    let mut up_buf: [u8; UP_BUF_SIZE] = [0u8; UP_BUF_SIZE];

    // 下行缓冲
    let mut down_buf: [u8; DOWN_BUF_SIZE] = [0u8; DOWN_BUF_SIZE];
    let mut down_line: String<DOWN_LINE_SIZE> = String::new();

    // 使用 NoBlockTrim 模式: 能写多少写多少, 不阻塞, 不丢弃
    up_ch.set_mode(ChannelMode::NoBlockTrim);

    loop {
        // ═══════════════════════════════════════════════════
        // 1. 上行遥测
        // ═══════════════════════════════════════════════════

        let mut up_len: usize = 0;

        watch_table::with_table(|table| {
            let n = table.len().min(config.max_entries as usize);
            for i in 0..n 
            {
                // 缓冲剩余空间不足一条最大长度时停止 (预留 64 字节余量)
                if up_len + 64 > up_buf.len() { break; }

                if let Some(entry) = table.get(i) 
                {
                    if let Some(val) = (entry.read_fn)(entry.ptr) 
                    {
                        // 拼接 "path=value\n"
                        let path_bytes = entry.path.as_bytes();
                        let val_bytes  = val.as_bytes();

                        // 检查是否放得下
                        let needed = path_bytes.len() + 1 + val_bytes.len() + 1;
                        if up_len + needed > up_buf.len() 
                        {
                            continue; // 跳过本条, 下个周期再发
                        }

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

        // 分批写入 RTT  NoBlockTrim 模式下每次 write 返回实际写入字节
        if up_len > 0 {
            let mut offset: usize = 0;
            while offset < up_len {
                let n = up_ch.write(&up_buf[offset..up_len]);
                if n == 0 {
                    // 缓冲区满, 剩余数据下个周期再发
                    break;
                }
                offset += n;
            }
        }

        //轮询下行指令
        let n = down_ch.read(&mut down_buf);
        if n > 0 
        {
            for &byte in &down_buf[..n] 
            {
                if byte == b'\n' {
                    // 收到完整一行 → 处理
                    handle_cmd(&down_line, &mut up_ch);
                    down_line.clear();
                } else if down_line.len() < down_line.capacity() {
                    // 行缓冲未满, 追加字符
                    let _ = down_line.push(byte as char);
                }
            }
        }

        // ═══════════════════════════════════════════════════
        // 3. 等待下一周期
        // ═══════════════════════════════════════════════════

        Timer::after(period).await;
    }
}

// ═══════════════════════════════════════════════════════════
// 下行命令处理
// ═══════════════════════════════════════════════════════════

/// 解析并执行一行命令, 结果回写到 up_ch
fn handle_cmd(line: &str, up_ch: &mut UpChannel) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }

    // 格式: "set path value"
    let Some(rest) = line.strip_prefix("set ") else {
        return;
    };

    // 找到 path 和 value 的分界 (最后一个空格)
    let Some(sep) = rest.rfind(' ') else {
        let _ = write!(up_ch, "ERR parse: 需要 'set path value'\n");
        return;
    };

    let path  = &rest[..sep];
    let value = rest[sep + 1..].trim();

    if path.is_empty() || value.is_empty() {
        let _ = write!(up_ch, "ERR parse: 空的 path 或 value\n");
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