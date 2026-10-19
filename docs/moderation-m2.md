# M2 第一阶段：发送调度预演

已接入规则队列与平台无关调度器。此阶段模拟出队时间和批次冻结，记录 `dispatch_preview`，没有浏览器发言命令、输入框操作或发送成功声明。UI 继续暂停。

后续第二阶段已提供可选的 [仅填入输入框模式](moderation-fill.md)。以下命令不启用填入；用户明确要求测试禁止真实发送。

## 浏览器集成测试

停止旧探针，使用已配置代理和 CA 的 Firefox；不需要运行单元测试。在项目根目录启动：

```sh
cargo run -p live-archive-core -- moderation probe \
  --room https://live.douyin.com/139244611014 \
  --directory data/moderation-probe \
  --proxy-port 2024 --duration-seconds 0 \
  --rules-preview --scheduler-preview
```

打开 [测试直播间](https://live.douyin.com/139244611014?live_archive_probe=scheduler-test-1)，每次重新启动后更换 query 值刷新，避免旧 HTML 缓存。在 Console 查看：

```js
window.__liveArchiveModerationV1?.getStatus()
```

应看到 `rules.mode: online_scheduler_preview`、`send_chat: false`，以及 `rules.scheduler` 中的 `pending`、`dispatch_previews`、`next_eligible_at_ms`。时间是本次探针进程启动后的毫秒数；`next_eligible_at_ms` 只针对目前队列计算，未来新事件可以改变顺序。没有能在过期前出队的动作时返回 `null`。

等待自然出现的礼物、进场或升级事件，无需购买礼物。符合规则后先记录 `planned`，此时未出队；到模拟出队时 Console 显示 `[Live Archive dispatch preview] 文案`。原有不带 `--scheduler-preview` 的即时文案预览继续可用。

| 场景 | 预期行为 |
| --- | --- |
| 同一用户连续送礼 | 最后一次有效新增后静默 5 秒才入队；重复、乱序和无新增结束包不延期 |
| 同一模拟账号连续出队 | 两次出队至少间隔 8 秒，滑动 60 秒最多 6 条；实际预览还包含桥接轮询延迟 |
| 礼物感谢等待预算期间又有新礼物 | 撤销旧动作，合并该批已有礼物与新增礼物，再静默等待 5 秒 |
| 感谢已经出队后又有新礼物 | 新批次独立感谢，不再感谢旧批次的礼物 |
| 多条动作同时等待 | 升级优先于礼物，礼物优先于欢迎；相同优先级按创建时间等稳定排序 |
| 等待超过动作有效期 | 欢迎 10 秒、礼物/升级 20 秒，从动作创建时计算；记录过期并释放队列，不消耗预算 |
| 传输缺口或房间结束 | 取消等待动作；暂停后不再出队 |

取消及过期会显示 `[Live Archive queue]` 和原因；详情保存在 `preview_audits`。因此不能只依据“5 秒到了却未出现文案”判断失败，应同时查看队列、限流和过期记录。未支持的单条礼物隔离行为沿用 [在线预览说明](moderation-online.md)。

## 边界与后续

规则引擎拥有唯一队列，调度器每次从当前队列选择，不缓存已经撤销或过期的动作。模拟预算按 `(platform, actor_user_id)` 区分，在同一个引擎中跨房间、场次和接收者共享；其他账号独立。默认预算是应用策略，不代表平台的实际频控规则。

当前网页仍无法验证真实发言账号，所以每个文档独立模拟，`actor_user_id` 仍为 `observe-only`。不同页面、不同进程和刷新后的预算不共享；尚不能用于真实执行。规则队列沿用每作用域最多 20 条，真实执行阶段还需建立整个房间的共享队列上限。进程重启不恢复等待队列或发送预算。

自动集成验证包含规范事件 → 规则引擎 → 调度 → 审计的虚拟时间管线，以及完整 SDK/解码器 → 真实本机 HTTP 桥接 → 调度 → SQLite → SDK Console 管线。覆盖跨房间账号预算、不同账号隔离、每分钟第七条、时间边界、排队合并、优先级、过期及缺口取消。合成数据验证不等于实际直播间验收；本阶段新模式尚待上述现场测试。

仅填入的页面检查、命令和回执已接入，见上方第二阶段文档。真实账号识别、发送命令及确认能力尚未实现，真实发送测试须另行获得用户明确授权。
