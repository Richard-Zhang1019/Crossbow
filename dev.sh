#!/bin/sh
# Crossbow 开发启动脚本：pnpm tauri dev 的包装。
#
# 本机若存在 CommandLineTools 与最新 macOS SDK 不匹配的问题（链接时报
# unknown architecture arm64e.x1），自动回退到可用的旧 SDK；路径不存在时
# 什么都不做，其他机器无副作用。
SDK_DIR="/Library/Developer/CommandLineTools/SDKs"
if [ -z "$SDKROOT" ] && [ -d "$SDK_DIR/MacOSX26.5.sdk" ]; then
  export SDKROOT="$SDK_DIR/MacOSX26.5.sdk"
fi
exec pnpm tauri dev
