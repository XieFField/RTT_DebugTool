fn main() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("RTT_DebugTool.ico"); // 绑定你的图标文件
    res.compile().unwrap();
}