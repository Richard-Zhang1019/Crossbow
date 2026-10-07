import { DownloadIcon, Logo } from "../components";
import { LATEST, REPO, VERSION } from "../site-meta";
import heroApp from "../assets/hero-app.png";

const FEATURES = [
  {
    tag: "订阅",
    title: "订阅管理",
    summary:
      "URL / 粘贴 / 文件 / 拖拽 / 深链接五种方式导入；流量与到期自动追踪；自动更新失败保留旧配置并指数退避重试。",
    detail: [
      "导入即校验：格式不合法当场报错，不会写坏现有配置。",
      "订阅卡片直观展示剩余流量与到期时间，即将到期高亮提醒。",
      "自动更新间隔可按订阅独立设置（默认 6 小时）；更新失败不会清空节点，旧配置继续可用。",
      "支持 crossbow://import?url=… 深链接，从任意分享页一键导入。",
    ],
  },
  {
    tag: "守护",
    title: "系统代理守护",
    summary:
      "开启前快照原设置，退出/崩溃精确还原；内核异常自动降级恢复——告别「退出后断网」。",
    detail: [
      "开启代理前对每个网络服务的 HTTP/HTTPS/SOCKS 原设置拍照存档（journal）。",
      "正常退出、崩溃、甚至进程被强杀，下次启动都会先执行一次全量还原，不留脏设置。",
      "内核崩溃时立即还原系统代理并提示；覆写导致的配置错误自动进入安全模式（禁用覆写重启一次）。",
      "混合端口被其他程序占用时，启动前即明确报错，不让你对着「启动中」干等。",
    ],
  },
  {
    tag: "诊断",
    title: "网络诊断向导",
    summary:
      "七项按依赖顺序自检：内核、控制器、配置、端口、系统代理指向、代理连通、DNS——失败项一键修复。",
    detail: [
      "逐层排查：先内核是否起来，再控制器是否就绪，然后配置是否合法、端口是否监听、系统代理是否指向本应用、代理是否真的能出网、DNS 是否可用。",
      "每一步都有结论与建议动作，常见问题（端口占用、覆写出错、订阅过期）可直接一键修复。",
      "诊断过程与结论持久化，方便反馈问题时附带。",
    ],
  },
  {
    tag: "覆写",
    title: "JS 覆写",
    summary:
      "用 function main(config) 改造订阅：过滤节点、改写规则、注入自定义配置。保存即校验，错误当场归因。",
    detail: [
      "内嵌 JS 引擎沙箱执行，无网络无文件副作用，只对订阅配置做纯函数变换。",
      "支持「脚本」与「Merge 补丁」两种覆写类型；可同时启用多条并绑定到指定订阅。",
      "保存时自动试运行：语法错误、运行时异常当场报错并定位到行。",
      "覆写出错不会拖垮内核：自动进入安全模式禁用全部覆写，修复后保存即恢复。",
    ],
  },
  {
    tag: "轻量",
    title: "轻量模式",
    summary:
      "关闭窗口 = 内核与代理保留、界面内存归零。重新打开自动接管，状态无缝恢复；托盘一键切换。",
    detail: [
      "轻量交接：GUI 进程只保留托盘与内核管理，webview 内存完全释放。",
      "内核以 detach 模式留给系统收养；重新打开应用自动收养并恢复全部状态。",
      "菜单栏托盘实时速率在轻量模式下照常跳动。",
    ],
  },
  {
    tag: "观测",
    title: "实时观测",
    summary:
      "菜单栏实时速率、速率曲线、按进程聚合的流量排行、单连接断开、分级日志——运行状态尽收眼底。",
    detail: [
      "菜单栏双行实时速率（上行/下行），数字恒宽显示不抖动。",
      "首页实时速率曲线 + 累计流量；当前出口节点与延迟一目了然。",
      "连接页按进程聚合流量排行，可单独断开任意连接。",
      "日志页分级过滤，排查规则命中问题不用翻配置。",
    ],
  },
  {
    tag: "引擎",
    title: "双内核引擎",
    summary:
      "mihomo 生态兼容性最佳，sing-box 新协议跟进快——应用内一键下载、切换即用。",
    detail: [
      "设置页一键下载对应内核（约 20MB，支持镜像加速），切换需内核已安装。",
      "切换即热重启：配置自动按引擎格式转换（sing-box 输出 JSON 并注入 Clash 兼容 API）。",
      "sing-box 专属：规则集（geoip/geosite）自动下载、本地缓存并每日自更新。",
      "切换后托盘速率、连接观测等全部功能无缝衔接。",
    ],
  },
];

function SectionHead({ micro, title, desc }: { micro: string; title: string; desc?: string }) {
  return (
    <div style={{ textAlign: "center", marginBottom: 56 }}>
      <span className="micro" style={{ display: "block", marginBottom: 12 }}>{micro}</span>
      <h2 style={{ fontSize: 30, fontWeight: 700, letterSpacing: "-.02em" }}>{title}</h2>
      {desc && <p style={{ color: "var(--dim)", marginTop: 12, fontSize: 15, maxWidth: 640, margin: "12px auto 0" }}>{desc}</p>}
    </div>
  );
}

export default function FeaturesPage() {
  return (
    <div style={{ maxWidth: 1080, margin: "0 auto", padding: "64px 24px 84px" }}>
      <SectionHead
        micro="Features"
        title="功能详解"
        desc="每个功能都对着一个真实存在的问题：断网、坏配置、无法排查、内存占用。这里把它们一次讲透。"
      />
      <div style={{ display: "flex", flexDirection: "column", gap: 18 }}>
        {FEATURES.map((f, i) => (
          <section
            key={f.title}
            style={{
              background: "linear-gradient(180deg, rgba(255,255,255,.015), transparent 45%), var(--surface)",
              border: "1px solid var(--line)", borderRadius: 16, padding: "30px 32px",
            }}
          >
            <div style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 6 }}>
              <span
                style={{
                  font: "600 11px var(--mono)", color: "var(--accent)",
                  background: "var(--accent-soft)", borderRadius: 6, padding: "3px 9px",
                }}
              >
                {String(i + 1).padStart(2, "0")} · {f.tag}
              </span>
              <h3 style={{ fontSize: 18, fontWeight: 650, letterSpacing: "-.01em" }}>{f.title}</h3>
            </div>
            <p style={{ color: "var(--text)", fontSize: 14, margin: "10px 0 16px", lineHeight: 1.7 }}>{f.summary}</p>
            <ul style={{ margin: 0, padding: 0, listStyle: "none", display: "grid", gap: 9 }}>
              {f.detail.map((d) => (
                <li key={d} style={{ display: "flex", gap: 10, color: "var(--dim)", fontSize: 13.5, lineHeight: 1.65 }}>
                  <span style={{ color: "var(--accent)", flex: "none", paddingTop: 1 }}>▸</span>
                  {d}
                </li>
              ))}
            </ul>
          </section>
        ))}
      </div>

      <div
        style={{
          marginTop: 56, textAlign: "center", padding: "36px 24px",
          background: "var(--surface)", border: "1px solid var(--line)", borderRadius: 16,
        }}
      >
        <h3 style={{ fontSize: 20, fontWeight: 700, letterSpacing: "-.01em" }}>还有更多进阶玩法</h3>
        <p style={{ color: "var(--dim)", fontSize: 14, margin: "10px auto 22px", maxWidth: 520, lineHeight: 1.7 }}>
          JS 覆写教程、双引擎切换实操、深链接导入、安全模式机制——见进阶指南。
        </p>
        <a
          href="#/guide"
          className="btn-primary"
          style={{
            display: "inline-flex", alignItems: "center", gap: 8, borderRadius: 10,
            padding: "11px 22px", fontSize: 14, fontWeight: 600, color: "#06111f",
            background: "linear-gradient(180deg, #85adff, #6d97ff)",
          }}
        >
          <DownloadIcon />
          查看进阶指南
        </a>
      </div>

      <div style={{ textAlign: "center", marginTop: 24, fontSize: 12.5, color: "var(--faint)", display: "flex", alignItems: "center", justifyContent: "center", gap: 8 }}>
        <Logo size={16} />
        Crossbow {VERSION} · 开源 · GPL-3.0 ·{" "}
        <a href={`${REPO}/issues`} target="_blank" rel="noopener" style={{ color: "var(--dim)" }}>功能建议欢迎提 Issue</a>
        · 或直接
        <a href={LATEST} target="_blank" rel="noopener" style={{ color: "var(--dim)" }}>下载体验</a>
      </div>
    </div>
  );
}
