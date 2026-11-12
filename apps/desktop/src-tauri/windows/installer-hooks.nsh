!include "LogicLib.nsh"
!include "FileFunc.nsh"
!define CROSSBOW_UPGRADE_HELPER "${__FILEDIR__}\prepare-upgrade.ps1"
!macro NSIS_HOOK_PREINSTALL
  ; A LocalSystem service must never execute out of a user-writable directory.
  ; Instead of restricting the install to Program Files, harden ACLs on the
  ; chosen path: strip inherited ACEs and grant SYSTEM/Administrators full
  ; control, Users read-execute only. Files installed afterwards inherit
  ; these ACEs, so the service binaries stay tamper-proof on any NTFS path.
  ; Well-known SIDs keep this locale-independent.
  CreateDirectory "$INSTDIR"
  nsExec::ExecToStack 'icacls.exe "$INSTDIR" /reset /T /Q /C'
  Pop $0
  Pop $1
  nsExec::ExecToStack 'icacls.exe "$INSTDIR" /inheritance:r /grant:r *S-1-5-18:(OI)(CI)F *S-1-5-32-544:(OI)(CI)F *S-1-5-32-545:(OI)(CI)RX'
  Pop $0
  Pop $1
  ${If} $0 != 0
    ; icacls fails on volumes without ACL support (exFAT/FAT32).
    MessageBox MB_ICONSTOP "The installation path must be on an NTFS volume so the CrossbowService binaries cannot be modified by unprivileged users. Please choose a different path." /SD IDOK
    SetErrorLevel 101
    Abort
  ${EndIf}
  ; The built-in check may return before Windows releases the image mapping.
  ; Close only the app registered against this installation, then verify files
  ; can be opened exclusively before Tauri's first File instruction truncates them.
  !insertmacro CheckIfAppIsRunning "$INSTDIR\crossbow-desktop.exe" "Crossbow"
  ; Upgrades must stop the broker AND its managed core before replacing binaries.
  nsExec::ExecToStack 'powershell.exe -NoProfile -NonInteractive -WindowStyle Hidden -Command "try { $$s=Get-Service CrossbowService -ErrorAction SilentlyContinue; if ($$s) { Stop-Service CrossbowService -ErrorAction Stop; $$s.WaitForStatus([System.ServiceProcess.ServiceControllerStatus]::Stopped,[TimeSpan]::FromSeconds(30)) }; exit 0 } catch { Write-Error $$_.Exception.Message; exit 1 }"'
  Pop $0
  Pop $1
  ${If} $0 != 0
    MessageBox MB_ICONSTOP "Cannot stop Crossbow service for upgrade. Please restart Windows and retry." /SD IDOK
    SetErrorLevel 102
    Abort
  ${EndIf}
  InitPluginsDir
  File "/oname=$PLUGINSDIR\prepare-upgrade.ps1" "${CROSSBOW_UPGRADE_HELPER}"
  nsExec::ExecToStack '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File "$PLUGINSDIR\prepare-upgrade.ps1" -InstallDir "$INSTDIR"'
  Pop $0
  Pop $1
  ${If} $0 != 0
    DetailPrint "$1"
    MessageBox MB_ICONSTOP "Crossbow files are still in use or cannot be written. No application files have been replaced. Restart Windows and rerun this installer. ($0)" /SD IDOK
    SetErrorLevel 107
    Quit
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Backslash-quoted inner quotes are for sc.exe's Windows argv parser, while
  ; outer quotes keep the complete image path (including spaces) in one argument.
  nsExec::ExecToStack '"$SYSDIR\sc.exe" create CrossbowService binPath= "\"$INSTDIR\crossbow-service.exe\"" start= auto obj= LocalSystem DisplayName= "Crossbow Service"'
  Pop $0
  Pop $1
  ${If} $0 != 0
  ${AndIf} $0 != 1073
    ; 静默安装（/S，winget/CI 沙盒）里服务注册可能被环境策略拒绝——中止会让
    ; 整个安装失败（winget 动态校验连续三次 shellExecute 失败）。应用本体照常
    ; 安装，服务由应用在首次开启 TUN 时自愈（install_elevated）。交互安装
    ; 保留弹窗中止，让用户当场知道。用 GetOptions
    ; 从 $CMDLINE 检测 /S（NSIS 无内置静默检测命令）。
    ${GetOptions} $CMDLINE "/S" $R0
    ${If} ${Errors}
      MessageBox MB_ICONSTOP "Cannot register Crossbow service ($0). Please rerun the installer as administrator." /SD IDOK
      SetErrorLevel 103
      Abort
    ${EndIf}
  ${EndIf}
  nsExec::ExecToStack '"$SYSDIR\sc.exe" config CrossbowService binPath= "\"$INSTDIR\crossbow-service.exe\"" start= auto obj= LocalSystem'
  Pop $0
  Pop $1
  ${If} $0 != 0
    ${GetOptions} $CMDLINE "/S" $R0
    ${If} ${Errors}
      MessageBox MB_ICONSTOP "Cannot configure Crossbow service ($0)." /SD IDOK
      SetErrorLevel 104
      Abort
    ${EndIf}
  ${EndIf}
  nsExec::ExecToStack '"$SYSDIR\sc.exe" start CrossbowService'
  Pop $0
  Pop $1
  ${If} $0 != 0
    ${GetOptions} $CMDLINE "/S" $R0
    ${If} ${Errors}
      MessageBox MB_ICONSTOP "Cannot start Crossbow service ($0). Please repair the installation." /SD IDOK
      SetErrorLevel 105
      Abort
    ${EndIf}
  ${EndIf}
!macroend

; #457：仅终止可执行路径归属 Crossbow 的残留内核（安装目录或数据目录）。
; 其他代理客户端的同名 mihomo/sing-box 进程不受影响。应用在运行时直接跳过
; ——受控内核由应用退出路径（恢复系统代理后）自行停止。归属前缀经环境
; 变量传入，避免 NSIS/PowerShell 双层引号拼接。
!macro CrossbowKillOwnedCores
  System::Call 'kernel32::SetEnvironmentVariable(t "CB_OWN_PREFIX_1", t "$INSTDIR")'
  System::Call 'kernel32::SetEnvironmentVariable(t "CB_OWN_PREFIX_2", t "$APPDATA\com.richard.crossbow")'
  nsExec::ExecToLog 'powershell.exe -NoProfile -NonInteractive -Command "Get-Process -Name mihomo,sing-box -ErrorAction SilentlyContinue | Where-Object { $$_.Path -and ($$_.Path.StartsWith($$env:CB_OWN_PREFIX_1) -or $$_.Path.StartsWith($$env:CB_OWN_PREFIX_2)) } | ForEach-Object { Stop-Process -Id $$_.Id -Force }"'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; #457：此钩子先于 Tauri 模板的 CheckIfAppIsRunning（该提示允许取消）。
  ; 取消路径上不得产生副作用——服务与受控内核一律不动，由应用自身的退出
  ; 流程（恢复系统代理 → 停内核）或 POSTUNINSTALL 兜底处理。仅清理无主
  ; 残留：应用未运行时，安装目录/数据目录下的同名内核即 #233 所述残留；
  ; 其他客户端的同名进程因路径不匹配而保留。
  nsExec::ExecToStack 'tasklist /FI "IMAGENAME eq crossbow-desktop.exe" /NH'
  Pop $0
  Pop $1
  ${If} $1 contains "crossbow-desktop.exe"
    DetailPrint "Crossbow is running; its managed processes are left to the app exit flow."
  ${Else}
    !insertmacro CrossbowKillOwnedCores
  ${EndIf}
  RMDir /r "$INSTDIR\service-runtime"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; #457：服务停止/删除与残留清理移到文件删除之后——PREUNINSTALL 不再
  ; 提前拆除服务，取消卸载时服务、内核、代理设置保持原状态。
  nsExec::ExecToStack 'powershell.exe -NoProfile -NonInteractive -WindowStyle Hidden -Command "try { $$s=Get-Service CrossbowService -ErrorAction SilentlyContinue; if ($$s) { Stop-Service CrossbowService -ErrorAction Stop; $$s.WaitForStatus([System.ServiceProcess.ServiceControllerStatus]::Stopped,[TimeSpan]::FromSeconds(30)) } ; exit 0 } catch { Write-Error $$_.Exception.Message; exit 1 }"'
  Pop $0
  Pop $1
  nsExec::Exec '"$SYSDIR\sc.exe" delete CrossbowService'
  !insertmacro CrossbowKillOwnedCores
  ; %APPDATA% 下 journal（sysproxy-journal.json / tun-proxy-journal.json）
  ; 有意保留：代理恢复未完成时，重装后的 recover_stale 会在归属核对下重试。
!macroend
