//! # 值类型与序列化
//!
//! 定义 Watch 系统支持的基础类型标签、读写权限, 以及统一的值类型 trait。
//!
//! ## 支持的类型
//!
//! | 类型 | 标签 |
//! |------|------|
//! | `f32`, `f64` | `F32`, `F64` |
//! | `i8` ~ `i64` | `I8` ~ `I64` |
//! | `u8` ~ `u64` | `U8` ~ `U64` |
//! | `bool` | `Bool` |
//! | 定长字符串 | `Str(N)` |
//!
//! ## 扩展新类型
//!
//! 为自定义类型实现 [`WatchValue`] trait 即可:
//!
//! ```ignore
//! impl WatchValue for MyType {
//!     fn watch_kind() -> WatchValueKind { WatchValueKind::Str(64) }
//!     fn watch_type_name() -> &'static str { "MyType" }
//!     fn watch_read(val: &Self) -> String<32> { ... }
//!     fn watch_write(raw: &str) -> Option<Self> { ... }
//! }
//! ```

use heapless::String;
use core::fmt::Write;

/// 值类型标签。
///
/// 宿主机根据此标签选择显示格式和编辑控件。
///
/// # 示例
///
/// ```ignore
/// // F32 类型显示为浮点输入框
/// let kind = WatchValueKind::F32;
///
/// // Str(32) 类型显示为文本输入框, 最大 32 字节
/// let kind = WatchValueKind::Str(32);
/// ```
#[derive(Clone, Copy, Debug)]
pub enum WatchValueKind {
    /// 32 位浮点
    F32,
    /// 64 位浮点
    F64,
    /// 有符号 8 位整数
    I8,
    /// 有符号 16 位整数
    I16,
    /// 有符号 32 位整数
    I32,
    /// 有符号 64 位整数
    I64,
    /// 无符号 8 位整数
    U8,
    /// 无符号 16 位整数
    U16,
    /// 无符号 32 位整数
    U32,
    /// 无符号 64 位整数
    U64,
    /// 布尔值
    Bool,
    /// 定长字符串, 参数为最大长度 (字节)
    Str(u8),
}

/// 读写权限。
///
/// # 示例
///
/// ```ignore
/// // 只读变量 — 宿主机端编辑框置灰
/// watch_scalar!("counter", &COUNTER, ReadOnly);
///
/// // 可读写变量 — 宿主机端可以修改
/// watch_scalar!("voltage", &VOLTAGE, ReadWrite);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Access {
    /// 只读, 宿主机不可写入
    ReadOnly,
    /// 可读写, 宿主机可以修改值
    ReadWrite,
}

/// 可观测值的统一 trait。
///
/// 所有支持 Watch 的基础类型均已实现此 trait。
/// 宏展开时通过 `<T as WatchValue>::watch_read(...)` 调用。
///
/// # 已实现类型
///
/// - 数值: `f32`, `f64`, `i8`..`i64`, `u8`..`u64`
/// - 布尔: `bool`
///
/// # 手动实现
///
/// ```ignore
/// use rtt_debug_tool_mcu::watch_value::{WatchValue, WatchValueKind};
/// use heapless::String;
///
/// struct Percentage(u8);
///
/// impl WatchValue for Percentage {
///     fn watch_kind() -> WatchValueKind { WatchValueKind::U8 }
///     fn watch_type_name() -> &'static str { "%" }
///
///     fn watch_read(val: &Self) -> String<32> {
///         let mut s = String::new();
///         core::write!(s, "{}%", val.0).ok();
///         s
///     }
///
///     fn watch_write(raw: &str) -> Option<Self> {
///         let v: u8 = raw.trim_end_matches('%').parse().ok()?;
///         Some(Percentage(v.min(100)))
///     }
/// }
/// ```
pub trait WatchValue: Sized + 'static {
    /// 返回此类型的标签 (供宿主机选择控件)
    fn watch_kind() -> WatchValueKind;
    /// 返回此类型的简短名称 (如 `"f32"`, `"bool"`)
    fn watch_type_name() -> &'static str;
    /// 将值序列化为字符串 (读方向)
    fn watch_read(val: &Self) -> String<32>;
    /// 从字符串反序列化 (写方向), 失败返回 `None`
    fn watch_write(raw: &str) -> Option<Self>;
}

// ── 内部: 批量实现数值类型 ──

macro_rules! impl_watch_num {
    ($ty:ty, $kind:ident, $name:literal) => {
        impl WatchValue for $ty {
            fn watch_kind() -> WatchValueKind { WatchValueKind::$kind }
            fn watch_type_name() -> &'static str { $name }
            fn watch_read(val: &Self) -> String<32> {
                let mut s = String::new();
                let _ = core::write!(s, "{}", val);
                s
            }
            fn watch_write(raw: &str) -> Option<Self> { raw.parse().ok() }
        }
    };
}

impl_watch_num!(f32, F32, "f32");
impl_watch_num!(f64, F64, "f64");
impl_watch_num!(i8,  I8,  "i8");
impl_watch_num!(i16, I16, "i16");
impl_watch_num!(i32, I32, "i32");
impl_watch_num!(i64, I64, "i64");
impl_watch_num!(u8,  U8,  "u8");
impl_watch_num!(u16, U16, "u16");
impl_watch_num!(u32, U32, "u32");
impl_watch_num!(u64, U64, "u64");

// ── 布尔值独立实现 (非数值格式化) ──

impl WatchValue for bool {
    fn watch_kind() -> WatchValueKind { WatchValueKind::Bool }
    fn watch_type_name() -> &'static str { "bool" }

    fn watch_read(val: &Self) -> String<32> {
        let mut s = String::new();
        // bool 序列化为 "1" / "0" (便于解析)
        s.push_str(if *val { "1" } else { "0" }).ok();
        s
    }

    fn watch_write(raw: &str) -> Option<Self> {
        match raw {
            "1" => Some(true),
            "0" => Some(false),
            _   => None,
        }
    }
}
