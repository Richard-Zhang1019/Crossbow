// PacketTunnel 扩展入口（M2 骨架占位）。
// M2 实现：从 App Group 读取渲染好的 sing-box JSON 配置，启动 libbox。

import NetworkExtension

class PacketTunnelProvider: NEPacketTunnelProvider {
    override func startTunnel(
        options: [String: NSObject]? = nil,
        completionHandler: @escaping (Error?) -> Void
    ) {
        // TODO(M2): 读取 group.com.crossbow.app 共享配置 → libbox start
        completionHandler(NSError(domain: "com.crossbow.app", code: -1,
                                  userInfo: [NSLocalizedDescriptionKey: "M2 待实现"]))
    }

    override func stopTunnel(with reason: NEProviderStopReason,
                             completionHandler: @escaping () -> Void) {
        // TODO(M2): libbox close
        completionHandler()
    }
}
