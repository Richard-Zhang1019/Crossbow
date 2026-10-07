import { useEffect } from "react";
import { Link, Route, Routes, useLocation } from "react-router-dom";
import { DownloadIcon, GitHubIcon, Logo } from "./components";
import { REPO, RELEASES } from "./site-meta";
import HomePage from "./pages/HomePage";
import FeaturesPage from "./pages/FeaturesPage";
import GuidePage from "./pages/GuidePage";

const NAV = [
  { to: "/", label: "首页" },
  { to: "/features", label: "功能详解" },
  { to: "/guide", label: "进阶指南" },
];

function ScrollToTop() {
  const { pathname } = useLocation();
  useEffect(() => {
    window.scrollTo(0, 0);
  }, [pathname]);
  return null;
}

function Nav() {
  const { pathname } = useLocation();
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
        <Link to="/" style={{ display: "flex", alignItems: "center", gap: 10, fontWeight: 650, fontSize: 15 }}>
          <Logo size={24} />
          Crossbow
        </Link>
        <div style={{ display: "flex", gap: 22, marginLeft: "auto", alignItems: "center" }}>
          {NAV.map((n) => {
            const on = pathname === n.to;
            return (
              <Link
                key={n.to}
                to={n.to}
                style={{
                  color: on ? "var(--text)" : "var(--dim)",
                  fontSize: 13,
                  fontWeight: on ? 600 : 400,
                }}
              >
                {n.label}
              </Link>
            );
          })}
          <a href={REPO} target="_blank" rel="noopener" style={{ display: "inline-flex", alignItems: "center", gap: 6, color: "var(--dim)", fontSize: 13 }}>
            <GitHubIcon />
            GitHub
          </a>
          <a
            href={RELEASES}
            target="_blank"
            rel="noopener"
            style={{
              display: "inline-flex", alignItems: "center", gap: 6, borderRadius: 8,
              padding: "6px 14px", fontSize: 12.5, fontWeight: 600, color: "#06111f",
              background: "linear-gradient(180deg, #85adff, #6d97ff)",
            }}
          >
            <DownloadIcon />
            下载
          </a>
        </div>
      </div>
    </nav>
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
              <span className="micro" style={{ display: "block", marginBottom: 10 }}>页面</span>
              <Link to="/" style={{ display: "block", color: "var(--dim)", fontSize: 13, marginBottom: 8 }}>首页</Link>
              <Link to="/features" style={{ display: "block", color: "var(--dim)", fontSize: 13, marginBottom: 8 }}>功能详解</Link>
              <Link to="/guide" style={{ display: "block", color: "var(--dim)", fontSize: 13, marginBottom: 8 }}>进阶指南</Link>
            </div>
            <div>
              <span className="micro" style={{ display: "block", marginBottom: 10 }}>项目</span>
              <a href={REPO} target="_blank" rel="noopener" style={{ display: "block", color: "var(--dim)", fontSize: 13, marginBottom: 8 }}>GitHub 仓库</a>
              <a href={RELEASES} target="_blank" rel="noopener" style={{ display: "block", color: "var(--dim)", fontSize: 13, marginBottom: 8 }}>Releases</a>
              <a href={`${REPO}/issues`} target="_blank" rel="noopener" style={{ display: "block", color: "var(--dim)", fontSize: 13, marginBottom: 8 }}>反馈 Issue</a>
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
      <ScrollToTop />
      <Nav />
      <Routes>
        <Route path="/" element={<HomePage />} />
        <Route path="/features" element={<FeaturesPage />} />
        <Route path="/guide" element={<GuidePage />} />
        <Route path="*" element={<HomePage />} />
      </Routes>
      <Footer />
    </>
  );
}
