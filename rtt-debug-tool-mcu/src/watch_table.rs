//! # 注册表与注册宏
//!
//! 全局静态注册表 `WATCH_TABLE` 存储所有观测条目的元信息和读写函数指针,
//! 运行时由 [`super::watch_task::debug_watch_task`] 遍历并遥测。
//!
//! ## 注册方式一览
//!
//! | 宏/函数 | 适用场景 | 权限控制 | 嵌套平铺 |
//! |---------|---------|---------|---------|
//! | [`watch_scalar!`] | 单个标量变量 | 调用时指定 | - |
//! | [`register_watch_fields`] | 自己的 struct | `#[watch(readonly)]` / `#[watch(skip)]` | 自动 |
//! | [`watch_struct!`] | 外部 struct 手动精选 | 每字段必写 `=>` | 手动 |
//! | [`watch_struct_all!`] | 外部 struct 全字段 | 省略 = 默认 ReadWrite | 手动 |
//!
//! ## 快速对比
//!
//! ```ignore
//! // 方式 1: 标量
//! watch_scalar!("voltage", &V_CELL, ReadWrite);
//!
//! // 方式 2: 自己的 struct — 一行全部展开
//! register_watch_fields("motor", &MOTOR_CELL);
//!
//! // 方式 3: 外部 struct — 手动逐字段
//! watch_struct!("ext", ExternalStruct, &EXT_CELL, {
//!     field_a: f32 => ReadOnly,
//!     field_b: i32 => ReadWrite,
//! });
//! ```

use core::cell::RefCell;
use critical_section::Mutex;
use heapless::{String, Vec};

use crate::watch_value::{WatchValueKind, Access};

// ═══════════════════════════════════════════════════════════
// 注册条目
// ═══════════════════════════════════════════════════════════

/// 单个观测条目的完整描述。
///
/// 包含宿主端需要的元信息 (`path`, `type_name`, `access`)
/// 以及运行时读取/写入所需的函数指针和 type-erased 指针。
///
/// # 字段说明
///s
/// - `path`: 完整路径, 如 `"arm.pitch.rpm"`
/// - `parent`: 父路径, 用于宿主机树形分组, 如 `"arm.pitch"`
/// - `ptr`: type-erased 指针, 实际指向 `&'static RefCell<具体类型>`
/// - `field_idx`: 嵌套调度索引, 非嵌套字段为 `0`
/// - `read_fn`: `(ptr, field_idx) → Option<值字符串>`
/// - `write_fn`: `(ptr, field_idx, 原始输入) → bool`
pub struct WatchEntry {
    /// 完整路径, 如 `"arm.pitch.rpm"`
    pub path:      String<64>,
    /// 父路径, 顶级为 `""` (空)
    pub parent:    String<64>,
    /// 类型名, 如 `"f32"`, `"bool"`
    pub type_name: &'static str,
    /// 值类型标签
    pub kind:      WatchValueKind,
    /// 读写权限
    pub access:    Access,

    /// type-erased 指针 → `&'static RefCell<实际类型>`
    pub ptr: *const (),

    /// 嵌套字段调度索引 (非嵌套字段为 `0`)
    pub field_idx: u16,

    /// 读函数: `(ptr, field_idx) → borrow() → 序列化字符串`
    pub read_fn:  fn(*const (), u16) -> Option<String<32>>,

    /// 写函数: `(ptr, field_idx, raw) → borrow_mut() → 写入`
    pub write_fn: fn(*const (), u16, raw: &str) -> bool,
}

unsafe impl Send for WatchEntry {}

fn build_path(parts: &[&str]) -> String<64> {
    let mut s = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 { let _ = s.push('.'); }
        let _ = s.push_str(p);
    }
    s
}

// ═══════════════════════════════════════════════════════════
// 全局注册表
// ═══════════════════════════════════════════════════════════

const MAX_ENTRIES: usize = 64;

/// 全局静态注册表。
///
/// 最多存储 [`MAX_ENTRIES`] 个 [`WatchEntry`], 通过临界区保护并发访问。
///
/// # 访问方式
///
/// - 注册: [`register_watch`] (初始化阶段)
/// - 读取: [`with_table`] (watch_task 遥测)
/// - 写入: [`apply_write`] (watch_task 下行命令)
pub struct WatchTable {
    entries: Vec<WatchEntry, MAX_ENTRIES>,
}

impl WatchTable {
    pub const fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// 追加观测条目。表满时返回 `false`
    pub fn register(&mut self, entry: WatchEntry) -> bool {
        self.entries.push(entry).is_ok()
    }

    /// 按索引获取条目
    pub fn get(&self, idx: usize) -> Option<&WatchEntry> {
        self.entries.get(idx)
    }

    /// 返回当前已注册条目数量
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 按 path 查找条目 (用于下行命令匹配)
    pub fn find_by_path(&self, path: &str) -> Option<&WatchEntry> {
        self.entries.iter().find(|e| e.path == path)
    }
}

/// 全局单例 — 初始化时注册, 运行时仅由 watch_task 读取
pub static WATCH_TABLE: Mutex<RefCell<WatchTable>> = Mutex::new(RefCell::new(WatchTable::new()));

/// 向全局注册表添加条目。
///
/// 由注册宏内部调用, 用户不应直接使用。
///
/// # 线程安全
///
/// 通过 `critical_section` 临界区保护, 可安全在中断/任务中使用。
pub fn register_watch(entry: WatchEntry) {
    critical_section::with(|cs| {
        WATCH_TABLE.borrow(cs).borrow_mut().register(entry);
    });
}

/// 在临界区内读取注册表。
///
/// 供 `debug_watch_task` 遥测时使用, 确保读取期间表不被修改。
pub fn with_table<R>(f: impl FnOnce(&WatchTable) -> R) -> R {
    critical_section::with(|cs| {
        let table = WATCH_TABLE.borrow(cs).borrow();
        f(&table)
    })
}

/// 查找 path 对应条目并执行写入。
///
/// 返回:
/// - `Ok(())` — 写入成功
/// - `Err("readonly")` — 条目为只读
/// - `Err("not found")` — path 不存在
/// - `Err("parse error")` — 值解析失败
///
/// # 示例
///
/// ```ignore
/// // 下行收到 "set arm.pitch.rpm 1000" → apply_write("arm.pitch.rpm", "1000")
/// match watch_table::apply_write("arm.pitch.rpm", "1000") {
///     Ok(()) => up_ch.write(b"OK arm.pitch.rpm=1000\n"),
///     Err(e) => up_ch.write(format!("ERR arm.pitch.rpm: {}\n", e).as_bytes()),
/// }
/// ```
pub fn apply_write(path: &str, value: &str) -> Result<(), &'static str> {
    critical_section::with(|cs| {
        let table = WATCH_TABLE.borrow(cs).borrow();
        let entry = table.find_by_path(path).ok_or("not found")?;
        if matches!(entry.access, Access::ReadOnly) {
            return Err("readonly");
        }
        if (entry.write_fn)(entry.ptr, entry.field_idx, value) {
            Ok(())
        } else {
            Err("parse error")
        }
    })
}

// ═══════════════════════════════════════════════════════════
// 路径工具
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

/// 从 `&str` 切片构建 `.` 分隔的路径 `String<64>`。
pub fn path_from_parts(parts: &[&str]) -> String<64> {
    build_path(parts)
}

/// 将 `&str` 转为 `String<64>`。
pub fn str_to_string64(s: &str) -> String<64> {
    let mut out = String::new();
    let _ = out.push_str(s);
    out
}

// ═══════════════════════════════════════════════════════════
// watch_scalar!
// ═══════════════════════════════════════════════════════════

/// 注册单个标量变量到观测表。
///
/// 这是最基础的注册方式, 适用于独立的 `f32`, `i32`, `bool` 等变量。
///
/// # 参数
///
/// - `path`: 观测路径 (如 `"battery"`, `"counter"`)
/// - `cell_ref`: `&'static RefCell<T>` 引用
/// - `access`: `ReadWrite` 或 `ReadOnly`
///
/// # 示例
///
/// ```ignore
/// use core::cell::RefCell;
/// use rtt_debug_tool_mcu::watch_value::Access::*;
///
/// static VOLTAGE: RefCell<f32> = RefCell::new(3.30);
///
/// watch_scalar!("battery", &VOLTAGE, ReadWrite);
/// watch_scalar!("counter", &COUNTER, ReadOnly);
/// ```
///
/// # 类型推导
///
/// 宏内部通过泛型函数自动推导 `T`, 无需手动指定类型:
///
/// - `&RefCell<f32>` → `T = f32`
/// - `&RefCell<bool>` → `T = bool`
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
                field_idx: 0,
                read_fn:   |ptr, _idx| {
                    let cell = unsafe { &*(ptr as *const ::core::cell::RefCell<T>) };
                    Some(T::watch_read(&cell.borrow()))
                },
                write_fn:  |ptr, _idx, raw| {
                    let cell = unsafe { &*(ptr as *const ::core::cell::RefCell<T>) };
                    if let Some(v) = T::watch_write(raw) {
                        *cell.borrow_mut() = v;
                        true
                    } else { false }
                },
            }
        }
        let entry = infer($cell_ref, $path, $crate::watch_value::Access::$access);
        $crate::watch_table::register_watch(entry);
    }};
}

// 辅助宏

#[macro_export]
#[doc(hidden)]
macro_rules! _access_or_default {
    () => { $crate::watch_value::Access::ReadWrite };
    ($a:ident) => { $crate::watch_value::Access::$a };
}

// ═══════════════════════════════════════════════════════════
// watch_struct! — 手动字段 + 显式权限
// ═══════════════════════════════════════════════════════════

/// 手动注册结构体字段, 每个字段必须显式指定权限。
///
/// 适用于**外部库的 struct** (你改不了源码加 `#[derive(Watch)]`), 或者
/// 你只想要观测部分字段的场景。
///
/// # 支持嵌套路径
///
/// 可以用 `outer.inner: type => access` 语法访问嵌套字段:
///
/// # 示例
///
/// ```ignore
/// // 外部 crate 的 ExternalMotor, 我们无法加 #[derive(Watch)]
/// watch_struct!("m1", ExternalMotor, &M1_CELL, {
///     rpm:     f32 => ReadOnly,      // 显式 ReadOnly
///     current: f32 => ReadWrite,     // 显式 ReadWrite
/// });
///
/// // 嵌套字段访问
/// watch_struct!("m1", DJI_Motor, &M1_CELL, {
///     base.rpm:    f32 => ReadOnly,
///     base.current: f32 => ReadWrite,
/// });
/// ```
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
                    field_idx: 0,
                    read_fn:   |ptr, _idx| {
                        let cell = unsafe { &*(ptr as *const ::core::cell::RefCell<$struct_ty>) };
                        Some(<$field_ty as WatchValue>::watch_read(&cell.borrow().$field $(.$sub)*))
                    },
                    write_fn:  |ptr, _idx, raw| {
                        let cell = unsafe { &*(ptr as *const ::core::cell::RefCell<$struct_ty>) };
                        if let Some(v) = <$field_ty as WatchValue>::watch_write(raw) {
                            cell.borrow_mut().$field $(.$sub)* = v;
                            true
                        } else { false }
                    },
                };
                $crate::watch_table::register_watch(_entry);
            }
        )+
    }};
}

// ═══════════════════════════════════════════════════════════
// watch_struct_all! — 批量注册
// ═══════════════════════════════════════════════════════════

/// 批量注册结构体字段, 省略 `=>` 时默认 `ReadWrite`, 只读字段覆写为 `=> ReadOnly`。
///
/// 同样适用于**外部库的 struct**, 无法加 `#[derive(Watch)]` 的场景。
///
/// # 与 `watch_struct!` 的区别
///
/// - `watch_struct!` → 每个字段**必须**写 `=> ReadWrite/ReadOnly`
/// - `watch_struct_all!` → 省略 `=>` 默认 ReadWrite, 要只读才加
///
/// # 示例
///
/// ```ignore
/// // 全部默认 ReadWrite
/// watch_struct_all!("sensor", Sensor, &S_CELL, {
///     temperature: f32,
///     humidity:    f32,
///     pressure:    f32,
/// });
///
/// // 部分覆写
/// watch_struct_all!("ext", ExternalMotor, &M_CELL, {
///     rpm:     f32,                 // 默认 ReadWrite
///     current: f32 => ReadOnly,     // 覆写 ReadOnly
///     temp:    u8  => ReadOnly,
/// });
/// ```
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
                let _path = $crate::watch_table::path_from_parts(
                    &[$parent, ::core::stringify!($field) $(, ::core::stringify!($sub))*]
                );
                let _parent = $crate::watch_table::str_to_string64($parent);
                let _entry = WatchEntry {
                    path:      _path,
                    parent:    _parent,
                    type_name: <$field_ty as WatchValue>::watch_type_name(),
                    kind:      <$field_ty as WatchValue>::watch_kind(),
                    access:    $crate::_access_or_default!($($field_access)?),
                    ptr:       $cell_ref as *const ::core::cell::RefCell<$struct_ty> as *const (),
                    field_idx: 0,
                    read_fn:   |ptr, _idx| {
                        let cell = unsafe { &*(ptr as *const ::core::cell::RefCell<$struct_ty>) };
                        Some(<$field_ty as WatchValue>::watch_read(&cell.borrow().$field $(.$sub)*))
                    },
                    write_fn:  |ptr, _idx, raw| {
                        let cell = unsafe { &*(ptr as *const ::core::cell::RefCell<$struct_ty>) };
                        if let Some(v) = <$field_ty as WatchValue>::watch_write(raw) {
                            cell.borrow_mut().$field $(.$sub)* = v;
                            true
                        } else { false }
                    },
                };
                $crate::watch_table::register_watch(_entry);
            }
        )+
    }};
}

// ═══════════════════════════════════════════════════════════
// WatchFields trait — #[derive(Watch)]
// ═══════════════════════════════════════════════════════════

/// 嵌套字段元信息。
///
/// 由 `#[derive(Watch)]` 为每个字段自动生成, 供外层结构体做嵌套平铺时使用。
///
/// # 字段
///
/// - `index`: 字段在 dispatch 中的索引
/// - `name`: 字段名 (如 `"rpm"`)
/// - `type_name`: 类型名 (如 `"f32"`)
/// - `kind`: 值类型标签
/// - `access`: 读写权限
pub struct WatchFieldMeta {
    pub index:     u16,
    pub name:      &'static str,
    pub type_name: &'static str,
    pub kind:      WatchValueKind,
    pub access:    Access,
}

/// `#[derive(Watch)]` 为结构体自动实现的 trait。
///
/// ## 自动生成的方法
///
/// - `walk_fields()` — 遍历所有字段, 生成 [`WatchEntry`] 并回调
/// - `field_meta()` — 返回字段元信息切片 (供嵌套平铺)
/// - `dispatch_read()` — 按 `field_idx` 从 `&self` 读取值
/// - `dispatch_write()` — 按 `field_idx` 写入 `&mut self`
///
/// 用户不应手动实现此 trait。
///
/// ## 嵌套平铺原理
///
/// 外层 struct 遍历内层 struct 的 `field_meta()`, 为每个字段生成
/// 新的 `WatchEntry`, 其 `read_fn` 调用内层的 `dispatch_read()`,
/// 而 `ptr` 始终指向最外层 `RefCell`。
pub trait WatchFields {
    /// 遍历所有字段并回调注册
    fn walk_fields(parent: &'static str, ptr: *const (), cb: &mut dyn FnMut(WatchEntry));

    /// 返回字段元信息 (供嵌套平铺使用)
    fn field_meta() -> &'static [WatchFieldMeta];

    /// 根据 `field_idx` 从引用读取字段值
    fn dispatch_read(field_idx: u16, this: &Self) -> Option<String<32>>;

    /// 根据 `field_idx` 写入字段值
    fn dispatch_write(field_idx: u16, this: &mut Self, raw: &str) -> bool;
}

/// 自动注册 `#[derive(Watch)]` 结构体的**全部字段** (含嵌套子结构体)。
///
/// 一行调用, 零字段名。权限由 `#[watch(readonly)]` 和 `#[watch(skip)]` 注解决定,
/// 嵌套 struct 字段**默认自动平铺**。
///
/// # 示例
///
/// ```ignore
/// use rtt_debug_tool_mcu::Watch;
/// use rtt_debug_tool_mcu::watch_table::register_watch_fields;
///
/// #[derive(Watch)]
/// struct Motor { rpm: f32, current: f32 }
///
/// #[derive(Watch)]
/// struct Arm {
///     voltage: f32,
///     pitch: Motor,           // ← 自动平铺
///     #[watch(readonly)]
///     joint: Joint,           // ← 自动平铺 + 全字段只读
/// }
///
/// let arm: &'static RefCell<Arm> = ...;
/// register_watch_fields("arm", arm);
/// // → arm.voltage
/// // → arm.pitch.rpm, arm.pitch.current
/// // → arm.joint.angle, arm.joint.speed
/// ```
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
