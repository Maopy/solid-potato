# 抖音在线规则预览：集成测试

本阶段连接现有注入链路与 Rust 规则核心，提供只读在线预览。默认 `probe` 仍只采样；加 `--rules-preview` 才开启二进制解码和规则。没有发言执行器或发送命令，`send_chat` 始终为 `false`。UI 继续暂停。

可选的 `--rules-preview --scheduler-preview` 已提供限流和排队预演，使用步骤与不同的 Console 日志见 [M2 调度集成测试](moderation-m2.md)。以下步骤继续使用即时规则预览。

再增加 `--fill-preview` 可 [只填入直播输入框](moderation-fill.md)，仍不会发送；本页命令不启用该模式。

## 你运行的测试步骤

你只需要在真实直播间做集成测试，不需要运行单元测试。

后续现场集成测试统一使用抖音直播间 [139244611014](https://live.douyin.com/139244611014)。用户已确认连续礼物感谢日志修复后的浏览器测试通过；下方历史验证记录保留当时的房间号。

1. 如果旧探针仍在运行，先在它的终端按 Ctrl-C。保留之前成功使用的 Firefox 代理和 CA 配置，使用同一个数据目录。
2. 在项目根目录启动：

   ```sh
   cargo run -p live-archive-core -- moderation probe \
     --room https://live.douyin.com/139244611014 \
     --directory data/moderation-probe \
     --proxy-port 2024 --duration-seconds 0 --rules-preview
   ```

3. 在已配置 `127.0.0.1:2024` 代理的 Firefox 打开下面的页面，房间必须与命令的 `--room` 一致。改变 query 值可避开此前测试中遇到的旧 HTML 缓存：

   ```text
   https://live.douyin.com/139244611014?live_archive_probe=online-test-1
   ```

4. 在开发者工具 Console 输入：

   ```js
   JSON.stringify(window.__liveArchiveModerationV1?.getStatus(), null, 2)
   ```

   应看到 `connected: true`、`send_chat: false`、非空的 `decoder` 和 `rules`。`decoder.frames` 随直播消息增长；`rules.mode` 是 `online_preview`。缺失 SDK 或证书错误的排查见 [M0 文档](moderation-m0.md)。

5. 等待直播间自然发生礼物事件，无需自行购买或发送礼物。Console 中出现 `[Live Archive preview] 感谢…` 表示控制端生成了拟发送文案。在没有缺口且礼物字段完整时，应在最后一次确认新增的礼物后约 5 秒出现合并感谢，另加最多约一次桥接轮询的延迟。不同用户独立计时。`rules.planned` 增长，`previews` 中可查看对应动作与 `gift_batch`。
6. 出现进场或粉丝团升级时，同样检查预览。缺少可靠时间戳、身份、接收者或原生语义的消息会记录抑制原因，不要求每条 UI 提示都产生动作。消费等级候选仅采样，不产生荣誉/财富升级祝贺。
7. 切换房间应使原 SDK 停止；重新配对需要匹配的新 `--room`。测试结束在终端按 Ctrl-C。停止探针不会自动改动 Firefox 的手动代理设置。

`previews` 是最近 10 条**决策记录**，不是待发送清单。在线观察模式生成预览后记录 `previewed` 并结束该批次，后续新增礼物重新独立等待 5 秒，避免再次感谢已展示的旧礼物。这个完成状态只表示预览已生成，不表示浏览器或直播平台已确认接收，更不表示发送成功。离线回放仍可测试等待发送、撤销和动作过期。

多件礼物只要具有稳定的连击组 ID 和有效累计 `repeat_count`，按该累计值归一化，不再因 `group_count > 1` 拒绝，也不把两个字段相乘。累计 10→20→20→结束 20 只计 20 件；新的连击组独立计数。

未支持的礼物会在 Console 显示 `[Live Archive skipped gift]`，状态中的 `rules.skipped_messages` 和 `rules.last_warning` 可查看数量与原因。它只跳过本条消息；若能确认用户和接收者，则取消该用户未完成的礼物窗口，避免对不完整批次生成文案。同一帧的其它消息、其它用户和后续有效礼物继续处理。所有文案都没有真正发送；原输入框保持正常，可供用户手动使用。

## 暂停及诊断

`rules.paused: true` 表示该页面的规则观察已停止，查看 `rules.last_reason` 和 `decoder.last_error`。暂停会取消等待的预览。刷新页面建立新作用域才能恢复；重连不合并旧页面的礼物窗口。

| 原因 | 行为 |
| --- | --- |
| `gift_combo_identity_missing` | 本条连击缺少稳定组 ID，跳过并警告，不暂停整页 |
| `gift_recipient_missing` | 本条礼物无法确认接收者，跳过并警告 |
| `gift_quantity_unsupported` / `gift_end_flag_unsupported` | 本条数量或结束标志无效，跳过并警告 |
| `protobuf_*` / `gzip_*` | 格式、大小或解压异常造成缺口，暂停 |
| `decoder_queue_full` / `page_buffer_full` | 缓冲容量耗尽，暂停 |
| `unverified_or_historical_input` | 某条输入缺少时间戳/网络证据，只抑制该输入 |
| `stale_observation` | 桥接延迟超过 30 秒，抑制该输入 |

进场 `action=1` 是当前支持的原生候选；其它成员/管理员变化不当作欢迎。消息时间支持秒和毫秒，超过 30 秒的历史消息和未来超过 5 秒的消息不进入规则。时间缺失只采样。粉丝团原生升级要求类型及其用户对象中的粉丝团等级、主播 ID 一致；初次粉丝团快照只建立基线，默认不从快照上升推断升级。

当前只解码已有直播 WebSocket，HTTP 只读取既有 room-enter 响应的房间/主播标识，DOM 评论继续独立采样。没有主动请求平台 API、签名算法、平台 ACK 写入或新的 WebSocket 客户端。若当前页面使用另一种消息来源或格式，其覆盖仍需后续适配。

## 存储与隔离

规范化候选仍写入独立 `moderation-probe.sqlite3` 的 `probe_events`，规则决策写入 `preview_audits`，保留来源与动作原因。不会存储原始 protobuf、gzip 帧、网络正文、Cookie、头像或签名参数。有限的房间、用户、礼物、等级及昵称数据是观察日志的一部分。

每个已认证页面有独立引擎，内部房间号、run/binding 会话、礼物接收者组成作用域。执行账号明确标为 `observe-only`，`actor_verified: false`。页面声明的解码数据还不能作为真实发言授权依据。本阶段没有跨页面执行器协调、直播场次结束后自动切换、崩溃恢复或在线重试发送。

数据库写失败会阻止后续预览轮询并要求重新启动，不继续展示未持久化的新决策。状态是内存模型，数据库用于审计，不用于重启恢复。浏览器最近记录和桥接队列均有界；`preview_records_dropped` 表示最近记录缓存发生淘汰，完整已提交审计仍在数据库。其它规则容量与 TTL 见 [M1 文档](moderation-m1.md)。

## 实现与验证记录

自行实现有界 protobuf 读取和原生 `DecompressionStream` 解压，没有引入第三方运行时代码或整份 schema。字段线索来自 [wwengg 的协议定义](https://github.com/wwengg/douyin/blob/master/proto/message.proto)及 [opedium 的传输层定义](https://github.com/opedium/douyin-live-proto/blob/master/douyin_live.proto)；时间单位差异参考 [dyhub 的规范化代码](https://github.com/ymstar/dyhub/blob/7ec26d9e94723ff9e6cab77c9258c2a63cbe0c7d/src/pipeline/normalizer.ts)。这些都是非官方协议线索，需要现场验证。

2026-10-12 在现有 Firefox 房间 `517107254662` 的只读验证中，确认了 `msg` 帧类型、封装与业务 ID 不同、真实礼物和进场候选，以及一条真实礼物感谢预览。随后用户复测也遇到多件礼物造成整页暂停，现场有限计数样本为 `group_count=8/10、repeat_count=8/10、combo_count=1`。现已移除该拒绝分支，按累计值处理；尚未覆盖的礼物语义采用单条隔离处理。粉丝团升级尚缺真实现场事件验证；荣誉/财富升级映射尚未完成。

开发侧集成测试 `tests/moderation_online_integration.rs` 使用完整 SDK、适配器、gzip/非压缩合成帧和真实本机 HTTP 桥接，验证大整数 ID、累计乱序/重复/结束包、5 秒等待、连续两批独立预览、多件礼物不重复乘算、同帧未支持礼物隔离、受影响用户窗口取消、欢迎、粉丝团原生升级、真实缺口暂停及数据库审计。它不访问直播平台，也不会替代上述真实直播验收。

修复后的复测：先停止旧探针，再用上面的命令启动新进程（已运行的进程仍含旧脚本），随后更换 URL query 值刷新页面。观察同一用户先后两轮送礼，预期每轮结束后约 5 秒各生成一条独立感谢；遇到单条未支持礼物可以出现跳过警告，但 `rules.paused` 应保持 `false`，后续有效礼物继续出日志。遇到损坏帧、缓冲溢出或房间身份变化仍会明确暂停。
