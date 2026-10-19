# M2 第二阶段：仅填入输入框

用户要求测试止于输入框，禁止真实发送。本阶段只实现 `fill_input`：没有点击发送、Enter/键盘事件、表单提交、平台发送接口或 WebSocket 写入。默认模式仍只观察，必须增加 `--fill-preview` 才允许填入。UI 继续暂停。

## 固定文案集成测试

此模式不需要等礼物，建议先用它确认输入框适配。你只需浏览器集成测试，不需要运行单元测试。

1. 停止旧探针。在 Firefox 的直播互动输入框中清空此前的测试文案，将焦点移出输入框。保留现有代理和 CA 配置。
2. 在项目根目录启动：

   ```sh
   cargo run -p live-archive-core -- moderation probe \
     --room https://live.douyin.com/139244611014 \
     --directory data/moderation-probe \
     --proxy-port 2024 --duration-seconds 0 \
     --rules-preview --scheduler-preview --fill-preview \
     --fill-test-text '场控填入测试（不会发送）'
   ```

3. 打开 [测试直播间](https://live.douyin.com/139244611014?live_archive_probe=fill-test-1)，更换 query 值刷新以避开旧 HTML。页面配对后应把固定文字填入直播互动输入框，保持未提交。
4. 在开发者工具 Console 查看：

   ```js
   window.__liveArchiveModerationV1?.getStatus()
   ```

   预期 `mode: fill_only`、`send_chat: false`、`fill.last_result.outcome: filled`、`reason: input_filled`、`fill.pending_result: false`。Console 显示 `[Live Archive fill] filled input_filled`。填入后 `fill.input_ready` 为 `false`、`input_reason` 为 `input_busy` 是正常的，因为输入框已有待检查的文案。

固定文案每个文档只尝试一次，`fill.bridge.manual_only` 为 `true`。规则和调度仍可预览自然事件，但不会用其他感谢覆盖固定文案。如果首次尝试遇到正在输入、未找到控件等原因，先解决原因，再更换 query 值刷新，建立新文档重新测试；不会在当前文档反复自动尝试。

## 礼物等规则文案集成测试

去掉 `--fill-test-text`，保留三个预览标志，重新启动并刷新页面。等自然出现的有效礼物、进场或原生升级事件，无需购买礼物。文案经过既有规则与限流调度，再交给输入框执行器。礼物仍按用户静默 5 秒合并；出队至少间隔 8 秒、滑动 60 秒最多 6 条。

首次填入后会保留待检查文案，后续命令遇到非空输入框返回 `input_busy`，不会覆盖，也不会自行清空。检查完文案后可以手动清空并移出焦点，再观察后续的新动作。之前被跳过的动作不会重新填入。

| 原因 | 行为 |
| --- | --- |
| `input_busy` | 有文字、输入框获得焦点或正在输入法组合，保留内容 |
| `input_not_found` / `input_not_unique` | 未匹配到唯一可见的直播互动输入框，跳过 |
| `input_disabled` / `input_hidden` / `page_hidden` | 控件不可编辑或页面不在可见状态，跳过 |
| `input_changed` / `input_error` | 控件结构、填入后文字校验或事件通知异常；可能已有部分文字，保留现场，不重写或回滚 |
| `binding_mismatch` / `room_changed` / `stopped` | 连接、文档或房间不一致，或页面已停止，拒绝写入 |
| `ttl_expired` | 扣除轮询和本地处理耗时后已过期，跳过 |
| `observation_paused` | 数据缺口或观察暂停，取消待填入动作 |
| `text_too_long` / `invalid_fill_text` | 超过 80 个 Unicode 字符、空白或包含控制字符，不截断填入 |

停止探针用终端 Ctrl-C，页面可调用 `window.__liveArchiveModerationV1.dispose()` 立即释放本页观察和执行器监听。切房、页面关闭或暂停不会清空已经填入的文案。

## 协议、审计和边界

控制端只有 `fill_input` 命令，携带连接、binding、document、房间 URL、action ID、文字和剩余有效期。一个连接只有一条在途命令；命令及派发状态在回传 poll 前写入独立 `fill_audits`。回执只允许 `filled` 或 `rejected`，通过现有同源代理的 `results` 路由送回本机桥接，不发往平台。

命令只交付一次；后续轮询发现交付超过 5 秒仍未收到结果时转为 `unknown`，不重新派发。SDK 缓存结果，同 action ID 不重复写入；网络错误只重试回执。相同回执幂等，冲突回执拒绝；迟到的有效回执可以补记填入结果。每文档最多保留 1000 个命令，待填队列上限 20，容量耗尽拒绝新动作。

抖音适配只匹配 `webcast-chatroom___input-container` 内已观察到的 Slate 编辑器或指定 placeholder 的 textarea，不使用全站任意 contenteditable。Slate 只写文字叶子，保留编辑器结构；其末尾零宽光标占位不算文案变化。最多 80 字是本应用填入限制，不代表平台发送限制。

`filled` 只证明页面显示了待检查文字，不证明平台编辑器内部发送模型已同步，更不表示已经发送。真实账号仍未验证，文档仍独立模拟预算；没有发送租约、跨页面账号预算、提交确认或跨重启恢复。

2026-10-12 在 Firefox 的 `139244611014` 真实页面上检查了输入框结构，并单独执行了相同填入适配器：固定文案可见，结果为 `filled/input_filled`；再次尝试返回 `rejected/input_busy`，原文案保留。未点击发送或按直播输入框的 Enter。该现场验证只覆盖 DOM 执行器，完整控制链路使用合成直播帧、本机真实 HTTP、完整 SDK/解码器/调度器、富文本输入框夹具和 SQLite 做集成验证，包含回执丢失重试及不覆盖已有文字。完整新进程的现场测试按上述步骤进行。
