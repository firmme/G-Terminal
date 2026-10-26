# OneDrive 连接列表同步

本版本实现个人微软账号的浏览器登录和 OneDrive 应用文件夹同步，不依赖 OneDrive 客户端。发布者注册一次应用，普通用户只需要登录并授权。

官方 G-Terminal 应用已配置默认 Client ID：`bfc1025c-64f7-434f-bdf9-6a3cee653a55`。正常构建和运行无需设置环境变量；下列注册步骤供发布者维护应用或使用独立应用的分支参考。

## 发布者配置（一次）

1. 在 [Microsoft Entra 应用注册](https://entra.microsoft.com/) 注册 `G-Terminal`。
2. 支持的账号类型选择“仅个人 Microsoft 帐户”，或者“任何组织目录中的帐户和个人 Microsoft 帐户”。当前程序使用 `consumers` 端点，只接受个人账号。
   如果现有应用原来是“仅我的组织”，请在“清单 → Microsoft Graph 应用清单（新）”中同时设置 `signInAudience` 为 `PersonalMicrosoftAccount`，以及 `api.requestedAccessTokenVersion` 为 `2`。后者为 `null` 或 `1` 时，切换为个人账号会报 `Property api.requestedAccessTokenVersion is invalid`。只修改这两个字段，保留原有回调和权限。
3. 在“身份验证”中添加“移动和桌面应用程序”平台，回调 URI 填 `http://localhost`。不要选 Web / SPA 平台。程序使用随机本地端口；微软对 localhost 回调匹配忽略端口。
4. 在 Microsoft Graph 中添加**委托权限** `Files.ReadWrite.AppFolder`，并允许请求 `offline_access`。无需整盘读写权限。
5. 复制“应用程序（客户端）ID”。无需创建或分发客户端密码。
6. 如果需要覆盖内置的官方应用，在构建前设置环境变量：

```powershell
$env:GTERMINAL_ONEDRIVE_CLIENT_ID = '你的应用程序-ID'
./scripts/build.ps1 -Action build -Release
```

覆盖的 Client ID 会编译进程序。也支持运行时设置同名环境变量，便于开发调试。优先级为运行时变量、构建时变量、内置官方 ID。普通用户无需配置该变量。显式覆盖为无效 ID 时，设置页显示尚未配置，不会尝试登录。

参考：[应用注册](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app)、[桌面应用回调](https://learn.microsoft.com/en-us/entra/identity-platform/reply-url)、[PKCE 授权流程](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow)、[应用文件夹](https://learn.microsoft.com/en-us/graph/onedrive-sharepoint-appfolder)。

## 用户操作

偏好设置 → 连接列表同步 · OneDrive → 登录微软账号，在系统浏览器中确认授权，随后执行同步。首次连接、切换账号时，本机列表与该账号的云端列表合并。

云端路径为该应用的 `Apps/<应用注册名称>/connections-v1.json`。连接配置以 JSON 原文保存，包含服务器地址和用户名；本版本没有额外的客户端加密。

同步内容：保存的 SSH / 串口配置、跳板机、端口转发、分组及颜色。连接和分组的显示顺序以各设备为准，新下载项追加到末尾。

不上传密码、私钥文件、私钥路径、串口设备名、SSH config 只读导入项、窗口布局、终端历史。新设备上的私钥路径需要另行选择，串口端口默认为 `auto`。

登录 refresh token 保存到系统凭据库（Windows Credential Manager / macOS Keychain / Linux Secret Service），不写入 settings.json。系统凭据库不可用时停止登录或同步，不降级明文保存。Linux 构建需要 libdbus 开发库，运行时需要 Secret Service。

## 同步与冲突

- 修改连接后等待 2 秒同步；自动同步时每 2 分钟检查云端。断网、限流等失败后延长重试间隔，最长 15 分钟；离线时仍可编辑本机列表。
- 连接具有稳定 ID，重命名和移动分组不会产生新连接。旧列表在首次保存时迁移；复制连接生成新 ID。
- 以该账号上次同步的内容为基线，合并本机、云端的新增 / 修改 / 删除。基线按应用 ID 和 OneDrive drive ID 分开保存。
- 不同字段的修改自动合并；同字段修改、删除与修改冲突时暂停，设置页可选择“冲突项使用本机”或“冲突项使用云端”，其余字段仍自动合并。
- 写入使用条件请求，遇到版本冲突重新获取并合并，最多尝试 3 次。
- 云端同步文件被删除时暂停，用户可选择重新上传本机列表，或以云端空列表为准清空同步项。私钥和设备偏好不受影响。
- 同步期间产生的本机编辑会保留并在下一轮上传。同步结果不会更改已打开的 SSH 会话。
- 断开同步删除本机应用登录凭据，保留连接与同步基线，不删除 OneDrive 文件，也不撤销微软账号页面上的授权。

## 验收

### 登录失败诊断

程序区分“用授权码换取令牌”和“刷新登录令牌”，失败时显示微软返回的 OAuth 错误类型、具体说明和 AADSTS 编号；授权码、PKCE verifier、刷新令牌会被隐藏。浏览器显示 `Authorization received` 只表示本机收到了回调，不代表令牌接口已经成功。

开发者也可单独构建登录诊断程序，使用相同登录实现验证真实授权码交换及随后的刷新步骤，不上传或修改连接列表。成功时会将该账号的凭据保存到 G-Terminal 使用的系统凭据项。

```powershell
./scripts/build.ps1 -Action build -Example onedrive_login
./target/debug/examples/onedrive_login.exe
# 已登录时，只检查刷新凭据：
./target/debug/examples/onedrive_login.exe --refresh-only
```

第一次运行会输出一条临时授权地址，用系统浏览器打开并登录。请不要分享回调中的授权码，也不要将系统凭据导出用于诊断。

自动测试覆盖数据筛选、本机路径保留、旧数据迁移、同字段和跨字段冲突、离线删除、首次合并、条件写入竞争和 OAuth 回调校验。HTTP 模拟验证读取和条件写入请求。工具箱脚本执行测试验证普通用户及 root 环境均向连接用户写入公钥，重复执行不重复追加。

发布前必须用真实 Client ID 完成两台设备的浏览器授权、上传 / 下载、离线修改后重连、同时新增 / 编辑 / 删除、切换账号和撤销授权验证；尤其要确认微软服务对条件写入与应用文件夹权限的实际行为。未完成应用注册时不能宣称真实云端联调已通过。
