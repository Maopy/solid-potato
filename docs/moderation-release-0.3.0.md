# 0.3.0 多平台、多直播间控制入口

新增一个控制进程统一管理多个房间任务，所有任务共用一份本地 CA、一个 mitmdump 代理和一次系统代理设置。任务的采集连接、礼物去重、用户 5 秒合并窗口、感谢规则、输入/发送队列和数据库独立。同一配置账号跨房间共用自动发送预算，不同账号独立。现有单房间命令和 version 1 配置继续可用。

## 启动

解压后，在包目录执行：

```sh
./live-archive-core moderation check-controller --config config/controller.yml
./live-archive-core moderation serve --config config/controller.yml
```

也可以运行 `./run-controller.sh`。先停止旧 `moderation run` 进程，避免占用同一代理端口。示例包含两个抖音房间和一个视频号任务，默认不自动发送。示例抖音使用填入模式，视频号使用观察模式；观察模式仍需在诊断面板点击开始采集。

仓库中对应示例是 `moderation.control.example.yml`，可通过 `cargo run -p live-archive-core -- moderation serve --config moderation.control.example.yml` 启动。

## 总配置与房间配置

```yaml
version: 2
directory: ../data/controller
injection:
  system_proxy: true
  proxy_port: 2024
  mitmdump: mitmdump
accounts:
  - id: douyin-main
    platform: douyin
    scheduler: {min_interval_ms: 8000, account_per_minute: 6}
  - id: wechat-main
    platform: wechat_channels
    scheduler: {min_interval_ms: 8000, account_per_minute: 6}
tasks:
  - id: douyin-a
    label: 抖音直播间甲
    account_id: douyin-main
    config: douyin.yml
  - id: douyin-b
    label: 抖音直播间乙
    account_id: douyin-main
    config: douyin-second.yml
  - id: wechat-a
    label: 视频号直播间
    account_id: wechat-main
    config: wechat-observe.yml
```

任务继续引用现有 version 1 YAML，复用已有感谢模板、emoji、收礼目标、规则、发送开关和保留策略。`config` 路径相对于总配置所在目录解析。增加一个房间只需新增房间配置并增加一个任务条目；同平台也可以声明多个账号。每个任务 ID 唯一，最多 64 个 ASCII 字母、数字、短横线、下划线；配置最多 32 个任务和 32 个账号。

总配置决定统一代理端口、mitmdump、HTTP/2、系统代理和 CA 目录；任务配置中原有代理和 storage.directory 设置在控制模式下不生效。任务不能启用 TLS 诊断模式，任务 duration_seconds 必须为 0；总配置可设置 duration_seconds（0 表示等待 Ctrl-C，非零最多 3600 秒）。诊断模式仍使用旧 run 命令。

新日志路径为 `directory/tasks/<任务ID>/moderation-probe.sqlite3`。原始消息 7 天、感谢和筛选事件 30 天、原始数据预算 2 GiB 的默认策略继续适用，**预算按每个任务计算，不是整个控制服务的合计上限**。房间较多时可降低各房间的 raw_max_mib。旧目录和旧日志不自动搬迁或覆盖；需要继续使用旧 CA 时，将总配置 directory 设为原 CA 所在目录，新任务日志仍放在其 tasks 子目录。

所有任务在创建目录、启动代理和修改系统设置之前先完成配置检查。重复任务 ID、同一账号重复配置相同房间/收礼目标、账号平台不匹配等都会在预检时拒绝。创建任务失败会停止已启动的内部桥接服务；运行时单个任务的连接/日志状态独立，代理进程退出则整体停止。Ctrl-C 同时通知各任务停止采集并尝试排空，随后停止内部服务、恢复系统代理、退出共享代理。

## 页面绑定

抖音 URL 能明确对应唯一任务时自动绑定；同一个 URL 对应多个账号任务时需要选择。视频号多个直播页 URL 相同，因此每次打开页面都先显示任务选择，选择与当前主播、登录账号相符的任务后才加载采集/执行脚本。诊断面板会显示当前任务 ID。自动发送的任务仍只允许一个活动页面连接。

视频号保持原来的显式 recipient_user_id 配置和目标过滤，不能仅凭页面 URL 判断主播。收到可识别的原生 JoinLive/EnterRoom 调用或 JoinLive 事件时，控制模式会停止当前采集与执行并提示重新打开页面。客户端未经过这些可观察入口的切房方式仍无法保证被识别，因此换房应关闭并重新打开页面、重新选择任务；当前实现不宣称已自动核验视频号房间或账号身份。

页面的启动票据、会话令牌、事件序号、执行命令和回执都属于具体任务。跨任务复用凭证会被拒绝。未知任务路径、旧版保留路径和来源不匹配的桥接请求在本地直接拒绝，避免把本地令牌请求转发到平台。

## 自动感谢与账号限流

视频号感谢任务可将 config 指向已经填写好目标的 wechat.yml 或 wechat-auto.yml；多个主播各用一份配置和各自的目标 ID。抖音同理可以指向 douyin.yml 或 douyin-auto.yml。自动发送始终取决于房间配置 sending.automatic，不会因为使用 serve 自动开启。

账号层 scheduler 默认最短间隔 8 秒、每分钟最多 6 条，与房间自身的队列/限流共同生效。只有下发自动发送命令时占用账号预算，状态轮询和观察不占用；未知结果不会退还已占用的预算。排队仍有原有时效，繁忙时可能过期。

account_id 是当前阶段手工声明的逻辑账号，不负责登录，也未自动核对平台实际登录身份。同一个实际账号务必使用同一个 account_id；不同账号应在对应客户端或浏览器配置文件中登录，并在各页面选择对应任务。

## 开发边界与验收

本版实现配置驱动的一次性启动、并发运行和停止；修改任务列表或房间规则后需要重启控制服务并重新打开页面。后台 Controller 暴露任务状态和单任务停止方法，便于以后接 UI；本版没有新增管理 UI，也没有开放未经认证的本地管理 HTTP 接口。新增平台仍通过平台作用域、解码/注入和执行适配器接入，控制器不把房间规则写死在平台分支里。

集成测试使用四个任务、两个平台、三个逻辑账号，验证同一观众/消息 ID 的跨房间隔离、跨任务认证拒绝、同账号限流与不同账号并发、单任务停止、共享代理路由和页面选择、CSP nonce、未知保留路径拒绝、切房停止。另启动一个真实 mitmdump 临时进程验证多任务脚本加载、双平台 TLS 范围和退出清理。测试不修改系统代理，不访问真实直播间，不发送真实评论或礼物；32 个任务为配置上限，尚未进行该规模的真实直播压力测试。
