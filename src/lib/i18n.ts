export const messages = {
  "zh-CN": {
    "nav.library": "游戏库", "nav.detail": "游戏详情", "nav.agent": "Agent 对话",
    "nav.discover": "Mod 发现", "nav.plugins": "插件中心", "nav.trash": "回收站",
    "nav.history": "历史对话", "nav.settings": "设置", "app.name": "Agent Mod Manager",
    "common.cancel": "取消", "common.confirm": "确认", "common.error": "操作失败",
    "connection.native": "本地服务", "connection.preview": "浏览器预览",
  },
  "en-US": {
    "nav.library": "Library", "nav.detail": "Game Details", "nav.agent": "Agent Chat",
    "nav.discover": "Discover Mods", "nav.plugins": "Plugins", "nav.trash": "Trash",
    "nav.history": "Chat History", "nav.settings": "Settings", "app.name": "Agent Mod Manager",
    "common.cancel": "Cancel", "common.confirm": "Confirm", "common.error": "Operation Failed",
    "connection.native": "Local Service", "connection.preview": "Browser Preview",
  },
} as const;
export type Locale = keyof typeof messages;
export type MessageKey = keyof typeof messages["zh-CN"];
export const t = (key: MessageKey, locale: Locale = "zh-CN") => messages[locale][key];
