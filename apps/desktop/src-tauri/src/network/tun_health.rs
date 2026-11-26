//! TUN 健康看门狗的故障分层（#661/#662）。
//!
//! 看门狗的隧道探测（不经代理直连境外目标）在 TUN 开启时测的是「TUN 加
//! 当前出口节点」整条链路：栈失效、节点不通、系统没有网络，结果都是失败。
//! 把三者混为一谈会误动作——睡眠暗唤醒时系统没有网络，看门狗连续两次
//! 重启内核后关闭了 TUN；节点抖动时同样会重启内核。
//!
//! 分层的依据是三条互不相同的通道：
//! - 隧道：看门狗现有探测，流量被 TUN 捕获。
//! - 物理：经混合端口访问国内目标，按规则走 DIRECT 出站。内核自身的出站
//!   绑定物理网卡、不经过 TUN，这条通道因此绕开了隧道。
//! - 节点：经混合端口访问境外目标，走当前出口节点。
//!
//! 隧道失败时用后两条通道定位故障层，只有「物理与节点都可达、唯独隧道
//! 不通」才是 TUN 自身的问题。
//!
//! 仅 macOS 启用：实证场景都在 macOS，Windows 的 TUN 由服务托管，栈与
//! 路由行为不同，待有实测案例再扩展（与 #331 的范围划分一致）。

/// 对照探测的单请求超时，与看门狗隧道探测同量级。
const COMPARISON_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// 国内目标：经混合端口时按规则走 DIRECT 出站，代表物理网络。
/// 与故障取证（fault_evidence）的直连目标同源。
const PHYSICAL_TARGETS: [&str; 2] = [
    "http://connect.rom.miui.com/generate_204",
    "http://wifi.vivo.com.cn/generate_204",
];

/// 境外目标：经混合端口时走当前出口节点。
const NODE_TARGETS: [&str; 2] = [
    "https://www.gstatic.com/generate_204",
    "https://cp.cloudflare.com/generate_204",
];

const EXPECTED_STATUS: u16 = 204;

/// 隧道失败时的故障层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureLayer {
    /// 系统没有可用网络：睡眠暗唤醒、Wi-Fi 未就绪、上游断网。
    /// 重启内核或关闭 TUN 都无济于事，看门狗不动作。
    Upstream,
    /// 物理网络可达，但当前出口节点不通。
    /// 关闭 TUN 不能恢复连通（系统代理模式走同一个节点），看门狗不动作。
    Node,
    /// 内核控制接口不可达：进程僵死、监听线程失效，或正在切换配置。
    /// 这种情况下混合端口拨不出去，重启内核是有效动作，与隧道自身故障同样处理。
    Core,
    /// 物理网络与节点都可达，只有 TUN 路径不通：栈或路由自身的问题。
    /// 按既有预算重启内核，预算用尽才关闭 TUN。
    Tunnel,
}

impl FailureLayer {
    /// 决策日志用的短标签。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Upstream => "upstream",
            Self::Node => "node",
            Self::Core => "core",
            Self::Tunnel => "tunnel",
        }
    }
}

/// 纯判定：在隧道已失败的前提下，用两条对照通道定位故障层。
pub(crate) fn classify(physical_reachable: bool, node_reachable: bool) -> FailureLayer {
    if !physical_reachable {
        FailureLayer::Upstream
    } else if !node_reachable {
        FailureLayer::Node
    } else {
        FailureLayer::Tunnel
    }
}

/// 纯判定：内核控制接口先于两条对照通道——僵死的内核与「系统没有网络」
/// 在混合端口上表现一样（都拨不出去），不能把它当成上游故障按住不动。
pub(crate) fn classify_with_core(
    core_alive: bool,
    physical_reachable: bool,
    node_reachable: bool,
) -> FailureLayer {
    if !core_alive {
        FailureLayer::Core
    } else {
        classify(physical_reachable, node_reachable)
    }
}

/// 两条对照通道的可达性。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Comparison {
    pub(crate) physical: bool,
    pub(crate) node: bool,
}

/// 经当前内核的混合端口执行两条对照通道。
pub(crate) fn compare_via_mixed_port(mixed_port: u16) -> Comparison {
    compare(
        &format!("http://127.0.0.1:{mixed_port}"),
        &PHYSICAL_TARGETS,
        &NODE_TARGETS,
    )
}

fn compare(proxy_url: &str, physical: &[&str; 2], node: &[&str; 2]) -> Comparison {
    let Ok(proxy) = ureq::Proxy::new(proxy_url) else {
        return Comparison {
            physical: false,
            node: false,
        };
    };
    let agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .timeout(COMPARISON_TIMEOUT)
        .redirects(0)
        .proxy(proxy)
        .build();
    let node = reachable(&agent, node);
    Comparison {
        // 节点服务器可达同样证明物理网络可用
        physical: node || reachable(&agent, physical),
        node,
    }
}

/// 主目标通即止，主目标不通再试复核目标。
fn reachable(agent: &ureq::Agent, targets: &[&str; 2]) -> bool {
    targets
        .iter()
        .any(|url| expected_status(agent.head(url).call()))
}

/// 只有预期的 204 才算通。经 HTTP 代理访问明文目标时，内核拨号失败会以
/// 5xx 响应，不能把「有响应」当作可达（隧道探测没有这一层，任意响应
/// 都说明隧道本身通）。
fn expected_status(outcome: Result<ureq::Response, ureq::Error>) -> bool {
    outcome.is_ok_and(|response| response.status() == EXPECTED_STATUS)
}

/// 看门狗循环里的分层状态：TUN 进入开启状态后的稳定期，以及隧道失败时
/// 最近一次定位到的故障层。
#[derive(Debug, Default)]
pub(crate) struct FailureState {
    tun_enabled: bool,
    core_pid: Option<u32>,
    /// 当前稳定期的起点；TUN 未开启时为 None。
    since: Option<std::time::Instant>,
    layer: Option<FailureLayer>,
}

impl FailureState {
    /// 每轮开头调用。TUN 由关变开、或 TUN 开启期间内核换了进程，就开始
    /// 一个新的稳定期（路由与 DNS 尚在建立），返回 true。应用启动时 TUN
    /// 已处于开启状态，同样算一次由关变开。
    pub(crate) fn observe(
        &mut self,
        tun_enabled: bool,
        core_pid: Option<u32>,
        now: std::time::Instant,
    ) -> bool {
        let turned_on = tun_enabled && !self.tun_enabled;
        let core_changed = self.core_pid != core_pid;
        self.tun_enabled = tun_enabled;
        self.core_pid = core_pid;
        if !tun_enabled {
            self.since = None;
            self.layer = None;
            return false;
        }
        if turned_on || core_changed || self.since.is_none() {
            self.since = Some(now);
            self.layer = None;
            return true;
        }
        false
    }

    /// 隧道探测失败时调用：稳定期内的失败不累计。
    pub(crate) fn in_grace(&self, now: std::time::Instant, grace: std::time::Duration) -> bool {
        self.since
            .is_some_and(|since| now.duration_since(since) < grace)
    }

    /// 记录一次定位结果，返回它与上一次相比是否变化（决策日志据此去抖）。
    pub(crate) fn note_layer(&mut self, layer: FailureLayer) -> bool {
        let changed = self.layer != Some(layer);
        self.layer = Some(layer);
        changed
    }

    /// 隧道恢复时调用，返回此前所处的故障层。
    pub(crate) fn recovered(&mut self) -> Option<FailureLayer> {
        self.layer.take()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    /// 最小 HTTP 代理桩：按请求行里的路径回答指定状态码，其余一律 502。
    /// 返回监听端口。
    fn serve(rules: Vec<(&'static str, u16)>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let rules = rules.clone();
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let Ok(reader) = stream.try_clone() else {
                        return;
                    };
                    let mut reader = BufReader::new(reader);
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    // 读完并丢弃请求头，客户端才会等响应
                    loop {
                        let mut header = String::new();
                        match reader.read_line(&mut header) {
                            Ok(0) | Err(_) => break,
                            Ok(_) if header == "\r\n" || header == "\n" => break,
                            Ok(_) => {}
                        }
                    }
                    let status = rules
                        .iter()
                        .find(|(path, _)| line.contains(path))
                        .map(|(_, status)| *status)
                        .unwrap_or(502);
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status} X\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx"
                    );
                    let _ = stream.flush();
                });
            }
        });
        port
    }

    /// 对照探测用 http 目标，代理桩不需要 TLS。
    const PHYSICAL: [&str; 2] = ["http://direct.test/primary", "http://direct.test/review"];
    const NODE: [&str; 2] = ["http://node.test/primary", "http://node.test/review"];

    fn compare_stub(port: u16) -> Comparison {
        compare(&format!("http://127.0.0.1:{port}"), &PHYSICAL, &NODE)
    }

    #[test]
    fn dead_core_is_its_own_layer() {
        assert_eq!(classify_with_core(false, true, true), FailureLayer::Core);
        assert_eq!(classify_with_core(false, false, false), FailureLayer::Core);
        assert_eq!(classify_with_core(true, true, false), FailureLayer::Node);
        assert_ne!(FailureLayer::Core.label(), FailureLayer::Tunnel.label());
    }

    #[test]
    fn classify_separates_the_three_layers() {
        assert_eq!(classify(false, false), FailureLayer::Upstream);
        assert_eq!(classify(true, false), FailureLayer::Node);
        assert_eq!(classify(true, true), FailureLayer::Tunnel);
        assert_ne!(FailureLayer::Upstream.label(), FailureLayer::Node.label());
    }

    /// 系统没有网络：两条通道都拨不出去，判为上游故障。
    #[test]
    fn all_channels_failing_is_upstream() {
        let port = serve(vec![]);
        let result = compare_stub(port);
        assert_eq!(
            result,
            Comparison {
                physical: false,
                node: false,
            }
        );
        assert_eq!(
            classify_with_core(false, result.physical, result.node),
            FailureLayer::Core,
            "内核没在监听时不能判成上游故障，否则僵死内核永远不会被重启"
        );
    }

    /// 节点抖动：直连业务通、经节点的业务不通。5xx 响应不算可达。
    #[test]
    fn direct_ok_but_node_failing_is_node_layer() {
        let port = serve(vec![("/primary", 204), ("/review", 204)]);
        let result = compare(
            &format!("http://127.0.0.1:{port}"),
            &PHYSICAL,
            &["http://node.test/blocked", "http://node.test/blocked"],
        );
        assert_eq!(
            result,
            Comparison {
                physical: true,
                node: false,
            }
        );
        assert_eq!(classify(result.physical, result.node), FailureLayer::Node);
    }

    /// 物理与节点都通，才可能是隧道自身的问题。
    #[test]
    fn both_channels_ok_leaves_only_the_tunnel() {
        let port = serve(vec![("/primary", 204), ("/review", 204)]);
        let result = compare_stub(port);
        assert_eq!(
            result,
            Comparison {
                physical: true,
                node: true,
            }
        );
        assert_eq!(classify(result.physical, result.node), FailureLayer::Tunnel);
    }

    /// 主目标不通时复核目标补位；主目标通了就不再发第二个请求。
    #[test]
    fn review_target_covers_a_failing_primary() {
        let port = serve(vec![("/review", 204)]);
        let result = compare_stub(port);
        assert_eq!(
            result,
            Comparison {
                physical: true,
                node: true,
            }
        );

        let port = serve(vec![("/primary", 204)]);
        let result = compare(
            &format!("http://127.0.0.1:{port}"),
            &PHYSICAL,
            &["http://node.test/blocked", "http://node.test/blocked"],
        );
        assert_eq!(
            result,
            Comparison {
                physical: true,
                node: false,
            }
        );
    }

    /// 非 204 的成功状态码同样不算可达。
    #[test]
    fn unexpected_success_status_is_not_reachable() {
        let port = serve(vec![("/primary", 200), ("/review", 302)]);
        let result = compare_stub(port);
        assert_eq!(
            result,
            Comparison {
                physical: false,
                node: false,
            }
        );
    }

    /// 内核没在监听（已退出或正在重启）：两条通道都不可达。
    #[test]
    fn closed_proxy_port_is_unreachable_on_both_channels() {
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
            // listener 在此离开作用域，端口随即释放
        };
        std::thread::sleep(Duration::from_millis(50));
        let result = compare_stub(port);
        assert_eq!(
            result,
            Comparison {
                physical: false,
                node: false,
            }
        );
    }

    /// 稳定期：TUN 由关变开、或开启期间内核换进程时开始；TUN 关闭期间内核
    /// 换进程不算，之后打开时再算。
    #[test]
    fn startup_grace_follows_tun_state_and_core_pid() {
        let t0 = std::time::Instant::now();
        let grace = Duration::from_secs(30);
        let mut state = FailureState::default();

        assert!(!state.observe(false, Some(1), t0), "TUN 未开启：没有稳定期");
        assert!(!state.in_grace(t0, grace));

        assert!(state.observe(true, Some(1), t0), "由关变开：稳定期开始");
        assert!(state.in_grace(t0 + Duration::from_secs(29), grace));
        assert!(
            !state.in_grace(t0 + Duration::from_secs(30), grace),
            "稳定期结束"
        );

        // 开启期间内核换了进程：稳定期重新开始
        let t1 = t0 + Duration::from_secs(120);
        assert!(state.observe(true, Some(2), t1));
        assert!(state.in_grace(t1 + Duration::from_secs(1), grace));
        // 同一进程继续运行：不再重新开始
        assert!(!state.observe(true, Some(2), t1 + Duration::from_secs(5)));

        // TUN 关闭期间内核换进程：不算稳定期；再打开时才算
        let t2 = t1 + Duration::from_secs(200);
        assert!(!state.observe(false, Some(3), t2));
        assert!(!state.in_grace(t2, grace));
        assert!(state.observe(true, Some(3), t2 + Duration::from_secs(10)));
        assert!(state.in_grace(t2 + Duration::from_secs(39), grace));

        // 应用启动时 TUN 已开启：第一轮即由关变开
        assert!(FailureState::default().observe(true, Some(9), t0));
    }

    /// 故障层去抖与恢复：同一层只在首次记录时算变化，恢复后清空。
    #[test]
    fn layer_changes_are_deduplicated_and_cleared_on_recovery() {
        let mut state = FailureState::default();
        assert_eq!(state.recovered(), None, "从未失败：没有可报告的恢复");

        assert!(state.note_layer(FailureLayer::Upstream), "首次定位算变化");
        assert!(
            !state.note_layer(FailureLayer::Upstream),
            "同一层持续：不算变化"
        );
        assert!(state.note_layer(FailureLayer::Node), "换了一层：算变化");
        assert_eq!(state.recovered(), Some(FailureLayer::Node));
        assert_eq!(state.recovered(), None, "只报告一次");

        // TUN 关闭会清掉此前的故障层
        state.note_layer(FailureLayer::Tunnel);
        state.observe(false, Some(1), std::time::Instant::now());
        assert_eq!(state.recovered(), None);
    }
}
