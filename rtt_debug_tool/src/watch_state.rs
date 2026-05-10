//! 协议解析与变量树。
//!
//! MCU 上行遥测为扁平行 `"arm.pitch.rpm=1000.5\n"`。
//! 本模块按 `.` 拆分 path 重建树形结构,
//! 叶子节点存储当前值, 分支节点为结构体/分组。

use std::collections::HashMap;

// ═══════════════════════════════════════════════════════════
// 基础类型
// ═══════════════════════════════════════════════════════════

/// 读写权限 (与 MCU 侧 `Access` 保持一致)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    ReadWrite,
}

impl std::fmt::Display for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Access::ReadOnly  => write!(f, "RO"),
            Access::ReadWrite => write!(f, "RW"),
        }
    }
}

/// 观测变量的完整信息 (树节点)
#[derive(Clone, Debug)]
pub struct VarInfo {
    /// 变量名 (不含父级前缀), 如 `"rpm"`
    pub name: String,
    /// 完整路径, 如 `"arm.pitch.rpm"`
    pub path: String,
    /// 当前值字符串
    pub value: String,
    /// 类型名, 如 `"f32"`, `"bool"`, `"MotorState"` (来自 MCU 的 type_name)
    pub type_name: String,
    /// 读写权限
    pub access: Access,
    /// 是否为结构体 (有子节点)
    pub is_struct: bool,
    /// 子节点列表 (仅结构体有)
    pub children: Vec<VarInfo>,
}

// ═══════════════════════════════════════════════════════════
// WatchState — 变量树 + 下行命令编码
// ═══════════════════════════════════════════════════════════

/// 全局观测状态, 由 RTT Reader 线程写入, UI 线程读取。
pub struct WatchState {
    /// 顶层条目列表 (按注册顺序)
    pub roots: Vec<VarInfo>,
    /// path → leaf 快速查找 (用于更新值)
    index: HashMap<String, usize>,
    /// 所有叶子节点的平坦列表 (按注册顺序, 用于下行查找)
    leaves: Vec<LeafRef>,
}

/// 叶子节点引用
struct LeafRef {
    path: String,
    access: Access,
}

impl WatchState {
    pub fn new() -> Self {
        Self {
            roots: Vec::new(),
            index: HashMap::new(),
            leaves: Vec::new(),
        }
    }

    /// 清空当前状态 (通常在重新连接时调用)
    pub fn clear(&mut self) {
        self.roots.clear();
        self.index.clear();
        self.leaves.clear();
    }

    /// 处理一行上游遥测数据。
    ///
    /// 格式:
    /// - `"arm.pitch.rpm=1000.5"` → 遥测, 更新值
    /// - `"OK arm.pitch.rpm=1100.0"` → 写成功确认 (值已被 MCU 更新, 下个遥测帧会反映)
    /// - `"ERR arm.pitch.rpm: readonly"` → 写失败, 忽略
    pub fn handle_line(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() { return; }

        // 跳过反馈行 (仅日志用, 不影响树)
        if line.starts_with("OK ") || line.starts_with("ERR ") {
            return;
        }

        // "path=value"
        if let Some((path, value)) = line.split_once('=') {
            self.upsert(path, value);
        }
    }

    /// 插入或更新一个 path 的值, 重建沿途树节点
    fn upsert(&mut self, path: &str, value: &str) {
        let parts: Vec<&str> = path.split('.').collect();
        if parts.is_empty() { return; }

        // 在 roots 中查找/创建顶层节点
        let root_name = parts[0].to_string();
        let root_idx = self.roots.iter().position(|r| r.name == root_name);

        let root = if let Some(idx) = root_idx {
            &mut self.roots[idx]
        } else {
            let root = VarInfo {
                name: root_name.clone(),
                path: root_name.clone(),
                value: String::new(),
                type_name: String::new(),
                access: Access::ReadWrite, // 默认, 后续会由叶子覆盖
                is_struct: parts.len() > 1,
                children: Vec::new(),
            };
            self.roots.push(root);
            self.roots.last_mut().unwrap()
        };

        if parts.len() == 1 {
            // 叶子节点 — 直接更新值
            root.value = value.to_string();
            root.is_struct = false;
            self.update_index(path, Access::ReadWrite);
        } else {
            // 多级路径 — 递归创建/更新子节点
            root.is_struct = true;
            Self::upsert_children(root, &parts[1..], value, path);
            self.update_index(path, Access::ReadWrite);
        }
    }

    fn upsert_children(parent: &mut VarInfo, parts: &[&str], value: &str, full_path: &str) {
        let name = parts[0].to_string();
        let child_idx = parent.children.iter().position(|c| c.name == name);

        let child = if let Some(idx) = child_idx {
            &mut parent.children[idx]
        } else {
            let current_path = format!("{}.{}", parent.path, name);
            let child = VarInfo {
                name: name.clone(),
                path: current_path,
                value: String::new(),
                type_name: String::new(),
                access: Access::ReadWrite,
                is_struct: parts.len() > 1,
                children: Vec::new(),
            };
            parent.children.push(child);
            parent.children.last_mut().unwrap()
        };

        if parts.len() == 1 {
            child.value = value.to_string();
            child.is_struct = false;
        } else {
            child.is_struct = true;
            Self::upsert_children(child, &parts[1..], value, full_path);
        }
    }

    /// 更新索引条目 (如果 path 不在索引中则新增)
    fn update_index(&mut self, path: &str, access: Access) {
        if let Some(&idx) = self.index.get(path) {
            // 已存在, 更新 access
            self.leaves[idx].access = access;
        } else {
            let idx = self.leaves.len();
            self.index.insert(path.to_string(), idx);
            self.leaves.push(LeafRef {
                path: path.to_string(),
                access,
            });
        }
    }

    /// 查找 path 对应的叶子节点, 返回其读写权限。
    pub fn get_access(&self, path: &str) -> Option<Access> {
        self.index.get(path).map(|&i| self.leaves[i].access)
    }

    /// 编码下行写命令。
    ///
    /// ```text
    /// set arm.pitch.rpm 1100.0\n
    /// ```
    pub fn encode_write_cmd(path: &str, value: &str) -> String {
        format!("set {} {}\n", path, value)
    }
}

// ═══════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_scalar() {
        let mut ws = WatchState::new();
        ws.handle_line("counter=42");
        assert_eq!(ws.roots.len(), 1);
        assert_eq!(ws.roots[0].name, "counter");
        assert_eq!(ws.roots[0].value, "42");
        assert!(!ws.roots[0].is_struct);
    }

    #[test]
    fn test_nested_struct() {
        let mut ws = WatchState::new();
        ws.handle_line("arm.pitch.rpm=1000.5");
        ws.handle_line("arm.pitch.current=2.3");
        ws.handle_line("arm.voltage=24.0");

        assert_eq!(ws.roots.len(), 1);
        let arm = &ws.roots[0];
        assert_eq!(arm.name, "arm");
        assert!(arm.is_struct);
        assert_eq!(arm.children.len(), 2); // pitch, voltage

        // pitch 是结构体
        let pitch = &arm.children[0];
        assert_eq!(pitch.name, "pitch");
        assert!(pitch.is_struct);
        assert_eq!(pitch.children.len(), 2); // rpm, current
        assert_eq!(pitch.children[0].value, "1000.5");
        assert_eq!(pitch.children[1].value, "2.3");

        // voltage 是叶子
        let voltage = &arm.children[1];
        assert_eq!(voltage.name, "voltage");
        assert!(!voltage.is_struct);
        assert_eq!(voltage.value, "24.0");
    }

    #[test]
    fn test_value_update() {
        let mut ws = WatchState::new();
        ws.handle_line("counter=1");
        assert_eq!(ws.roots[0].value, "1");
        ws.handle_line("counter=99");
        assert_eq!(ws.roots[0].value, "99");
    }

    #[test]
    fn test_multiple_roots() {
        let mut ws = WatchState::new();
        ws.handle_line("voltage=3.3");
        ws.handle_line("pid.kp=2.5");
        ws.handle_line("pid.ki=0.1");

        assert_eq!(ws.roots.len(), 2);
        assert_eq!(ws.roots[0].name, "voltage");
        assert_eq!(ws.roots[1].name, "pid");
        assert_eq!(ws.roots[1].children.len(), 2);
    }

    #[test]
    fn test_encode_write_cmd() {
        let cmd = WatchState::encode_write_cmd("arm.pitch.rpm", "1100.0");
        assert_eq!(cmd, "set arm.pitch.rpm 1100.0\n");
    }
}
