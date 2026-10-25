# 凭据重放覆盖 403 重试修复

## 根因与范围

403 后从 `Job.headers` 移除 `__cf_bm`，再次进入 `fetch_follow()` 时，`headers_for_request()` 会把 `replay_json` 中的旧 Cookie 加回来。curl 底层也会重复重放，覆盖已按当前跳转地址匹配的请求头。浏览器或手工请求身份经凭据库保存、下载前解密后会走此路径，仅测试普通 `Job.headers` 无法发现。

旧代码扩展既有回归后实际失败：普通 headers 来源先通过，下一种顶层 cookie 来源返回 `HTTP 403`，测试退出 101。后三种来源没有在旧代码执行到，不记为已复现；见 [red.log](results/red.log)。最初一次测试编译因工具链 PATH 不完整失败，随后显式设置 RUSTC/RUSTDOC/PATH 后构建成功；原始失败也保留在 results。

修复将一次 Cookie 修正放在实际逐跳请求头上，HTTP/WinHTTP 与 curl 均在收到 403 后才移除 `__cf_bm`，保留登录和亲和 Cookie。底层 curl 直接发送已准备好的请求头；403 的跨传输回退也通过逐跳重定向入口。不改写已保存凭据，不预先移除首次请求身份，不增加依赖、配置、常驻线程或重试队列。

## 已执行验证

宿主仅编译，所有产品测试在同一个 Windows 沙盒执行，未在宿主安装或运行。测试构建、release Engine 构建均使用已有离线依赖、locked Cargo 配置。

| 验证 | 实际结果 |
| --- | --- |
| 常规 Core 回归 | 634 通过，12 ignored |
| 显式 curl 回归 | 4 通过，含五种重放来源、首次 Cookie、截断正文、解压后范围限制、暂停/取消 |
| 显式 WinHTTP 回归 | 4 通过，含五种重放来源、范围/POST/HEAD、暂停/取消 |
| 持久化请求身份 | 9 场景通过：DPAPI 入库后重启、四种重放格式、同域路径切换、跨域有/无目标凭据、永久 403 及后续下载 |
| Range/HLS 故障 | 5 场景通过：三种坏 Content-Range、两种超长分片正文；失败任务不阻塞后续任务，成品字节一致，删除与完成缓存清理通过 |

共执行 642 个不同 Rust 回归，另 4 个 ignored 未执行。HTTP/WinHTTP/curl 的重放回归分别检查五种身份来源：普通 headers、顶层 cookie、request_headers、origin scoped、cookie jar。每次实际收到两次请求，首次含旧 Cookie、第二次不含旧 Cookie但保留 `__cflb` 和 session，原 replay 不变。既有 curl 正文测试改从逐跳入口调用，以覆盖真实调用契约。

持久化验收通过 Core IPC 保存实际 DPAPI 密文，退出并重启 Engine，再创建真实任务。七种有凭据场景成品与夹具字节一致，路径和跨域跳转使用目标身份；无目标上下文时不携带来源 Cookie/Authorization。永久 403 在两次实际请求后失败，之后的合法任务完成。任务删除和完成缓存均核查，数据库中的原密文未修改。仅使用回环测试服务与合成凭据，没有访问被拒绝的直播站点或上传音频。

Engine SHA-256：`e406c0485567f3ee1dc2ee25853176e6cf44aed31bc6b310e87eaafa54396c75`。

原始测试日志、构建日志、退出码及真实请求的脱敏结果见 results。最终 MSI 验收与桌面交付记录在完成后追加。

## 限制

本次不能证明指定直播网站的 403 已解决，也没有复现用户原始 HLS 卡死现场。新库初始化慢点已收窄到 SQLite 写入阶段，但先前 1645.09ms 以及历史首次启动超时仍未闭环。Chromium 任意下载完全不出现浏览器下载界面、完整 UI、真实 TLS 网站、收费云端、实体投屏设备及长期负载未在本批全量验收，不能声称所有功能无问题或已与 IDM/AB 全面对等。

本次只重建 Engine，UI/Host/Updater/Presenter/播放器/双插件沿用此前验收版本，不签名、不更新 GitHub Release。一个 main、一个 worktree；历史构建缓存清理曾被自动审批拒绝，本批不绕过，不能声称工作目录只剩源码。
