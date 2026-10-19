# YAML 场控配置与集成测试

场控使用独立的 YAML 文件，录屏仍使用原来的 `live-archive.toml`。抖音支持 `douyin` / `douyin.live_page`。视频号新增 `wechat_channels` / `wechat_channels.desktop_probe`，用于微信桌面客户端内的直播页面，目前仅支持 observe 通信探测；见 [视频号桌面注入验收](moderation-wechat.md)。加载器均沿用同源代理注入，官方助手适配尚未实现。

## 启动

在仓库根目录执行：

```sh
cp moderation.example.yml moderation.yml
cargo run -p live-archive-core -- moderation check --config moderation.yml
cargo run -p live-archive-core -- moderation run --config moderation.yml
```

`check` 只读取和验证配置，不启动代理、不打开数据库。`run` 验证通过后启动现有场控链路。浏览器使用之前测试成功的代理和 CA 配置，刷新 `https://live.douyin.com/139244611014`。`Ctrl-C` 停止。旧的 `moderation probe` 命令仍可使用。

## 模式

| mode | 行为 |
| --- | --- |
| `observe` | 只采集观察事件；省略 mode 的默认值 |
| `rules_preview` | 输出规则文案，不写输入框 |
| `scheduler_preview` | 按预算调度并输出预览，不写输入框；示例采用此模式 |
| `fill_only` | 调度后尝试填入输入框，不发送 |

所有模式均 `send_chat=false`，配置不接受 `execute`、发送开关或发送动作。仅填入模式遇到非空输入框会记录 `input_busy`，保留原文；不会清空输入框。`fill_test_text` 只能用于 `fill_only`，它会在每次新页面配对后填入一次固定测试文案，同时关闭自动规则填入。

## 可配置项

规则包括 `gift`、`welcome`、`supporter_welcome`、`fansclub_joined`、`upgrade`；每项有 `enabled` 和 `template`。升级还有 `fansclub`、`honor`、`wealth` 三个独立开关及 `allow_inferred_levels`。默认不根据等级快照推断升级。开启某个规则不等于平台已支持该事件：当前抖音采集覆盖仍以现有解码器为准，荣誉/财富等级的可靠在线事件尚未打通。

模板支持的变量：

| 模板 | 变量 |
| --- | --- |
| `gift.template` | `{nickname}`、`{gift_list}` |
| `gift.overflow_template` | `{nickname}` |
| 欢迎、入团模板 | `{nickname}`（进场欢迎为合并名单，入团为单人昵称） |
| 升级模板 | `{nickname}`、`{level_name}`、`{new_level}` |

`{{` / `}}` 表示字面大括号。模板不会执行脚本；空模板、控制字符、未知变量会报错，关闭的规则也会检查模板。礼物固定按用户独立等待静默 5 秒，期间有新增礼物则重新等待 5 秒；礼物名称拼接超过 30 个 Unicode 字符时采用 `overflow_template`。这两个数值暂不开放配置。

调度默认间隔 8000ms、每分钟 6 条，可以收紧到间隔 8000–60000ms、每分钟 1–6 条。欢迎 TTL 10 秒、感谢和升级 TTL 20 秒保持不变，预算过紧会令排队文案过期。预算仍隔离到页面文档，尚未验证发言账号，不能视作跨浏览器共享账号限流。

资源上限：`max_text_chars` 1–500（`fill_only` 最多 80）、`max_active_users` 1–500、`max_gift_items` 1–100、`max_pending_actions` 1–20、`max_dedupe_entries` 1–20000。超长文案会被抑制并记录，避免截断感谢内容。YAML 默认文案上限 80 字符。

`storage.directory` 相对配置所在目录解析；`mitmdump` 为单个名称时通过 PATH 查找，`./bin/mitmdump` 等显式相对路径也按配置目录解析。`duration_seconds: 0` 持续运行，非零范围 1–3600。文件最多 64 KiB；未知字段、重复字段、错误类型、不支持的版本/平台/模式和不合法房间会拒绝启动。

视频号桌面适配可以设置 `injection.system_proxy: true`，启动时复用录屏系统代理和本地 CA 流程，停止时恢复原 HTTP/HTTPS 代理；视频号示例已启用。该字段默认 false，抖音不允许启用。

本版配置在启动时固定，不支持热加载。修改后停止旧进程、重启并刷新页面重新配对；旧待处理动作不会恢复。当前运行的 `run_id` 用于日志隔离，尚未实现跨配置修订重新规划。

## 手工集成验收

不需要运行单元测试，使用上面的命令和浏览器验收即可：

1. 保持 `scheduler_preview`，打开控制台，观察礼物静默后出现感谢预览，输入框保持原样。
2. 将 `gift.template` 改成 `'谢谢{nickname}的{gift_list}'`，重启并刷新页面；新的礼物感谢应使用新文案。
3. 将 `gift.enabled` 设为 `false`，重启并刷新；礼物仍可采集，但不会规划感谢，其他开启规则继续工作。
4. 故意把字段名写错，运行 `check`，应报错；恢复后再运行。
5. 如需验证填入，将 mode 改为 `fill_only` 并启用注释中的 `fill_test_text`；只确认输入框出现文案，不点击发送、不按 Enter。测试后恢复预览模式。

仓库自动化集成测试覆盖 YAML → 规则 → 调度、CLI 启动前拒绝错误配置，以及 YAML → 注入 SDK → 本地 HTTP 桥接 → 预览/仅填入 → SQLite 回执。测试使用合成事件和本地模拟输入框，不向真实直播间发言。

进场欢迎按直播间每 30 秒合并一条，支持抖音原生进场和视频号明确的“昵称 来了”通知。发布配置已启用；详见 [进场用户合并欢迎](moderation-welcome.md)。

`rules.supporter_welcome` 控制超粉、星守护的进场欢迎，支持 `{nickname}`、`{title}`，每个用户每分钟最多生成一次，与普通欢迎的 30 秒合并独立。当前视频号按明确原生徽章匹配，抖音身份字段尚未接入。
