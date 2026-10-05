import { useState } from "react";
import heroApp from "./assets/hero-app.png";
import { DownloadIcon, GitHubIcon, Logo } from "./components";
import { DMG_SHA256, DMG_URL, LATEST, MANIFEST_URL, REPO, RELEASES, VERSION } from "./site-meta";

const NAV = [
  { href: "#features", label: "特性" },
  { href: "#start", label: "快速上手" },
  { href: "#download", label: "下载" },
  { href: "#faq", label: "FAQ" },
];

function Nav() {
  return (
    <nav
      style={{
        position: "sticky",
        top: 0,
        zIndex: 50,
        backdropFilter: "blur(12px)",
        background: "rgba(10,12,16,.75)",
        borderBottom: "1px solid var(--line)",
      }}
    >
      <div
        style={{
          maxWidth: 1080,
          margin: "0 auto",
          padding: "0 24px",
          height: 60,
          display: "flex",
          alignItems: "center",
          gap: 26,
        }}
      >
        <a href="#top" style={{ display: "flex", alignItems: "center", gap: 10, fontWeight: 650, fontSize: 15 }}>
          <Logo size={24} />
          Crossbow
        </a>
        <div style={{ display: "flex", gap: 22, marginLeft: "auto", alignItems: "center" }}>
          {NAV.map((n) => (
            <a key={n.href} href={n.href} style={{ color: "var(--dim)", fontSize: 13 }}>
              {n.label}
            </a>
          ))}
          <a href={REPO} target="_blank" rel="noopener" style={{ display: "inline-flex", alignItems: "center", gap: 6, color: "var(--dim)", fontSize: 13 }}>
            <GitHubIcon />
            GitHub
          </a>
        </div>
      </div>
    </nav>
  );
}

function Hero() {
  return (
    <header id="top" style={{ padding: "96px 0 56px", textAlign: "center" }}>
      <div style={{ display: "inline-flex", alignItems: "center", gap: 8, font: "12px var(--mono)", color: "var(--dim)", border: "1px solid var(--line-strong)", borderRadius: 99, padding: "5px 14px", marginBottom: 26 }}>
        <span style={{ color: "var(--accent)" }}>{VERSION}</span>
        <span>开源 · GPL-3.0</span>
        <span>macOS · Apple Silicon</span>
      </div>
      <h1 style={{ fontSize: 46, lineHeight: 1.18, fontWeight: 700, letterSpacing: "-.025em", maxWidth: 720, margin: "0 auto 20px" }}>
        轻量、克制、可靠的
        <br />
        <span
          style={{
            background: "linear-gradient(120deg, #7aa2ff 10%, #b79bff 60%, #8bb8ff)",
            WebkitBackgroundClip: "text",
            backgroundClip: "text",
            color: "transparent",
          }}
        >
          开源代理客户端
        </span>
      </h1>
      <p style={{ color: "var(--dim)", fontSize: 17, maxWidth: 560, margin: "0 auto 34px" }}>
        Crossbow 以 <span className="mono">mihomo</span> 为内核，为 macOS
        而生：订阅自动更新、网络诊断向导、JS 覆写、轻量模式——
        把「为什么上不了网」变成一键自检。
      </p>
      <div style={{ display: "flex", gap: 12, justifyContent: "center", alignItems: "center", flexWrap: "wrap" }}>
        <a
          className="btn-primary"
          href={LATEST}
          target="_blank"
          rel="noopener"
          style={{
            display: "inline-flex", alignItems: "center", gap: 8, borderRadius: 10,
            padding: "12px 24px", fontSize: 14.5, fontWeight: 600, color: "#06111f",
            background: "linear-gradient(180deg, #85adff, #6d97ff)",
            boxShadow: "0 4px 20px rgba(122,162,255,.3), inset 0 1px 0 rgba(255,255,255,.35)",
          }}
        >
          <DownloadIcon />
          下载 {VERSION}
        </a>
        <a
          href={REPO}
          target="_blank"
          rel="noopener"
          style={{
            display: "inline-flex", alignItems: "center", gap: 8, borderRadius: 10,
            padding: "12px 24px", fontSize: 14.5, fontWeight: 600,
            background: "var(--surface-2)", color: "var(--text)", border: "1px solid var(--line-strong)",
          }}
        >
          查看源码
        </a>
      </div>
      <div style={{ marginTop: 14, font: "11.5px var(--mono)", color: "var(--faint)" }}>
        免费开源 · 安装包 31 MB · 无追踪 · macOS 11+ Apple Silicon
      </div>

      <div
        style={{
          maxWidth: 980, margin: "56px auto 0", borderRadius: 14,
          border: "1px solid var(--line-strong)",
          boxShadow: "0 30px 80px rgba(0,0,0,.55), 0 0 0 1px rgba(255,255,255,.03)",
          overflow: "hidden",
        }}
      >
        <img src={heroApp} alt="Crossbow 主界面：仪表盘、实时速率曲线与配置档案" style={{ display: "block", width: "100%", height: "auto" }} />
      </div>
    </header>
  );
}

const FEATURES = [
  {
    icon: <path d="M4 5h16v14H4z" />,
    icon2: <path d="M4 10h16M9 5v14" />,
    title: "订阅管理",
    text: "URL / 粘贴 / 文件 / 拖拽 / 深链接导入，流量与到期一目了然；自动更新失败保留旧配置并指数退避重试。",
  },
  {
    icon: <path d="M12 3l8 4v5c0 5-3.5 8-8 9-4.5-1-8-4-8-9V7z" />,
    icon2: <path d="M9 12l2 2 4-4" />,
    title: "系统代理守护",
    text: "开启前快照原设置，退出/崩溃精确还原；内核异常自动降级恢复——告别「退出后断网」。",
  },
  {
    icon: <path d="M3 12h4l2-6 4 12 2-6h6" />,
    title: "网络诊断向导",
    text: "七项按依赖顺序自检：内核、控制器、配置、端口、系统代理指向、代理连通、DNS——失败项一键修复。",
  },
  {
    icon: <path d="M8 9l-4 3 4 3M16 9l4 3-4 3M13 5l-2 14" />,
    title: "JS 覆写",
    text: "用 function main(config) 改造订阅：过滤节点、改写规则。保存即校验，错误当场归因；沙箱执行无副作用。",
  },
  {
    icon: <path d="M13 2L5 13h6l-2 9 8-13h-6z" />,
    title: "轻量模式",
    text: "关闭窗口 = 内核与代理保留、界面内存归零。重新打开自动接管，状态无缝恢复；托盘一键切换。",
  },
  {
    icon: <path d="M2 12h4l2-5 3 10 3-7 2 2h6" />,
    title: "实时观测",
    text: "速率曲线、按进程聚合的流量排行、单连接断开、分级日志——运行状态尽收眼底。",
  },
];

function Features() {
  return (
    <section id="features" style={{ maxWidth: 1080, margin: "0 auto", padding: "84px 24px" }}>
      <div style={{ textAlign: "center", marginBottom: 48 }}>
        <span className="micro" style={{ display: "block", marginBottom: 12 }}>Features</span>
        <h2 style={{ fontSize: 30, fontWeight: 700, letterSpacing: "-.02em" }}>把复杂留给内核，把简单交给你</h2>
        <p style={{ color: "var(--dim)", marginTop: 12, fontSize: 15 }}>每个特性都来自真实的日常使用反馈</p>
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 14 }}>
        {FEATURES.map((f) => (
          <div
            key={f.title}
            style={{
              background: "linear-gradient(180deg, rgba(255,255,255,.018), transparent 40%), var(--surface)",
              border: "1px solid var(--line)", borderRadius: 14, padding: "24px 22px",
            }}
          >
            <div style={{ width: 38, height: 38, borderRadius: 10, marginBottom: 16, display: "grid", placeItems: "center", background: "var(--accent-soft)" }}>
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
                {f.icon}
                {f.icon2}
              </svg>
            </div>
            <h3 style={{ fontSize: 15, fontWeight: 600, marginBottom: 8 }}>{f.title}</h3>
            <p style={{ color: "var(--dim)", fontSize: 13.5 }}>{f.text}</p>
          </div>
        ))}
      </div>
    </section>
  );
}

function Steps() {
  return (
    <section id="start" style={{ maxWidth: 1080, margin: "0 auto", padding: "40px 24px 84px" }}>
      <div style={{ textAlign: "center", marginBottom: 48 }}>
        <span className="micro" style={{ display: "block", marginBottom: 12 }}>Get Started</span>
        <h2 style={{ fontSize: 30, fontWeight: 700, letterSpacing: "-.02em" }}>三步上手</h2>
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 14 }}>
        <div style={{ background: "var(--surface)", border: "1px solid var(--line)", borderRadius: 14, padding: "26px 22px" }}>
          <div style={{ font: "600 13px var(--mono)", color: "var(--accent)", marginBottom: 14 }}>01</div>
          <h3 style={{ fontSize: 15, fontWeight: 600, marginBottom: 8 }}>下载并安装</h3>
          <p style={{ color: "var(--dim)", fontSize: 13.5 }}>
            下载 dmg 拖入「应用程序」。首次打开右键 → 打开，或执行 <code style={{ font: "12px var(--mono)", background: "var(--surface-2)", border: "1px solid var(--line)", borderRadius: 5, padding: "1px 6px" }}>xattr -dr com.apple.quarantine /Applications/Crossbow.app</code>。
          </p>
        </div>
        <div style={{ background: "var(--surface)", border: "1px solid var(--line)", borderRadius: 14, padding: "26px 22px" }}>
          <div style={{ font: "600 13px var(--mono)", color: "var(--accent)", marginBottom: 14 }}>02</div>
          <h3 style={{ fontSize: 15, fontWeight: 600, marginBottom: 8 }}>导入订阅</h3>
          <p style={{ color: "var(--dim)", fontSize: 13.5 }}>
            粘贴订阅链接回车，或直接把 .yaml 配置文件拖进窗口。设置自动更新间隔，流量与到期自动追踪。
          </p>
        </div>
        <div style={{ background: "var(--surface)", border: "1px solid var(--line)", borderRadius: 14, padding: "26px 22px" }}>
          <div style={{ font: "600 13px var(--mono)", color: "var(--accent)", marginBottom: 14 }}>03</div>
          <h3 style={{ fontSize: 15, fontWeight: 600, marginBottom: 8 }}>一键开启</h3>
          <p style={{ color: "var(--dim)", fontSize: 13.5 }}>
            首页点亮开关：内核自动拉起、系统代理即时生效。选个节点，或让诊断向导帮你确认链路。
          </p>
        </div>
      </div>
    </section>
  );
}

function Download() {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    navigator.clipboard?.writeText(DMG_SHA256).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };
  return (
    <section id="download" style={{ maxWidth: 1080, margin: "0 auto", padding: "40px 24px 84px" }}>
      <div style={{ textAlign: "center", marginBottom: 48 }}>
        <span className="micro" style={{ display: "block", marginBottom: 12 }}>Download</span>
        <h2 style={{ fontSize: 30, fontWeight: 700, letterSpacing: "-.02em" }}>下载 Crossbow</h2>
        <p style={{ color: "var(--dim)", marginTop: 12, fontSize: 15 }}>应用内也可一键检查更新（minisign 签名校验）</p>
      </div>
      <div
        style={{
          maxWidth: 720, margin: "0 auto", textAlign: "center", padding: 36,
          background: "linear-gradient(180deg, rgba(255,255,255,.02), transparent 50%), var(--surface)",
          border: "1px solid var(--line-strong)", borderRadius: 16,
        }}
      >
        <div style={{ font: "600 13px var(--mono)", color: "var(--accent)" }}>{VERSION} · 2026-10-05</div>
        <h3 style={{ fontSize: 22, fontWeight: 700, margin: "10px 0 6px" }}>Crossbow for macOS</h3>
        <div style={{ color: "var(--dim)", fontSize: 13 }}>Apple Silicon (M 系列) · macOS 11+ · 安装包 31 MB</div>
        <div style={{ display: "flex", gap: 12, justifyContent: "center", margin: "26px 0 18px", flexWrap: "wrap" }}>
          <a
            href={DMG_URL}
            target="_blank"
            rel="noopener"
            style={{
              display: "inline-flex", alignItems: "center", gap: 8, borderRadius: 10,
              padding: "12px 24px", fontSize: 14.5, fontWeight: 600, color: "#06111f",
              background: "linear-gradient(180deg, #85adff, #6d97ff)",
              boxShadow: "0 4px 20px rgba(122,162,255,.3), inset 0 1px 0 rgba(255,255,255,.35)",
            }}
          >
            <DownloadIcon />
            下载 dmg
          </a>
          <a
            href={RELEASES}
            target="_blank"
            rel="noopener"
            style={{ display: "inline-flex", alignItems: "center", borderRadius: 10, padding: "12px 24px", fontSize: 14.5, fontWeight: 600, background: "var(--surface-2)", color: "var(--text)", border: "1px solid var(--line-strong)" }}
          >
            全部版本
          </a>
        </div>
        <div
          onClick={copy}
          title="点击复制"
          style={{
            font: "11px var(--mono)", color: "var(--faint)", wordBreak: "break-all",
            background: "var(--surface-2)", border: "1px solid var(--line)",
            borderRadius: 8, padding: "8px 12px", display: "inline-block", cursor: "pointer",
          }}
        >
          {copied ? "SHA256 · 已复制 ✓" : `SHA256 · 点击复制：${DMG_SHA256.slice(0, 32)}…`}
        </div>
        <div style={{ marginTop: 16, fontSize: 12.5 }}>
          <a href={RELEASES} target="_blank" rel="noopener" style={{ color: "var(--accent)", margin: "0 8px" }}>历史版本</a>
          {" · "}
          <a href={MANIFEST_URL} target="_blank" rel="noopener" style={{ color: "var(--accent)", margin: "0 8px" }}>更新清单</a>
        </div>
      </div>
    </section>
  );
}

const FAQS = [
  {
    q: "打开提示「无法验证」或「已损坏」？",
    a: <>应用未做 Developer ID 公证。首次打开右键 → <b>打开</b>；或终端执行 <code style={{ font: "12px var(--mono)", background: "var(--surface-2)", border: "1px solid var(--line)", borderRadius: 5, padding: "1px 6px" }}>xattr -dr com.apple.quarantine /Applications/Crossbow.app</code> 后正常打开。</>,
  },
  {
    q: "支持哪些协议？",
    a: <>内核为 mihomo，支持 Shadowsocks / VMess / VLESS / Trojan / Hysteria2 / TUIC / WireGuard 等全部 mihomo 所支持的协议，兼容 Clash 订阅与配置格式。</>,
  },
  {
    q: "支持 TUN 模式吗？",
    a: <>暂缓评估中。当前通过系统代理覆盖浏览器与大部分应用；当你遇到「某应用流量不走代理」的真实需求时我们会重启 TUN 开发。</>,
  },
  {
    q: "有 Windows / iOS 版本吗？",
    a: <>在路线图中：Windows 为 M1.5，iOS 为 M2（复用同一套核心逻辑）。目前 macOS 版本已可日常使用。</>,
  },
  {
    q: "升级会丢配置吗？",
    a: <>不会。配置存储于 ~/Library/Application Support/com.crossbow.app/，每次保存前自动快照（保留 5 份），升级自动继承；坏覆写会触发安全模式兜底。</>,
  },
  {
    q: "如何参与或反馈？",
    a: <>项目开源于 GitHub，Issue 与 PR 欢迎提交。日常使用中的每一条不便都是最高优先级的输入。</>,
  },
];

function Faq() {
  const [open, setOpen] = useState(0);
  return (
    <section id="faq" style={{ maxWidth: 1080, margin: "0 auto", padding: "40px 24px 84px" }}>
      <div style={{ textAlign: "center", marginBottom: 48 }}>
        <span className="micro" style={{ display: "block", marginBottom: 12 }}>FAQ</span>
        <h2 style={{ fontSize: 30, fontWeight: 700, letterSpacing: "-.02em" }}>常见问题</h2>
      </div>
      <div style={{ maxWidth: 720, margin: "0 auto" }}>
        {FAQS.map((f, i) => (
          <details
            key={i}
            open={open === i}
            onToggle={() => setOpen(i)}
            style={{ background: "var(--surface)", border: "1px solid var(--line)", borderRadius: 12, padding: "0 20px", marginBottom: 10 }}
          >
            <summary
              style={{
                cursor: "pointer", listStyle: "none", padding: "16px 0",
                fontSize: 14.5, fontWeight: 550,
                display: "flex", justifyContent: "space-between", alignItems: "center",
              }}
            >
              {f.q}
              <span style={{ color: "var(--faint)", fontSize: 16 }}>{open === i ? "−" : "+"}</span>
            </summary>
            <div style={{ color: "var(--dim)", fontSize: 13.5, paddingBottom: 18 }}>{f.a}</div>
          </details>
        ))}
      </div>
    </section>
  );
}

function Footer() {
  return (
    <footer style={{ borderTop: "1px solid var(--line)", padding: "40px 0 48px", marginTop: 40 }}>
      <div style={{ maxWidth: 1080, margin: "0 auto", padding: "0 24px" }}>
        <div style={{ display: "flex", alignItems: "flex-start", gap: 24, flexWrap: "wrap" }}>
          <div style={{ display: "flex", alignItems: "center", gap: 9, fontWeight: 600, fontSize: 13.5 }}>
            <Logo size={20} />
            Crossbow
          </div>
          <div style={{ display: "flex", gap: 48, marginLeft: "auto" }}>
            <div>
              <span className="micro" style={{ display: "block", marginBottom: 10 }}>项目</span>
              <a href={REPO} target="_blank" rel="noopener" style={{ display: "block", color: "var(--dim)", fontSize: 13, marginBottom: 8 }}>GitHub 仓库</a>
              <a href={RELEASES} target="_blank" rel="noopener" style={{ display: "block", color: "var(--dim)", fontSize: 13, marginBottom: 8 }}>Releases</a>
              <a href={`${REPO}/issues`} target="_blank" rel="noopener" style={{ display: "block", color: "var(--dim)", fontSize: 13, marginBottom: 8 }}>反馈 Issue</a>
            </div>
            <div>
              <span className="micro" style={{ display: "block", marginBottom: 10 }}>资源</span>
              <a href="#features" style={{ display: "block", color: "var(--dim)", fontSize: 13, marginBottom: 8 }}>特性</a>
              <a href="#download" style={{ display: "block", color: "var(--dim)", fontSize: 13, marginBottom: 8 }}>下载</a>
              <a href="#faq" style={{ display: "block", color: "var(--dim)", fontSize: 13, marginBottom: 8 }}>FAQ</a>
            </div>
            <div>
              <span className="micro" style={{ display: "block", marginBottom: 10 }}>协议</span>
              <a href={`${REPO}/blob/master/LICENSE`} target="_blank" rel="noopener" style={{ display: "block", color: "var(--dim)", fontSize: 13, marginBottom: 8 }}>GPL-3.0 License</a>
            </div>
          </div>
        </div>
        <div
          style={{
            marginTop: 28, paddingTop: 20, borderTop: "1px solid var(--line)",
            color: "var(--faint)", fontSize: 11.5, lineHeight: 1.7,
          }}
        >
          Crossbow 基于 GPL-3.0 协议开源，仅供学习与研究使用，请遵守您所在地区的法律法规，勿用于任何非法用途。
          本项目与 mihomo 项目相互独立；mihomo 版权归其原作者所有。
          <br />
          © 2026 Crossbow Contributors
        </div>
      </div>
    </footer>
  );
}

export default function App() {
  return (
    <>
      <Nav />
      <Hero />
      <Features />
      <Steps />
      <Download />
      <Faq />
      <Footer />
    </>
  );
}
