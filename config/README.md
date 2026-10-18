# 受版本管理的产品合同

| 文件 | 用途 |
| --- | --- |
| [release/feature-parity.json](release/feature-parity.json) | 产品版本、功能状态、发布就绪合同及排除项，供构建和发布脚本读取 |
| [release/update-signer-trust.json](release/update-signer-trust.json) | 编译进更新程序的签名者轮换信任配置，不接受网络元数据扩充 |

这些是源码配置，不能作为构建缓存删除。用户运行设置、任务库、Cookie、API Key 和个人备份不放在这里。安装包和当次验收输出仍生成到被 Git 忽略的 `artifacts`。
