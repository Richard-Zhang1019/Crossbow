# Hosted-runner only: verifies the real NSIS hooks, broker lifetime, and rejection
# of an unrelated local client. No user configuration or TUN/network changes.
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'This smoke test may only run on an ephemeral Windows Actions runner'
}
$installer = (Get-ChildItem target/release/bundle/nsis/*-setup.exe | Select-Object -First 1).FullName
if (-not $installer) { throw 'NSIS installer not found' }
function Install-Crossbow {
    $process = Start-Process -FilePath $installer -ArgumentList '/S' -PassThru
    if (-not $process.WaitForExit(120000)) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        throw 'Installer timed out after 120 seconds'
    }
    $process.Refresh()
    if ($process.ExitCode -ne 0) { throw "Installer failed: $($process.ExitCode)" }
    $service = Get-Service CrossbowService
    $service.WaitForStatus([System.ServiceProcess.ServiceControllerStatus]::Running, [TimeSpan]::FromSeconds(10))
}
try {
    Install-Crossbow
    # Exercise the same command builder as the broker, with canonical Windows
    # paths and the installed core. Config validation only; no TUN or listeners.
    $previousCoreBin = $env:CROSSBOW_CORE_BIN
    try {
        $env:CROSSBOW_CORE_BIN = Join-Path $env:ProgramFiles 'Crossbow/mihomo.exe'
        if (-not (Test-Path $env:CROSSBOW_CORE_BIN)) { throw 'Installed mihomo not found' }
        cargo test --locked -p crossbow-service core_process::tests -- --nocapture
        if ($LASTEXITCODE -ne 0) { throw 'Windows core path regression tests failed' }
    } finally { $env:CROSSBOW_CORE_BIN = $previousCoreBin }
    $pipe = [IO.Pipes.NamedPipeClientStream]::new('.', 'crossbow-service', [IO.Pipes.PipeDirection]::InOut)
    try {
        $pipe.Connect(5000)
        $writer = [IO.StreamWriter]::new($pipe)
        $writer.AutoFlush = $true
        $reader = [IO.StreamReader]::new($pipe)
        $writer.WriteLine('{"cmd":"start","binary":"C:\\Windows\\System32\\cmd.exe","config":"not-opened","log":"not-opened"}')
        $reply = $reader.ReadLineAsync()
        if (-not $reply.Wait(5000)) { throw 'Service response timed out' }
        $value = $reply.Result | ConvertFrom-Json
        $writer.WriteLine('') # Reply ACK before the server disconnects.
        if ($value.ok -ne $false -or $value.error -notmatch 'only the installed Crossbow GUI') {
            throw 'Broker did not reject the foreign client before opening files'
        }
    } finally { $pipe.Dispose() }
    $idle = [IO.Pipes.NamedPipeClientStream]::new('.', 'crossbow-service', [IO.Pipes.PipeDirection]::InOut)
    $idle.Connect(5000)
    # A connected client that never writes must not block SCM stop.
    Stop-Service CrossbowService
    $idle.Dispose()
    (Get-Service CrossbowService).WaitForStatus([System.ServiceProcess.ServiceControllerStatus]::Stopped, [TimeSpan]::FromSeconds(10))
    # Keep the service stopped: preflight checks its EXE as well as the GUI.
    # Real file locks: preflight must fail without changing the installed EXE,
    # and must wait for a delayed handle release (antivirus/terminating process).
    $gui = Join-Path $env:ProgramFiles 'Crossbow/crossbow-desktop.exe'
    $beforeHash = (Get-FileHash $gui).Hash
    $helper = (Resolve-Path apps/desktop/src-tauri/windows/prepare-upgrade.ps1).Path
    # Establish an unlocked baseline so the negative case cannot pass because
    # an unrelated service/DLL lock was left behind by fixture setup.
    & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $helper -InstallDir (Split-Path $gui)
    if ($LASTEXITCODE -ne 0) { throw 'Preflight rejected unlocked installation baseline' }
    $lock = [IO.File]::Open($gui, 'Open', 'Read', 'Read')
    try {
        & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $helper -InstallDir (Split-Path $gui) -TimeoutSeconds 1
        if ($LASTEXITCODE -eq 0) { throw 'Preflight accepted a locked executable' }
    } finally { $lock.Dispose() }
    if ((Get-FileHash $gui).Hash -ne $beforeHash) { throw 'Preflight modified the locked executable' }
    $lock = [IO.File]::Open($gui, 'Open', 'Read', 'Read')
    try {
        $probe = Start-Process powershell.exe -ArgumentList "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$helper`" -InstallDir `"$(Split-Path $gui)`"" -PassThru
        Start-Sleep -Seconds 2
        if ($probe.HasExited) { throw 'Preflight did not wait for locked EXE' }
    } finally { $lock.Dispose() }
    if (-not $probe.WaitForExit(20000)) { throw 'Preflight failed to resume after lock release' }
    $probe.Refresh()
    if ($probe.ExitCode -ne 0) { throw 'Preflight rejected released executable' }
    Start-Service CrossbowService
    (Get-Service CrossbowService).WaitForStatus([System.ServiceProcess.ServiceControllerStatus]::Running, [TimeSpan]::FromSeconds(10))
    # Exercise the actual installer abort with the service initially running
    # and stopped. An external sharing lock must survive Restart Manager.
    $serviceExe = Join-Path (Split-Path $gui) 'crossbow-service.exe'
    $coreExe = Join-Path (Split-Path $gui) 'mihomo.exe'
    $serviceHash = (Get-FileHash $serviceExe).Hash
    $coreHash = (Get-FileHash $coreExe).Hash
    foreach ($expectedStatus in @('Running', 'Stopped')) {
        $service = Get-Service CrossbowService
        if ($expectedStatus -eq 'Stopped') {
            Stop-Service -InputObject $service
            $service.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(10))
        }
        # Restart Manager registers the GUI only. Lock a sidecar so the
        # installer reaches our preflight and cannot close the smoke runner.
        $lock = [IO.File]::Open($coreExe, 'Open', 'Read', 'Read')
        try {
            $blocked = Start-Process -FilePath $installer -ArgumentList '/S' -PassThru
            if (-not $blocked.WaitForExit(90000)) {
                Stop-Process -Id $blocked.Id -Force -ErrorAction SilentlyContinue
                throw "Locked upgrade timed out with service initially $expectedStatus"
            }
            $blocked.Refresh()
            if ($blocked.ExitCode -ne 107) {
                throw "Expected upgrade preflight exit 107, got $($blocked.ExitCode)"
            }
        } finally { $lock.Dispose() }
        if ((Get-Service CrossbowService).Status -ne $expectedStatus) {
            throw "Failed upgrade did not preserve service state $expectedStatus"
        }
        if ((Get-FileHash $gui).Hash -ne $beforeHash -or (Get-FileHash $serviceExe).Hash -ne $serviceHash -or (Get-FileHash $coreExe).Hash -ne $coreHash) {
            throw 'Failed upgrade changed an installed executable'
        }
    }
    Start-Service CrossbowService
    (Get-Service CrossbowService).WaitForStatus('Running', [TimeSpan]::FromSeconds(10))
    # Install while the service AND an EXE at the exact installed GUI path run.
    # Use an inert process fixture: no user profile, proxy, TUN or GUI interaction.
    $fixtureDir = Join-Path $env:RUNNER_TEMP 'crossbow-upgrade-fixture'
    New-Item -ItemType Directory -Force $fixtureDir | Out-Null
    $source = Join-Path $fixtureDir 'fixture.cs'
    Set-Content $source 'class Fixture { static void Main() { System.Threading.Thread.Sleep(120000); } }'
    $foreignGui = Join-Path $fixtureDir 'crossbow-desktop.exe'
    $csc = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
    & $csc /nologo /target:winexe "/out:$foreignGui" $source
    if ($LASTEXITCODE -ne 0) { throw 'Cannot compile upgrade fixture' }
    Copy-Item $foreignGui $gui -Force
    $installedProcess = Start-Process $gui -PassThru
    $foreignProcess = Start-Process $foreignGui -PassThru
    try {
        Install-Crossbow
        if (-not $installedProcess.WaitForExit(10000)) { throw 'Old installed GUI survived upgrade' }
        $foreignProcess.Refresh()
        if ($foreignProcess.HasExited) { throw 'Installer killed unrelated same-name process' }
        if ((Get-FileHash $gui).Hash -ne $beforeHash) { throw 'Installer did not restore the correct GUI executable' }
    } finally {
        Stop-Process -Id $installedProcess.Id, $foreignProcess.Id -Force -ErrorAction SilentlyContinue
    }
    Write-Host 'Windows service installation, rejection, stop/restart and upgrade checks passed'
} catch {
    Get-CimInstance Win32_Service -Filter "Name='CrossbowService'" |
        Select-Object Name, State, PathName | Format-List
    throw
} finally {
    $service = Get-Service CrossbowService -ErrorAction SilentlyContinue
    if ($service -and $service.Status -ne 'Stopped') { Stop-Service CrossbowService }
}
