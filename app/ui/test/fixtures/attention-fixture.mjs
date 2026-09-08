// Shared synthetic 12-card sample from approved B v01. No production content.
export default {
  "version": "v01",
  "cards": [
    {
      "id": "01",
      "project": "Atlas",
      "group": "Working",
      "purpose": "开发",
      "title": "登录权限确认",
      "state": "needs-input",
      "read": true,
      "pin": true,
      "detail": "等待确认访问权限",
      "source": "hook"
    },
    {
      "id": "02",
      "project": "Atlas",
      "group": "Working",
      "purpose": "验证",
      "title": "回归测试",
      "state": "working",
      "detail": "工具执行中 · 45 秒无输出",
      "source": "hook"
    },
    {
      "id": "03",
      "project": "Atlas",
      "group": "Working",
      "purpose": "审查",
      "title": "接口审查",
      "state": "turn-done",
      "read": true,
      "detail": "本轮结束 · 已打开查看",
      "source": "hook"
    },
    {
      "id": "04",
      "project": "Atlas",
      "group": "Queued",
      "purpose": "开发",
      "title": "迁移草稿",
      "state": "recent",
      "detail": "3 秒前有输出 · 无有效 agent 状态",
      "source": "none"
    },
    {
      "id": "05",
      "project": "Atlas",
      "group": "Queued",
      "purpose": "验证",
      "title": "兼容性检查",
      "state": "stopped",
      "detail": "启动时已无存活 session",
      "source": "none"
    },
    {
      "id": "06",
      "project": "Atlas",
      "group": "Parked",
      "purpose": "运维",
      "title": "本地 shell",
      "state": "quiet",
      "detail": "安静 4 分钟 · shell 前台",
      "source": "none"
    },
    {
      "id": "07",
      "project": "Beacon",
      "group": "Working",
      "purpose": "审查",
      "title": "支付接口审查",
      "state": "turn-done",
      "read": false,
      "detail": "本轮结束 · 尚未打开查看",
      "source": "hook"
    },
    {
      "id": "08",
      "project": "Beacon",
      "group": "Parked",
      "purpose": "开发",
      "title": "沙箱权限确认",
      "state": "needs-input",
      "read": false,
      "detail": "等待权限确认",
      "source": "hook"
    },
    {
      "id": "09",
      "project": "Beacon",
      "group": "Queued",
      "purpose": "验证",
      "title": "静默构建",
      "state": "quiet",
      "detail": "安静 2 分钟 · 构建进程前台",
      "source": "none"
    },
    {
      "id": "10",
      "project": "Cedar",
      "group": "Working",
      "purpose": "审查",
      "title": "发布说明检查",
      "state": "turn-done",
      "read": false,
      "detail": "本轮结束 · 尚未打开查看",
      "source": "hook"
    },
    {
      "id": "11",
      "project": "Cedar",
      "group": "Working",
      "purpose": "开发",
      "title": "搜索索引",
      "state": "working",
      "pin": true,
      "detail": "正在执行本轮",
      "source": "hook"
    },
    {
      "id": "12",
      "project": "Cedar",
      "group": "Parked",
      "purpose": "审查",
      "title": "文档链接检查",
      "state": "turn-done",
      "read": true,
      "detail": "本轮结束 · 已打开查看",
      "source": "hook"
    }
  ]
};
