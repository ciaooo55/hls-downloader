# 队列拖动修复候选验收

构建源码 `a2a75fe8899e859e907517f99a7d520a3b0cda6f`；MSI SHA256 `179c0ea3684c19c90e2ca0085c114a4ec9379e723942fee8c62d0c08ed88d355`。正式 `scripts/build-v7.ps1 -Task candidate` 在干净工作区重建，EXE/MSI/Portable及双浏览器插件输出位于 `artifacts/v7-productization/candidate`。构建时 provenance 和功能矩阵在本目录原字节保存，不按验收结果改写。

源码修复：父列表框选在Main阶段尊重子控件已消费的拖动；使用平台拖动阈值；排序位移按实际缩放行高就近换位，取消时清空位移。23项纯选择/键盘测试与编译通过，见相邻 `candidate/queue-gesture-source-checks`。原包队列拖动失败、主题/搜索/列宽补验、全部性能失败样本仍见相邻 `candidate`，没有覆盖。

新沙盒定向验收通过：默认缩放上下各一行；JVM1.5倍缩放上下各两行；两种缩放框选均为2项；空白点击清空；列宽拖动和还原；实际安装关键组件10项、32MiB暂停重启续传成品SHA及卸载保留成品。新沙盒结果及范围见 `summary.json`、`candidate-queue-ui/candidate.json`、`native-finished.json`、截图和只读Core状态。原生输入只发生于禁网Windows Sandbox；宿主没有安装或运行产品。应用1.5倍缩放若被使用属于JVM模拟，不代表修改或认证系统DPI。

关键安装组件哈希按新便携包核对；`component-comparison.json`逐项记录旧新Core/Host/Presenter/Updater/媒体二进制及扩展ZIP内部文件是否相同。旧包浏览器矩阵保留适用身份，不冒称对新MSI重跑过浏览器矩阵。

首轮构建被PATH中的pnpm11.25.0拦下；指定缓存11.7.0后又因未指定已有FFmpeg缓存拦下；补齐现有依赖路径后正式构建完成。所有失败日志保留，没有因此改门禁或重复下载媒体依赖。旧候选完整产物移至 `artifacts/v7-productization/candidate-history/ce9a290b`。

正式发布状态仍为 `release_ready=false`：完整原生矩阵、稳定性能门槛、原始HLS卡死/Privatecams直播403、收费云字幕及实体投屏等未据本批宣称通过。Privatecams目前只提供站点名，仍需具体有效直播页和触发步骤；未录制内容。未签名、未推送、未发布。任务沙盒已关闭，见 `candidate-queue-ui/cleanup.json`；任务构建守护进程已停止；项目内构建缓存和诊断证据保留。

构建日志原样保留WiX ICE39/ICE61/ICE69警告，未以修改日志或门禁消除。构建快照中的待验收措辞属于构建当时状态，后续验收由本报告及summary补充，不回写快照。
