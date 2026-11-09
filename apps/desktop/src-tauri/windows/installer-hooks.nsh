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

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToStack 'powershell.exe -NoProfile -NonInteractive -WindowStyle Hidden -Command "try { $$s=Get-Service CrossbowService -ErrorAction SilentlyContinue; if ($$s) { Stop-Service CrossbowService -ErrorAction Stop; $$s.WaitForStatus([System.ServiceProcess.ServiceControllerStatus]::Stopped,[TimeSpan]::FromSeconds(30)) }; exit 0 } catch { Write-Error $$_.Exception.Message; exit 1 }"'
  Pop $0
  Pop $1
  ${If} $0 != 0
    MessageBox MB_ICONSTOP "Cannot stop Crossbow service. Please restart Windows before uninstalling." /SD IDOK
    SetErrorLevel 106
    Abort
  ${EndIf}
  nsExec::Exec '"$SYSDIR\sc.exe" delete CrossbowService'
  ; 卸载器只处理主程序：内核子进程必须在此先杀，否则残留内核占着
  ; 混合端口，重装后首次启动报 os error 10048（#233）。按镜像名清理，
  ; 覆盖 sidecar 三元组命名与 data 目录下载命名。
  nsExec::ExecToLog 'taskkill /F /IM "mihomo.exe" /T'
  Pop $0
  nsExec::ExecToLog 'taskkill /F /IM "mihomo-x86_64-pc-windows-msvc.exe" /T'
  Pop $0
  nsExec::ExecToLog 'taskkill /F /IM "sing-box.exe" /T'
  Pop $0
  nsExec::ExecToLog 'taskkill /F /IM "sing-box-x86_64-pc-windows-msvc.exe" /T'
  Pop $0
  ; Service-owned config/cache must not survive an uninstall with user secrets.
  RMDir /r "$INSTDIR\service-runtime"
!macroend
