## RTT-DEBUG-TOOL-MCU

本库是RTT-DebugTool的MCU侧配套协议库代码。
请配合RTT-DebugTool使用
`https://github.com/XieFField/RTT_DebugTool`

本库使用的默认依赖芯片是`stm32h723zg`，串口模式依赖于`embassy-stm32`，目前仅支持stm32芯片
如果你想要更换其他芯片,你需要在你的Cargo.toml中这样写
```rust
[dependencies]
rtt-debug-tool-mcu = { version = "0.1.0", features = ["your stm32 mcu"] }
```