# 场控 M0：只读注入探针

2026-10-12 开始实现。当前完成的是 M0 的基础设施与抖音浏览器适配骨架；真实浏览器事件覆盖、稳定用户身份和网络消息解码仍待验证。详细设计中的 run、status、replay、TOML 配置以及动作执行接口尚未实现。

## 已实现

- Rust 独立场控模块、回环 HTTP 桥接与 SQLite 样本日志，不修改录制数据库。
- mitmproxy 同源脚本装载：限定一个抖音直播间，在 HTML 的 head 起始处插入可读 JS。保持页面 CSP；不通过移除策略解决加载失败。
- 页面使用 CSP nonce / `strict-dynamic` 时，从真实 script 标签提取匹配当前策略的 nonce，赋给探针标签。仅同源并不足以满足 `strict-dynamic`；不修改策略响应头，也不复用其他页面的 nonce。
- 单次配对票据、文档连接与房间绑定、鉴权、心跳过期、页面离开释放钩子。`actor_verified=false`，当前绑定仅表示页面/房间范围，不代表已识别登录账号或直播场次。
- 序列号批量上传、事务提交后 ACK、重复上传幂等与断序拒绝。单批最多 50 条，页面缓存最多 500 条；溢出标记 `probe.gap`。每次运行最多 50,000 条，SQLite 主库页数上限 8192，日志满时停止采集。上限不等同于整个目录（WAL、证书等）的硬磁盘配额。
- 抖音 DOM 新评论的尽力观察；fetch/XHR/WebSocket 仅记录通道、格式、长度，不读取响应内容、二进制帧正文或 Cookie。网络钩子保持原请求返回与 socket 行为。
- 事件类型使用字符串，可接受未来类型；未知类型保留事件信封、剔除任意 payload 和身份字段。已知 payload 按白名单清洗。
- 没有输入框操作、点击发送、平台 socket 发送或任意脚本执行入口；能力报告固定 `send_chat=false`。

## 使用

需要 Rust 与可执行的 `mitmdump`。在项目根目录：

```sh
cargo run -p live-archive-core -- moderation probe \
  --room https://live.douyin.com/139244611014 \
  --directory data/moderation-probe \
  --proxy-port 2024 \
  --duration-seconds 60
```

`--mitmdump /path/to/mitmdump` 可指定程序；`--duration-seconds 0` 等待 Ctrl-C，非零时最长 3600 秒。运行目录保存样本和 mitmproxy CA；目录或代理端口被占用时拒绝启动。命令不读取录制账号配置，也不启动录制。

在用于验证的浏览器配置代理 `127.0.0.1:2024`，并为该浏览器配置输出路径中的 mitmproxy CA，然后重新加载目标房间。命令不会修改系统代理或安装信任证书。其他站点 HTTPS 使用隧道透传；场控请求只允许目标房间、固定同源路径和固定回环目标。不要与当前录制代理争用端口；共享代理租约还未实现。

浏览器控制台可查看：

```js
window.__liveArchiveModerationV1?.getStatus()
```

如果 Firefox 显示“Potential Security Risk”且高级错误码为 `SEC_ERROR_UNKNOWN_ISSUER` 或 `MOZILLA_PKIX_ERROR_MITM_DETECTED`，先检查当前测试配置的 CA 信任：打开 `about:preferences#privacy` → 证书 → 查看证书 → **证书颁发机构 / Authorities**，导入当前运行目录的 `mitmproxy-ca-cert.pem`，勾选“信任此 CA 用于识别网站”。已有该证书时选择它并编辑信任。不要将 CA 导入“您的证书 / Your Certificates”，也不要导入含私钥的 `mitmproxy-ca.pem`。

不同运行目录会生成不同 CA；原录制代理或默认 `~/.mitmproxy` 的 CA 不一定能验证本探针。导入后重新启动测试浏览器再打开房间。仍失败时记录高级错误码与证书签发者；不使用跳过校验代替配置正确的信任。相关说明：[Firefox 错误码](https://support.mozilla.org/en-US/kb/error-codes-secure-websites)、[mitmproxy CA 文件与信任](https://docs.mitmproxy.org/stable/concepts/certificates/)。

正常配对应显示 `connected: true`、`mode: "observe"`、`send_chat: false`。`gap: true` 表示采集曾有丢失；HTTP 鉴权/作用域失败会停止探针，需重新加载页面。初次配对响应丢失也需重新加载，不会静默重绑旧缓存。

`undefined` 且没有 `bootstrap.js` 请求时，首先确认浏览器地址与 `--room` 相同。若 HTML 已有探针标签但没有网络请求，需要检查 CSP nonce。旧版本未给探针标签赋 nonce，会被抖音的 `strict-dynamic` 阻止；更新后重新启动探针并刷新房间。手动从开发者工具装载成功不能代替自动装载验收。

已打开的标签页可能仍使用旧 HTML。若重启后标签仍缺 nonce，在地址栏重新导航到 `https://live.douyin.com/<房间号>?live_archive_probe=nonce-fix-1`，下一次使用不同的参数值；该参数只用于取得新的页面请求，房间鉴权忽略 query。此方法不改变 CSP 或证书信任。

命令停止时输出当前运行事件计数和能力报告。没有事件时计数为零，不能据此认定房间已验证。正常停止或启动失败清理临时 addon 和运行锁，保留 SQLite 日志及 CA。代理进程提前退出会结束探针并报告失败。强制杀死主进程可能留下运行锁，应先确认旧探针已经退出再清理。

## 当前边界

DOM 类名可能随抖音改版变化；启动时已有评论作为基线忽略，只观察后续节点。当前评论没有稳定用户 ID，不能用于按用户合并礼物。网络钩子仅能观察安装之后创建的连接，worker 或提前建立的连接可能遗漏。页面 CSP、Referer 策略、缓存/Service Worker 和脚本加载顺序也需要浏览器实测。

SQLite 样本含观察到的昵称和评论，应按个人直播数据管理。网络 payload、Cookie、认证请求头和完整 URL 不写入事件日志；不会保存网络原始帧。CA 私钥保留在运行目录，临时 addon 含本地桥接凭据，Unix 下创建为仅所有者读写并在停止时删除。

评论观察兼容当前 `.hts-live-text-user` 昵称节点，并跟踪虚拟列表行内容/索引变化及文本更新；DOM 观察仍可能遗漏或把系统提示作为评论观测，不能当作网络事件的完整覆盖证据。

尚未接入视频号浏览器或桌面客户端，也没有登录账号校验、直播场次校验、礼物/粉丝团/财富等级解码、规则引擎、发送队列或发送确认。当前 CLI 只支持 `moderation probe --room ...`；设计中的 `--config` 不是现有选项。

下一步先取得经清洗且保留稳定 ID 的事件样本，再加入规则引擎。已批准的礼物策略保持不变：各用户独立以最后一次新礼物为基准延后 5 秒，同窗口同礼物累加，礼物名称总计超过 30 字使用“感谢礼物飘屏”。连续送礼不设置强制提前发送窗口。

## 验证

```sh
cargo test --workspace
node --test tests/moderation_sdk.test.cjs
python3 tests/moderation_proxy_test.py
cargo fmt --all -- --check
```

Rust 测试覆盖房间限定、单次配对、多页面隔离、幂等 ACK、断序、可扩展事件与 HTTP 边界。JS 测试覆盖钩子透明性、重复装载、导航释放、缓存上限；Python 测试覆盖同源路由、只转发允许字段、注入范围和失败时不转发本地鉴权信息。另以真实 mitmdump 验证 CLI 启停与文件清理。

本次结果：工作区 58 项 Rust 测试通过，1 项真实 FFmpeg 测试默认忽略；5 项 JS 和 6 项 Python 测试通过，格式检查通过。新增测试覆盖 CSP `strict-dynamic` 使用已有 nonce、拒绝伪造 nonce 属性、当前昵称节点和虚拟行更新。实际 mitmdump 的 HTTPS 同源脚本获取、配对、合成事件入库、幂等 ACK、空命令轮询和关闭清理均通过。合成事件使用 `probe.status / synthetic_integration_test` 标记，不算真实直播事件。

最初对用户提供的抖音房间直接使用 curl 请求返回 HTTP 200、正文为空，不能由此推断浏览器验证结果。随后在用户 Firefox 的房间 `517107254662` 实测，发现并修复了 CSP nonce 缺失和旧昵称选择器/虚拟行观察问题。通过新的页面 URL 绕过旧 HTML 后，自动加载的探针显示 `connected=true`、`send_chat=false`、`gap=false`、`bridge_error=null`；采集窗口已入库至少 16 条真实 `chat.received` DOM 观测，以及网络元数据。此结果验证浏览器装载、配对和部分评论采集，仍不代表礼物、等级或完整网络事件覆盖。
