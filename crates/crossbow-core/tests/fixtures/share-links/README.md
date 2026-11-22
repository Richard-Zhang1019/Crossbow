# 可公开的客户端导出结构夹具（#599）

`clients.json` 的服务器、密码、UUID 和名称均为合成值，不来自用户订阅。它们按以下公开源码/规范重建，不声称已运行这些应用并抓取导出：

- [v2rayN TuicFmt](https://github.com/2dust/v2rayN/blob/b27ae11d1eb9cc3028590b92ad13b2439e61c499/v2rayN/ServiceLib/Handler/Fmt/TuicFmt.cs) 及同版本 [BaseFmt.ToUri](https://github.com/2dust/v2rayN/blob/b27ae11d1eb9cc3028590b92ad13b2439e61c499/v2rayN/ServiceLib/Handler/Fmt/BaseFmt.cs)：先拼 UUID:password，再整体 URL 编码认证字段。本夹具还含 IPv6、非默认端口和转义片段。
- [NekoBoxForAndroid TuicFmt](https://github.com/MatsuriDayo/NekoBoxForAndroid/blob/01bf49f76569ebf705d89f1d9d78d1fcf5c8773e/app/src/main/java/io/nekohasekai/sagernet/fmt/tuic/TuicFmt.kt)：HttpUrl builder 分别设置 username/password；包含拥塞控制、UDP relay、SNI、ALPN 和显式 allow_insecure。该夹具只检查显式 insecure 的保持；原生连接测试不使用 insecure。
- [Hysteria 官方 URI](https://v2.hysteria.network/docs/developers/URI-Scheme/)：userpass 认证，省略端口，salamander 和转义密码。

`share_links_native.rs` 常规测试验证认证/地址/SNI 字段保持，整批 Base64（标准及 URL-safe 无填充）与逐行结果一致，追加坏节点后整批拒绝。其他协议及异常转义、重复参数、空行、重复节点、大小边界在 `src/share_links.rs` 单元测试中覆盖。

`verify-share-links-native.py` 另以临时地址/凭据生成对应结构，使用公开导入器和固定真实内核验证连接。JSON 夹具中的保留域名和文档 IPv6 地址不会被网络访问。
