# 0.3.17 抖音采集接入修复

抖音用户脚本显式使用页面的 window 和 WebSocket，避免 Tampermonkey 隔离环境中面板能连接但无法采集直播消息。补齐 XHR 房间身份读取，保留连接本机期间已返回的房间信息。

新增 www.douyin.com 和 douyin.com 的 follow/live、root/live、live 数字房间入口；同页导航进入或切换直播间时重新绑定，旧页面停止执行。后端继续使用 live.douyin.com 的标准房间地址，录制也沿用该地址。

面板显示评论、礼物采集数量、解析错误及评论输入框不可用原因。场控默认关闭；关闭期间只采集，不生成或发送自动弹幕。每条发送命令仍只点击一次，不等待回显、不重发。

升级：启动本版后，在统一控制台更新抖音脚本，再刷新直播页。本次新增 unsafeWindow 和抖音主站匹配范围，脚本管理器可能要求确认更新权限；不需要安装证书或关闭浏览器安全设置。

新版 config 完整继承上一版。规则修改继续使用 config/douyin-auto.yml 和 config/wechat-auto.yml。
