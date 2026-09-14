# G-Terminal

Windows 优先的原生 GPU 终端，使用 Rust、egui，Windows 默认使用原生 Direct3D 11 渲染，其他平台保留 wgpu，无 Electron、Chromium 或 WebView。

当前版本 **0.2.2**。交付 Windows x64 便携版，在本机 Windows / MSVC 环境验证。macOS、Linux 的平台入口保留，尚未实机验证或发布。推荐 Windows 11；ConPTY 要求 Windows 10 1809 或更新版本。

## 启动与构建

解压发布包，运行 `g-terminal.exe`。默认打开 Windows PowerShell，工作目录为用户主目录。PowerShell 7 和 WSL 需自行安装。

源码构建需要 Rust（本次验证为 1.98.1）、Visual Studio C++ Build Tools 和 Windows SDK：

```powershell
./scripts/build.ps1 -Action build -Release
./target/release/g-terminal.exe
./scripts/package.ps1 -SkipBuild
```

构建脚本会发现本机 MSVC 和 SDK。发布包包含第三方许可，旁附 SHA256 校验文件。使用 `Cargo.lock`；RSA 依赖的 pkcs8 暂固定在兼容的预发布版本。

## 界面与会话

- 紧凑单行顶栏：左上菜单、侧栏收起按钮、横向标签和关闭按钮。品牌信息、设置、帮助与会话操作归入菜单；终端上方不再重复显示标题和工具栏。
- 可调整宽度的连接导航栏、自定义连接分组；连接名称留空时默认显示主机 IP / 主机名。提示文字与标签使用不同颜色。
- 左右、上下任意嵌套分屏，每标签最多 32 个窗格；拖动分隔线调整比例。
- 默认恢复标签、分屏比例、焦点和会话类型。本地 Shell 重新启动；远程会话恢复为断开状态，通过“重新连接”认证后在原窗格连接。不会恢复旧进程、执行中的命令或终端输出。
- 查询当前窗格整个保留历史，支持跨自动换行匹配、跳转和可见匹配高亮。区分大小写，最多返回 10,000 个命中；不搜索其他窗格或上次运行的输出。
- 设置提供“选中自动复制”（默认关闭）。鼠标中键直接粘贴，右键打开终端菜单。应用启用鼠标上报时按住 Shift 可拖选。
- 保留底部状态栏、退出状态、错误信息、文件入口和转发状态。

## SSH、文件与传输

内置 `russh` / `russh-sftp` / Tokio；GUI SSH 不依赖本机 OpenSSH。支持密码和私钥（包括私钥口令）认证。首次连接显示主机指纹，确认后记录到 `~/.ssh/known_hosts`；已知主机密钥变化会拒绝连接。密码不写入配置。

连接编辑器提供单级 ProxyJump 和多条本地转发规则。转发仅监听 `127.0.0.1`，可查看状态并停止全部转发。当前不支持多跳链、远程转发、动态 SOCKS、自动读取 SSH config、SSH agent 或键盘交互 MFA。当前建立连接时同时初始化 SFTP，服务器必须启用 SFTP 子系统。

文件窗口展示本地和远程目录，支持进入目录、刷新、建远程目录、重命名、单文件上传和下载。传输队列每连接串行执行，可暂停、继续、失败重试。续传使用 `.gterminal.part` 临时文件，先逐字节校验已有前缀，再追加剩余内容；目标存在时拒绝覆盖。重启后临时文件保留，需要重新选择同一源和目标加入队列。下载提交使用硬链接，需要 NTFS 等支持硬链接的文件系统。不支持目录递归传输、持久化队列或自动同步。

“sudo 放置”将已上传的文件复制到指定特权路径，要求远端 POSIX Shell 和已配置免密码 sudo（`sudo -n`）；不修改 sudo 配置，不提供交互式 sudo 密码传输。

ZMODEM 使用独立 SSH 二进制通道，文件窗口中选择文件后显式发送 / 接收；远端需安装 `rz` / `sz`。支持单文件，大小小于 4 GiB；不自动接管交互终端中手动执行的 rz/sz，也不提供 ZMODEM 断点恢复。

远程 Agent **仅预留协议**：版本 1、4 字节大端长度前缀、JSON 请求/响应、1 MiB 帧限制，预留 Hello / SystemInfo / Cancel。没有部署、启动服务端或开放端口。

## 终端能力与边界

| 能力 | 当前支持范围 |
| --- | --- |
| VT / xterm | 光标、擦除、16/256/RGB 色、备用屏幕、应用光标键、光标位置查询、括号粘贴等；基于 vt100，尚非完整 xterm 实现 |
| 鼠标 | legacy、UTF-8、SGR 坐标编码，按钮、移动、拖动和滚轮；Shift 绕过上报进行选择 |
| 图像 | OSC 1337 inline PNG，限制解码尺寸和内存，只保留最近一张图像；不支持 Kitty / Sixel，也不将图片保留到滚动历史 |
| 文字 | 中文宽字符和字体回退；cosmic-text 高级 shaping、Swash 字形栅格化及彩色字体 Emoji，依赖系统字体；复杂双向文字、组合 Emoji 的终端单元格定位仍有兼容边界 |
| 输入法 | IME 预编辑、提交与候选窗位置接入；尚未完成各输入法、多屏 DPI 的实机矩阵验证 |
| 无障碍 | 启用 AccessKit，终端可见文本提供可访问标签；尚未完成 Narrator 验收、逐行输出播报和完整终端读屏导航 |

默认保留每会话 10,000 行输出。选区基于当前屏幕，输出变化或调整尺寸会清除；不支持跨屏拖选。只绘制可见行，shaping 纹理缓存限制 256 项 / 16 MiB。已增加可复现的 Windows 启动内存采样脚本 `scripts/measure-memory.ps1`。内存修复与测量条件见下文；仍无完整负载与延迟基准，不承诺跨机器固定内存占用。

## 快捷键

| 快捷键 | 操作 |
| --- | --- |
| Ctrl+Shift+T | 新建标签 |
| Ctrl+Shift+W | 关闭当前窗格，最后一个窗格关闭标签 |
| Ctrl+Tab | 下一个标签 |
| Ctrl+Shift+D / Ctrl+Shift+E | 左右 / 上下分屏 |
| Alt+Right | 切换窗格 |
| Ctrl+Shift+C | 复制选区 |
| Ctrl+Shift+V / Ctrl+V / 鼠标中键 | 粘贴 |
| Ctrl+C | 发送 Shell 中断 |
| Ctrl+Shift+F | 搜索当前窗格保留的全部历史 |
| Shift+PageUp / PageDown | 翻阅历史 |
| Ctrl+Plus / Minus / 0 | 放大 / 缩小 / 重置字号 |
| Ctrl+Comma | 设置 |
| Ctrl+Shift+B | 展开 / 收起侧栏 |

关闭窗格终止对应 Shell。粘贴发送给当前 Shell，含换行的文本可能执行；单次粘贴限制 1 MiB。

## 配置与架构

Windows 配置通常位于 `%APPDATA%\gterminal\G-Terminal\config\settings.json`，以设置页显示的实际路径为准。旧版配置自动补充新字段；设置历史上限对新会话生效。

`native_dx11.rs` 管理 Windows 窗口、D3D11、截图及 egui-winit 输入/AccessKit 桥接；`app.rs` 管理布局与持久化，`layout.rs` 管理分屏树；`view.rs` / `shaping.rs` 渲染终端；`terminal.rs` / `graphics.rs` 解析输出；`session.rs` 连接 ConPTY 或原生 SSH 通道；`remote.rs` / `remote_ui.rs` 实现 SSH、SFTP、转发及文件界面；`ztransfer.rs` 实现 ZMODEM；`agent.rs` 定义预留协议。

## 验证

```powershell
cargo fmt --check
./scripts/build.ps1 -Action clippy
./scripts/build.ps1 -Action test
```

测试覆盖真实 Windows PowerShell / CMD ConPTY、键盘路由、嵌套布局恢复、自动复制、滚动历史查找、鼠标编码，以及本机临时 SSH 服务器上的认证、主机密钥变化拒绝、ProxyJump、本地转发、SFTP 双向断点续传和覆盖拒绝。ZMODEM 使用真实协议完成二进制往返测试。测试不连接用户远程服务器；sudo、真实 OpenSSH / lrzsz、读屏及不同系统彩色字体仍需实际环境验收。

原生窗口截图冒烟检查（约四秒后截图并退出，不加载或写入已保存的工作区）：

```powershell
./target/release/g-terminal.exe --screenshot D:/path/to/window.png
```


## 0.2.1 启动内存修复

上一版使用 wgpu 默认的性能优先分配策略，且 epaint 将拥有所有权的系统字体数据完整复制到字体解析器。0.2.1 改为 `MemoryHints::MemoryUsage`、一帧排队目标，系统字体通过进程内 OnceLock 保存单份字节并以静态借用交给 epaint。没有裁掉中文、历史、分屏或 SSH 功能，也没有调用工作集清理 API 制造较低读数。

2026-09-13，本机 Windows、AMD Radeon(TM) Graphics、驱动 32.0.21043.12001，Release，1280×800 逻辑窗口 / 1920×1200 截图、一个默认 PowerShell、未连接远程。每版启动三次，采样启动后约 3.2 秒、截图读回之前的数据：

| 主进程指标（MiB） | 0.2.0 三次 | 0.2.1 三次 | 中位数变化 |
| --- | --- | --- | --- |
| 工作集 | 264.7 / 265.2 / 264.7 | 173.1 / 172.4 / 172.6 | 264.7 → 172.6，下降 34.8% |
| 私有提交 | 241.2 / 241.3 / 241.3 | 144.1 / 143.2 / 143.4 | 241.3 → 143.4，下降 40.6% |

工作集包含进程驻留的共享页，私有提交不是任务管理器的“专用工作集”；两者不可混用。这里均不包含 PowerShell / ConHost 子进程或独立统计的 GPU 显存，不代表任务管理器折叠进程组的总数。驱动、DPI、窗口面积和字体会影响结果。这是启动烟测，不能据此证明长时间运行无泄漏或所有场景低延迟。

复测：`./scripts/measure-memory.ps1 -Executable ./target/release/g-terminal.exe -Runs 3`。脚本输出四个启动阶段样本，使用最后一个作比较；截图模式不加载或写回工作区，结束后删除临时截图。原始本机样本保存在 `dist/memory-baseline.csv` 和 `dist/memory-optimized.csv`。

## 0.2.2 原生 Windows 渲染与字体按需加载

Windows 默认切换到 Direct3D 11，使用两个与窗口实际尺寸一致的翻转交换链缓冲，保留 GPU 渲染。窗口事件、键盘、剪贴板、IME、DPI 和 AccessKit 继续通过 egui-winit 接入。硬件设备创建失败时使用系统 WARP；非 Windows 平台仍采用 eframe/wgpu。渲染器采用 [egui-directx11 0.12.0](https://docs.rs/egui-directx11/0.12.0/egui_directx11/)，兼容现有 egui 0.33 界面。

Windows 字体改为只读文件映射，页面按需读取，不再把整个微软雅黑字体集合放进私有堆。文件句柄在使用期间禁止写入/删除共享，以保证映射内容稳定。保留完整字体与原有功能，不预删中文字形，也不清空工作集或周期性调用内存回收 API 修改任务管理器读数。

直接从构建目录启动，本机同样的 Release、1280×800 逻辑窗口 / 1920×1200 截图、一个默认 PowerShell、无 SSH 连接。3 次启动，取约 3.2 秒、截图读回之前的样本（单位 MiB）：

| 主进程指标 | 第 1 次 | 第 2 次 | 第 3 次 |
| --- | --- | --- | --- |
| 总工作集 | 50.2 | 50.2 | 50.2 |
| 专用工作集 | 22.7 | 22.7 | 22.7 |
| 私有提交 | 35.0 | 35.0 | 34.8 |

相比 0.2.1 的总工作集中位数 172.6 MiB，本机下降约 71%；相比 0.2.0 的 264.7 MiB，下降约 81%。新增专用工作集采样使用 Windows QueryWorkingSet 遍历驻留页面，仅统计非共享页，不将 PrivateMemorySize64 的私有提交误称为任务管理器专用工作集。数据不含 PowerShell/ConHost 子进程与独立统计的显存；不同显卡驱动、窗口大小、输出与字体使用会改变结果。Windows Terminal 的“10 MB”没有在本次任务中用相同条件复测，不据此作直接比较。

原始数据：`dist/memory-v022.csv`。采样脚本仍为 `scripts/measure-memory.ps1`，新增 `PrivateWorkingMiB` 列。Windows 原生缩放、最小化、恢复及截图检查使用 `scripts/smoke-window.ps1`；它只操作自行启动的测试窗口，不改变用户配置。已通过这些烟测、19 项功能测试、Clippy 和格式检查。真实输入法、多屏 DPI 和读屏器的完整验收范围未扩大。

如果特定驱动在新渲染器上出现问题，可在 PowerShell 中只为当前启动会话使用旧路径：

```powershell
$env:GTERMINAL_RENDERER = 'wgpu'
./g-terminal.exe
Remove-Item Env:GTERMINAL_RENDERER
```

默认渲染路径不受 WGPU_BACKEND 影响；该变量只在选择 wgpu 回退路径时适用。

### 交付便携包的复测

将同一 EXE 打包后，从 `dist/G-Terminal-0.2.2-windows-x64/g-terminal.exe` 再启动三次，测得总工作集 **63.0 / 63.5 / 63.8 MiB**、专用工作集 **28.1 / 28.6 / 28.9 MiB**、私有提交 **41.3 / 42.1 / 42.4 MiB**。EXE 哈希与构建目录相同；观测到的启动差异尚未归因，交付结果采用这组更保守的数据。相比上版总工作集中位数 172.6 MiB，便携包的 63.5 MiB 下降约 **63%**，落在本次要求的 40–80 MB 范围。原始数据为 `dist/memory-v022-packaged.csv`。
