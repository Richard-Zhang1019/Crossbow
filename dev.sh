#!/bin/sh
# Crossbow 开发启动脚本：pnpm tauri dev 的包装。
#
# 仅当仍在使用 CommandLineTools（未装/未选完整 Xcode）且存在已知的
# SDK 兼容问题时，才回退到旧 SDK（链接报 unknown architecture arm64e.x1）。
# 已切换到完整 Xcode 的机器不受影响。
SELECTED="$(xcode-select -p 2>/dev/null)"
case "$SELECTED" in
  *CommandLineTools*)
    SDK_DIR="/Library/Developer/CommandLineTools/SDKs"
    if [ -z "$SDKROOT" ] && [ -d "$SDK_DIR/MacOSX26.5.sdk" ]; then
      export SDKROOT="$SDK_DIR/MacOSX26.5.sdk"
    fi
    ;;
esac
exec pnpm tauri dev
