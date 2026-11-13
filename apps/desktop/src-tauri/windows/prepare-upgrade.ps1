# NSIS invokes this after Restart Manager has closed the GUI. The helper owns
# the stop/preflight/recovery sequence so a failed preflight cannot strand the
# previous service. Opening with FileMode.Open never truncates installed files.
param(
    [Parameter(Mandatory = $true)][string]$InstallDir,
    [ValidateRange(1, 60)][int]$TimeoutSeconds = 15,
    [switch]$StopServiceForUpgrade
)
$ErrorActionPreference = 'Stop'
$service = $null
$wasRunning = $false
$timeout = [TimeSpan]::FromSeconds(30)

function Assert-FilesAvailable {
    $files = @(
        'crossbow-desktop.exe', 'crossbow-service.exe', 'mihomo.exe', 'sing-box.exe',
        'mihomo-x86_64-pc-windows-msvc.exe', 'sing-box-x86_64-pc-windows-msvc.exe', 'wintun.dll'
    )
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        $handles = @()
        $failure = $null
        try {
            foreach ($name in $files) {
                $path = Join-Path $InstallDir $name
                if (Test-Path -LiteralPath $path) {
                    $handles += [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
                }
            }
        } catch { $failure = $_.Exception.Message }
        finally { foreach ($handle in $handles) { $handle.Dispose() } }
        if (-not $failure) { return }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Crossbow upgrade preflight failed: $failure"
}

$failureCode = 2
try {
    if ($StopServiceForUpgrade) {
        # An absent service is normal on first install. Enumerate so a real
        # SCM query error aborts instead of looking like an absent service.
        $service = Get-Service | Where-Object Name -eq 'CrossbowService'
        if ($service) {
            $service.Refresh()
            switch ($service.Status) {
                'StartPending' { $service.WaitForStatus('Running', $timeout) }
                'StopPending' { $service.WaitForStatus('Stopped', $timeout) }
            }
            $service.Refresh()
            if ($service.Status -notin 'Running', 'Stopped') {
                throw "CrossbowService is in an unsupported state: $($service.Status)"
            }
            $wasRunning = $service.Status -eq 'Running'
            if ($wasRunning) {
                $service.Stop()
                $service.WaitForStatus('Stopped', $timeout)
            }
        }
    }
    $failureCode = 1
    Assert-FilesAvailable
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    if ($wasRunning) {
        try {
            $service.Refresh()
            if ($service.Status -eq 'StopPending') {
                $service.WaitForStatus('Stopped', $timeout)
                $service.Refresh()
            }
            if ($service.Status -eq 'Stopped') {
                $service.Start()
            }
            $service.WaitForStatus('Running', $timeout)
        } catch {
            [Console]::Error.WriteLine("CrossbowService could not be restored: $($_.Exception.Message)")
            exit 3
        }
    }
    exit $failureCode
}
exit 0
