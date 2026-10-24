# 按直播间设置场控

现有 `douyin-auto.yml` 和 `wechat-auto.yml` 中的配置继续作为各平台的默认配置。新增 `room_profiles`，按直播间标识匹配；找到专属项后，只覆盖该项填写的字段。没有专属项时，使用默认配置。

抖音在 `config/douyin-auto.yml` 添加（直播间号为 `https://live.douyin.com/` 后的数字；键必须加引号）：

```yaml
room_profiles:
  "123456789":
    rules:
      upgrade:
        template: "恭喜{nickname}升到{new_level}级！"
      welcome:
        enabled: false
    timed_messages:
      enabled: true
      interval_seconds: 300
      min_comments: 10
      messages:
        - "欢迎来到这个直播间！"
    keyword_replies:
      enabled: false
  "987654321":
    likes:
      batch_count: 100
```

视频号在 `config/wechat-auto.yml` 添加，使用主播的完整账号 ID（如 `v2_…@finder`，须替换为实际 ID）。不要用主播昵称、观众用户 ID、标签页 ID或会变化的直播场次 ID。

```yaml
room_profiles:
  "主播完整账号ID":
    rules:
      upgrade:
        template: "恭喜{nickname}升到{new_level}级！"
    timed_messages:
      min_comments: 20
```

统一入口会从视频号页面现有的 `WXU.check_live_existing` 只读获取主播账号，自动选择配置；若页面助手稍晚加载，会短暂等待。未能读取时使用默认配置和原有默认接收目标，不猜测主播身份。读取到账号但没有专属配置时，使用默认规则，礼物接收目标绑定当前账号。切换主播时仍沿用原有停止机制，重新打开页面后重新匹配。多个标签页的配置互不覆盖。

支持覆盖 `rules`、`timed_messages`、`keyword_replies`、`nickname_mapping`、`likes`、`limits`。嵌套对象逐字段继承，列表整体替换（例如专属 `messages` 替换默认消息列表）。`nickname_mapping.users` 可以只添加或覆盖少量用户昵称；相对昵称文件路径仍相对于当前平台配置文件。

账号、全局发送限额、代理、录屏目录和运行模式由原有配置统一管理，不在 `room_profiles` 中重复设置。每个平台最多 32 个专属配置，所有配置在启动时校验；专属配置存在错误会明确报错，不会悄悄忽略。

升级时原有默认值、昵称映射、专属项和其他配置文件均保留。配置修改后重启后台，再重新打开或刷新对应直播页生效。无需更新浏览器脚本；此次选择逻辑在后台及视频号代理加载器中实现。当前不支持运行中热加载。
