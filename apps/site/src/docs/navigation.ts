// One navigation source; each document owns a real Astro/Markdown route file.
export type DocLang = "zh" | "en";
export const groups = [{"id": "start", "zh": "开始使用", "en": "Getting started"}, {"id": "routing", "zh": "配置与分流", "en": "Configuration & routing"}, {"id": "network", "zh": "连接与网络", "en": "Connection & network"}, {"id": "observe", "zh": "观测与排障", "en": "Observability & diagnosis"}, {"id": "maintenance", "zh": "维护与恢复", "en": "Maintenance & recovery"}] as const;
export const documents = [
  {
    "group": "start",
    "slug": "quick-start",
    "title": {
      "zh": "快速开始",
      "en": "Quick start"
    },
    "description": {
      "zh": "从安装到验证第一次连接。",
      "en": "Install, import and verify your first connection."
    }
  },
  {
    "group": "start",
    "slug": "subscriptions",
    "title": {
      "zh": "订阅与配置切换",
      "en": "Subscriptions & profiles"
    },
    "description": {
      "zh": "更新订阅，查看节点变化与失败原因。",
      "en": "Update subscriptions and inspect changes or failures."
    }
  },
  {
    "group": "start",
    "slug": "local-import",
    "title": {
      "zh": "分享链接与本地导入",
      "en": "Local & share-link import"
    },
    "description": {
      "zh": "导入链接、Base64内容并检查内核兼容性。",
      "en": "Import links and Base64 content with engine checks."
    }
  },
  {
    "group": "start",
    "slug": "deeplink",
    "title": {
      "zh": "从浏览器导入",
      "en": "Browser import links"
    },
    "description": {
      "zh": "使用 crossbow:// 链接导入订阅。",
      "en": "Import subscriptions using crossbow:// links."
    }
  },
  {
    "group": "start",
    "slug": "linux",
    "title": {
      "zh": "Linux 安装与支持范围",
      "en": "Linux support"
    },
    "description": {
      "zh": "实验版安装与 TUN 权限边界。",
      "en": "Experimental packages and TUN permissions."
    }
  },
  {
    "group": "start",
    "slug": "windows",
    "title": {
      "zh": "Windows 安装与服务",
      "en": "Windows installation"
    },
    "description": {
      "zh": "安装器、TUN服务与升级修复。",
      "en": "Installer, TUN service and upgrade recovery."
    }
  },
  {
    "group": "routing",
    "slug": "nodes",
    "title": {
      "zh": "节点选择与测速",
      "en": "Nodes & latency"
    },
    "description": {
      "zh": "选择出口，区分自动选择与手动固定。",
      "en": "Choose an outbound and understand automatic selection."
    }
  },
  {
    "group": "routing",
    "slug": "custom-groups",
    "title": {
      "zh": "自定义策略组",
      "en": "Custom policy groups"
    },
    "description": {
      "zh": "独立管理节点成员和自动测试。",
      "en": "Manage independent node groups and URL tests."
    }
  },
  {
    "group": "routing",
    "slug": "user-rules",
    "title": {
      "zh": "域名与 IP 规则",
      "en": "Domain & IP rules"
    },
    "description": {
      "zh": "为网站或网段添加有序分流规则。",
      "en": "Create ordered routing rules for domains and networks."
    }
  },
  {
    "group": "routing",
    "slug": "process-rules",
    "title": {
      "zh": "进程分流规则",
      "en": "Process rules"
    },
    "description": {
      "zh": "按应用选择直连、拦截或代理。",
      "en": "Choose direct, block or a policy for each application."
    }
  },
  {
    "group": "routing",
    "slug": "rule-sets",
    "title": {
      "zh": "远程规则集",
      "en": "Remote rule sets"
    },
    "description": {
      "zh": "下载、缓存与更新独立规则列表。",
      "en": "Download, cache and update routing lists."
    }
  },
  {
    "group": "routing",
    "slug": "override",
    "title": {
      "zh": "脚本与 Merge 覆写",
      "en": "Scripts & merge overrides"
    },
    "description": {
      "zh": "在订阅基础上调整配置并绑定覆写链。",
      "en": "Apply scripts or merge patches to a subscription."
    }
  },
  {
    "group": "network",
    "slug": "proxy-vs-tun",
    "title": {
      "zh": "系统代理与 TUN",
      "en": "System proxy & TUN"
    },
    "description": {
      "zh": "按应用与流量范围选择接管方式。",
      "en": "Choose how applications reach the proxy."
    }
  },
  {
    "group": "network",
    "slug": "engines",
    "title": {
      "zh": "双内核与兼容性",
      "en": "Engines & compatibility"
    },
    "description": {
      "zh": "选择 mihomo 或 sing-box，保留配置语义。",
      "en": "Choose mihomo or sing-box without losing config meaning."
    }
  },
  {
    "group": "network",
    "slug": "dns",
    "title": {
      "zh": "DNS 设置",
      "en": "DNS settings"
    },
    "description": {
      "zh": "配置代理、直连及节点引导解析。",
      "en": "Configure proxy, direct and bootstrap resolution."
    }
  },
  {
    "group": "network",
    "slug": "network-scenes",
    "title": {
      "zh": "Wi-Fi 网络场景",
      "en": "Wi-Fi network scenes"
    },
    "description": {
      "zh": "按网络名称自动切换接管行为。",
      "en": "Change takeover behavior when joining a network."
    }
  },
  {
    "group": "network",
    "slug": "lan-sharing",
    "title": {
      "zh": "局域网代理共享",
      "en": "LAN proxy sharing"
    },
    "description": {
      "zh": "让同一网络的其他设备使用本机代理。",
      "en": "Use this computer’s proxy from another device."
    }
  },
  {
    "group": "network",
    "slug": "ports",
    "title": {
      "zh": "监听端口",
      "en": "Listening port"
    },
    "description": {
      "zh": "修改本地代理端口并应用。",
      "en": "Change and apply the local mixed proxy port."
    }
  },
  {
    "group": "observe",
    "slug": "connections",
    "title": {
      "zh": "连接与进程观测",
      "en": "Connections & processes"
    },
    "description": {
      "zh": "查看命中规则、出口链和连接详情。",
      "en": "Inspect matched rules, outbound chains and processes."
    }
  },
  {
    "group": "observe",
    "slug": "statistics",
    "title": {
      "zh": "本地流量统计",
      "en": "Local traffic statistics"
    },
    "description": {
      "zh": "按日期、域名和节点查看已观测流量。",
      "en": "Review observed traffic by date, domain and node."
    }
  },
  {
    "group": "observe",
    "slug": "node-quality",
    "title": {
      "zh": "节点质量历史",
      "en": "Node quality history"
    },
    "description": {
      "zh": "比较目标、成功率、延迟和抖动。",
      "en": "Compare target-specific success, latency and jitter."
    }
  },
  {
    "group": "observe",
    "slug": "node-tests",
    "title": {
      "zh": "连通性与有界吞吐",
      "en": "Connectivity & throughput"
    },
    "description": {
      "zh": "按需运行 TCP、HTTPS、UDP 与限量传输。",
      "en": "Run explicit TCP, HTTPS, UDP and bounded transfers."
    }
  },
  {
    "group": "observe",
    "slug": "troubleshooting",
    "title": {
      "zh": "网络诊断与日志",
      "en": "Diagnostics & logs"
    },
    "description": {
      "zh": "按依赖顺序定位问题并整理反馈证据。",
      "en": "Locate blockers and collect useful diagnostic evidence."
    }
  },
  {
    "group": "observe",
    "slug": "dns-diagnostics",
    "title": {
      "zh": "DNS 解析路径诊断",
      "en": "DNS path diagnostics"
    },
    "description": {
      "zh": "对比系统解析和内核查询结果。",
      "en": "Compare system resolution with the core DNS interface."
    }
  },
  {
    "group": "maintenance",
    "slug": "migration",
    "title": {
      "zh": "配置包导入与迁移",
      "en": "Portable configuration"
    },
    "description": {
      "zh": "预览并迁移配置及个人策略。",
      "en": "Preview and migrate profiles and personal policies."
    }
  },
  {
    "group": "maintenance",
    "slug": "recovery",
    "title": {
      "zh": "配置快照与恢复",
      "en": "Snapshots & recovery"
    },
    "description": {
      "zh": "恢复保存快照并检查实际生效状态。",
      "en": "Restore a saved snapshot and verify the active state."
    }
  },
  {
    "group": "maintenance",
    "slug": "updates",
    "title": {
      "zh": "软件更新",
      "en": "Software updates"
    },
    "description": {
      "zh": "检查版本、下载和重启安装。",
      "en": "Check releases, download and install updates."
    }
  },
  {
    "group": "maintenance",
    "slug": "notifications",
    "title": {
      "zh": "通知与操作结果",
      "en": "Notifications"
    },
    "description": {
      "zh": "理解通知、后台结果和失败详情。",
      "en": "Understand notifications and action results."
    }
  },
  {
    "group": "maintenance",
    "slug": "lightweight",
    "title": {
      "zh": "轻量托盘模式",
      "en": "Lightweight tray mode"
    },
    "description": {
      "zh": "关闭窗口后继续运行并重新打开。",
      "en": "Keep running after closing the main window."
    }
  },
  {
    "group": "maintenance",
    "slug": "tray-speed",
    "title": {
      "zh": "托盘实时速率",
      "en": "Tray traffic speed"
    },
    "description": {
      "zh": "在托盘查看当前网络速率。",
      "en": "See current traffic speed in the tray."
    }
  },
  {
    "group": "maintenance",
    "slug": "data-locations",
    "title": {
      "zh": "数据与日志位置",
      "en": "Data & log locations"
    },
    "description": {
      "zh": "查找本地数据、内核文件和日志。",
      "en": "Find local data, binaries and logs."
    }
  }
] as const;
export const overview = {slug:"", group:"start", title:{zh:"使用文档",en:"Documentation"}, description:{zh:"从第一次连接到日常分流、诊断与维护，找到你现在需要的操作。",en:"Find your next step, from the first connection to routing, diagnosis and maintenance."}};
export function docHref(lang:DocLang, slug="", base="") { return `${base.replace(/\/$/,"")}${lang==="zh"?"/zh":""}/guide${slug?"/"+slug:""}`; }
export const legacySections = ["quick-start", "deeplink", "proxy-vs-tun", "engines", "override", "ports", "lightweight", "subscriptions", "recovery", "data-locations", "tray-speed", "notifications", "linux", "windows", "troubleshooting", "updates"] as const;
