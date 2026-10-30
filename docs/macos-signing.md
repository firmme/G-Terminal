# macOS 自签名与钥匙串

打包必须在 Info.plist、图标、文档及许可全部写入后，对完整 `.app` 签名。
仅保留 Rust 链接器生成的 ad-hoc 签名不能视为已签名的应用包。

首次在发布机器执行：

```sh
python3 scripts/sign-macos.py --init
python3 scripts/verify-macos-signing.py
./scripts/package-macos.sh
```

初始化创建固定的自签名代码签名证书，保存在
`~/Library/Application Support/G-Terminal/signing/`。
它使用独立钥匙串，不读取登录钥匙串的密码或 OneDrive 令牌。
只在当前用户的信任设置中将该证书用于代码签名，不增加 TLS 信任。
系统可能要求用户确认信任设置。

必须私密备份整个签名目录（包含私钥钥匙串及其解锁密码），不得提交到 Git、
上传到 Release 或分发给用户。换机器构建时恢复同一目录；不要为每版生成新证书。
`GTERMINAL_SIGNING_DIR` 可指定恢复后的目录。
初始化发现已有目录时只复用，配置不完整时停止，不会自动更换证书。

打包脚本会自动复用该身份，缺少配置或签名验证失败时停止。
编译前会检查证书指纹与私密文件权限，签名前核对应用标识，避免签错应用或误用证书。
签名要求同时绑定固定应用标识 `dev.gterminal.G-Terminal` 和固定证书指纹，
不绑定每次构建都会变化的程序哈希。签名完成后才生成归档和 SHA-256。
`GTERMINAL_DIST_DIR` 可指定输出目录，便于保留正在运行的旧包。

`verify-macos-signing.py` 使用两个不同的测试程序验证相同签名要求，
并检查篡改资源会被拒绝。它在独立临时钥匙串中创建虚构凭据，禁用授权 UI，
验证同一进程重复读取以及换构建后读取均能成功。不读取真实凭据，
也不代替现有用户从 ad-hoc 版本迁移的实际授权测试。

旧 ad-hoc 版本迁移过来可能需要再次允许访问 OneDrive 钥匙串项。
完成迁移后仍需在本机验证：同一版本反复同步、重启以及升级不同构建后，
是否无需再次授权。拒绝或系统钥匙串锁定仍可能导致提示。
自签名不是 Apple Developer ID 或公证，不消除 Gatekeeper 的下载启动检查。
