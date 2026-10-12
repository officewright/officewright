<#
Tests of scripts/windows/make-reference.ps1 that need no Word. The script's functions are
loaded from its syntax tree, and a hidden ping.exe stands in for a Word process that hangs.
scripts/check.sh runs this on Windows: Windows PowerShell 5.1 exists only there.

The time-out tests are the regression test for a run in which the watchdog fired but the
hung Word survived the stop and the script waited forever: a step that does not answer must
end, with the stop checked, within the time-out plus the grace periods.
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$scriptPath = (Resolve-Path (Join-Path $PSScriptRoot '..\..\scripts\windows\make-reference.ps1')).ProviderPath
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

$parseTokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($scriptPath, [ref]$parseTokens, [ref]$parseErrors)
if ($parseErrors.Count -gt 0) { throw "make-reference.ps1 does not parse: $($parseErrors[0].Message)" }
$functions = $ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $false)
foreach ($function in $functions) { . ([ScriptBlock]::Create($function.Extent.Text)) }

$script:failures = 0
function Check([string] $Name, [bool] $Condition, [string] $Detail = '') {
    if ($Condition) {
        Write-Host "ok    $Name"
    } else {
        Write-Host "FAIL  $Name $Detail" -ForegroundColor Red
        $script:failures++
    }
}

function Start-Dummy {
    # A process that runs for ten minutes unless stopped: the stand-in for a hung Word.
    $ping = Join-Path $env:SystemRoot 'System32\PING.EXE'
    return Start-Process -FilePath $ping -ArgumentList '-n', '600', '127.0.0.1' -WindowStyle Hidden -PassThru
}

function Stop-Dummy($Process) {
    if (-not $Process.HasExited) { Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue }
}

$temp = Join-Path ([System.IO.Path]::GetTempPath()) ("make-reference-test-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $temp | Out-Null
try {
    # Printer names.
    Check 'Word printer name loses its port' ((Get-PrinterName 'Microsoft Print to PDF on Ne01:') -eq 'Microsoft Print to PDF')
    Check 'printer name without a port stays' ((Get-PrinterName 'Microsoft Print to PDF') -eq 'Microsoft Print to PDF')
    Check 'network printer name loses its port' ((Get-PrinterName '\\print01\Floor 3 on Ne02:') -eq '\\print01\Floor 3')
    Check 'default printer is read without error' ((Get-DefaultPrinter) -is [string])

    # Whole-word matching of personal strings.
    Check 'a name inside a longer word does not match' (-not (Test-Token 'Calibri' 'Cali'))
    Check 'a name as a word matches, in any case' (Test-Token 'by cali, today' 'Cali')

    # Personal data in a package and in PDF strings.
    $docx = Join-Path $temp 'named.docx'
    $zip = [System.IO.Compression.ZipFile]::Open($docx, 'Create')
    try {
        $entry = $zip.CreateEntry('docProps/core.xml')
        $writer = New-Object System.IO.StreamWriter($entry.Open())
        $writer.Write('<cp:coreProperties><cp:lastModifiedBy>Zork Testerson</cp:lastModifiedBy></cp:coreProperties>')
        $writer.Dispose()
    } finally {
        $zip.Dispose()
    }
    $pdf = Join-Path $temp 'named.pdf'
    $utf16 = [System.Text.Encoding]::BigEndianUnicode.GetBytes('Zork Testerson')
    $bytes = [System.Text.Encoding]::ASCII.GetBytes('%PDF-1.7 /Author (') + [byte[]](0xFE, 0xFF) + $utf16 + [System.Text.Encoding]::ASCII.GetBytes(')')
    [System.IO.File]::WriteAllBytes($pdf, [byte[]]$bytes)
    $token = New-Object PSObject -Property @{ Value = 'Zork Testerson'; Kind = 'the Office user name'; Always = $true }
    $found = @(Find-PersonalData $docx $pdf @($token))
    Check 'a name in the package is found' ($found -contains 'docProps/core.xml: the Office user name') ($found -join '; ')
    Check 'a UTF-16 name in the PDF is found' ($found -contains 'pdf: the Office user name') ($found -join '; ')

    # No WINWORD started after now.
    Check 'no Word process of this script exists yet' (@(Get-OwnWordProcess (Get-Date)).Count -eq 0)

    # Verified stops.
    $dummy = Start-Dummy
    try {
        $stop = Stop-ProcessVerified $dummy.Id
        Check 'a running process is stopped and checked' ($stop.Stopped -and $dummy.WaitForExit(5000)) $stop.Detail
        $again = Stop-ProcessVerified $dummy.Id
        Check 'a process that is gone counts as stopped' ($again.Stopped -and $again.Detail -match 'already exited') $again.Detail
    } finally {
        Stop-Dummy $dummy
    }

    $progress = [hashtable]::Synchronized(@{ Step = '' })
    $worker = New-WordWorker $progress
    try {
        $done = Invoke-WordStep $worker 'param($x) $x * 2' @(21) 10 { @() } $progress
        Check 'a step that answers returns its output' ($done.Status -eq 'done' -and $done.Output[0] -eq 42) $done.Status
        $failed = Invoke-WordStep $worker "throw 'no such document'" @() 10 { @() } $progress
        Check 'a step that fails returns its message' ($failed.Status -eq 'error' -and $failed.Message -eq 'no such document') $failed.Message

        # A call blocked until "Word" ends, as a COM call is: the time-out stops the process,
        # checks that it is gone, and the step ends with the step name in the message.
        $dummy = Start-Dummy
        try {
            $clock = [System.Diagnostics.Stopwatch]::StartNew()
            $blocked = '$progress.Step = ''waiting for Word''; (Get-Process -Id ' + $dummy.Id + ').WaitForExit(); ''returned'''
            $result = Invoke-WordStep $worker $blocked @() 2 { @($dummy.Id) } $progress
            $seconds = $clock.Elapsed.TotalSeconds
            Check 'a step without an answer times out' ($result.Status -eq 'timeout') "$($result.Status): $($result.Message)"
            Check 'the time-out names the step' ($result.Message -match 'during: waiting for Word') $result.Message
            Check 'the time-out stops the process and checks it' ($result.Message -match "stopped process $($dummy.Id)" -and $dummy.HasExited) $result.Message
            Check 'the time-out ends within its limits' ($seconds -lt 20) "$seconds s"
        } finally {
            Stop-Dummy $dummy
        }
    } finally {
        $worker.Runspace.Dispose()
        $worker.Dispose()
    }

    # A call that stays blocked after the process was stopped: the step reports 'stuck'.
    $worker = New-WordWorker $progress
    $dummy = Start-Dummy
    try {
        $clock = [System.Diagnostics.Stopwatch]::StartNew()
        $result = Invoke-WordStep $worker '$progress.Step = ''saving''; Start-Sleep -Seconds 600' @() 2 { @($dummy.Id) } $progress
        $seconds = $clock.Elapsed.TotalSeconds
        Check 'a step still blocked after the stop is reported as stuck' ($result.Status -eq 'stuck') "$($result.Status): $($result.Message)"
        Check 'being stuck is reported within its limits' ($seconds -lt 35) "$seconds s"
    } finally {
        Stop-Dummy $dummy
        $worker.Stop()
        $worker.Runspace.Dispose()
        $worker.Dispose()
    }

    # Exit-Stuck ends the process even though the worker thread is blocked.
    $child = Join-Path $temp 'stuck.ps1'
    $childScript = @'
$ErrorActionPreference = 'Stop'
$ast = [System.Management.Automation.Language.Parser]::ParseFile('{0}', [ref]$null, [ref]$null)
foreach ($f in $ast.FindAll({{ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] }}, $false)) {{ . ([ScriptBlock]::Create($f.Extent.Text)) }}
$progress = [hashtable]::Synchronized(@{{ Step = '' }})
$worker = New-WordWorker $progress
[void]$worker.AddScript('Start-Sleep -Seconds 600')
[void]$worker.BeginInvoke()
Start-Sleep -Seconds 1
Exit-Stuck 'test' ''
'@ -f $scriptPath
    [System.IO.File]::WriteAllText($child, $childScript)
    $powershell = Join-Path $PSHOME 'powershell.exe'
    $process = Start-Process -FilePath $powershell -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$child`"" -WindowStyle Hidden -PassThru
    # Windows PowerShell reports no ExitCode for a started process unless its handle was read.
    $null = $process.Handle
    $exited = $process.WaitForExit(30000)
    if (-not $exited) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
    Check 'Exit-Stuck ends the process with code 2' ($exited -and $process.ExitCode -eq 2) "exited: $exited"
} finally {
    Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
}

if ($script:failures -gt 0) {
    Write-Host "$($script:failures) failed" -ForegroundColor Red
    exit 1
}
Write-Host 'all passed'
exit 0
