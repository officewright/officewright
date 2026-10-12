<#
.SYNOPSIS
    Renders reference PDFs of Word documents with Microsoft Word for the officewright corpus.

.DESCRIPTION
    For every input document the script
      1. copies it into a staging folder under -OutputDir (Word opens files from
         \\wsl.localhost and other network paths in Protected View, which cannot export);
      2. opens the copy read-only in an invisible Word instance and saves it again as
         <name>.resaved.docx with personal information removed, so the file carries the
         w:lastRenderedPageBreak marks of Word's own pagination;
      3. reopens that re-saved file and exports <name>.pdf from it;
      4. checks both files for the Windows and Office user names, the computer name and
         the user profile path, and refuses them if any appears;
      5. writes <name>.json: hashes, Word version and build, the printer, page count,
         compatibility mode, and the fonts the document asks for that neither Windows nor
         Word has.
    scripts/corpus-import.py, run on the Linux side, checks these files again and copies
    them into tests/corpus/.

    References are made with Microsoft Print to PDF as Word's printer: a printer that does
    not answer can stall Word, and Word uses a printer's metrics for layout when a document
    sets w:usePrinterMetrics. The script prints the default printer and Word's printer and
    warns when either is another one.

    All calls into Word run in a worker thread that this script watches. When Word gives
    no answer within -TimeoutSeconds, the script stops the Word instance it started (a
    WINWORD.EXE started by COM, with /Automation on its command line, after the script
    began), checks that the process is gone, and says which step Word was in. If Word
    cannot be stopped, the script says so and exits with code 2 instead of waiting.

    The script never touches a Word session the user has open: it refuses to start while
    any WINWORD process runs. Do not open Word documents while it runs: Windows may hand
    them to the invisible Word instance. If that happens, the script stops before the next
    document, makes its Word instance visible and leaves it running instead of closing
    it. It changes no Word option and adds nothing to Word's recent files. Existing
    results are kept unless -Overwrite is given, so an interrupted run can simply be
    started again.

    Run it from Windows PowerShell 5.1 on the machine that has Word, not from WSL.
    tests/corpus-tools/test_make_reference.ps1 tests the parts that need no Word.

.PARAMETER InputPath
    Documents to render, or folders whose documents (matching -Pattern) are rendered.

.PARAMETER OutputDir
    Local folder for the results. Not a network path and not inside OneDrive.

.PARAMETER TimeoutSeconds
    How long Word may take to start, or to save and export one document.

.PARAMETER DryRun
    Check the inputs, the output folder, the fonts and the printer, print what would be
    rendered, and stop before Word starts.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File C:\officewright-ref\make-reference.ps1 -InputPath C:\officewright-ref\word-rules -OutputDir C:\officewright-ref\out
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string[]] $InputPath,

    [Parameter(Mandatory = $true)]
    [string] $OutputDir,

    [string] $Pattern = '*.docx',

    [switch] $Overwrite,

    [ValidateRange(10, 3600)]
    [int] $TimeoutSeconds = 120,

    [switch] $DryRun
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

# Bumped whenever the output files change shape; corpus-import.py checks it.
$ScriptVersion = 2

# The printer the references are made with.
$ReferencePrinter = 'Microsoft Print to PDF'

function Stop-WithError([string] $Message) {
    Write-Host "make-reference: $Message" -ForegroundColor Red
    exit 1
}

function Resolve-UserPath([string] $Path) {
    # Relative to the PowerShell location, which .NET's current directory does not follow.
    return $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Path)
}

function Get-Sha256([string] $Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Write-JsonFile([string] $Path, $Value) {
    $json = ConvertTo-Json -InputObject $Value -Depth 6
    $utf8 = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::WriteAllText($Path, $json + "`n", $utf8)
}

function Read-ZipEntries([string] $Path) {
    # Name and UTF-8 text of every entry of a DOCX package.
    $entries = @()
    $zip = [System.IO.Compression.ZipFile]::OpenRead($Path)
    try {
        foreach ($entry in $zip.Entries) {
            $reader = New-Object System.IO.StreamReader($entry.Open(), [System.Text.Encoding]::UTF8)
            try {
                $entries += New-Object PSObject -Property @{ Name = $entry.FullName; Text = $reader.ReadToEnd() }
            } finally {
                $reader.Dispose()
            }
        }
    } finally {
        $zip.Dispose()
    }
    return $entries
}

function Get-DocumentFonts([string] $Path) {
    # The font names in word/fontTable.xml, which lists every font a document asks for.
    $wordNs = 'http://schemas.openxmlformats.org/wordprocessingml/2006/main'
    $names = @()
    foreach ($entry in (Read-ZipEntries $Path)) {
        if ($entry.Name -ne 'word/fontTable.xml') { continue }
        $xml = New-Object System.Xml.XmlDocument
        $xml.LoadXml($entry.Text)
        $ns = New-Object System.Xml.XmlNamespaceManager($xml.NameTable)
        $ns.AddNamespace('w', $wordNs)
        foreach ($font in $xml.SelectNodes('/w:fonts/w:font', $ns)) {
            $names += $font.GetAttribute('name', $wordNs)
        }
    }
    return @($names | Sort-Object -Unique)
}

function Get-MissingFonts($Fonts, $Available) {
    $missing = @()
    foreach ($font in $Fonts) {
        if (-not $Available.Contains($font)) { $missing += $font }
    }
    return $missing
}

function Get-PackageText([string] $Path) {
    # All parts of a package as one string, to tell tokens Word added from tokens the
    # input already had.
    return ((Read-ZipEntries $Path) | ForEach-Object { $_.Text }) -join "`n"
}

function Test-Token([string] $Text, [string] $Value) {
    # A whole-word match: the user name "Cal" must not match "Calibri".
    $pattern = '(?<![\p{L}\p{N}])' + [regex]::Escape($Value) + '(?![\p{L}\p{N}])'
    return [regex]::IsMatch($Text, $pattern, [System.Text.RegularExpressions.RegexOptions]::IgnoreCase)
}

function Find-PersonalData([string] $DocxPath, [string] $PdfPath, $Tokens) {
    # Which parts of the outputs mention a personal token. Returns "part: kind" strings and
    # never the token itself, so the result can go into the JSON file.
    $found = @()
    $paths = @('\Users\', '/Users/')
    foreach ($entry in (Read-ZipEntries $DocxPath)) {
        foreach ($token in $Tokens) {
            if (Test-Token $entry.Text $token.Value) {
                $found += "$($entry.Name): $($token.Kind)"
            }
        }
        foreach ($path in $paths) {
            if ($entry.Text.IndexOf($path, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
                $found += "$($entry.Name): a user profile path"
            }
        }
    }
    # PDF strings are bytes, written literally or in hex: look for the UTF-8 and the
    # UTF-16BE spelling of every token in both forms.
    $latin1 = [System.Text.Encoding]::GetEncoding(28591)
    $pdf = $latin1.GetString([System.IO.File]::ReadAllBytes($PdfPath))
    foreach ($token in $Tokens) {
        $utf8 = [System.Text.Encoding]::UTF8.GetBytes($token.Value)
        $utf16 = [System.Text.Encoding]::BigEndianUnicode.GetBytes($token.Value)
        # Only the UTF-8 spelling is matched as a whole word: UTF-16 text has a NUL or a
        # byte order mark before every character, and hex strings run on without
        # boundaries. Both are specific enough to match as they are.
        $plain = @(
            $latin1.GetString($utf16),
            [System.BitConverter]::ToString($utf8).Replace('-', ''),
            [System.BitConverter]::ToString($utf16).Replace('-', '')
        )
        $hit = Test-Token $pdf $latin1.GetString($utf8)
        foreach ($form in $plain) {
            if ($pdf.IndexOf($form, [StringComparison]::OrdinalIgnoreCase) -ge 0) { $hit = $true }
        }
        if ($hit) { $found += "pdf: $($token.Kind)" }
    }
    foreach ($path in $paths) {
        if ($pdf.IndexOf($path, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
            $found += 'pdf: a user profile path'
        }
    }
    return @($found | Sort-Object -Unique)
}

function Get-PrinterName([string] $Printer) {
    # Word's ActivePrinter reads "<name> on <port>:", with "on" in Word's own language (a
    # German Word writes "auf"); the port is the last word. Keep the name.
    if ($Printer -match '^(.*) \S+ [^ :]+:$') { return $Matches[1] }
    return $Printer
}

function Get-DefaultPrinter {
    # The Windows default printer, from the registry ("<name>,winspool,<port>:"). Reading it
    # asks no printer driver, so it cannot hang.
    $key = Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows NT\CurrentVersion\Windows' -ErrorAction SilentlyContinue
    if ($key -and ($key.PSObject.Properties.Name -contains 'Device') -and $key.Device) {
        return ([string]$key.Device).Split(',')[0]
    }
    return ''
}

function Get-OwnWordProcess([DateTime] $Since, [string] $Name = 'WINWORD.EXE', [string] $Pattern = '[/-]Automation') {
    # The WINWORD processes COM started (/Automation on the command line) since $Since. The
    # script refuses to start while Word runs, so these belong to it; a Word the user opens
    # has no /Automation and is never stopped. -Name and -Pattern exist for the tests.
    $ids = @()
    $all = @(Get-CimInstance -ClassName Win32_Process -Filter "Name = '$Name'" -ErrorAction SilentlyContinue)
    foreach ($process in $all) {
        if ($process.CreationDate -ge $Since -and ([string]$process.CommandLine) -match $Pattern) {
            $ids += [int]$process.ProcessId
        }
    }
    return $ids
}

function Wait-ProcessExit([int] $Id, [int] $Seconds) {
    $until = (Get-Date).AddSeconds($Seconds)
    while (Get-Process -Id $Id -ErrorAction SilentlyContinue) {
        if ((Get-Date) -gt $until) { return $false }
        Start-Sleep -Milliseconds 200
    }
    return $true
}

function Stop-ProcessVerified([int] $Id, [int] $GraceSeconds = 10) {
    # Stops a process and checks that it is gone; returns Stopped and what was tried, with
    # every error. An unchecked Stop-Process that failed silently once let a hung Word
    # outlive the time-out.
    if (-not (Get-Process -Id $Id -ErrorAction SilentlyContinue)) {
        return New-Object PSObject -Property @{ Stopped = $true; Detail = "process $Id had already exited" }
    }
    $tried = @()
    try {
        Stop-Process -Id $Id -Force -ErrorAction Stop
        $tried += 'Stop-Process ok'
    } catch {
        $tried += "Stop-Process failed: $($_.Exception.Message)"
    }
    if (Wait-ProcessExit $Id $GraceSeconds) {
        return New-Object PSObject -Property @{ Stopped = $true; Detail = "stopped process $Id ($($tried -join '; '))" }
    }
    $taskkill = Join-Path $env:SystemRoot 'System32\taskkill.exe'
    $output = (& $taskkill /PID $Id /F /T 2>&1 | Out-String).Trim()
    $tried += "taskkill: $output"
    if (Wait-ProcessExit $Id $GraceSeconds) {
        return New-Object PSObject -Property @{ Stopped = $true; Detail = "stopped process $Id ($($tried -join '; '))" }
    }
    return New-Object PSObject -Property @{ Stopped = $false; Detail = "process $Id is still running ($($tried -join '; '))" }
}

function New-WordWorker($Progress) {
    # A single-threaded apartment of its own for Word's COM objects, so that this thread
    # never waits on Word without a time limit.
    $runspace = [System.Management.Automation.Runspaces.RunspaceFactory]::CreateRunspace()
    $runspace.ApartmentState = [System.Threading.ApartmentState]::STA
    $runspace.ThreadOptions = [System.Management.Automation.Runspaces.PSThreadOptions]::ReuseThread
    $runspace.Open()
    $runspace.SessionStateProxy.SetVariable('progress', $Progress)
    $worker = [PowerShell]::Create()
    $worker.Runspace = $runspace
    return $worker
}

function Get-InnerMessage($Exception) {
    while ($null -ne $Exception.InnerException) { $Exception = $Exception.InnerException }
    return $Exception.Message
}

function Invoke-WordStep($Worker, [string] $Script, [object[]] $Arguments, [int] $TimeoutSeconds, [scriptblock] $Targets, $Progress) {
    # Runs $Script in the worker and waits at most $TimeoutSeconds. The result's Status is
    #   'done'    with Output;
    #   'error'   with Message: the script failed;
    #   'timeout' with Message: no answer in time, the processes $Targets lists were stopped
    #             and the worker gave up;
    #   'stuck'   with Message: the worker is still blocked after that; only ending this
    #             process frees it.
    $Worker.Commands.Clear()
    $Worker.Streams.ClearStreams()
    [void]$Worker.AddScript($Script)
    foreach ($argument in $Arguments) { [void]$Worker.AddArgument($argument) }
    $Progress.Step = ''
    $handle = $Worker.BeginInvoke()
    if ($handle.AsyncWaitHandle.WaitOne($TimeoutSeconds * 1000)) {
        try {
            $output = $Worker.EndInvoke($handle)
            return New-Object PSObject -Property @{ Status = 'done'; Output = $output; Message = '' }
        } catch {
            return New-Object PSObject -Property @{ Status = 'error'; Output = $null; Message = (Get-InnerMessage $_.Exception) }
        }
    }
    $details = @()
    foreach ($id in @(& $Targets)) { $details += (Stop-ProcessVerified $id).Detail }
    if ($details.Count -eq 0) { $details += 'found no Word process of this script to stop' }
    $message = "no answer within $TimeoutSeconds seconds, during: $($Progress.Step); $($details -join '; ')"
    if ($handle.AsyncWaitHandle.WaitOne(15000)) {
        try { [void]$Worker.EndInvoke($handle) } catch { }
        return New-Object PSObject -Property @{ Status = 'timeout'; Output = $null; Message = $message }
    }
    return New-Object PSObject -Property @{ Status = 'stuck'; Output = $null; Message = $message }
}

function Exit-Stuck([string] $Message, [string] $Staging) {
    Write-Host "make-reference: Word does not answer and could not be stopped: $Message" -ForegroundColor Red
    Write-Host 'End WINWORD.EXE in Task Manager. Results of the documents that finished are in the output folder; start the script again to render the rest.' -ForegroundColor Red
    if ($Staging -and (Test-Path -LiteralPath $Staging)) {
        Remove-Item -LiteralPath $Staging -Recurse -Force -ErrorAction SilentlyContinue
    }
    # The worker thread is blocked in a call into Word; only ending the process frees it.
    [Environment]::Exit(2)
}

# Scripts the worker runs. Word's objects stay in the worker's session ($global:word); the
# shared $progress.Step names the call in progress for the time-out message.

$StartWordScript = @'
$ErrorActionPreference = 'Stop'
$progress.Step = 'starting Word'
$global:word = New-Object -ComObject Word.Application
$progress.Step = 'setting up Word'
$global:word.Visible = $false
$global:word.DisplayAlerts = 0            # wdAlertsNone
$global:word.AutomationSecurity = 3       # msoAutomationSecurityForceDisable, this session only
$progress.Step = 'reading Word''s font list'
$fonts = New-Object 'System.Collections.Generic.List[string]'
foreach ($name in $global:word.FontNames) { $fonts.Add([string]$name) }
$progress.Step = 'reading Word''s printer'
New-Object PSObject -Property @{
    Version = [string]$global:word.Version
    Build = [string]$global:word.Build
    ActivePrinter = [string]$global:word.ActivePrinter
    UserName = [string]$global:word.UserName
    Fonts = $fonts.ToArray()
}
'@

$RenderScript = @'
param($Staged, $Resaved, $Pdf)
$ErrorActionPreference = 'Stop'
$word = $global:word
$progress.Step = 'counting open documents'
if ($word.Documents.Count -gt 0) {
    # Not ours: every document this script opens is closed again. Windows handed a
    # document the user opened to this instance.
    return New-Object PSObject -Property @{ Foreign = $true; Pages = 0; Mode = 0 }
}
# Documents.Open has 16 parameters and COM calls from PowerShell are positional:
# FileName, ConfirmConversions, ReadOnly, AddToRecentFiles; the rest keep their defaults.
$progress.Step = 'Documents.Open'
$doc = $word.Documents.Open($Staged, $false, $true, $false)
try {
    if ($word.ProtectedViewWindows.Count -gt 0) { throw 'Word opened the document in Protected View' }
    $doc.RemovePersonalInformation = $true
    # Lay the document out first: w:lastRenderedPageBreak records the pagination at the
    # time of saving.
    $progress.Step = 'Repaginate before saving'
    $doc.Repaginate()
    # SaveAs2: FileName, FileFormat (wdFormatXMLDocument), LockComments, Password,
    # AddToRecentFiles. CompatibilityMode is left out, which keeps the document's mode.
    $progress.Step = 'SaveAs2'
    $doc.SaveAs2($Resaved, 12, $false, '', $false)
} finally {
    $progress.Step = 'closing the document'
    $doc.Close(0)                         # wdDoNotSaveChanges
    [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($doc)
}
# Export from the bytes that will be committed.
$progress.Step = 'Documents.Open (re-saved copy)'
$doc = $word.Documents.Open($Resaved, $false, $true, $false)
try {
    $progress.Step = 'Repaginate before exporting'
    $doc.Repaginate()
    $progress.Step = 'ComputeStatistics'
    $pages = [int]$doc.ComputeStatistics(2)       # wdStatisticPages
    $mode = [int]$doc.CompatibilityMode
    # ExportAsFixedFormat: OutputFileName, ExportFormat (wdExportFormatPDF), OpenAfterExport,
    # OptimizeFor (print), Range (whole document), From, To (both ignored), Item (content),
    # IncludeDocProps, KeepIRM, CreateBookmarks (none), DocStructureTags (off keeps the
    # content stream in reading order for pdftotext), BitmapMissingFonts (off references
    # fonts that forbid embedding instead of drawing them as bitmaps), UseISO19005_1.
    $progress.Step = 'ExportAsFixedFormat'
    $doc.ExportAsFixedFormat($Pdf, 17, $false, 0, 0, 1, 1, 0, $false, $false, 0, $false, $false, $false)
} finally {
    $progress.Step = 'closing the re-saved copy'
    $doc.Close(0)
    [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($doc)
}
New-Object PSObject -Property @{ Foreign = $false; Pages = $pages; Mode = $mode }
'@

$CloseWordScript = @'
$ErrorActionPreference = 'Stop'
$result = 'not started'
if ($null -ne $global:word) {
    $progress.Step = 'counting open documents'
    if ($global:word.Documents.Count -gt 0) {
        # Documents the user opened in this instance: quitting would discard them.
        $global:word.Visible = $true
        $result = 'left running'
    } else {
        $progress.Step = 'Quit'
        $global:word.Quit(0)              # wdDoNotSaveChanges
        $result = 'quit'
    }
    [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($global:word)
    $global:word = $null
}
[System.GC]::Collect()
[System.GC]::WaitForPendingFinalizers()
$result
'@

# 1. Inputs and output folder.

$outDir = Resolve-UserPath $OutputDir
if ($outDir.StartsWith('\\')) {
    Stop-WithError "-OutputDir must be a folder on a local disk, not the network path $outDir"
}
foreach ($oneDrive in @($env:OneDrive, $env:OneDriveCommercial, $env:OneDriveConsumer)) {
    if ($oneDrive -and $outDir.StartsWith($oneDrive.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) {
        Stop-WithError "-OutputDir is inside OneDrive ($oneDrive); choose a folder such as C:\officewright-ref\out"
    }
}
$drive = New-Object System.IO.DriveInfo ([System.IO.Path]::GetPathRoot($outDir))
if ($drive.DriveType -ne [System.IO.DriveType]::Fixed) {
    Stop-WithError "-OutputDir must be on a fixed local disk; $($drive.Name) is $($drive.DriveType)"
}

$inputs = @()
foreach ($item in $InputPath) {
    $path = Resolve-UserPath $item
    if (Test-Path -LiteralPath $path -PathType Container) {
        # Leave out Word's lock files and the re-saved copies this script writes.
        $inputs += @(Get-ChildItem -LiteralPath $path -File -Filter $Pattern | Where-Object {
                -not $_.Name.StartsWith('~$') -and -not $_.Name.EndsWith('.resaved.docx') })
    } elseif (Test-Path -LiteralPath $path -PathType Leaf) {
        $inputs += Get-Item -LiteralPath $path
    } else {
        Stop-WithError "input $path does not exist"
    }
}
$inputs = @($inputs | Sort-Object Name)
if ($inputs.Count -eq 0) {
    Stop-WithError "no documents matching $Pattern in $($InputPath -join ', ')"
}
$duplicates = @($inputs | Group-Object { $_.BaseName.ToLowerInvariant() } | Where-Object { $_.Count -gt 1 })
if ($duplicates.Count -gt 0) {
    Stop-WithError "two inputs share the name $($duplicates[0].Name); results would overwrite each other"
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
Add-Type -AssemblyName System.Drawing

$wordRunning = @(Get-Process -Name WINWORD -ErrorAction SilentlyContinue)
if ($wordRunning.Count -gt 0) {
    $message = 'Microsoft Word is running. Close every Word window (and any WINWORD.EXE left in Task Manager) and start the script again.'
    if (-not $DryRun) { Stop-WithError $message }
    Write-Host "make-reference: $message" -ForegroundColor Yellow
}

# 2. Fonts. Windows' own list now; Word's list (which includes Office cloud fonts such as
# Aptos) once Word runs.

$windowsFonts = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
foreach ($family in (New-Object System.Drawing.Text.InstalledFontCollection).Families) {
    [void]$windowsFonts.Add($family.Name)
}
$plan = @()
foreach ($file in $inputs) {
    $fonts = @(Get-DocumentFonts $file.FullName)
    $skip = $false
    $json = Join-Path $outDir ($file.BaseName + '.json')
    if (-not $Overwrite -and (Test-Path -LiteralPath $json)) {
        try {
            $skip = (Get-Content -LiteralPath $json -Raw | ConvertFrom-Json).status -eq 'rendered'
        } catch {
            $skip = $false
        }
    }
    $plan += New-Object PSObject -Property @{
        File = $file
        Fonts = $fonts
        NotInWindows = @(Get-MissingFonts $fonts $windowsFonts)
        Skip = $skip
    }
}

Write-Host "Documents: $($inputs.Count) in, results to $outDir"
foreach ($entry in $plan) {
    $state = 'render'
    if ($entry.Skip) { $state = 'skip (rendered before; -Overwrite renders it again)' }
    Write-Host ("  {0,-34} {1}" -f $entry.File.Name, $state)
    if ($entry.NotInWindows.Count -gt 0) {
        Write-Host ("  {0,-34} fonts not installed in Windows: {1}" -f '', ($entry.NotInWindows -join ', ')) -ForegroundColor Yellow
    }
}
# 3. Printer. A printer that does not answer can stall Word, and Word uses the printer's
# metrics for layout when a document sets w:usePrinterMetrics.

$defaultPrinter = Get-DefaultPrinter
Write-Host "Default printer: $defaultPrinter"
if ($defaultPrinter -ne $ReferencePrinter) {
    Write-Host "Warning: the default printer is not $ReferencePrinter. A printer that does not answer can stall Word; make $ReferencePrinter the default printer (Settings, Bluetooth & devices, Printers & scanners) before making references." -ForegroundColor Yellow
}
$spooler = Get-Service -Name Spooler -ErrorAction SilentlyContinue
if (-not $spooler -or $spooler.Status -ne 'Running') {
    Write-Host 'Warning: the Print Spooler service is not running; Word may wait for it.' -ForegroundColor Yellow
}

if ($DryRun) {
    Write-Host 'Dry run: Word was not started. Fonts not installed in Windows may still be Office cloud fonts that Word has.'
    exit 0
}

# 4. Render.

New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$staging = Join-Path $outDir '.staging'
if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
New-Item -ItemType Directory -Path $staging | Out-Null

$osKey = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' -ErrorAction SilentlyContinue
$windowsVersion = [Environment]::OSVersion.Version.ToString()
$windowsDisplay = ''
if ($osKey -and ($osKey.PSObject.Properties.Name -contains 'DisplayVersion')) { $windowsDisplay = [string]$osKey.DisplayVersion }

# Every WINWORD with /Automation started from now on is the instance this script starts.
$since = (Get-Date).AddSeconds(-1)
$targets = { Get-OwnWordProcess $since }
$progress = [hashtable]::Synchronized(@{ Step = '' })
$worker = New-WordWorker $progress
$results = @()
$aborted = $false
$userDocuments = $false
$leftRunning = $false
try {
    $start = Invoke-WordStep $worker $StartWordScript @() $TimeoutSeconds $targets $progress
    if ($start.Status -eq 'stuck') { Exit-Stuck $start.Message $staging }
    if ($start.Status -ne 'done') { throw "Word did not start: $($start.Message)" }
    $info = $start.Output[0]
    $wordIds = @(Get-OwnWordProcess $since)
    $printer = Get-PrinterName $info.ActivePrinter
    Write-Host "Word $($info.Version), build $($info.Build), process $($wordIds -join ', '); Windows $windowsVersion $windowsDisplay"
    Write-Host "Word's printer: $printer"
    if ($printer -ne $ReferencePrinter) {
        Write-Host "Warning: Word's printer is $printer, not $ReferencePrinter." -ForegroundColor Yellow
    }
    if ($wordIds.Count -eq 1) {
        # From now on only this process is stopped, not another Word that some other COM
        # client starts meanwhile.
        $targets = { $wordIds }
    } else {
        Write-Host "Warning: expected one Word process started by this script, found $($wordIds.Count)." -ForegroundColor Yellow
    }

    $wordFonts = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    foreach ($name in $windowsFonts) { [void]$wordFonts.Add($name) }
    foreach ($name in $info.Fonts) { [void]$wordFonts.Add([string]$name) }

    # Personal strings that must not reach the repository, with a description that may.
    # Names are always searched for. Machine names are skipped when the input contains
    # them, because then they are words of the document rather than something Word added.
    $candidates = @(
        @($env:USERNAME, 'the Windows user name', $true),
        @($env:USERPROFILE, 'the user profile path', $true),
        @([string]$info.UserName, 'the Office user name', $true),
        @($env:COMPUTERNAME, 'the computer name', $false),
        @($env:USERDOMAIN, 'the Windows domain', $false)
    )
    $officeUser = Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Office\Common\UserInfo' -ErrorAction SilentlyContinue
    foreach ($source in @(
            @($osKey, 'RegisteredOwner', 'the registered owner of Windows'),
            @($osKey, 'RegisteredOrganization', 'the registered organization of Windows'),
            @($officeUser, 'UserName', 'the Office user name'),
            @($officeUser, 'Company', 'the Office company'))) {
        $key = $source[0]
        if ($key -and ($key.PSObject.Properties.Name -contains $source[1])) {
            $candidates += , @([string]$key.($source[1]), $source[2], $true)
        }
    }
    $tokens = @()
    foreach ($candidate in $candidates) {
        $value = [string]$candidate[0]
        # Shorter strings (initials) match too much by chance to be useful.
        if ($value.Trim().Length -ge 4) {
            $tokens += New-Object PSObject -Property @{ Value = $value.Trim(); Kind = $candidate[1]; Always = $candidate[2] }
        }
    }

    foreach ($entry in $plan) {
        $file = $entry.File
        if ($entry.Skip) {
            $results += New-Object PSObject -Property @{ File = $file.Name; Status = 'skipped'; Missing = @() }
            continue
        }
        $base = $file.BaseName
        $staged = Join-Path $staging $file.Name
        $resaved = Join-Path $outDir ($base + '.resaved.docx')
        $pdf = Join-Path $outDir ($base + '.pdf')
        $jsonPath = Join-Path $outDir ($base + '.json')
        foreach ($old in @($resaved, $pdf, $jsonPath)) {
            if (Test-Path -LiteralPath $old) { Remove-Item -LiteralPath $old -Force }
        }
        Copy-Item -LiteralPath $file.FullName -Destination $staged
        Unblock-File -LiteralPath $staged

        $record = [ordered]@{
            script_version = $ScriptVersion
            file = $file.Name
            sha256 = Get-Sha256 $staged
            resaved_file = $base + '.resaved.docx'
            resaved_sha256 = ''
            pdf_file = $base + '.pdf'
            pdf_sha256 = ''
            status = 'failed'
            error = ''
            pages = 0
            compatibility_mode = 0
            fonts = @($entry.Fonts)
            fonts_missing = @()
            privacy_findings = @()
            word_version = [string]$info.Version
            word_build = [string]$info.Build
            printer = $printer
            default_printer = $defaultPrinter
            windows_version = $windowsVersion
            windows_display_version = $windowsDisplay
            rendered = (Get-Date).ToString('yyyy-MM-dd')
            export_seconds = 0
        }
        Write-Host "Rendering $($file.Name) ..."
        $started = Get-Date
        $step = Invoke-WordStep $worker $RenderScript @($staged, $resaved, $pdf) $TimeoutSeconds $targets $progress
        if ($step.Status -eq 'done' -and $step.Output[0].Foreign) {
            $userDocuments = $true
            Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
            Write-Host 'Stopping: a document was opened in the Word instance this script uses. Word stays open and visible; save your work there, close Word, and start the script again.' -ForegroundColor Red
            break
        }
        if ($step.Status -eq 'done') {
            $record.pages = [int]$step.Output[0].Pages
            $record.compatibility_mode = [int]$step.Output[0].Mode
            $record.export_seconds = [math]::Round(((Get-Date) - $started).TotalSeconds, 1)
            $record.resaved_sha256 = Get-Sha256 $resaved
            $record.pdf_sha256 = Get-Sha256 $pdf
            $record.fonts = @(Get-DocumentFonts $resaved)
            $record.fonts_missing = @(Get-MissingFonts $record.fonts $wordFonts)
            # A machine name the input already contains is a word of the document.
            $inputText = Get-PackageText $staged
            $docTokens = @($tokens | Where-Object { $_.Always -or -not (Test-Token $inputText $_.Value) })
            $record.privacy_findings = @(Find-PersonalData $resaved $pdf $docTokens)
            if ($record.privacy_findings.Count -gt 0) {
                $record.status = 'privacy-failed'
                $record.error = 'personal data in the output; corpus-import.py refuses this document'
            } else {
                $record.status = 'rendered'
            }
        } elseif ($step.Status -eq 'error') {
            $record.error = $step.Message
            if (@(Get-OwnWordProcess $since).Count -eq 0) {
                $record.error = "Word exited: $($step.Message)"
                $aborted = $true
            }
        } else {
            $record.error = "Word stopped: $($step.Message)"
            $aborted = $true
        }
        Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
        Write-JsonFile $jsonPath $record
        if ($step.Status -eq 'stuck') { Exit-Stuck $step.Message $staging }

        $color = 'Green'
        if ($record.status -ne 'rendered') { $color = 'Red' }
        $detail = "$($record.pages) pages, compatibility mode $($record.compatibility_mode)"
        if ($record.status -ne 'rendered') { $detail = $record.error }
        Write-Host "  $($record.status): $detail" -ForegroundColor $color
        foreach ($finding in $record.privacy_findings) { Write-Host "    $finding" -ForegroundColor Red }
        if ($record.fonts_missing.Count -gt 0) {
            Write-Host "    fonts Word does not have: $($record.fonts_missing -join ', ')" -ForegroundColor Yellow
        }
        $results += New-Object PSObject -Property @{ File = $file.Name; Status = $record.status; Missing = $record.fonts_missing }
        if ($aborted) {
            Write-Host 'Stopping: the Word instance is gone. Start the script again to render the remaining documents.' -ForegroundColor Red
            break
        }
    }
} finally {
    $close = Invoke-WordStep $worker $CloseWordScript @() 60 $targets $progress
    if ($close.Status -eq 'stuck') { Exit-Stuck $close.Message $staging }
    if ($close.Status -eq 'done' -and $close.Output[0] -eq 'left running') {
        $leftRunning = $true
        Write-Host 'Word was left running because it holds documents this script did not open.' -ForegroundColor Yellow
    } else {
        # Word exits by itself after Quit; one that does not within ten seconds is stopped.
        foreach ($id in @(Get-OwnWordProcess $since)) {
            if (-not (Wait-ProcessExit $id 10)) {
                Write-Host "Word did not exit after Quit: $((Stop-ProcessVerified $id).Detail)" -ForegroundColor Yellow
            }
        }
    }
    $worker.Runspace.Dispose()
    $worker.Dispose()
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue }
}

# 5. Summary.

$rendered = @($results | Where-Object { $_.Status -eq 'rendered' }).Count
$skipped = @($results | Where-Object { $_.Status -eq 'skipped' }).Count
$failed = @($results | Where-Object { $_.Status -ne 'rendered' -and $_.Status -ne 'skipped' })
$notRun = $plan.Count - $results.Count
$missing = @($results | ForEach-Object { $_.Missing } | Where-Object { $_ } | Sort-Object -Unique)

Write-Host ''
Write-Host "Rendered: $rendered, skipped: $skipped, failed: $($failed.Count), not started: $notRun"
foreach ($item in $failed) { Write-Host "  failed: $($item.File) ($($item.Status))" -ForegroundColor Red }
if ($missing.Count -gt 0) {
    Write-Host "Fonts Word substituted: $($missing -join ', ')" -ForegroundColor Yellow
} else {
    Write-Host 'Fonts Word substituted: none'
}
Write-Host "Results: $outDir"
if ($failed.Count -gt 0 -or $notRun -gt 0 -or $userDocuments) { exit 1 }
exit 0
