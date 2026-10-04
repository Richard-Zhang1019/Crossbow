// Crossbow iOS 主入口（M2 骨架占位）。
// SwiftUI + TabView，四个 Tab 对应 DESIGN §4.2 的信息架构。

import SwiftUI

@main
struct CrossbowApp: App {
    var body: some Scene {
        WindowGroup {
            TabView {
                Text("首页：连接总开关 / 当前节点 / 实时速率")
                    .tabItem { Label("首页", systemImage: "house") }
                Text("代理：策略组与节点测速")
                    .tabItem { Label("代理", systemImage: "bolt") }
                Text("配置：订阅管理 + iCloud 同步状态")
                    .tabItem { Label("配置", systemImage: "doc") }
                Text("更多：设置 / 诊断 / 日志")
                    .tabItem { Label("更多", systemImage: "ellipsis") }
            }
        }
    }
}
