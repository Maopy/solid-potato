# 定时文案

修改 unified.yml 实际引用的房间配置，例如 config/douyin-auto.yml 或 config/wechat-auto.yml：

```yaml
timed_messages:
  enabled: true
  interval_seconds: 60
  min_comments: 6
  count_scope: text_comments
  messages:
    - '欢迎来到直播间'
```

时间和条数同时满足才生成一条，生成后重新计时、计数。`text_comments` 只统计新文字评论，包含场控自己发送的文字；进场、礼物、关注、点赞和系统通知均不计数。没有新文字评论时，时间到了也不发送。public_screen 包括场控自己的自动消息；comments 仅统计符合实时资格的观众文字评论并排除自动回复，默认 comments 兼容已有配置。min_comments 为 0 时只按时间。

抖音按新出现的公屏行计数：礼物、进场、关注、点赞提示、系统提示等只要在列表中新增一行就算一次。刷新前已有内容不计入，同一行礼物数量更新不增加计数。

视频号使用原生公屏消息列表与已识别的礼物展示通知，按消息 ID 去重。列表之外未识别的提示可能漏计，平台过滤、折叠也可能造成列表数与实际可见行数的差异。

timed-messages.example.yml 只是示例片段，不会自动加载。面板显示当前轮计数、累计计数和等待条件。停止场控后清零，重新开启重新累计；场控关闭期间不计数，不回补历史消息。
