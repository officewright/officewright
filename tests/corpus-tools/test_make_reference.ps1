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

function Stop-Tree($Process) {
    # cmd.exe with its ping.exe child.
    $taskkill = Join-Path $env:SystemRoot 'System32\taskkill.exe'
    if (-not $Process.HasExited) { & $taskkill /PID $Process.Id /F /T 2>&1 | Out-Null }
}

$temp = Join-Path ([System.IO.Path]::GetTempPath()) ("make-reference-test-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $temp | Out-Null
try {
    # Printer names.
    Check 'Word printer name loses its port' ((Get-PrinterName 'Microsoft Print to PDF on Ne01:') -eq 'Microsoft Print to PDF')
    Check 'printer name without a port stays' ((Get-PrinterName 'Microsoft Print to PDF') -eq 'Microsoft Print to PDF')
    Check 'network printer name loses its port' ((Get-PrinterName '\\print01\Floor 3 on Ne02:') -eq '\\print01\Floor 3')
    Check 'German Word printer name loses its port' ((Get-PrinterName 'Microsoft Print to PDF auf Ne01:') -eq 'Microsoft Print to PDF')
    $russianOn = -join ([char]0x043D, [char]0x0430)
    Check 'Russian Word printer name loses its port' ((Get-PrinterName "Microsoft Print to PDF $russianOn Ne03:") -eq 'Microsoft Print to PDF')
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

    # Which processes count as this script's Word: started since the given time, with the
    # marker on the command line. cmd.exe plays WINWORD.EXE, "rem /Automation" the marker.
    Check 'no Word process of this script exists yet' (@(Get-OwnWordProcess (Get-Date)).Count -eq 0)
    $cmd = Join-Path $env:SystemRoot 'System32\cmd.exe'
    $marker = 'rem officewright-test-' + [guid]::NewGuid().ToString('N') + ' /Automation'
    $before = Start-Process -FilePath $cmd -ArgumentList '/c', "ping -n 600 127.0.0.1 & $marker" -WindowStyle Hidden -PassThru
    Start-Sleep -Seconds 2
    $since = Get-Date
    $marked = Start-Process -FilePath $cmd -ArgumentList '/c', "ping -n 600 127.0.0.1 & $marker" -WindowStyle Hidden -PassThru
    $unmarked = Start-Process -FilePath $cmd -ArgumentList '/c', 'ping -n 600 127.0.0.1' -WindowStyle Hidden -PassThru
    try {
        $pattern = [regex]::Escape($marker)
        $own = @(Get-OwnWordProcess $since 'cmd.exe' $pattern)
        Check 'a process started since then with the marker is selected' ($own -contains $marked.Id) ($own -join ', ')
        Check 'a process started before is not selected' (-not ($own -contains $before.Id)) ($own -join ', ')
        Check 'a process without the marker is not selected' (-not ($own -contains $unmarked.Id)) ($own -join ', ')
    } finally {
        foreach ($process in @($before, $marked, $unmarked)) { Stop-Tree $process }
    }

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

    # Word's computed formatting, read from objects that stand in for Word's.
    Check 'automatic colour' ((ConvertTo-HexColor -16777216 0) -eq 'auto')
    Check 'mixed colour' ($null -eq (ConvertTo-HexColor 9999999 0))
    Check 'red from RGB' ((ConvertTo-HexColor 255 255) -eq 'FF0000')
    Check 'blue from RGB' ((ConvertTo-HexColor 16711680 16711680) -eq '0000FF')
    Check 'a theme colour takes TextColor.RGB' ((ConvertTo-HexColor -738132071 0x336699) -eq '996633')
    Check 'Word true' ((ConvertTo-Flag -1) -eq $true)
    Check 'Word false' ((ConvertTo-Flag 0) -eq $false)
    Check 'mixed flag' ($null -eq (ConvertTo-Flag 9999999))
    Check 'points rounded to 0.01' ((ConvertTo-PointValue 12.3456) -eq 12.35)
    Check 'mixed length' ($null -eq (ConvertTo-PointValue 9999999))
    Check 'alignment names' ((Get-AlignmentName 3) -eq 'justify' -and (Get-AlignmentName 6) -eq 'other-6')
    Check 'line rule names' ((Get-LineRuleName 4) -eq 'exactly' -and (Get-LineRuleName 5) -eq 'multiple')
    Check 'separator stories are left out' ($null -eq (Get-StoryName 12) -and (Get-StoryName 7) -eq 'primary-header')
    $excerpt = Get-TextExcerpt ("Name:`tValue" + [char]11 + 'x' * 70 + [char]13 + [char]7)
    Check 'excerpt without marks, controls as spaces, 60 characters' ($excerpt.Length -eq 60 -and $excerpt.StartsWith('Name: Value x'))

    function New-Font($Name, $Size, $Bold = 0, $Italic = 0, $Color = -16777216, $Rgb = 0, $ThemeIndex = -1, $TintShade = 0) {
        $textColor = New-Object PSObject -Property @{ RGB = $Rgb; ObjectThemeColor = $ThemeIndex; TintAndShade = $TintShade }
        return New-Object PSObject -Property @{ Name = $Name; Size = $Size; Bold = $Bold; Italic = $Italic; Color = $Color; TextColor = $textColor }
    }
    # Word's font box placeholders are in the language of its user interface; the stand-in
    # classifies a German one, as Word would through a scratch document.
    $classified = New-Object System.Collections.ArrayList
    $theme = @{
        Fonts = @{ minorHAnsi = 'Calibri'; majorEastAsia = '' }
        Seen = @{}
        Classify = {
            param($Placeholder)
            [void]$classified.Add($Placeholder)
            switch ($Placeholder) { '+Textkoerper' { 'minorHAnsi' } '+Ueberschriften Asiatisch' { 'majorEastAsia' } '+Kaputt' { throw 'no' } default { $null } }
        }.GetNewClosure()
    }
    $font = Get-FontRecord (New-Font '+Textkoerper' 11 -1 0 255 255) $theme
    Check 'a theme font is classified and named' ($font.name -eq 'Calibri' -and $font.theme -eq 'minorHAnsi' -and $font.bold -eq $true -and $font.color -eq 'FF0000')
    [void](Get-FontRecord (New-Font '+Textkoerper' 11) $theme)
    Check 'a placeholder is classified once' ($classified.Count -eq 1) "$($classified.Count)"
    $unknown = Get-FontRecord (New-Font '+Unbekannt' 11) $theme
    $broken = Get-FontRecord (New-Font '+Kaputt' 11) $theme
    $empty = Get-FontRecord (New-Font '+Ueberschriften Asiatisch' 11) $theme
    Check 'a placeholder never leaves: unresolved has no name' ($unknown.theme -eq 'unresolved' -and $null -eq $unknown.name -and $broken.theme -eq 'unresolved' -and $null -eq $broken.name)
    Check 'a theme without that font gives no name' ($empty.theme -eq 'majorEastAsia' -and $null -eq $empty.name)
    $flat = '<pkg:part pkg:name="/word/styles.xml"><w:rFonts w:asciiTheme="majorHAnsi"/></pkg:part>' +
        '<pkg:part pkg:name="/word/document.xml"><w:p><w:pPr><w:rPr><w:rFonts w:asciiTheme="majorBidi"/></w:rPr></w:pPr>' +
        '<w:r><w:rPr><w:rFonts w:asciiTheme="minorEastAsia" w:hAnsiTheme="minorEastAsia"/></w:rPr><w:t>x</w:t></w:r></w:p></pkg:part>'
    Check 'the theme kind is read from the run x only' ((Get-ThemeKind $flat) -eq 'minorEastAsia')
    $complex = '<pkg:part pkg:name="/word/document.xml"><w:r><w:rPr><w:rFonts w:cstheme="majorBidi"/></w:rPr><w:t>x</w:t></w:r></pkg:part>'
    Check 'another theme attribute is taken when asciiTheme is missing' ((Get-ThemeKind $complex) -eq 'majorBidi')
    Check 'no theme on the run gives null' ($null -eq (Get-ThemeKind '<pkg:part pkg:name="/word/document.xml"><w:r><w:t>x</w:t></w:r></pkg:part>'))
    # The first K2 run: Word gave no RGB value for theme colours.
    $accent = Get-FontRecord (New-Font 'Arial' 11 0 0 -738132071 -738132071 4 -0.25) $theme
    Check 'a theme colour without RGB names the theme colour and its shade' ($null -eq $accent.color -and $accent.theme_color -eq 'accent1' -and $accent.tint_shade -eq -0.25) "$($accent.theme_color) $($accent.tint_shade)"
    $plain = Get-FontRecord (New-Font 'Arial' 11 0 0 255 255) $theme
    Check 'a colour with RGB has no theme colour' ($plain.color -eq 'FF0000' -and -not $plain.Contains('theme_color'))
    $unknown = Get-FontRecord (New-Font 'Arial' 11 0 0 -738132071 -738132071 -1) $theme
    Check 'no theme colour index, no theme colour' (-not $unknown.Contains('theme_color') -and $null -eq $unknown.color)
    # WdThemeColorIndex 0 to 15 in Word's VBA reference, as ST_ThemeColor (ECMA-376 17.18.97).
    $expected = 'dark1,light1,dark2,light2,accent1,accent2,accent3,accent4,accent5,accent6,hyperlink,followedHyperlink,background1,text1,background2,text2'
    $table = @(0..15 | ForEach-Object { Get-ThemeColorName $_ }) -join ','
    Check 'theme colour names' ($table -eq $expected -and $null -eq (Get-ThemeColorName 16) -and $null -eq (Get-ThemeColorName -1)) $table
    $both = Get-FontRecord (New-Font 'Arial' 11 0 0 -738132071 0x336699 4 -0.25) $theme
    Check 'a theme colour Word gives RGB for keeps the RGB only' ($both.color -eq '996633' -and -not $both.Contains('theme_color'))
    $throwing = New-Object PSObject -Property @{ RGB = $null; TintAndShade = 0 }
    $throwing | Add-Member ScriptProperty ObjectThemeColor { throw 'not available' }
    $odd = New-Object PSObject -Property @{ Name = 'Arial'; Size = 11; Bold = 0; Italic = 0; Color = -738132071; TextColor = $throwing }
    $oddRecord = Get-FontRecord $odd $theme
    Check 'a theme colour Word cannot name is left out' ($null -eq $oddRecord.color -and -not $oddRecord.Contains('theme_color'))
    $noTint = New-Object PSObject -Property @{ RGB = $null; ObjectThemeColor = 4 }
    $noTint | Add-Member ScriptProperty TintAndShade { throw 'not available' }
    $noTintRecord = Get-FontRecord (New-Object PSObject -Property @{ Name = 'Arial'; Size = 11; Bold = 0; Italic = 0; Color = -738132071; TextColor = $noTint }) $theme
    Check 'a theme colour without its tint or shade is left out' (-not $noTintRecord.Contains('theme_color'))
    $mixed = Get-FontRecord (New-Font '' 9999999 9999999 0) $theme
    Check 'mixed font values are null' ($null -eq $mixed.name -and $null -eq $mixed.size -and $null -eq $mixed.bold)

    # A range over runs of formatting: Start and End are character positions, MoveEnd(13)
    # moves the end to the next run boundary, as Word's wdCharacterFormatting does.
    # -Quirky stands in for what the first K2 run showed of Word through PowerShell: a
    # Duplicate that is the same range, and a Collapse that ignores its direction.
    # -LosesUnit stands in for a Word that drops MoveEnd's unit and count, as a by-reference
    # argument PowerShell does not pass would: it moves one character. Font is mixed for a
    # range over more than one run or past -Limit, as Word's is past the paragraph's end.
    function New-Range($Runs, [int] $Start, [int] $End, [int] $Step = 13, [int] $Delay = 0, [switch] $Quirky, [switch] $LosesUnit, [int] $Limit = [int]::MaxValue) {
        $range = New-Object PSObject -Property @{ Runs = $Runs; Start = $Start; End = $End; Unit = $Step; Delay = $Delay; Quirky = [bool]$Quirky; LosesUnit = [bool]$LosesUnit; Limit = $Limit }
        $range | Add-Member ScriptProperty Duplicate {
            if ($this.Quirky) { return $this }
            New-Range $this.Runs $this.Start $this.End $this.Unit $this.Delay -LosesUnit:$this.LosesUnit -Limit $this.Limit
        }
        $range | Add-Member ScriptMethod Collapse {
            param($Direction)
            if ($Direction -eq 1 -or $this.Quirky) { $this.End = $this.Start } else { $this.Start = $this.End }
        }
        $range | Add-Member ScriptMethod SetRange { param($From, $To) $this.Start = $From; $this.End = $To }
        $range | Add-Member ScriptMethod MoveEnd {
            param($Unit, $Count)
            if ($this.Delay -gt 0) { Start-Sleep -Milliseconds $this.Delay }
            if ($this.LosesUnit) { $this.End = $this.End + 1; return 1 }
            if ($Unit -ne $this.Unit) { return 0 }
            $position = 0
            foreach ($run in $this.Runs) {
                $position += $run.Length
                if ($position -gt $this.End) { $this.End = $position; return 1 }
            }
            return 0
        }
        $range | Add-Member ScriptProperty Font {
            $position = 0
            foreach ($run in $this.Runs) {
                if ($this.Start -ge $position -and $this.Start -lt $position + $run.Length) {
                    if ($this.End -gt $position + $run.Length -or $this.End -gt $this.Limit) { return New-Font '' 9999999 9999999 9999999 9999999 }
                    return $run.Font
                }
                $position += $run.Length
            }
        }
        return $range
    }
    $runs = @(
        (New-Object PSObject -Property @{ Length = 5; Font = (New-Font 'Arial' 10) }),
        (New-Object PSObject -Property @{ Length = 3; Font = (New-Font 'Arial' 10 -1) }),
        (New-Object PSObject -Property @{ Length = 1; Font = (New-Font 'Arial' 12) })
    )
    $later = [DateTime]::UtcNow.AddMinutes(5)
    $read = Get-RunRecords (New-Range $runs 0 9) $later $theme
    Check 'runs of formatting are read with their lengths and fonts' ($read.Count -eq 3 -and $read[0].length -eq 5 -and $read[1].font.bold -eq $true -and $read[2].font.size -eq 12) "$($read.Count)"
    $part = Get-RunRecords (New-Range $runs 0 7 -Limit 7) $later $theme
    Check 'a run past the range end is cut at the end, its font read inside it' ($part.Count -eq 2 -and $part[1].length -eq 2 -and $part[1].font.bold -eq $true) "$($part.Count)"
    Check 'runs past the deadline give null' ($null -eq (Get-RunRecords (New-Range $runs 0 9) ([DateTime]::UtcNow.AddSeconds(-1)) $theme))
    $refused = $false
    try { [void](Get-RunRecords (New-Range $runs 0 9 99) $later $theme) } catch { $refused = $true }
    Check 'a unit Word does not move over is an error, not an endless loop' $refused
    $quirky = Get-RunRecords (New-Range $runs 0 9 -Quirky) $later $theme
    Check 'runs are read when Duplicate shares the range and Collapse ignores its direction' ($quirky.Count -eq 3 -and $quirky[0].length -eq 5 -and $quirky[1].length -eq 3 -and $quirky[2].length -eq 1 -and $quirky[1].font.bold -eq $true) "$($quirky.Count)"
    $middle = Get-RunRecords (New-Range $runs 5 9 -Quirky) $later $theme
    Check 'a paragraph that starts after the first run' ($middle.Count -eq 2 -and $middle[0].length -eq 3 -and $middle[1].font.size -eq 12) "$($middle.Count)"
    # A Word that drops MoveEnd's unit gives one run per character without any error: only
    # the check on runs whose lengths are known notices it.
    $single = Get-RunRecords (New-Range $runs 0 9 -LosesUnit) $later $theme
    Check 'a MoveEnd without its unit reads one run per character' ($single.Count -eq 9) "$($single.Count)"
    Check 'the run reader passes its check with Word as documented' ((Get-RunReaderFault (New-Range $runs 0 9)) -eq '')
    Check 'the run reader passes its check with the quirks of the first K2 run' ((Get-RunReaderFault (New-Range $runs 0 9 -Quirky)) -eq '')
    $lost = Get-RunReaderFault (New-Range $runs 0 9 -LosesUnit)
    Check 'the run reader fails its check when MoveEnd drops its unit, saying what it read' ($lost -like '*read 9 runs: 1 bold=False, 1 bold=False*') $lost
    $stuck = Get-RunReaderFault (New-Range $runs 0 9 99)
    Check 'the run reader fails its check when Word does not move, with the message' ($stuck -like 'reading failed: *did not move*') $stuck
    # Runs that differ from the scratch document's in one length or one bold flag each.
    function New-Runs($First, $FirstBold, $Second, $SecondBold) {
        return @(
            (New-Object PSObject -Property @{ Length = $First; Font = (New-Font 'Arial' 10 $FirstBold) }),
            (New-Object PSObject -Property @{ Length = $Second; Font = (New-Font 'Arial' 12 $SecondBold) }),
            (New-Object PSObject -Property @{ Length = 9 - $First - $Second; Font = (New-Font 'Arial' 10) })
        )
    }
    foreach ($case in @(@(4, 0, 3, -1), @(5, 0, 2, -1), @(5, -1, 3, -1), @(5, 0, 3, 0))) {
        $other = New-Runs $case[0] $case[1] $case[2] $case[3]
        Check "the run reader fails its check on runs $($case -join '/')" ((Get-RunReaderFault (New-Range $other 0 9)) -ne '')
    }

    function New-Paragraph($Text, $Runs, $Builtin, $StyleName, $ListType = 0, $ListString = '', $Step = 13, $Delay = 0, $ReadDelay = 0) {
        $length = 0
        foreach ($run in $Runs) { $length += $run.Length }
        $range = New-Range $Runs 0 $length $Step $Delay
        $characters = New-Object PSObject -Property @{ First = (New-Object PSObject -Property @{ Font = $Runs[0].Font }) }
        if ($ReadDelay -gt 0) {
            # A paragraph whose text takes $ReadDelay milliseconds to read, as in a slow Word.
            $range | Add-Member NoteProperty ReadDelay $ReadDelay
            $range | Add-Member NoteProperty PlainText $Text
            $range | Add-Member ScriptProperty Text { Start-Sleep -Milliseconds $this.ReadDelay; $this.PlainText }
        } else {
            $range | Add-Member NoteProperty Text $Text
        }
        $range | Add-Member NoteProperty Characters $characters
        $range | Add-Member NoteProperty ListFormat (New-Object PSObject -Property @{ ListType = $ListType; ListString = $ListString; ListLevelNumber = 2 })
        $format = New-Object PSObject -Property @{
            LeftIndent = 36; RightIndent = 0; FirstLineIndent = -18; SpaceBefore = 6; SpaceAfter = 9999999
            SpaceBeforeAuto = 0; SpaceAfterAuto = -1; LineSpacing = 13.8; LineSpacingRule = 5; Alignment = 3
        }
        $style = New-Object PSObject -Property @{ BuiltIn = $Builtin; NameLocal = $StyleName }
        return New-Object PSObject -Property @{ Range = $range; Format = $format; Style = $style }
    }
    $listed = New-Paragraph ('1.1 Scope' + [char]13) $runs $true 'Localized heading' 2 '1.1'
    $record = Get-ParagraphRecord $listed 4 $theme
    Check 'a built-in style is flagged, its localized name left out' ($record.style_builtin -eq $true -and $null -eq $record.style)
    Check 'paragraph format values' ($record.left_indent -eq 36 -and $record.first_line_indent -eq -18 -and $null -eq $record.space_after -and $record.space_after_auto -eq $true -and $record.line_spacing_rule -eq 'multiple' -and $record.alignment -eq 'justify')
    Check 'the list label and level' ($record.list_string -eq '1.1' -and $record.list_level -eq 2 -and $record.index -eq 4)
    Check 'the first character''s font' ($record.font.name -eq 'Arial' -and $record.font.size -eq 10)
    $custom = Get-ParagraphRecord (New-Paragraph ('Body' + [char]13) $runs $false 'Contract Clause') 0 $theme
    Check 'a style the document defines keeps its name; no list' ($custom.style -eq 'Contract Clause' -and $null -eq $custom.list_level)
    $nameless = Get-ParagraphRecord (New-Paragraph ('Body' + [char]13) $runs $false '') 0 $theme
    Check 'an empty style name is null' ($null -eq $nameless.style)

    function New-Story([int] $Type, $Paragraphs, $Next = $null) {
        return New-Object PSObject -Property @{ StoryType = $Type; Paragraphs = $Paragraphs; NextStoryRange = $Next }
    }
    $header2 = New-Story 7 @((New-Paragraph ('Second header' + [char]13) $runs $false 'Header B'))
    $document = New-Object PSObject -Property @{ StoryRanges = @(
            (New-Story 1 @($listed, (New-Paragraph ('Body' + [char]13) $runs $true ''))),
            (New-Story 7 @((New-Paragraph ('Header' + [char]13) $runs $true '')) $header2),
            (New-Story 12 @((New-Paragraph ([string][char]3) $runs $true '')))
        )
    }
    $progress = @{ Step = '' }
    $export = (Export-Formatting $document 600 $theme $progress).Export
    $names = @($export.stories | ForEach-Object { "$($_.story)/$($_.instance)" }) -join ','
    Check 'every story and linked story is exported, separators left out' ($names -eq 'main/0,primary-header/0,primary-header/1') $names
    Check 'paragraphs are numbered per story' (@($export.stories[0].paragraphs).Count -eq 2 -and $export.stories[0].paragraphs[1].index -eq 1)
    Check 'runs are kept when they take no longer than the paragraphs' ($export.runs -eq $true -and @($export.stories[0].paragraphs[0].runs).Count -eq 3 -and $export.complete -eq $true)
    # Each paragraph takes 50 ms to read, longer than any tick of the clock, so with a budget
    # of 0 the second one starts after the deadline on fast and slow runners alike. The fast
    # paragraphs above take about 1 ms in all, and whether the clock ticked during that pass
    # decided the result (#166).
    $sluggish = New-Object PSObject -Property @{ StoryRanges = @((New-Story 1 @(
                    (New-Paragraph ('First' + [char]13) $runs $true '' -ReadDelay 50),
                    (New-Paragraph ('Second' + [char]13) $runs $true '' -ReadDelay 50)
                )))
    }
    $late = (Export-Formatting $sluggish 0 $theme $progress).Export
    Check 'a document over its time budget is marked incomplete, without runs' ($late.complete -eq $false -and @($late.stories[0].paragraphs).Count -lt 2 -and $late.runs -eq $false -and $late.runs_note -eq 'not collected: the paragraphs did not fit in the time budget') "complete $($late.complete), $(@($late.stories[0].paragraphs).Count) paragraphs, $($late.runs_note)"
    $unchecked = (Export-Formatting $document 600 $theme $progress $false).Export
    Check 'runs are left out with a fixed note when the run reader failed its check' ($unchecked.complete -eq $true -and $unchecked.runs -eq $false -and $unchecked.runs_note -eq 'left out: Word failed the check of the run reader' -and -not $unchecked.stories[0].paragraphs[0].Contains('runs')) "$($unchecked.runs_note)"
    $failing = New-Object PSObject -Property @{ StoryRanges = @((New-Story 1 @((New-Paragraph ('Body' + [char]13) $runs $true '' 0 '' 99)))) }
    $failed = Export-Formatting $failing 600 $theme $progress
    Check 'a failed run pass gives a fixed note; the message stays apart' ($failed.Export.runs -eq $false -and $failed.Export.runs_note -eq 'left out: reading them failed' -and $failed.RunError -like '*did not move*')
    $slow = New-Object PSObject -Property @{ StoryRanges = @((New-Story 1 @((New-Paragraph ('Body' + [char]13) $runs $true '' 0 '' 13 1500)))) }
    $clipped = (Export-Formatting $slow 1 $theme $progress).Export
    Check 'runs cut short by the time budget say so' ($clipped.complete -eq $true -and $clipped.runs -eq $false -and $clipped.runs_note -eq 'left out: they did not fit in the time budget') "$($clipped.runs_note)"
    $unresolved = @((Export-Formatting $document 600 $theme $progress).Unresolved)
    Check 'unresolved placeholders are reported apart' ($unresolved -contains '+Unbekannt' -and $unresolved -contains '+Kaputt' -and -not ($unresolved -contains '+Textkoerper'))
    # The worker has only the functions' text: in a runspace of its own, the export of a
    # document must find every function it calls.
    $isolated = [PowerShell]::Create()
    try {
        [void]$isolated.AddScript('param($Functions, $Document) . ([ScriptBlock]::Create($Functions)); (Export-Formatting $Document 600 @{ Fonts = @{}; Seen = @{}; Classify = { $null } } @{ Step = "" }).Export')
        [void]$isolated.AddArgument((Get-FormattingFunctions)).AddArgument($document)
        $alone = $isolated.Invoke()
        $errors = @($isolated.Streams.Error)
        Check 'the worker gets every function the export needs' ($errors.Count -eq 0 -and @($alone[0].stories).Count -eq 3) "$($errors | Select-Object -First 1)"
    } finally {
        $isolated.Dispose()
    }
    # Every function of this script that the worker's code calls must be in the text it gets:
    # a missing one fails every document in Word, or loses a value inside a try.
    $shippedAst = [System.Management.Automation.Language.Parser]::ParseInput((Get-FormattingFunctions), [ref]$null, [ref]$null)
    $shipped = @($shippedAst.FindAll({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $false) | ForEach-Object { $_.Name })
    $defined = @($functions | ForEach-Object { $_.Name })
    $assignment = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -eq '$FormattingScript' }, $false)
    $workerErrors = $null
    $workerAst = [System.Management.Automation.Language.Parser]::ParseInput($assignment.Right.Expression.Value, [ref]$null, [ref]$workerErrors)
    $called = @(@($shippedAst, $workerAst) | ForEach-Object { $_.FindAll({ param($node) $node -is [System.Management.Automation.Language.CommandAst] }, $true) } | ForEach-Object { $_.GetCommandName() } | Where-Object { $defined -contains $_ } | Sort-Object -Unique)
    $missing = @($called | Where-Object { $shipped -notcontains $_ })
    Check 'the formatting step parses' ($workerErrors.Count -eq 0 -and $called -contains 'Export-Formatting' -and $called -contains 'Get-RunReaderFault') ($called -join ', ')
    Check 'the worker gets every function its code calls' ($missing.Count -eq 0) ($missing -join ', ')
    $json = ConvertTo-Json -InputObject $export -Depth 10 -Compress
    Check 'the export is JSON with the run lengths' ($json.Contains('"runs":[{"length":5') -and $json.Contains('"format":1'))

    # -FormattingOnly reads a document again only when its rendered result is still there.
    $again = Join-Path $temp 'again'
    $results = Join-Path $again 'out'
    New-Item -ItemType Directory -Path $results -Force | Out-Null
    $letter = Join-Path $again 'letter.docx'
    [System.IO.File]::WriteAllBytes($letter, [byte[]](1, 2, 3))
    $copy = Join-Path $results 'letter.resaved.docx'
    [System.IO.File]::WriteAllBytes($copy, [byte[]](4, 5, 6))
    $recordPath = Join-Path $results 'letter.json'
    $letterHash = Get-Sha256 $letter
    $copyHash = Get-Sha256 $copy
    function Write-Record($Changes) {
        $record = [ordered]@{
            script_version = 3; file = 'letter.docx'; sha256 = $letterHash
            resaved_file = 'letter.resaved.docx'; resaved_sha256 = $copyHash; status = 'rendered'; pages = 2
        }
        foreach ($key in $Changes.Keys) { $record[$key] = $Changes[$key] }
        Write-JsonFile $recordPath $record
    }
    Check 'a document never rendered is skipped' ((Get-FormattingOnlyState $letter $results 3).Reason -like 'not rendered yet*')
    Write-Record @{}
    $state = Get-FormattingOnlyState $letter $results 3
    Check 'a rendered document is read again, with its record in order' ($state.Ready -and @($state.Record.Keys)[0] -eq 'script_version' -and $state.Record.pages -eq 2 -and @($state.Record.Keys)[-1] -eq 'pages') "$($state.Reason)"
    Write-Record @{ status = 'formatting-failed' }
    Check 'a document whose formatting failed is read again' (Get-FormattingOnlyState $letter $results 3).Ready
    Write-Record @{ status = 'privacy-failed' }
    Check 'a document refused for personal data is not' ((Get-FormattingOnlyState $letter $results 3).Reason -like '*did not succeed (privacy-failed)*')
    Write-Record @{ script_version = 2 }
    Check 'a result of an older format is not' ((Get-FormattingOnlyState $letter $results 3).Reason -like '*format 2*')
    Write-Record @{ sha256 = ('0' * 64) }
    Check 'an input that changed is not' ((Get-FormattingOnlyState $letter $results 3).Reason -like '*input changed*')
    Write-Record @{ file = 'other.docx' }
    Check 'a record of another input is not' ((Get-FormattingOnlyState $letter $results 3).Reason -like '*input changed*')
    Write-Record @{ resaved_sha256 = ('0' * 64) }
    Check 'a re-saved copy that is not the recorded one is not' ((Get-FormattingOnlyState $letter $results 3).Reason -like '*re-saved copy*')
    Write-Record @{}
    Remove-Item -LiteralPath $copy
    Check 'a missing re-saved copy is not' ((Get-FormattingOnlyState $letter $results 3).Reason -like '*re-saved copy*')
    Set-Content -LiteralPath $recordPath -Value 'not json'
    Check 'an unreadable record is not' ((Get-FormattingOnlyState $letter $results 3).Reason -like '*cannot be read*')

    # A record read back is written again with its lists as lists, which corpus-import.py
    # requires.
    [System.IO.File]::WriteAllBytes($copy, [byte[]](4, 5, 6))
    Write-Record @{ fonts = @('Arial'); fonts_missing = @(); privacy_findings = @('a', 'b'); word_version = '16.0'; word_build = '16.0.1' }
    $state = Get-FormattingOnlyState $letter $results 3
    $rewritten = Join-Path $results 'rewritten.json'
    Write-JsonFile $rewritten $state.Record
    $text = [System.IO.File]::ReadAllText($rewritten)
    Check 'a record written again keeps its lists' ($state.Ready -and $text -match '"fonts":\s*\[\s*"Arial"\s*\]' -and $text -match '"fonts_missing":\s*\[\s*\]' -and $text -match '"privacy_findings":\s*\[\s*"a",\s*"b"\s*\]') $text
    Check 'the Word that rendered a document reads it again' ((Get-WordChange $state.Record '16.0' '16.0.1') -eq '')
    Check 'another Word build does not' ((Get-WordChange $state.Record '16.0' '16.0.2') -like '*render it again*')
    $complete = @{ formatting_complete = $true; formatting_runs = $true }
    Check 'an incomplete export does not replace a complete one' ((Get-WorseFormatting $complete @{ formatting_complete = $false; formatting_runs = $false }) -like '*incomplete*')
    Check 'an export without runs does not replace one with runs' ((Get-WorseFormatting $complete @{ formatting_complete = $true; formatting_runs = $false }) -like '*no runs*')
    Check 'an export as good or better replaces the earlier one' ((Get-WorseFormatting @{ formatting_complete = $true; formatting_runs = $false } $complete) -eq '' -and (Get-WorseFormatting $complete $complete) -eq '')
    $foreign = New-Object PSObject -Property @{ Status = 'done'; Output = @((New-Object PSObject -Property @{ Foreign = $true })) }
    $exported = New-Object PSObject -Property @{ Status = 'done'; Output = @((New-Object PSObject -Property @{ Export = @{}; RunError = $null })) }
    $broken = New-Object PSObject -Property @{ Status = 'error'; Output = @() }
    Check 'a step that found a document of the user''s says so' ((Test-ForeignStep $foreign) -and -not (Test-ForeignStep $exported) -and -not (Test-ForeignStep $broken))
} finally {
    Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
}

if ($script:failures -gt 0) {
    Write-Host "$($script:failures) failed" -ForegroundColor Red
    exit 1
}
Write-Host 'all passed'
exit 0
