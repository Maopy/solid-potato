# 0.3.3 热烈风格的礼物感谢预设

重新编写全部 38 条预设，去掉“收到啦”“心意很珍贵”“衷心感谢”等偏正式表达，改用“霸气”“给力”“排面”“燃爆”“炸裂”等直播间风格，随礼物价值档位增强语气。

| 礼物单价范围 | 文案示例 |
| --- | --- |
| 0–99 | 感谢{nickname}的{gift_list}，给力！ |
| 100–999 | 霸气！感谢{nickname}送来的{gift_list} |
| 1000–9999 | 太霸气了！感谢{nickname}的{gift_list}，排面拉满 |
| 10000 及以上 | {nickname}这波封神了！感谢{gift_list} |
| 价格未知 | {nickname}太给力了！感谢{gift_list} |

四档各 8 条，未知价格通用池 6 条。完整可编辑预设在 `config/gift-wording.example.yml`，它是配置片段，合并到现有房间 YAML 使用，不能单独启动。

已启用 `rules.gift.variation.enabled: true` 且未自定义 tiers / unknown_templates 的配置，替换程序后重启并重新打开直播页即可使用新版内置文案。若 YAML 已复制旧文案池，需要把对应文案段替换为新版。新包的感谢配置继续默认启用随机文案；未启用的旧配置，在现有 `rules.gift` 下添加：

```yaml
variation:
  enabled: true
```

本次只修改内置文案和可编辑预设；礼物价值阈值、每用户 5 秒合并、避免连续重复、随机 emoji 和超长礼物名称处理仍沿用 0.3.2。自定义的单条 template 和 overflow_template 保留用户自己的内容。价格仍使用平台原始单位，不换算人民币。完整配置方式见 `docs/moderation-release-0.3.2.md`。

使用现有配置与文案集成测试验收，未运行单元测试，未向真实直播间发送测试消息。
