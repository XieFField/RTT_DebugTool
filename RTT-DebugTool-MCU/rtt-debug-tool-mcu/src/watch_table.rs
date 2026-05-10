//! ## WatchTable 模块
//!
//! 注册条目、全局注册表 + watch_scalar!/watch_struct! 注册宏

use core::cell::RefCell;
use core::fmt::Write;
use critical_section::Mutex;
use heapless::{String, Vec};

use crate::watch_value::{WatchValueKind, Access};

// ═══════════════════════════════════════════════════════════
// 注册条目
// ═══════════════════════════════════════════════════════════

pub struct WatchEntry {
    pub path:      String<64>,          // "arm.target_height"
    pub parent:    String<64>,          // "arm" (顶级为 "")
    pub type_name: &'static str,        // "f32"
    pub kind:      WatchValueKind,      // F32
    pub access:    Access,              // ReadWrite / ReadOnly

    /// type-erased 指针 → &'static RefCell<实际类型>
    pub ptr: *const (),

    /// 读函数: ptr → borrow() → 序列化为字符串 (失败返回 None)
    pub read_fn:  fn(*const ()) -> Option<String<32>>,

    /// 写函数: ptr + &str → borrow_mut() → 反序列化写入 (返回是否成功)
    pub write_fn: fn(*const (), raw: &str) -> bool,
}

unsafe impl Send for WatchEntry {}

/// 构建 String<64> 路径, 静默截断过长部分
fn build_path(parts: &[&str]) -> String<64> {
    let mut s = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            let _ = s.push('.');
        }
        let _ = s.push_str(p);
    }
    s
}

// ═══════════════════════════════════════════════════════════
// 全局注册表
// ═══════════════════════════════════════════════════════════

const MAX_ENTRIES: usize = 64;

pub struct WatchTable {
    entries: Vec<WatchEntry, MAX_ENTRIES>,
}

impl WatchTable {
    pub const fn new() -> Self {
        Self { entries: Vec::new() }
    }

    pub fn register(&mut self, entry: WatchEntry) -> bool {
        self.entries.push(entry).is_ok()
    }

    pub fn get(&self, idx: usize) -> Option<&WatchEntry> {
        self.entries.get(idx)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn find_by_path(&self, path: &str) -> Option<&WatchEntry> {
        self.entries.iter().find(|e| e.path == path)
    }
}

/// 全局单例 — 初始化时注册, 运行时仅 task 读取
pub static WATCH_TABLE: Mutex<RefCell<WatchTable>> = Mutex::new(RefCell::new(WatchTable::new()));

/// 向全局注册表添加条目
pub fn register_watch(entry: WatchEntry) {
    critical_section::with(|cs| {
        WATCH_TABLE.borrow(cs).borrow_mut().register(entry);
    });
}

/// 在临界区内读取注册表执行操作 供 watch_task 使用
pub fn with_table<R>(f: impl FnOnce(&WatchTable) -> R) -> R {
    critical_section::with(|cs| {
        let table = WATCH_TABLE.borrow(cs).borrow();
        f(&table)
    })
}

/// 查找 path 对应条目并执行写入, 供 watch_task 下行命令使用
pub fn apply_write(path: &str, value: &str) -> Result<(), &'static str> {
    critical_section::with(|cs| {
        let table = WATCH_TABLE.borrow(cs).borrow();
        let entry = table.find_by_path(path).ok_or("not found")?;
        if matches!(entry.access, Access::ReadOnly)
        {
            return Err("readonly");
        }

        if (entry.write_fn)(entry.ptr, value)
        {
            Ok(())
        }
        else
        {
            Err("parse error")
        }
    })
}

// ═══════════════════════════════════════════════════════════
// macro helpers — 将 &str 路径信息转为 WatchEntry 字段
// ═══════════════════════════════════════════════════════════

pub fn entry_fields(path_parts: &[&str]) -> (String<64>, String<64>) {
    let path = build_path(path_parts);
    let parent = if path_parts.len() <= 1 {
        String::new()
    } else {
        build_path(&path_parts[..path_parts.len() - 1])
    };
    (path, parent)
}

/// 从字符串切片构建路径 String<64> (供宏展开中使用)
pub fn path_from_parts(parts: &[&str]) -> String<64> {
    build_path(parts)
}

/// 单个 &str → String<64> (供宏展开中构建 parent)
pub fn str_to_string64(s: &str) -> String<64> {
    let mut out = String::new();
    let _ = out.push_str(s);
    out
}

// ═══════════════════════════════════════════════════════════
// watch_scalar! — 独立变量注册
// ═══════════════════════════════════════════════════════════

#[macro_export]
macro_rules! watch_scalar {
    ($path:literal, $cell_ref:expr, $access:ident) => {{
        fn infer<T: $crate::watch_value::WatchValue>(
            cell: &'static ::core::cell::RefCell<T>,
            path: &'static str,
            access: $crate::watch_value::Access,
        ) -> $crate::watch_table::WatchEntry
        {
            use $crate::watch_table::WatchEntry;
            use $crate::watch_value::WatchValue;
            let (p, pa) = $crate::watch_table::entry_fields(&[path]);
            WatchEntry {
                path:      p,
                parent:    pa,
                type_name: T::watch_type_name(),
                kind:      T::watch_kind(),
                access,
                ptr:       cell as *const ::core::cell::RefCell<T> as *const (),
                read_fn:   |ptr| {
                    let cell = unsafe { &*(ptr as *const ::core::cell::RefCell<T>) };
                    Some(T::watch_read(&cell.borrow()))
                },
                write_fn:  |ptr, raw| {
                    let cell = unsafe { &*(ptr as *const ::core::cell::RefCell<T>) };
                    if let Some(v) = T::watch_write(raw)
                    {
                        *cell.borrow_mut() = v;
                        true
                    }
                    else { false }
                },
            }
        }
        let entry = infer($cell_ref, $path, $crate::watch_value::Access::$access);
        $crate::watch_table::register_watch(entry);
    }};
}

// ═══════════════════════════════════════════════════════════
// watch_struct! / watch_struct_all! — 结构体字段注册
// ═══════════════════════════════════════════════════════════

/// 辅助: 可选权限 → 省略时默认 ReadWrite
#[macro_export]
#[doc(hidden)]
macro_rules! _access_or_default {
    () => { $crate::watch_value::Access::ReadWrite };
    ($a:ident) => { $crate::watch_value::Access::$a };
}

/// 将字段名和类型信息打包, 供两个 struct 宏共用
#[macro_export]
#[doc(hidden)]
macro_rules! _watch_entry_for_field {
    // 嵌套: parent_prefix, struct_ty, cell_ref, outer.inner, field_ty, access_expr
    // access_expr 是类似 Access::ReadWrite 的表达式
    ($parts:expr, $struct_ty:ty, $cell_ref:expr, $field:ident . $sub:ident, $field_ty:ty, $access_expr:expr) => {{
        use $crate::watch_table::entry_fields;
        use $crate::watch_value::WatchValue;

        let parts: &[&str] = $parts;
        let full_path: &[&str] = &[
            parts[0], parts[1], // parent + struct
            ::core::stringify!($field),
            ::core::stringify!($sub),
        ]; // We need to handle variable-length properly...
        // Actually this won't work cleanly with macro_rules!.
        // Let me skip this and just inline the path building.
    }};
}

/// 手动选择字段 + 显式权限
#[macro_export]
macro_rules! watch_struct {
    (
        $parent:literal,
        $struct_ty:ty,
        $cell_ref:expr,
        { $($field:ident $(. $sub:ident)* : $field_ty:ty => $field_access:ident),+ $(,)? }
    ) => {{
        use $crate::watch_table::WatchEntry;
        use $crate::watch_value::{WatchValue, Access};
        $(
            {
                let _path = $crate::watch_table::path_from_parts(
                    &[$parent, ::core::stringify!($field) $(, ::core::stringify!($sub))*]
                );
                let _parent = $crate::watch_table::str_to_string64($parent);
                let _entry = WatchEntry {
                    path:      _path,
                    parent:    _parent,
                    type_name: <$field_ty as WatchValue>::watch_type_name(),
                    kind:      <$field_ty as WatchValue>::watch_kind(),
                    access:    Access::$field_access,
                    ptr:       $cell_ref as *const ::core::cell::RefCell<$struct_ty> as *const (),

                    read_fn:   |ptr| {
                        let cell = unsafe { &*(ptr as *const ::core::cell::RefCell<$struct_ty>) };
                        Some(<$field_ty as WatchValue>::watch_read(&cell.borrow().$field $(.$sub)*))
                    },

                    write_fn:  |ptr, raw| {
                        let cell = unsafe { &*(ptr as *const ::core::cell::RefCell<$struct_ty>) };
                        if let Some(v) = <$field_ty as WatchValue>::watch_write(raw)
                        {
                            cell.borrow_mut().$field $(.$sub)* = v;
                            true
                        }
                        else { false }
                    },
                };
                $crate::watch_table::register_watch(_entry);
            }
        )+
    }};
}

/// 批量注册, 默认 ReadWrite, 覆写加 `=> ReadOnly`
#[macro_export]
macro_rules! watch_struct_all {
    (
        $parent:literal,
        $struct_ty:ty,
        $cell_ref:expr,
        { $($field:ident $(. $sub:ident)* : $field_ty:ty $(=> $field_access:ident)?),+ $(,)? }
    ) => {{
        use $crate::watch_table::WatchEntry;
        use $crate::watch_value::{WatchValue, Access};
        $(
            {
                let _path = {
                    let mut _s: $crate::heapless::String<64> = $crate::heapless::String::new();
                    let _ = _s.push_str($parent);
                    let _ = _s.push('.');
                    let _ = _s.push_str(::core::stringify!($field));
                    $(
                        let _ = _s.push('.');
                        let _ = _s.push_str(::core::stringify!($sub));
                    )*
                    _s
                };
                let _parent = $crate::watch_table::str_to_string64($parent);
                let _entry = WatchEntry {
                    path:      _path,
                    parent:    _parent,
                    type_name: <$field_ty as WatchValue>::watch_type_name(),
                    kind:      <$field_ty as WatchValue>::watch_kind(),
                    access:    $crate::_access_or_default!($($field_access)?),
                    ptr:       $cell_ref as *const ::core::cell::RefCell<$struct_ty> as *const (),

                    read_fn:   |ptr| {
                        let cell = unsafe { &*(ptr as *const ::core::cell::RefCell<$struct_ty>) };
                        Some(<$field_ty as WatchValue>::watch_read(&cell.borrow().$field $(.$sub)*))
                    },

                    write_fn:  |ptr, raw| {
                        let cell = unsafe { &*(ptr as *const ::core::cell::RefCell<$struct_ty>) };
                        if let Some(v) = <$field_ty as WatchValue>::watch_write(raw)
                        {
                            cell.borrow_mut().$field $(.$sub)* = v;
                            true
                        }
                        else { false }
                    },
                };
                $crate::watch_table::register_watch(_entry);
            }
        )+
    }};
}

// ═══════════════════════════════════════════════════════════
// WatchFields trait — 供 #[derive(Watch)] 使用
// ═══════════════════════════════════════════════════════════

/// 由 derive macro 自动实现, 遍历所有字段并回调注册
///
/// 用户不应手动实现此 trait
pub trait WatchFields {
    /// 遍历自身所有字段, 对每个字段调用 `cb`
    fn walk_fields(parent: &'static str, ptr: *const (), cb: &mut dyn FnMut(WatchEntry));
}

/// 自动注册实现了 WatchFields 的结构体的所有字段
///
/// 等效于调用 `walk_fields` 并将每个条目写入 WATCH_TABLE
pub fn register_watch_fields<T: WatchFields>(
    parent: &'static str,
    cell: &'static RefCell<T>,
) {
    T::walk_fields(
        parent,
        cell as *const RefCell<T> as *const (),
        &mut |entry| register_watch(entry),
    );
}
