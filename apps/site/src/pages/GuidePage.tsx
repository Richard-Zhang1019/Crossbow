import { DownloadIcon } from "../components";
import { LATEST, REPO } from "../site-meta";

const CODE_FILTER = `function main(config) {
  // 只保留名字含「香港」或「JP」的节点
  config.proxies = config.proxies.filter(p =>
    /香港|JP/.test(p.name)
  );

  // 规则：流媒体走「自动选择」，其余 MATCH 兜底
  config.rules = [
    "DOMAIN-SUFFIX,netflix.com,自动选择",
    "GEOIP,CN,DIRECT",
    "MATCH,自动选择",
  ];
  return config;
}`;

const CODE_DEEPLINK = `crossbow://import?url=https%3A%2F%2Fexample.com%2Fsub`;

function H2({ children }: { children: React.ReactNode }) {
  return (
    <h2 style={{ fontSize: 24, fontWeight: 700, letterSpacing: "-.01em", margin: "0 0 8px" }}>{children}</h2>
  );
}

function Card({ children }: { children: React.ReactNode }) {
  return (
    <section
      style={{
        background: "linear-gradient(180deg, rgba(255,255,255,.015), transparent 45%), var(--surface)",
        border: "1px solid var(--line)", borderRadius: 16, padding: "30px 32px",
      }}
    >
      {children}
    </section>
  );
}

function Step({ n, title, children }: { n: string; title: string; children: React.ReactNode }) {
  return (
    <div style={{ display: "flex", gap: 16, marginTop: 18 }}>
      <div style={{ font: "600 12px var(--mono)", color: "var(--accent)", flex: "none", paddingTop: 2 }}>{n}</div>
      <div>
        <div style={{ fontSize: 14.5, fontWeight: 600, marginBottom: 6 }}>{title}</div>
        <div style={{ color: "var(--dim)", fontSize: 13.5, lineHeight: 1.75 }}>{children}</div>
      </div>
    </div>
  );
}

function Code({ children }: { children: string }) {
  return (
    <pre
      style={{
        font: "12px/1.7 var(--mono)", background: "var(--surface-2)",
        border: "1px solid var(--line)", borderRadius: 10,
        padding: "14px 16px", overflowX: "auto", margin: "12px 0 4px",
        color: "var(--text)",
      }}
    >
      {children}
    </pre>
  );
}

export default function GuidePage() {
  return (
    <div style={{ maxWidth: 860, margin: "0 auto", padding: "64px 24px 84px" }}>
      <div style={{ textAlign: "center", marginBottom: 56 }}>
        <span className="micro" style={{ display: "block", marginBottom: 12 }}>Advanced Guide</span>
        <h2 style={{ fontSize: 30, fontWeight: 700, letterSpacing: "-.02em" }}>进阶指南</h2>
        <p style={{ color: "var(--dim)", marginTop: 12, fontSize: 15, maxWidth: 620, margin: "12px auto 0", lineHeight: 1.7 }}>
          基础使用三步就能跑通（见首页）；这一页写给想把 Crossbow 用到极致的你。
        </p>
      </div>

      <div style={{ display: "flex", flexDirection: "column", gap: 22 }}>
        <Card>
          <H2>双内核引擎：mihomo ↔ sing-box</H2>
          <p style={{ color: "var(--dim)", fontSize: 13.5, lineHeight: 1.75, margin: "10px 0 4px" }}>
            Crossbow 同时支持两颗内核。mihomo 与 Clash 生态完全兼容、规则语法最全；sing-box 新协议（如
            Reality、Hysteria2 的最新实现）跟进更快。配置会按引擎自动转换格式，切换即热重启。
          </p>
          <Step n="01" title="下载内核">
            设置 → 内核引擎 → 选择 sing-box → 点「下载内核」（约 20MB，内置镜像加速；已下载的内核不会重复下载）。
          </Step>
          <Step n="02" title="一键切换">
            下载完成后点「启用」。切换会热重启内核：订阅按引擎自动转换格式（sing-box 输出 JSON 并注入
            Clash 兼容 API），托盘速率、连接观测无缝衔接。
          </Step>
          <Step n="03" title="注意">
            切换前确认当前订阅的节点类型被目标内核支持（绝大多数常见协议两者都支持；个别小众协议建议留在
            mihomo）。规则集（geoip/geosite）在 sing-box 下会自动下载并每日自更新。
          </Step>
        </Card>

        <Card>
          <H2>JS 覆写：把订阅改成你想要的样子</H2>
          <p style={{ color: "var(--dim)", fontSize: 13.5, lineHeight: 1.75, margin: "10px 0 4px" }}>
            配置页 → 覆写组 → 新建「JS 脚本」，编写 <code style={{ font: "12px var(--mono)" }}>function main(config)</code>，
            返回改造后的配置。沙箱执行、无网络无文件副作用，保存时自动试运行。
          </p>
          <Code>{CODE_FILTER}</Code>
          <Step n="01" title="编写并绑定">
            新建脚本 → 粘贴代码 → 保存（自动校验，语法/运行时错误当场报）→ 勾选「绑定到当前订阅」并启用。
          </Step>
          <Step n="02" title="多条覆写按序执行">
            可同时启用多条覆写与 Merge 补丁，按列表顺序依次应用到订阅上。
          </Step>
          <Step n="03" title="出错兜底">
            覆写导致内核启动失败时会自动进入「安全模式」：全部覆写临时禁用并重启内核。修复脚本后保存即恢复正常。
          </Step>
        </Card>

        <Card>
          <H2>深链接导入：从任意页面一键订阅</H2>
          <p style={{ color: "var(--dim)", fontSize: 13.5, lineHeight: 1.75, margin: "10px 0 4px" }}>
            网页上放一个这样的链接，已安装 Crossbow 的用户点击即弹出导入确认：
          </p>
          <Code>{CODE_DEEPLINK}</Code>
          <p style={{ color: "var(--dim)", fontSize: 13, lineHeight: 1.7 }}>
            url 参数需要 URL 编码。未安装的用户可回落到官网下载页——适合机场/自建服务者在文档里引用。
          </p>
        </Card>

        <Card>
          <H2>菜单栏实时速率</H2>
          <p style={{ color: "var(--dim)", fontSize: 13.5, lineHeight: 1.75, margin: "10px 0 4px" }}>
            内核运行时，菜单栏图标旁实时显示 ↑ 上行 / ↓ 下行（每秒刷新，双行紧凑排版，宽度恒定不抖动）。
            轻量模式下照常工作；内核停止自动消失。该功能目前仅 macOS 提供。
          </p>
        </Card>

        <Card>
          <H2>混合端口与绕过</H2>
          <p style={{ color: "var(--dim)", fontSize: 13.5, lineHeight: 1.75, margin: "10px 0 4px" }}>
            混合监听端口（HTTP + SOCKS5 共用）默认 7897，可在设置中修改——修改后内核运行中会自动热重启。
            绕过列表内置本机与局域网直连（127.0.0.1 / localhost / 内网段 / *.local）。
          </p>
          <p style={{ color: "var(--dim)", fontSize: 13, lineHeight: 1.7 }}>
            提示：若 7897 被其他代理工具占用（常见于 Clash Verge 等），开启时会明确报错——换一个端口即可。
          </p>
        </Card>

        <Card>
          <H2>轻量模式与托盘</H2>
          <p style={{ color: "var(--dim)", fontSize: 13.5, lineHeight: 1.75, margin: "10px 0 4px" }}>
            设置 → 轻量模式开启后：点关闭窗口 = 代理照常、界面内存释放、托盘常驻。再点托盘图标即恢复窗口。
            彻底退出请走托盘菜单「退出」。内核运行时关闭窗口，内核会以交接模式保留，重新打开自动接管。
          </p>
        </Card>

        <Card>
          <H2>配置与数据的位置</H2>
          <p style={{ color: "var(--dim)", fontSize: 13.5, lineHeight: 1.75, margin: "10px 0 4px" }}>
            全部数据在 <code style={{ font: "12px var(--mono)" }}>~/Library/Application Support/com.crossbow.app/</code>：
            store.json（订阅/设置/覆写，保存前自动快照保留 5 份）、runtime/（内核运行时与日志）、binaries/（内核本体）。
            备份这个目录 = 备份一切。
          </p>
        </Card>
      </div>

      <div
        style={{
          marginTop: 48, textAlign: "center", padding: "32px 24px",
          background: "var(--surface)", border: "1px solid var(--line)", borderRadius: 16,
        }}
      >
        <h3 style={{ fontSize: 18, fontWeight: 700 }}>还没安装？</h3>
        <p style={{ color: "var(--dim)", fontSize: 13.5, margin: "8px auto 20px" }}>免费开源，macOS Apple Silicon 一键下载。</p>
        <a
          href={LATEST}
          target="_blank"
          rel="noopener"
          className="btn-primary"
          style={{
            display: "inline-flex", alignItems: "center", gap: 8, borderRadius: 10,
            padding: "11px 22px", fontSize: 14, fontWeight: 600, color: "#06111f",
            background: "linear-gradient(180deg, #85adff, #6d97ff)",
          }}
        >
          <DownloadIcon />
          下载 Crossbow
        </a>
        {" "}
        <a
          href={`${REPO}/blob/master/docs`}
          target="_blank"
          rel="noopener"
          style={{ marginLeft: 10, color: "var(--dim)", fontSize: 13.5 }}
        >
          设计文档（DESIGN / MVP）
        </a>
      </div>
    </div>
  );
}
