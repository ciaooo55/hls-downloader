# 连接等待与 WinHTTP 停止响应验收

## 根因与改动

继续追查下载的停止路径，确认两个缺口：连接名额满时只等待其他任务释放、不检查本任务控制；Windows HTTPS/代理使用同步 WinHTTP，在响应头和正文读取时要等待网络调用返回，才能处理暂停/取消。另有媒体实体读取将控制中断统一映射成普通失败，丢失了暂停/取消语义。

连接预算现在在等待期间检查控制，检查时不持全局预算锁。WinHTTP 改为系统异步通知，由原下载线程等待完成或检查控制；取消时关闭当前请求，等待最终关闭通知后才释放回调引用和读写缓冲区。完成通知立即唤醒，不为正常请求固定增加轮询延迟，不新增依赖、常驻线程或服务，不降低网络超时、不重发请求来轮询。媒体普通/范围实体读取保留控制结果。

实现遵守微软的[WinHTTP 并发约束](https://learn.microsoft.com/en-us/windows/win32/winhttp/concurrency-in-winhttp)、[异步读取缓冲区与回调约束](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpreaddata)、[发送缓冲区生命周期](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpsendrequest)及[关闭通知生命周期](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpclosehandle)。没有跨线程强关同步请求。

## 已执行验证

Core release 测试程序及 Engine 构建成功，Windows 沙盒最终执行 114 项、全部通过：HTTP 61、真实 curl 2、显式 WinHTTP 3、连接预算/限速策略 13、HLS 26、重试/刷新/凭据 3、缓存边界 6。HTTP 默认组的 5 个 ignored 用例已分别显式运行，未把跳过算作通过。源码、测试程序、Engine 哈希与各组参数/日志见 `results/`。

WinHTTP 使用本地实际 HTTP 代理进入产品传输路径，覆盖响应头、正文和 chunked 停顿中的暂停/取消，媒体实体及范围实体的控制语义，180000 字节普通/分段下载逐字节核对，以及实际 POST 请求正文与 HEAD 无正文。停止结果必须在测试服务端释放连接前返回。连接预算回归在保持满额名额期间发出停止，确认无名额泄漏，之后仍能重新获取。

首轮 WinHTTP 停顿回归中，普通/分段、POST/HEAD 与媒体控制通过，但测试误要求 WinHTTP 在读缓冲区未填满时已经写出 3 字节，因此一项失败。修正为先确认响应头已完成、下载已进入正文阶段，再检查停止；仍要求在服务端释放前返回。最终重新编译和执行全部上述分组。首轮失败日志保留在 `results/first-attempt/`。

沙盒显示连接曾报告 0x80072746，客体共享目录命令及后端回归仍正常执行。本轮没有修改 UI/插件，也没有将日志验证当作新的实屏或浏览器接管验收。

## 交付与边界

最终 MSI 的安装验收及桌面交付记录在完成后写入本目录。此前指定直播站点 403 和原 HLS 卡死现场根因仍未闭环；本批只证明上述停止响应缺陷已修复，不能证明所有站点、所有浏览器下载和整体性能全面达到 IDM/AB。没有在宿主安装产品、调用云端或重试此前被拒绝的站点录制。
