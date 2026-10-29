# 基础圆角与弹窗滚动条修正复验

来源 `c95c61ce8cbe3b01a0cb02e346432f6234eeee96`；MSI SHA256 `be7bf1c650df10cfa897b01aae07c30adac76ec3ae2354de2d6ab28f28af5843`。安装器EXE内嵌MSI校验通过，便携包SHA256 `41bdbf29d1cc9f2c3dccb8da90620d37153e215a1a1d34cda228d76b84e17a70`。当前候选位于 `artifacts/v7-productization/candidate`，未签名、推送或发布。

此前 `4699166` 已修共享 Surface、按钮、输入框、复选框、开关、菜单及列表的背景/边框/裁切和缩放顺序，并按控件尺寸区分4/6/8/12dp圆角。首次实屏检查发现150%新建弹窗滚动条覆盖输入框右侧描边。本次只在可滚动的 WorkbenchDialog 内容区增加12dp右留白，保持独立滚动条占位。非滚动设置页布局不变，Core协议和持久数据格式未改。

## 实际验证

- 离线 `compileKotlin`通过；正式candidate构建通过，WiX既有警告仍见日志。之前共享控件静态守卫4项通过归档在nonrecording；本次单行留白未重复运行未受影响的测试。
- 在Windows Sandbox全新安装，核对10项关键组件SHA。只在沙盒操作产品窗口，没有宿主产品安装/运行。
- 原生实屏：JVM 150%浅/深主题新建弹窗的输入框右边框与滚动条已分开，圆角完整；浅色滚动到底可见开关，底部操作区保持可见。
- 原生键盘：Tab从链接到文件名、Shift+Tab返回链接，焦点边框完整；Escape关闭弹窗。
- 主题按钮可浅/深切换，深色设置在GUI重启后保持。默认JVM缩放的正常小窗口中，浅/深主界面及新建弹窗均位于工作区，四角及底部操作区可见。没有修改Windows系统DPI。
- 用Core协议准备的32MiB完成文件SHA正确；正常卸载返回0，卸载后同SHA保留。这不代表本轮完成了原生新建提交。
- 两个插件ZIP与a2a75fe、4699166相比，各17个内部文件字节全部一致；Core/Host等关键组件SHA亦一致。ZIP外层哈希不同不代表内部功能变化，也不把它说成当前MSI浏览器矩阵重新执行。

本轮 `passed=true` **只针对上述弹窗留白和基础轮廓回归**。全页面/所有状态、完整输入提交、导入导出和全部键盘矩阵尚未完成；之前文本注入失败、旧候选视觉失败和IDM安装准备失败均独立保留，竞品性能套件本次未重跑。不能据此声明全部非录制功能或整个UI已完全对标。

## 证据与收尾

[整轮结果](reports/acceptance.json)、[原生限定结果](reports/native-finished.json)、[截图索引](reports/native-input/actions.json)、[插件文件对照](reports/component-comparison.json)。修前证据在相邻 `candidate-visual-controls`，不会覆盖。构建provenance及artifact manifest按原字节复制。

沙盒Client16916（2026-10-29 22:47:25启动）在卸载和成品SHA核对后精确关闭，构建Gradle守护进程已停止。旧拒绝清理的缓存未重试。当前与历史候选及 `.tool-cache/nonrecording-20261029` 的原始材料保留，未将诊断证据当临时文件删除。
