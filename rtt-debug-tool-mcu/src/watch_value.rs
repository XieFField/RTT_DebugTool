// @file watch_value.rs
// @brief 定义观测值类型标签 + WatchValue trait + 所有基础类型的序列化/反序列化

use heapless::String;
use core::fmt::Write;

/// 值类型标签, 宿主机据此选择显示格式和编辑控件
#[derive(Clone, Copy, Debug)]
pub enum WatchValueKind {
    F32, F64,
    I8, I16, I32, I64,
    U8, U16, U32, U64,
    Bool,
    Str(u8),  // 定长字符串最大长度
}

/// 读写权限
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Access {
    ReadOnly,
    ReadWrite,
}

/// 可观测值的统一 trait: 类型元信息 + 序列化(读) + 反序列化(写)
/// 宏展开时通过 `<T as WatchValue>::watch_read(...)` 调用
pub trait WatchValue: Sized + 'static {
    fn watch_kind()      -> WatchValueKind;
    fn watch_type_name() -> &'static str;
    fn watch_read(val: &Self) -> String<32>;
    fn watch_write(raw: &str) -> Option<Self>;
}


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

impl WatchValue for bool {
    fn watch_kind() -> WatchValueKind { WatchValueKind::Bool }

    fn watch_type_name() -> &'static str { "bool" }

    fn watch_read(val: &Self) -> String<32> {
        let mut s = String::new();
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
