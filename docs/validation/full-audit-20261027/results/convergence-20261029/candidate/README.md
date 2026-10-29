# 同 SHA 候选包验收

候选源码提交 `ce9a290b117f0638d15c55b498ba257f47f64e1c`，tree `19fbf7488598eba523d3bc75b904cde80a5a657d`。候选 MSI SHA256 为 `15410198df9d67d3998a0a19087807e6ca375689399c4e9a534f8ae26cbd161b`。包位于项目 `artifacts/v7-productization/candidate/`，未签名、未推送、未发布，`release_ready=false`。

`BUILD-PROVENANCE.json` 和 `FEATURE-PARITY.json` 是构建时快照，不随验收结果改写。后续证据提交不改变该候选的源码身份。构建使用正式 `build-v7.ps1 -Task candidate`，完成时工作区干净。Windows PowerShell 5.1 首次解析失败通过给发布门禁脚本添加 UTF-8 BOM 修复；随后正式构建退出 0。WiX ICE39/ICE69 警告保留。

## 已完成批次

| 目录 | 实际结果 |
| --- | --- |
| candidate-reports | r22 基线 12 连接：14.77 MiB/s，未达 20 MiB/s；成品 SHA、完成后零额外流量和内存门槛通过。该批在基线失败处停止。 |
| candidate-lifecycle | 注入 InstallFinalize 前故障，返回 1603；旧产品、二进制、数据库及暂停任务保留。之后升级成功，但夹具错用 App Image 的 Skiko JAR 名称，文件核对失败；不记为产品文件丢失。 |
| candidate-installed | 改为同 MSI 行政提取的真实 JAR 清单后，运行中升级、全部期望组件哈希、暂停任务恢复成品 SHA、升级卸载、新装哈希通过。后续性能 Core IPC 未在 10 秒就绪，原因未确证；没有性能 JSON，未计性能通过。 |
| candidate-remaining | 同 MSI 新装及 Engine 自检通过。1 连接 34.19 MiB/s，通过；12 连接 12.19 MiB/s，未达 20 MiB/s。两者 256 MiB 成品 SHA 正确，完成后额外流量 0，内存增长分别 5.80 / 11.36 MiB。后续被沙盒 PowerShell Archive 模块加载失败阻断，未进入浏览器矩阵。 |

这些是单次本地沙盒样本，不能证明稳定性能收益、真实公网速度或优于 IDM/AB。12 连接结果不支持增加默认连接数，保留现有默认策略。首次 IPC 失败与后续新装成功分别记录，不能据此宣布首次原因已经定位。

原始失败报告和各次 runner 保留。超过 100 KiB 的 MSI 原始 `.log` 用无损 `.log.gz` 归档；解压后是原日志。宿主没有安装、运行产品或修改其注册表；安装、产品执行和本地协议验证均在禁网 Windows Sandbox 内。

## 安装组件的浏览器与原生验收

`candidate-independent/` 使用实际安装的 release Core、Host、Presenter 及从安装包扩展 ZIP 解压的插件。Firefox 159 与有窗口 Edge 156 各 8 项通过，首次 Host 离线各 2 项通过：一次性 GET、POST、Blob、拒绝保留源文件、丢回执、扩展重载、浏览器重启后原身份、单任务及成品 SHA 正确。普通响应入口 `zero_download_item=false`。Firefox 重启重装临时扩展，未验证商店签名升级。

`candidate-browser-ui/` 的 headless Edge 前 7 项通过，重启后 ping Promise 一直 pending，故障代理未收到重启后的新请求。追加 CDP ServiceWorker/异常事件观测，并改为有窗口 Edge 后完整矩阵通过；这是不同运行方式的独立结果，不能删除原失败或声称根因已确证。Firefox 仍使用原有 headless Marionette 路径。

原生验收准备了完成任务、3 个未来任务和接受/拒绝确认夹具。用户物理 Escape 停止 Computer Use，尚未完成任何本批原生按钮或快捷键操作。只读 UI API 状态不算操作通过。停止标记如实写 `passed=false`，供客体结束等待并卸载，不伪造原生完成。原生矩阵保持未完成，不自动恢复输入。

`candidate-independent/candidate.json` 的整体 `passed=false` 包含原生停止；性能未达门槛另见 `candidate-remaining/`，仍不满足整个计划和正式发布完成条件。原始特定媒体故障、收费云字幕和实体投屏等既有外部限制继续有效。

停止后客体正常卸载完成，产品登记及启动器移除，夹具成品保留；见最终 `candidate-independent/candidate.json` 的 `fresh-uninstall-preserves-task-output`。本任务沙盒随后关闭。项目内构建缓存和原始失败证据保留，未重试删除此前审批拒绝的缓存。

## 用户继续后：限定原生输入与性能诊断

`candidate-native-resume/` 在新的禁网沙盒中安装同 SHA 的 MSI。原生操作与截图、只读状态分别记录，45 个动作记录节点不等于 45 个独立用例：

- 接受确认后仅新增一个完成任务（32 MiB）；拒绝确认没有创建任务，原文件在最终卸载前仍存在。此次接受成品没有另取 SHA，不用任务状态代替哈希验证。
- 单选 1 项、Ctrl+A 5 项、Escape 清空、Shift+↓ 2 项、↓ 单选正确；Enter、双击打开对应任务，完成任务右键菜单目标正确。
- 详情、日志和完成连接图实际显示；完成连接图为 100% 和“已完成”。11 个设置页均实际导航，安全页仅查看；不代表各页所有开关、文字输入、搜索、拖动排序、云端或设备功能通过。
- 固定 audit 宽高和沙盒动态缩放时出现视口裁剪。去除 audit 宽高后同包正常窗口可以显示保存按钮；保留两种窗口元数据及截图，不据此确认产品布局根因。主题开关只改过草稿，未完成保存和恢复。

本批整体 `candidate.json` 仍为 `passed=false`：外层 Run 默认 180 秒，而新增 observer 的原生设置等待为 1200 秒，外层先超时并终止 observer。失败收尾卸载日志返回 0；本批未执行卸载后的输出保留断言，不能借用上一批通过结果改写本批。用户继续后的实操通过与上一批 Escape 停止分开保留。任务沙盒已关闭。

补验主题的准备与启动组合命令被自动审批以 `blocked by policy` 拒绝，没有具体原因；该命令未执行，未创建补验环境，也没有主题通过结果。

同批 12 连接 profile 样本为 49.28 MiB/s（256 MiB，5.19458 秒），SHA256 正确、完成后额外流量 0、内存增长 14.10 MiB。与此前 12.19 MiB/s 失败并列，不能用一次通过宣布稳定收益。日志中首个 checkpoint 为 1.628 秒，后续约 19–362 毫秒；这只提供 I/O 耗时线索，不能确证首要瓶颈。未降低持久化保障、修改默认策略或重建候选。整个计划及正式发布条件仍未满足。


## 后续补验：主题、搜索、列宽及队列缺陷

`candidate-completion-ui/` 在正常窗口完成真实输入：外观深色保存并重启保留、工作台切回浅色、搜索单项、原生键输入产生空结果、重置筛选、列宽拖动持久化及恢复默认列宽通过。`08-search-empty-state` 的文本替换未生效，不能作为空结果通过证据；后续 `z` 单键输入截图才确认空结果。两次未命中列宽拖柄也原样保留。

队列排序实际失败：拖动排序柄后出现两行选择、队列顺序未变。父列表在 Initial 阶段消费 Move，先于子排序控件处理；源码修复改为 Main 阶段尊重子控件消费，使用系统拖动阈值，排序按缩放后的行高就近换位。原包失败不改写为通过，新源码须以新候选单独验收。正常卸载返回0、成品SHA保留，沙盒已关闭。

`candidate-completion/` 最初使用自定义Core管道，但桌面仍连接默认管道，界面停在启动中；这是夹具错误，不是产品缺陷。原runner快照为 `candidate-completion-runners/guest-at-pipe-fixture-error.py`，后续修正runner为 `guest.py`；历史WSB映射目录当时内容不同，以对应runner快照为准。

同批固定顺序测试无重试：256MiB持久化顺序写基线75.17MiB/s，1连接53.72、12连接69.17、12连接13.88、1连接11.10MiB/s。最后两项未达20门槛；所有成品SHA正确、完成后额外流量0、内存门槛通过。慢样本checkpoint累计I/O耗时分别约22.78和21.42秒，单次最高约2.4和2.23秒；并行累计不等于墙钟时间。这支持调查持久化I/O，不证明增加连接数能解决，未降低持久化保障。

`queue-gesture-source-checks/` 保留离线缺依赖失败及补齐依赖后的编译、两个选择/键盘测试类通过；未在宿主执行GUI测试。Privatecams现场仅收到站点名称，具体直播页链接仍待提供，未录制内容或绕过此前自动审批限制。
