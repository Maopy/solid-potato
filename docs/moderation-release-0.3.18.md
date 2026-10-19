# 0.3.18 抖音消息编码兼容

包含 0.3.17 的页面对象接入、XHR 房间信息、抖音主站入口与同页切换修复。

真实直播中验证发现，部分 PushFrame 的 payload_encoding 标为 pb，而 payload_type 为 msg。gzip 数据原有逻辑已可读取，但未压缩 protobuf 帧会被错误标为 push_encoding_unsupported，导致后续执行暂停。本版补齐该组合；内容仍经过有界 protobuf 解析，其他未知编码继续拒绝。

升级：启动本版，从统一控制台更新抖音脚本，刷新直播页清除旧文档的解析暂停状态。场控默认关闭，录制独立。每条发送命令只点击一次，不等待回显、不重发。

完整继承上一版 config；抖音规则仍修改 config/douyin-auto.yml，视频号规则修改 config/wechat-auto.yml。只读验收时关闭自动发送并保持场控关闭。
