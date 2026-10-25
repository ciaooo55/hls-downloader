# 活动尾段拆分实验：未进入交付

## 结论

本轮尝试让空闲连接帮助其他连接的大尾段。候选文件正确性通过，但不能据现有小样本证明稳定改善；已撤回本轮生产代码和新增的候选单元测试，未替换桌面 MSI、插件或 GitHub Release。当前生产实现仍与 `82b0f9f` 一致。本目录保存实验补丁及全部原始结果，不将撤回候选的测试计入当前生产版本。

IDM 的[动态分段说明](https://support.internetdownloadmanager.com/support/segmentation.html)和 AB 的[固定版本 HttpDownloadJob](https://github.com/amir1376/ab-download-manager/blob/08bfd440a30d367ae369739d866d30f8c1d4b3af/downloader/core/src/commonMain/kotlin/ir/amirab/downloader/downloaditem/http/HttpDownloadJob.kt)提供了行为对照。当前实现优先分配尚未请求的范围，活动慢尾段仍可能让其他连接闲置；这项差异没有因实验撤回而消失，不能标记为已经与竞品等效。

## 候选及正确性

候选在 pending 队列为空时二分最大活动尾段，两侧至少 1MiB；保留既有自适应连接数限制。锁外最多一个 256KiB 写批次，通过最小范围边界避免与接手尾段重叠。没有新依赖、常驻进程或配置。原响应提前关闭仍可能产生在途额外数据，不能宣称网络传输完全无重叠。

旧 Engine SHA-256 为 `8d9200cb821c899f604cf8b420eb742e6650df3a840f55dbcab9f37ab3e77f9e`，候选为 `80df0bca7b92288830739c1065a6eb98bcab1915eeb2a38f971de2abea96ade9`，候选增加 60,416 字节。实验补丁见 [results/experimental-candidate.patch](results/experimental-candidate.patch)。

新增回归在旧逻辑下确实失败，在候选下通过。候选默认 Core 635 passed、0 failed、10 ignored；另执行 curl 3 项及 WinHTTP 3 项，共 641 个不同回归，仍有 4 项环境测试没有执行。原始测试结果与日志均在 results，不表示全功能或当前撤回后的产品新增了 641 项认证。

## 沙盒实测及异常

夹具为 64MiB 随机非周期内容、强 ETag、4 连接、16 个初始范围。正常连接每请求 16MiB/s，慢尾连接每请求 1MiB/s；仅模拟单条连接变慢，不模拟资源总限速。旧 A/B 是完全相同的二进制，三轮轮换顺序。所有 27 个完整下载样本及 2 个帮助期间暂停/恢复样本的成品 SHA、完成缓存清理、删除成品均通过。

| 三轮耗时中位数 | 同一旧版 A | 同一旧版 B | 候选 |
| --- | ---: | ---: | ---: |
| 正常连接 / raw HTTP | 1.885s | 6.183s | 1.677s |
| 慢尾连接 / raw HTTP | 6.092s | 5.334s | 3.508s |
| 慢尾连接 / WinHTTP 代理 | 5.341s | 5.340s | 2.487s |

不能只用中位数判断：候选第三轮 raw HTTP 慢尾样本为 **14.367s**，没有发出帮助请求；同轮旧版 A/B 为 6.092/5.334s。旧程序自身也出现 12.264s、9.799s 等慢样本，说明这批环境存在明显波动，但不能由此认定候选异常完全由环境导致。保留所有样本，见 [tail-comparison.json](results/tail-comparison.json)。

候选正常连接场景服务器成功写入额外约 704–768KiB（约 1.1%）；发生帮助的慢尾场景额外约 64KiB。该指标是服务器 socket 成功写入的 payload，不能当作精确链路流量。CPU 采样粒度为 0.015625s，RSS 约 14MiB/16MiB；未证明 CPU、内存全面下降。

两组帮助期间暂停后，durable checkpoint 保留；raw HTTP 恢复后服务器只写 10,977,281 字节，WinHTTP 只写 11,862,017 字节，未重下整包。最终随机内容 SHA-256 均为 `7357f049fecbdeda42c24168a8caa5948314f88b73fa1ccc27b671d87e4b1cf2`。

## 针对性诊断与决定

额外使用产品已有 `HLS_HTTP_IO_PROFILE=1`，按旧/新/新/旧做四次 raw HTTP 慢尾定向实测。旧版耗时 5.360/5.328s，候选 3.539/3.294s；四次最大 checkpoint 耗时分别 101.512/168.735/89.692/93.238ms，均未达到既有自适应降连接阈值 250ms。见 [tail-io-diagnostic.json](results/tail-io-diagnostic.json)，含各进程原始日志。

这证明补测中没有触发该磁盘阈值，但没有捕获 14.367s 异常当时的 I/O，因此不能把“慢盘触发降连接”写成已确认根因。补测不是对原异常的解释，也不能抹去原样本。按用户要求仅交付有证据的优化，选择撤回候选，保留现有已验证包；没有为了绕过异常增加降级层、配置或更大并发。

## 交付与边界

桌面现有 MSI SHA-256 保持 `e6c49715dfd122c1858f8b568c69ec45e4273c9ba6c58ce3f0b4e60f696493cf`，插件及源码 ZIP 未改。没有生成新的候选 MSI、安装到宿主、调用云端或重试指定直播录制。

指定直播 403、原始 HLS 卡死现场、历史首次安装启动超时、Chromium MV3/POST/Blob、实体设备、云端及长期负载边界仍未全部闭环，不能宣称全部功能无问题或全面等同 IDM/AB。历史构建缓存删除曾被自动审批拒绝，本轮未换方法重试；项目 `.tool-cache` 仍有构建与实验暂存，不声称工作目录只剩源码。没有创建分支或 worktree。

本轮收尾核对：宿主产品进程、三个常用 Windows 卸载注册表位置的本产品记录、沙盒进程均为零；桌面清单的七项成品全部哈希相符，见 [handoff-state.json](results/handoff-state.json)。这不是全注册表或整机零残留证明。报告整理命令也被自动审批以 `blocked by policy` 拒绝，未提供具体原因；保留实验补丁文件，没有重试删除。
