# M1：离线场控规则与回放

本阶段提供独立的 Rust 规则核心和虚拟时钟回放。输入是规范化的事件，输出是拟发送文案及决策日志。没有浏览器执行器，不会在直播间发送文字，也不会读取录制配置或写入录制数据库。UI 仍暂停开发。

## 直接测试

在项目根目录运行，不需要启动代理、打开直播间或登录：

```sh
cargo run -p live-archive-core -- moderation replay \
  --fixture tests/moderation/fixtures/gift-sliding.jsonl
```

标准输出是 JSONL 决策日志；标准错误是汇总，包含 `mode: offline_preview` 和 `send_chat: false`。寻找 `kind: planned` 的记录，查看 `action.text`：

| 虚拟时间 | 预期文案 |
| --- | --- |
| 6000 ms | 感谢乙送来的星星×1 |
| 8000 ms | 感谢甲送来的玫瑰×1、跑车×2 |

甲在 0 秒送玫瑰、3 秒送跑车，所以感谢延至 8 秒；乙在 1 秒送星星，在 6 秒感谢。

累计连击及乱序样例：

```sh
cargo run -p live-archive-core -- moderation replay \
  --fixture tests/moderation/fixtures/gift-cumulative.jsonl
```

累计数量依次是 1、3、3、2，最后结束包数量仍为 3。只在 0 秒和 1 秒确认新增，最终在 6 秒生成 `感谢甲送来的玫瑰×3`。

可用 `--output /tmp/moderation-audit.jsonl` 保存日志（文件必须不存在，不覆盖已有文件），用 `--until-ms 30000` 延长虚拟回放观察动作过期。默认结束时间是最后一帧后 5 秒。输出文件在 Unix 上采用 0600 权限。

## 已实现的行为

- 礼物按可靠用户 ID 独立聚合，每次新增重置 5 秒等待，持续送礼不强制提前感谢。礼物数量区分增量、累计和未知；未知数量只列名称。
- 同名礼物只占一个名称位置，按首次出现排序。名称总长度按 Unicode 字符计算，不包含昵称、数量和分隔符；30 字及以下列名称，超过 30 字生成 `感谢{nickname}，感谢礼物飘屏`。
- 感谢仍在队列时又收到礼物，撤销旧动作并保留整批礼物重新等待。模拟开始发送后冻结旧批次，后续礼物另起一批。已过期的动作不合并重发。
- 欢迎按用户冷却 60 秒，礼物和升级优先，可撤销待发送欢迎。欢迎动作有效期 10 秒，礼物和升级 20 秒；队列最多 20 个动作。
- 粉丝团、荣誉、财富等级分别处理和去重。默认只有已验证的原生升级事件触发祝贺；首次等级快照只建基线。`--allow-inferred-levels` 可启用同一来源快照上升的推断祝贺，动作标记 `inferred`。下降、来源切换和重复升级不触发。
- 平台、房间、直播会话、执行账号和礼物接收者共同隔离状态，未来平台可使用同一事件接口。未知事件类型忽略；缺失可靠身份或证据不足时记录抑制原因。
- 数据缺口、会话结束、计数溢出、事件 ID 冲突或资源容量耗尽会暂停该作用域并撤销等待动作。恢复需要新的直播会话作用域，不提前发送未完整的感谢。

## 输入格式

每行是一帧，`at_ms` 是虚拟时钟的毫秒时间。同一时刻的事件按文件顺序处理，全部事件处理完后再触发该时刻的计时器；因此恰好在感谢截止时刻到达的新礼物会延后感谢。

```json
{"at_ms":0,"event":{"event_id":"e1","scope":{"platform":"douyin","room_id":"demo-room","live_session_id":"demo-session","actor_user_id":"demo-assistant","recipient_user_id":"demo-anchor"},"native_event_id":"native-e1","source":"fixture","evidence":"verified","user":{"id":"u1","nickname":"甲"},"type":"gift.observed","data":{"gift_id":"rose","name":"玫瑰","recipient_user_id":"demo-anchor","quantity_mode":"incremental","quantity":1,"combo_id":null}}}
{"at_ms":5000}
```

支持类型：`gift.observed`、`member.entered`、`fansclub.joined`、`level.snapshot`、`level.upgraded`、`probe.gap`、`room.ended`。等级数据为 `{"kind":"fansclub","name":"粉丝团","new_level":3}`，`kind` 也支持 `honor`、`wealth`。欢迎和会话事件不需要 `data`。

累计礼物需 `quantity_mode: cumulative`、累计 `quantity` 和稳定 `combo_id`；增量礼物需 `incremental` 和正数数量；未知数量需 `unknown`、`quantity: null`。增量和未知数量都需稳定的 `native_event_id`，原生去重键包括用户、礼物和原生消息 ID。同一原生消息中同一用户的同一礼物，应在适配器中先合并为一条规范化事件。`combo_end: true` 的增量结束包不会增加数量；累计结束包只有数量超过已知最大值时才算新增。

`event_id` 需在当前作用域中稳定且唯一；重复 ID 的内容必须相同。`evidence: verified` 是适配器完成身份和语义验证后的承诺，离线核心不会替适配器验证平台数据。昵称不能代替用户 ID。

可用单独一帧 `{"at_ms":5000,"simulate_dispatch":"action-1"}` 模拟开始发送；具体动作 ID 以日志为准。这只用于检查批次冻结，不代表消息已送达，也没有真实发送或重试。

## 边界与后续接入

后续增量已提供 `probe --rules-preview` 在线观察入口，真实集成测试与目前覆盖见 [在线预览文档](moderation-online.md)。回放仍要求规范化 JSONL，不直接读取探针数据库；不能把原始 DOM 采样直接作为回放输入。默认 M0 的昵称评论样本不会触发礼物或升级规则，在线入口只有符合解码与身份条件的候选事件才进入规则。

当前规则配置由 Rust `RulesConfig` 提供；CLI 使用默认配置，仅开放推断升级选项，尚未接入设计文档中的 TOML 配置。模板支持变量校验和单次插值，不执行表达式。[M2 调度预演](moderation-m2.md) 已模拟账号限速和优先级；真实发送、跨页面全局账号预算、发送确认、持久化恢复及视频号适配器属于后续阶段。

这是有界的离线状态模型：单文件最多 4 MiB、10000 帧、24 小时；最多 16 个作用域，每作用域最多 500 个活动用户、每批 100 种礼物及 1000 条事件引用。礼物去重/连击状态保留 10 分钟，等级基线和升级去重保留 30 分钟。未来在线接入仍需处理超过保留期的迟到事件、重连和持久化，不能把当前内存去重当作跨重启保证。

验证入口：

```sh
cargo test -p live-archive-core --test moderation_rules
cargo test --workspace
```

测试覆盖滑动等待、累计乱序、重复包、同名礼物、30/31 字边界、排队撤销、批次冻结、动作过期、欢迎冷却、等级基线、未来消息类型、作用域隔离、资源耗尽以及 CLI 的离线隔离和输出保护。
