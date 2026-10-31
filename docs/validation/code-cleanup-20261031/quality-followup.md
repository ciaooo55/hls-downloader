# 代码质量收敛补充（2026-10-31）

从 `355310eb59ca63695224762d70d287c875989e72` 的干净工作区继续，针对生产客户端与验证工具的重复实现和错误依赖进行修改。

## 修改结果

| 区域 | 原问题 | 当前实现 |
| --- | --- | --- |
| 扩展清单探测 | HLS/DASH 各自维护请求头过滤、上下文回退、超时及大小估算；DASH 备用请求失败后还会进入 catch 再请求一次。 | 共用 `manifestInspection.ts`，限定一次回退，沿用同一个超时信号；替换被拒响应前取消旧响应体；共用有限数值的大小估算。解析器仍各自处理媒体格式。 |
| 桌面 Core IPC | 管道和 TCP 分别编码长度、循环读帧及检查上限。 | 共用 `CoreFrameChannel`，统一 4 MiB 限制、小端帧头、完整读取及截断诊断；去掉多余接口和重复长度检查。TCP 打开失败时关闭已创建的 socket。帧头整块读写，保留原有管道 I/O 粒度。 |
| Python 验证工具 | 多份 TCP 帧实现；普通下载验收从 MSI 测试夹具导入通用连接和状态查询。 | `v7_core_ipc.py` 统一连接、帧交换、端口、快照与任务等待；各调用方直接导入，只有使用 HTTP 夹具的脚本继续依赖夹具模块。 |
| 导入副作用 | 9 个脚本为取得 `free_port` 导入性能测试模块，其顶层创建 256 MiB PAYLOAD。 | 这些工具直接导入公共 IPC 模块，消除这条无关的内存分配路径。没有改变性能测试本身的负载，也没有把源码分析写成实测性能收益。 |
| 帧与连接边界 | 验证脚本没有统一的响应长度和对象检查，握手失败可能遗留连接。 | 读响应体前检查长度，拒绝非对象响应，失败握手关闭连接；Core error 对象仍交由场景检查，TVBox 保留立即报错的现有语义。 |
| MSI 门禁 | 生命周期和浏览器推送门禁重复查询 MSI 属性和已安装产品。 | 共用只读 `V7MsiInspection.ps1`，统一 COM 对象释放；调用方保留各自安装、卸载、证据和回滚流程。 |

没有增加第三方依赖、产品账号或收费前置条件。没有修改 Rust 下载引擎或将独立安全校验合并掉。

## 当前源码的验证

- 桌面：离线执行 `gradlew.bat test --offline --no-daemon --tests com.hlsdownloader.desktop.CoreFrameChannelTest --tests com.hlsdownloader.desktop.ProtocolTest`，编译成功，30 项测试通过（4 + 26），0 失败、0 跳过。覆盖中文帧、分次读取、连续帧、无效长度、截断、写入上限和关闭传输。结果见 [帧测试](CoreFrameChannelTest.xml) 与 [协议测试](ProtocolTest.xml)。最终帧头实现调整后已重新编译并执行该命令。
- 扩展：`tsc --noEmit --project extension/tsconfig.json` 通过；manifestInspection、hlsInspection、dashInspection、dashInspection.namespace、boundedResponse 共 5 个测试文件、32 项测试通过，包含现有估算溢出与格式边界用例。
- Python：`python -m unittest discover -s scripts/validation -p test_v7_core_ipc.py -v`，7 项通过；90 个脚本 AST 语法检查通过，278 处本地导入符号检查通过。确认没有脚本继续为工具函数导入性能测试模块。
- PowerShell：58 个脚本静态检查及正式 gate ID、证据与版本合同检查通过。共享 MSI 查询在 PowerShell 7 中通过真实候选包的产品名、版本、缺失属性及不存在产品测试；Windows PowerShell 5.1 中通过版本读取测试。查询未安装或修改 MSI。
- `git diff --check` 通过。

桌面编译中现有 UI 测试的弃用 API 和非空断言产生警告；这些测试没有在本轮执行，也没有为消除警告扩大修改范围。本轮只运行以上定向测试，没有启动产品 UI、浏览器自动化、下载集成场景或性能测试。

## 交付边界

本轮交付源码和定向验证。候选安装包、便携包没有重建，完整同包产品验收未因此通过，`release_ready` 继续为 false。没有推送、发布、签名或清除既有诊断与恢复数据。

完整项目仍按实际调用与职责判断是否复用；少量独立安全边界和测试场景的相似语句保留，不以文本零重复替代维护质量。
