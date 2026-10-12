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
      5. writes <name>.json: hashes, Word version and build, page count, compatibility
         mode, and the fonts the document asks for that neither Windows nor Word has.
    scripts/corpus-import.py, run on the Linux side, checks these files again and copies
    them into tests/corpus/.

    The script never touches a Word session the user has open: it refuses to start while
    any WINWORD process runs. Do not open Word documents while it runs: Windows may hand
    them to the invisible Word instance. If that happens, the script stops before the next
    document, makes its Word instance visible and leaves it running instead of closing
    it. It changes no Word option, adds nothing to Word's recent files, and stops the Word
    instance it started (and only that one) when it is done or when a document takes
    longer than -TimeoutSeconds. Existing results are kept unless -Overwrite is given, so
    an interrupted run can simply be started again.

    Run it from Windows PowerShell 5.1 on the machine that has Word, not from WSL.

.PARAMETER InputPath
    Documents to render, or folders whose documents (matching -Pattern) are rendered.

.PARAMETER OutputDir
    Local folder for the results. Not a network path and not inside OneDrive.

.PARAMETER DryRun
    Check the inputs, the output folder and the fonts, print what would be rendered, and
    stop before Word starts.

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
$ScriptVersion = 1

# Word constants (Word VBA reference).
$wdAlertsNone = 0
$msoAutomationSecurityForceDisable = 3
$wdFormatXMLDocument = 12
$wdDoNotSaveChanges = 0
$wdStatisticPages = 2
$wdExportFormatPDF = 17
$wdExportOptimizeForPrint = 0
$wdExportAllDocument = 0
$wdExportDocumentContent = 0
$wdExportCreateNoBookmarks = 0

$WordNs = 'http://schemas.openxmlformats.org/wordprocessingml/2006/main'

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
    $names = @()
    foreach ($entry in (Read-ZipEntries $Path)) {
        if ($entry.Name -ne 'word/fontTable.xml') { continue }
        $xml = New-Object System.Xml.XmlDocument
        $xml.LoadXml($entry.Text)
        $ns = New-Object System.Xml.XmlNamespaceManager($xml.NameTable)
        $ns.AddNamespace('w', $WordNs)
        foreach ($font in $xml.SelectNodes('/w:fonts/w:font', $ns)) {
            $names += $font.GetAttribute('name', $WordNs)
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

function Set-Deadline([string] $File, [int] $Seconds) {
    $ticks = 0
    if ($Seconds -gt 0) { $ticks = [DateTime]::UtcNow.AddSeconds($Seconds).Ticks }
    # The watchdog may be reading the file at this moment; try again briefly.
    for ($attempt = 1; ; $attempt++) {
        try {
            [System.IO.File]::WriteAllText($File, [string]$ticks)
            return
        } catch [System.IO.IOException] {
            if ($attempt -ge 20) { throw }
            Start-Sleep -Milliseconds 50
        }
    }
}

function Release-Com($Object) {
    if ($null -ne $Object) {
        [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($Object)
    }
}

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
if ($DryRun) {
    Write-Host 'Dry run: Word was not started. Fonts not installed in Windows may still be Office cloud fonts that Word has.'
    exit 0
}

# 3. Render.

New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$staging = Join-Path $outDir '.staging'
if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
New-Item -ItemType Directory -Path $staging | Out-Null
$deadlineFile = Join-Path $staging 'deadline'
$stopFile = Join-Path $staging 'stop'
$timeoutFile = Join-Path $staging 'timed-out'
Set-Deadline $deadlineFile 0

$osKey = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' -ErrorAction SilentlyContinue
$windowsVersion = [Environment]::OSVersion.Version.ToString()
$windowsDisplay = ''
if ($osKey -and ($osKey.PSObject.Properties.Name -contains 'DisplayVersion')) { $windowsDisplay = [string]$osKey.DisplayVersion }

$word = $null
$wordPid = 0
$watchdog = $null
$results = @()
$aborted = $false
$userDocuments = $false
try {
    $word = New-Object -ComObject Word.Application
    $wordProcesses = @(Get-Process -Name WINWORD -ErrorAction SilentlyContinue)
    if ($wordProcesses.Count -ne 1) {
        throw "expected the one Word process this script started, found $($wordProcesses.Count)"
    }
    $wordPid = $wordProcesses[0].Id

    # A document that hangs Word (a modal dialog nobody can see) is stopped by a watchdog in
    # a separate process: COM calls block this one. It is armed from here on, so a first-run
    # dialog during the setup below cannot hang the script either.
    $watchdog = Start-Job -ArgumentList $wordPid, $deadlineFile, $stopFile, $timeoutFile -ScriptBlock {
        param($WordPid, $DeadlineFile, $StopFile, $TimeoutFile)
        while (-not (Test-Path -LiteralPath $StopFile)) {
            $deadline = [int64]0
            try {
                # Shared for writing, so the main script can update the file meanwhile.
                $stream = [System.IO.File]::Open($DeadlineFile, 'Open', 'Read', 'ReadWrite')
                try {
                    $deadline = [int64](New-Object System.IO.StreamReader($stream)).ReadToEnd()
                } finally {
                    $stream.Dispose()
                }
            } catch { }
            if ($deadline -gt 0 -and [DateTime]::UtcNow.Ticks -gt $deadline) {
                [System.IO.File]::WriteAllText($TimeoutFile, 'timeout')
                Stop-Process -Id $WordPid -Force -ErrorAction SilentlyContinue
                return
            }
            Start-Sleep -Milliseconds 500
        }
    }
    Set-Deadline $deadlineFile $TimeoutSeconds

    $word.Visible = $false
    $word.DisplayAlerts = $wdAlertsNone
    $word.AutomationSecurity = $msoAutomationSecurityForceDisable
    $wordVersion = [string]$word.Version
    $wordBuild = [string]$word.Build
    Write-Host "Word $wordVersion, build $wordBuild; Windows $windowsVersion $windowsDisplay; printer: $($word.ActivePrinter)"

    $wordFonts = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    foreach ($name in $windowsFonts) { [void]$wordFonts.Add($name) }
    foreach ($name in $word.FontNames) { [void]$wordFonts.Add([string]$name) }

    # Personal strings that must not reach the repository, with a description that may.
    # Names are always searched for. Machine names are skipped when the input contains
    # them, because then they are words of the document rather than something Word added.
    $candidates = @(
        @($env:USERNAME, 'the Windows user name', $true),
        @($env:USERPROFILE, 'the user profile path', $true),
        @([string]$word.UserName, 'the Office user name', $true),
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
    Set-Deadline $deadlineFile 0

    foreach ($entry in $plan) {
        $file = $entry.File
        if ($word.Documents.Count -gt 0) {
            # Not ours: the script closes every document it opens. Windows handed a document
            # the user opened to this instance.
            $userDocuments = $true
            Write-Host 'Stopping: a document was opened in the Word instance this script uses. Word stays open and visible; save your work there, close Word, and start the script again.' -ForegroundColor Red
            break
        }
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
            word_version = $wordVersion
            word_build = $wordBuild
            windows_version = $windowsVersion
            windows_display_version = $windowsDisplay
            rendered = (Get-Date).ToString('yyyy-MM-dd')
            export_seconds = 0
        }
        Write-Host "Rendering $($file.Name) ..."
        $doc = $null
        $started = Get-Date
        try {
            Set-Deadline $deadlineFile $TimeoutSeconds
            # Documents.Open has 16 parameters and COM calls from PowerShell are positional:
            # FileName, ConfirmConversions, ReadOnly, AddToRecentFiles; the rest keep their defaults.
            $doc = $word.Documents.Open($staged, $false, $true, $false)
            if ($word.ProtectedViewWindows.Count -gt 0) {
                throw 'Word opened the document in Protected View'
            }
            $doc.RemovePersonalInformation = $true
            # Lay the document out first: w:lastRenderedPageBreak records the pagination at
            # the time of saving.
            $doc.Repaginate()
            # SaveAs2: FileName, FileFormat, LockComments, Password, AddToRecentFiles. The
            # compatibility mode is left alone (CompatibilityMode defaults to keeping it).
            $doc.SaveAs2($resaved, $wdFormatXMLDocument, $false, '', $false)
            $doc.Close($wdDoNotSaveChanges)
            Release-Com $doc
            $doc = $null

            # Export from the bytes that will be committed.
            $doc = $word.Documents.Open($resaved, $false, $true, $false)
            $doc.Repaginate()
            $record.pages = [int]$doc.ComputeStatistics($wdStatisticPages)
            $record.compatibility_mode = [int]$doc.CompatibilityMode
            # ExportAsFixedFormat: OutputFileName, ExportFormat, OpenAfterExport, OptimizeFor,
            # Range, From, To (ignored for the whole document), Item, IncludeDocProps, KeepIRM,
            # CreateBookmarks, DocStructureTags, BitmapMissingFonts, UseISO19005_1.
            # DocStructureTags off keeps the content stream in reading order for pdftotext;
            # BitmapMissingFonts off references fonts that forbid embedding instead of
            # drawing them as bitmaps.
            $doc.ExportAsFixedFormat($pdf, $wdExportFormatPDF, $false, $wdExportOptimizeForPrint,
                $wdExportAllDocument, 1, 1, $wdExportDocumentContent, $false, $false,
                $wdExportCreateNoBookmarks, $false, $false, $false)
            $doc.Close($wdDoNotSaveChanges)
            Release-Com $doc
            $doc = $null
            Set-Deadline $deadlineFile 0

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
        } catch {
            $record.error = $_.Exception.Message
            if (Test-Path -LiteralPath $timeoutFile) {
                $record.error = "Word did not finish within $TimeoutSeconds seconds and was stopped"
                $aborted = $true
            } elseif (-not (Get-Process -Id $wordPid -ErrorAction SilentlyContinue)) {
                $record.error = "Word exited: $($record.error)"
                $aborted = $true
            }
        } finally {
            if (-not $aborted) { Set-Deadline $deadlineFile 0 }
            if ($null -ne $doc -and -not $aborted) {
                try { $doc.Close($wdDoNotSaveChanges) } catch { }
            }
            Release-Com $doc
            if (Test-Path -LiteralPath $staged) {
                Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
            }
        }
        Write-JsonFile $jsonPath $record
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
    $leaveWord = $false
    if ($null -ne $word) {
        try {
            if ($word.Documents.Count -gt 0) {
                # Documents the user opened in this instance: quitting would discard them.
                $leaveWord = $true
                $word.Visible = $true
            }
        } catch { }
        if (-not $leaveWord) {
            try { $word.Quit($wdDoNotSaveChanges) } catch { }
        }
        Release-Com $word
        $word = $null
    }
    [System.GC]::Collect()
    [System.GC]::WaitForPendingFinalizers()
    if ($leaveWord) {
        Write-Host 'Word was left running because it holds documents this script did not open.' -ForegroundColor Yellow
    } elseif ($wordPid -ne 0) {
        $process = Get-Process -Id $wordPid -ErrorAction SilentlyContinue
        if ($process -and -not $process.WaitForExit(10000)) {
            Stop-Process -Id $wordPid -Force -ErrorAction SilentlyContinue
        }
    }
    if ($null -ne $watchdog) {
        [System.IO.File]::WriteAllText($stopFile, 'stop')
        Stop-Job -Job $watchdog -ErrorAction SilentlyContinue
        Remove-Job -Job $watchdog -Force -ErrorAction SilentlyContinue
    }
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue }
}

# 4. Summary.

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
