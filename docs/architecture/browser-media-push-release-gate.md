# 浏览器媒体推送的真实链路门禁

## 要证明什么

独立的浏览器、MSI 注册或 Core 推送测试不能证明整条链路可用。正式 `browser_media_push` 证据必须绑定同一 candidate 和源码提交，并证明：

1. candidate MSI 安装后，Edge/Firefox 的实际 Native Messaging 登记指向该包的 Host；核对 host 名称、浏览器 allowlist、manifest 和可执行文件哈希。
2. 两种真实浏览器通过生产扩展触发媒体推送，经 Native Host 和 Core 形成持久化请求。
3. Compose 的生产 DevicePicker 展示该来源，用户操作路径选择配置的真实接收端并确认；直接 Core `share_media` 不能替代此步骤。
4. 浏览器收到明确成功结果；指定的局域网接收端自身实际拉取确定性媒体 fixture。
5. 报告记录接收端身份、来源地址、字节/Range、fixture 哈希及时间，能将拉取归属于被选择的设备。

专用 MSI 生命周期位置为 `E:\h`；Host 通常是安装内 Engine 的同级文件。manifest 可按生产注册逻辑保存在可写用户目录，不能只凭文件存在或某个旧注册项判定成功。该专用测试约束不限制普通 MSI 自选安装目录。

## 证据身份与顺序

记录 source commit/tree、版本、candidate manifest digest、MSI digest、浏览器身份、两套注册及 Host 哈希、预期/发现/选择接收端、fixture 哈希、拉取记录、结果和时间。

正式流程按 browser、performance、browser_media_push、installer、rollback 执行。媒体推送门禁拥有安装与清理生命周期；不能假设别的卸载测试之后产品仍在。任何缺失、错误设备、不可达或拉取失败均为失败，不以 skip 放行。

## 预就绪工作流与当前合同

`v7-media-push-readiness.yml` 用于在 readiness 决策前收集窄范围证据，只接受当前 main、干净同 SHA 源码、交互式 Windows runner、真实接收端及 `release_ready=false`。它不修改元数据、不建 tag、不发布。

当前 canonical 的状态与排除项见 [验证状态](../validation/v7-verification.md)；若实际为 `release_ready=true`，现有预就绪工作流会拒绝执行，不能把旧任务顺序当作当前可直接运行的方案。

预就绪报告不能代替最终 SHA 的正式门禁。功能状态调整、readiness 决定和公开发布分别需要相应证据及授权；不能靠改 gap/status 消除实测要求。真实接收设备是外部条件，缺失时如实保留未覆盖范围。
