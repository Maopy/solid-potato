# 智能场控开源参考调研

调研日期：2026-10-12。范围遵循 [当前架构](smart-moderation-architecture.md)：抖音注入；视频号注入和官方助手页面自动化；未来平台可扩展；暂停 UI 开发。

本轮只读检查公开仓库、关键源码、许可证和 GitHub 提交记录，没有安装脚本、引入依赖或实测第三方项目。提交日期不代表当前直播能力可用。用户现用脚本是 `TanMusong/livebot-assets`，静态检查结果见下节。

## 用户现用脚本：LiveBot

来源：[TanMusong/livebot-assets](https://github.com/TanMusong/livebot-assets)。检查固定快照 `afa291a1cae0e54df652cbcabd9dc364d5d77670`（2026-09-18，v0.2.7），核心文件 [live-bot.user.js](https://github.com/TanMusong/livebot-assets/blob/afa291a1cae0e54df652cbcabd9dc364d5d77670/live-bot.user.js)。这是用户确认正在使用的项目，浏览器中安装的具体版本尚未核对。

文件经过压缩和字符串混淆，仓库未提供拆分源码或构建工程。本次只静态阅读、解码字符串常量，没有执行整个脚本。脚本头标注 `@license MIT`，但仓库没有独立 LICENSE；不能把 GitHub 的 license=null 误读为脚本没有任何许可声明。引入前应补齐授权文本及版权归属记录。

| 项目 | 静态检查结果 | 对当前架构的影响 |
| --- | --- | --- |
| 采集 | 使用 MutationObserver 读取入场、聊天、点赞及礼物区域；依赖选择器和页面提示文本 | 可以作为首版 DOM 适配参考，但只覆盖页面呈现的事件，不能声称完整数据流 |
| 发送 | `es` 查找输入区域，修改内容并派发事件，等待约 300ms 后向发送 SVG 派发点击 | 是页面操作路线；函数没有返回平台接受的结构化确认，点击后记录日志不能当成功回执 |
| 规则 | 欢迎、感谢、模板、冷却和 AI 调用都在浏览器内 | 我们把这些决策移到 Rust，脚本保留采集与执行 |
| 身份 | 已检查的事件对象主要用 username 保存显示昵称 | 不作为可靠用户 ID；无法取得 ID 时标记身份可信度并限制升级推断 |
| 等级 | 高等级欢迎来自特殊入场文案 | 不等于粉丝团或荣誉升级；两类升级需要另找事件来源 |
| 礼物 | 读取可见礼物名称和数量，用最近一条“昵称/礼物/数量”组合抑制重复 | 缺少平台消息 ID 和连击组语义；相同送礼可能漏报，交错 DOM 变化可能重复，需样本验证 |
| 队列 | 发送服务设待发容量 3、发送后延迟 500ms，满时丢弃最旧项 | 借鉴有界队列，但生产限流另定；排队分支不会等待实际发送完成，控制端需按 request_id 跟踪 |
| 停止 | 主停止路径断开观察器，发送服务另有定时出队 | 已检查的停止路径未见清空发送队列；必须验证停止、切房及 AI 延迟返回后是否还会发言 |
| 通信 | 有脚本存储和通过 GM_xmlhttpRequest 访问用户配置的 AI 服务 | 已检查代码未提供与 Rust 本地控制端的事件/命令协议，需要新增桥接 |

LiveBot 可提供 `douyin_dom` 来源与页面文字执行器的参考，保留事件原始文案和来源可信度。它不是必须采用的实现。进一步调研后，建议优先探测页面已有网络消息，再决定首版主来源；DOM 作为补充或降级。脚本使用固定命令类型、握手认证、房间/账号/会话校验和请求回执；规则与队列只由控制端统一调度。

验证顺序：现有安装版本及只读事件样本 → DOM 重建/切房/重复礼物回放 → 单条授权文字发送与结果识别 → 停止和超时测试 → 接入欢迎/感谢规则。升级在来源确认之前保持禁用。能力探针期间应关闭原脚本的自动回复，防止两个执行器同时发言；本轮未操作用户已安装脚本。

## 扩大调研后的新增参考

### oba-live-tool：视频号助手自动化的优先参考

来源：[qiutongxue/oba-live-tool](https://github.com/qiutongxue/oba-live-tool)。2026-10-12 下载 main 源码静态阅读，查询当前提交为 `db22b63ee9bcca3e96b6631b6a0c88061ed0777d`；下载与查询相邻进行。根 [LICENSE](https://github.com/qiutongxue/oba-live-tool/blob/db22b63ee9bcca3e96b6631b6a0c88061ed0777d/LICENSE) 为 MIT。没有安装、构建或实测该工具。搜索到的 TLS-live-tool 同类仓库 README 仍指向该项目的构建步骤，优先研究此原项目，避免把派生项目当作独立实现。

重点文件：

- [IPlatform.ts](https://github.com/qiutongxue/oba-live-tool/blob/db22b63ee9bcca3e96b6631b6a0c88061ed0777d/electron/main/platforms/IPlatform.ts)：拆分评论监听、文字执行等能力接口，适合借鉴能力声明。
- [视频号 commentListener.ts](https://github.com/qiutongxue/oba-live-tool/blob/db22b63ee9bcca3e96b6631b6a0c88061ed0777d/electron/main/platforms/wechat-channels/commentListener.ts)：监听 Playwright 页面 response，解析已有消息响应，保留消息与用户标识。
- [视频号 index.ts](https://github.com/qiutongxue/oba-live-tool/blob/db22b63ee9bcca3e96b6631b6a0c88061ed0777d/electron/main/platforms/wechat-channels/index.ts)：连接助手中控台、登录和文字执行。
- [helper.ts](https://github.com/qiutongxue/oba-live-tool/blob/db22b63ee9bcca3e96b6631b6a0c88061ed0777d/electron/main/platforms/helper.ts)：填写评论框并触发发送，必要时回退 Enter。

适合参考“页面已有网络响应采集 + DOM 发言”组合。已检查的视频号监听器仅处理普通评论及回复，没有礼物/入场/两类升级的完整映射；助手的浏览器实现也不能直接代表微信桌面客户端支持。发送 helper 的返回值涉及置顶结果，没有平台接受确认；视频号 disconnect 仍抛出未实现异常。采用相关思路时仍需补齐退出清理和发送状态。

其抖音适配面向抖店/百应等商家中控和罗盘大屏，需要相应账号可访问；不等于任意 live.douyin.com 观众账号可用。可作为未来附加适配器线索，不替代当前抖音注入路线。

### qinuoyun/douyin：通信桥接参考，非直播场控

来源：[qinuoyun/douyin](https://github.com/qinuoyun/douyin)。2026-10-12 静态阅读 main 的 `scripts/douyin.user.js`、`lib/server/router.js` 和 `lib/shared/protocol.js`；根 LICENSE 为 MIT。

这是短视频评论管理工具。其油猴端用 GM_xmlhttpRequest 与本地服务长轮询通信，提供 token、请求 ID、结果回传和失败重连，适合对照浏览器沙箱与本地控制端的通信。它接收表达式并用页面 eval 执行；我们的桥接应改为明确的事件/命令类型，限定 send_text 等能力，并按房间、账号和会话管理连接，不能照搬按站点调度。HTTP 长轮询可作为受浏览器/脚本环境约束时的备选传输，WS 是否可用应实测。

### 其他筛选结果

[tian-xiu/LiveStream-Agent](https://github.com/tian-xiu/LiveStream-Agent) 的 README 展示连接器、过滤和调度分层，但主输出是本机 TTS；本轮没有完成源码检查，未确认独立许可证，不纳入文字发送主参考。私信助手、作品评论和视频发布项目也不能算作直播场控已实现。

## 综合参考排序

当前建议：视频号助手消息结构和页面发言步骤参考 oba-live-tool，但不采用其 Playwright 运行方式；所有当前环境统一使用脚本注入。抖音网络事件与字段对照 DyHub 和 DouyinBarrageGrab；DOM 观察和文字操作对照 LiveBot；本地桥接参考 qinuoyun/douyin 的生命周期思路。最终接入保持自主的薄适配器和 Rust 控制核心，不绑定任何一个项目。协议和实现阶段见 [详细设计](smart-moderation-detailed-design.md)。

## 优先参考

| 项目 | 已检查的内容与用途 | 边界 |
| --- | --- | --- |
| [ymstar/dyhub](https://github.com/ymstar/dyhub) | 浏览器 CDP 采集、事件归一化和分发；适合对照我们的采集/控制端边界 | CDP 观察不是油猴注入；已检查的归一化器没有粉丝团升级分支，不能视为完整场控 |
| [ape-byte/DouyinBarrageGrab](https://github.com/ape-byte/DouyinBarrageGrab) | 礼物增量/连续数量、粉丝团事件及用户等级字段；适合协议和测试夹具设计 | 系统代理及 Windows/.NET 路线不同于当前抖音注入路线；字段存在不保证平台持续推送 |
| [ltaoo/wx_channels_download](https://github.com/ltaoo/wx_channels_download) | 视频号注入和桥接的既有参考，当前项目已经保留部分适配资源 | 下载器不等于场控；许可证包含 Commons Clause，不能当成纯 MIT |

### DyHub

检查快照：`7ec26d9e94723ff9e6cab77c9258c2a63cbe0c7d`；最新提交 2026-10-08，修复礼物/进场字段及时间单位。根 [LICENSE](https://github.com/ymstar/dyhub/blob/7ec26d9e94723ff9e6cab77c9258c2a63cbe0c7d/LICENSE) 为 MIT；实际引入任何文件前仍需核查该文件及其上游来源。

重点文件：[liveSession.ts](https://github.com/ymstar/dyhub/blob/7ec26d9e94723ff9e6cab77c9258c2a63cbe0c7d/src/collector/liveSession.ts)、[normalizer.ts](https://github.com/ymstar/dyhub/blob/7ec26d9e94723ff9e6cab77c9258c2a63cbe0c7d/src/pipeline/normalizer.ts)。借鉴事件翻译层，保留 `groupId`、`repeatCount`、`repeatEnd`。其会话尝试解码页面所有 WS 帧，并将任何 WS 关闭视为会话错误；我们的适配器应通过连接标识关联直播 WS，分别记录网页房间号和内部房间号。宣传中的稳定性/无丢失不作为验收依据。

### DouyinBarrageGrab

检查快照：`c9343da287cf77ce637c11e32333b2a511cd62e9`；最新提交 2026-04-11。根许可证标为 MIT，README 提醒 GitHub 更新可能滞后。

重点：[BarrageMessages.cs](https://github.com/ape-byte/DouyinBarrageGrab/blob/c9343da287cf77ce637c11e32333b2a511cd62e9/BarrageGrab/Modles/JsonEntity/BarrageMessages.cs)。类型区分 `GiftCount` 增量与 `RepeatCount` 连续数量；粉丝团消息含类型和等级；用户对象含 `PayLevel`。这些适合作为映射线索，具体语义以本项目采集样本验证。用户对象上的等级不能直接当作一次升级事件。README 提到的付费技术支持能力不能计入公开源码已实现能力。

### 视频号现有参考

上游检查快照：`124f044235bc706fe796c043a14ff442a150ae5d`；最新提交 2026-09-15。当前项目历史使用版本另见 [THIRD_PARTY_NOTICES](../THIRD_PARTY_NOTICES.md)，本轮没有升级资源。

上游 [LICENSE](https://github.com/ltaoo/wx_channels_download/blob/124f044235bc706fe796c043a14ff442a150ae5d/LICENSE) 是 MIT 加 Commons Clause，明确排除其定义的 Sell 权利。若后续产品涉及收费或分发，需要据实际使用范围处理授权；本轮不新增代码复用。现有录屏桥接和未来场控连接应分别管理。

## 次要参考及排除项

| 项目 | 检查结果 | 处理 |
| --- | --- | --- |
| [saermart/DouyinLiveWebFetcher](https://github.com/saermart/DouyinLiveWebFetcher) | `liveMan.py` 含 gzip/protobuf 解码及消息分派，直接 WS 路线；快照 `40a904bdb0561a5a9c958a95634a29681cc4fbf4`，最新提交 2026-09-08 是 README 更新 | 仅对照协议。根许可证 AGPL-3.0，README 另有禁止商业谋利声明，复用前必须澄清；不直接移植礼物打印逻辑作为计数实现 |
| [BarryWangQwQ/UniBarrage](https://github.com/BarryWangQwQ/UniBarrage) | 多平台目录与统一消息格式；快照 `ea0d7d0314615a2743e424f0f30a6a700018f01a`，最新提交 2024-12-08 是 README 更新 | 借鉴接口分层。其 LICENSE 为 GPLv3 描述加商业授权要求，GitHub 识别为 Other，不能标成普通 GPL 或默认可直接复用 |
| [wordweb/Get-Tencent-Video-Live-Barrage-Gifts](https://github.com/wordweb/Get-Tencent-Video-Live-Barrage-Gifts) | `webapi.py` 有助手扫码、消息轮询与礼物 payload 解析；快照 `002d1186f138cb8e15d2b389a9bc71cc797b49f9`，最新提交 2023-10-17 | 没找到明确许可证，只作行为线索；这是调用助手内部接口的旧实现，不是官方开放 API，也不是页面自动化实现 |

视频号旧源码和 oba-live-tool 的监听器提示助手侧消息可能来自 HTTP 轮询，而非只有 WS；能力探针应同时观察页面已有 fetch/XHR、WS 和 DOM。但旧消息类型编号不能直接写入当前版本。已找到助手评论采集和文字执行的源码参考，尚未实测完整助手场控实现。

## 对当前方案的落地建议

1. 优先检查用户现用油猴脚本：启动时机、页面/沙箱上下文、采集来源、房间和用户标识、文字输入与发送、升级语义、断线恢复、外部依赖和许可证。
2. 抖音脚本做薄适配层：采集标准事件、接收明确的文字发送命令、报告执行结果；规则、礼物合并和欢迎冷却留在 Rust 控制端。
3. 单独验证采集与发送。能读取弹幕不代表能发送；点击发送按钮不代表平台已接受。优先保留现有发送方式并增加房间校验、限流和结果回执。
4. 视频号浏览器助手和微信客户端分别探测，按实际事件覆盖声明能力；升级事件无法确认时不触发自动祝贺。
5. 用脱敏样本回放验证连击礼物、重复消息、首次等级基线、重连和超时结果，再接通真实发送。新增平台只补适配器，不改规则核心。

上述参考没有证明任何一个项目完整覆盖“欢迎 + 礼物感谢 + 两类升级 + 可靠文字发送”。这一完整闭环仍由本项目实现并验收。
