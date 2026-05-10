#![no_std]
//!
//! # RTT Debug Tool — MCU 侧库
//!
//! 基于 RTT (Real-Time Transfer) 的嵌入式调试工具, 无需暂停 CPU 即可实时观测和修改
//! 目标板上的变量, 配合宿主机端 `RTT_DebugTool` 使用可实现类似 Keil Watch 窗口的体验。
//!
//! ## 快速开始
//!
//! ```ignore
//! use rtt_debug_tool_mcu::Watch;
//! use rtt_debug_tool_mcu::watch_table::register_watch_fields;
//! use rtt_debug_tool_mcu::watch_task::{debug_watch_task, WatchConfig};
//! use rtt_debug_tool_mcu::{watch_scalar, watch_config};
//!
//! // 1. 定义观测结构体
//! #[derive(Watch)]
//! struct Motor { rpm: f32, current: f32 }
//!
//! // 2. 注册
//! register_watch_fields("motor", &MOTOR_CELL);
//!
//! // 3. 启动后台任务
//! spawner.must_spawn(debug_watch_task(up_ch, down_ch, watch_config!()));
//! ```
//!
//! ## 模块结构
//!
//! - [`watch_value`] — 值类型定义与序列化
//! - [`watch_table`] — 注册表、注册条目、注册宏
//! - [`watch_task`] — 后台遥测任务与协议
//!
//! ## 三种注册方式
//!
//! | 方式 | 适用场景 | 代码量 |
//! |------|---------|--------|
//! | [`watch_scalar!`] | 单个标量变量 | 1 行 |
//! | [`register_watch_fields`] | 自己的 struct, 全字段自动展开 | 1 行 |
//! | [`watch_struct_all!`] | 外部库的 struct, 手动列出字段 | 每个字段 1 行 |

pub mod watch_value;
pub mod watch_table;
pub mod watch_task;

pub use rtt_debug_tool_mcu_derive::Watch;
