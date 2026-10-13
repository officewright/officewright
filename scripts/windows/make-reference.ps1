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
      5. writes <name>.formatting.json: for every paragraph of every story (body, headers,
         footers, notes, text boxes) the formatting Word computes for it: whether its style
         is built in (and the name of a style the document defines), indents, spacing
         before and after, line spacing, alignment, the list label Word shows
         (ListFormat.ListString) and the font of its first character (name, size, bold,
         italic, colour; a theme font by its ST_Theme value, such as minorHAnsi); and, when
         that takes no longer than the paragraphs did, the same font properties for every
         run of formatting;
      6. writes <name>.json: hashes, Word version and build, the printer, page count,
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

.PARAMETER FormattingSeconds
    How long reading the formatting of one document may take. A document that needs more
    keeps the paragraphs read so far and is marked incomplete.

.PARAMETER FormattingOnly
    Read the formatting again for documents already rendered into -OutputDir, without
    rendering them: Word opens the re-saved copy there and <name>.formatting.json and the
    record's formatting fields are written again; the PDF and the re-saved copy are left as
    they are. A document is read only when its record says rendered or formatting-failed,
    the input is the file the record was made from, and the re-saved copy is the one the
    record names; others are skipped with the reason. When reading fails, or the new export
    holds personal data, the earlier files are kept.

.PARAMETER DryRun
    Check the inputs, the output folder, the fonts and the printer, print what would be
    rendered, and stop before Word starts.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File C:\officewright-ref\make-reference.ps1 -InputPath C:\officewright-ref\word-rules -OutputDir C:\officewright-ref\out

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File C:\officewright-ref\make-reference.ps1 -InputPath C:\officewright-ref\word-rules -OutputDir C:\officewright-ref\out -FormattingOnly
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

    [ValidateRange(30, 7200)]
    [int] $FormattingSeconds = 900,

    [switch] $FormattingOnly,

    [switch] $DryRun
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

# Bumped whenever the output files change shape; corpus-import.py checks it.
$ScriptVersion = 3

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

function ConvertTo-OrderedRecord($Object) {
    # A record read back with ConvertFrom-Json, as the ordered dictionary the script writes,
    # so that its fields keep their order.
    $record = [ordered]@{}
    foreach ($property in $Object.PSObject.Properties) { $record[$property.Name] = $property.Value }
    return $record
}

function Get-FormattingOnlyState([string] $InputFile, [string] $OutDir, [int] $Version) {
    # Whether the formatting of a rendered document can be read again: Ready, a Reason when
    # it cannot, and the Record to update.
    $name = [System.IO.Path]::GetFileName($InputFile)
    $base = [System.IO.Path]::GetFileNameWithoutExtension($InputFile)
    $state = New-Object PSObject -Property @{ Ready = $false; Reason = ''; Record = $null }
    $jsonPath = Join-Path $OutDir ($base + '.json')
    if (-not (Test-Path -LiteralPath $jsonPath)) {
        $state.Reason = 'not rendered yet; run without -FormattingOnly first'
        return $state
    }
    try {
        $record = ConvertTo-OrderedRecord (Get-Content -LiteralPath $jsonPath -Raw | ConvertFrom-Json)
    } catch {
        $state.Reason = 'its record cannot be read'
        return $state
    }
    $resaved = Join-Path $OutDir ($base + '.resaved.docx')
    if ($record['script_version'] -ne $Version) {
        $state.Reason = "rendered by format $($record['script_version']) of this script, not $Version"
    } elseif ($record['status'] -ne 'rendered' -and $record['status'] -ne 'formatting-failed') {
        $state.Reason = "its rendering did not succeed ($($record['status']))"
    } elseif ($record['file'] -ne $name -or $record['sha256'] -ne (Get-Sha256 $InputFile)) {
        $state.Reason = 'the input changed since it was rendered'
    } elseif (-not (Test-Path -LiteralPath $resaved) -or $record['resaved_sha256'] -ne (Get-Sha256 $resaved)) {
        $state.Reason = 'the re-saved copy is missing or not the one its record names'
    } else {
        $state.Ready = $true
        $state.Record = $record
    }
    return $state
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

# Word's computed formatting. These functions run in the worker (Export-Formatting passes
# their text to it) and are tested without Word on objects that stand in for Word's.

function ConvertTo-PointValue($Value) {
    # A length in points, rounded to 0.01 pt; $null for Word's "differs within the range".
    if ($null -eq $Value) { return $null }
    $number = [double]$Value
    if ($number -ge 9999999) { return $null }
    return [math]::Round($number, 2)
}

function ConvertTo-Flag($Value) {
    # Word's True (-1) and False (0); $null for wdUndefined (9999999), which means mixed.
    if ($null -eq $Value) { return $null }
    $number = [long]$Value
    if ($number -eq 9999999) { return $null }
    return ($number -ne 0)
}

function ConvertTo-HexColor($Color, $Rgb) {
    # Font.Color is wdColorAutomatic (-16777216) for automatic and 9999999 when mixed;
    # Font.TextColor.RGB is the colour itself, theme colours resolved, as R + G*256 + B*65536.
    if ($null -eq $Color) { return $null }
    $code = [long]$Color
    if ($code -eq -16777216) { return 'auto' }
    if ($code -eq 9999999 -or $null -eq $Rgb) { return $null }
    $value = [long]$Rgb
    if ($value -lt 0 -or $value -gt 16777215) { return $null }
    return '{0:X2}{1:X2}{2:X2}' -f ($value -band 255), (($value -shr 8) -band 255), (($value -shr 16) -band 255)
}

function Get-AlignmentName([int] $Value) {
    switch ($Value) {
        0 { 'left' } 1 { 'center' } 2 { 'right' } 3 { 'justify' } 4 { 'distribute' }
        5 { 'justify-medium' } 7 { 'justify-high' } 8 { 'justify-low' } 9 { 'thai-justify' }
        default { "other-$Value" }
    }
}

function Get-LineRuleName([int] $Value) {
    switch ($Value) {
        0 { 'single' } 1 { 'one-and-a-half' } 2 { 'double' } 3 { 'at-least' } 4 { 'exactly' }
        5 { 'multiple' } default { "other-$Value" }
    }
}

function Get-StoryName([int] $Value) {
    # WdStoryType; the separator stories (12 to 17) hold no paragraphs of the document.
    switch ($Value) {
        1 { 'main' } 2 { 'footnotes' } 3 { 'endnotes' } 4 { 'comments' } 5 { 'text-boxes' }
        6 { 'even-pages-header' } 7 { 'primary-header' } 8 { 'even-pages-footer' }
        9 { 'primary-footer' } 10 { 'first-page-header' } 11 { 'first-page-footer' }
        default { $null }
    }
}

function Get-TextExcerpt([string] $Text) {
    # The paragraph's first 60 characters, without the paragraph and cell marks; other
    # control characters (tabs, line breaks, field marks) as spaces.
    $text = $Text.TrimEnd([char]13, [char]7)
    $text = [regex]::Replace($text, '[\x00-\x1f]', ' ')
    if ($text.Length -gt 60) { $text = $text.Substring(0, 60) }
    return $text
}

function Get-ThemeKind([string] $Xml) {
    # The theme font Word wrote for the run "x" of a scratch document, from its
    # Range.WordOpenXML: an ST_Theme value such as minorHAnsi or majorEastAsia. Only that
    # run's properties are read: the styles part and paragraph marks name theme fonts too.
    $part = [regex]::Match($Xml, '(?s)pkg:name="/word/document\.xml".*?</pkg:part>').Value
    $run = [regex]::Match($part, '(?s)<w:r>(?:(?!</w:r>).)*?<w:t(?: [^>]*)?>x</w:t>').Value
    foreach ($attribute in @('asciiTheme', 'hAnsiTheme', 'eastAsiaTheme', 'cstheme')) {
        $match = [regex]::Match($run, "w:$attribute=`"((?:major|minor)(?:HAnsi|Ascii|EastAsia|Bidi))`"")
        if ($match.Success) { return $match.Groups[1].Value }
    }
    return $null
}

function Get-FontRecord($Font, $Themes) {
    # Name, size, bold, italic and colour as Word computes them. Font.Name gives a theme
    # font as the placeholder of Word's font box ("+Body" in an English Word), which is in
    # the language of Word's user interface: it never leaves this function. $Themes.Classify
    # turns it into the theme font it stands for (once per placeholder, kept in
    # $Themes.Seen), and $Themes.Fonts names that font in the document's theme. A
    # placeholder that cannot be classified gives the theme "unresolved" and no name.
    $name = [string]$Font.Name
    $theme = $null
    if ($name.StartsWith('+')) {
        if (-not $Themes.Seen.ContainsKey($name)) {
            $kind = $null
            try { $kind = & $Themes.Classify $name } catch { $kind = $null }
            $Themes.Seen[$name] = $kind
        }
        $theme = $Themes.Seen[$name]
        $name = $null
        if ($theme) { $name = $Themes.Fonts[$theme] } else { $theme = 'unresolved' }
    }
    if ($name -eq '') { $name = $null }
    $rgb = $null
    try { $rgb = $Font.TextColor.RGB } catch { $rgb = $null }
    $record = [ordered]@{
        name = $name
        size = ConvertTo-PointValue $Font.Size
        bold = ConvertTo-Flag $Font.Bold
        italic = ConvertTo-Flag $Font.Italic
        color = ConvertTo-HexColor $Font.Color $rgb
    }
    if ($theme) { $record.theme = $theme }
    if ($null -eq $record.color -and [long]$Font.Color -ne 9999999) {
        # A theme colour: Word gives no RGB value for it, so the record names the theme
        # colour (ST_ThemeColor) and Word's lightening or darkening of it.
        try {
            $themeColor = Get-ThemeColorName ([int]$Font.TextColor.ObjectThemeColor)
            if ($themeColor) {
                $record.theme_color = $themeColor
                $record.tint_shade = [math]::Round([double]$Font.TextColor.TintAndShade, 4)
            }
        } catch { }
    }
    return $record
}

function Get-ThemeColorName([int] $Index) {
    # WdThemeColorIndex as the ST_ThemeColor name of ECMA-376; $null for no theme colour.
    $names = @('dark1', 'light1', 'dark2', 'light2', 'accent1', 'accent2', 'accent3', 'accent4',
        'accent5', 'accent6', 'hyperlink', 'followedHyperlink', 'background1', 'text1',
        'background2', 'text2')
    if ($Index -lt 0 -or $Index -ge $names.Count) { return $null }
    return $names[$Index]
}

function Get-ParagraphRecord($Paragraph, [int] $Index, $Themes) {
    $range = $Paragraph.Range
    $format = $Paragraph.Format
    # Only the name of a style the document defines: built-in styles have names in the
    # language of Word's user interface, which says where Word runs. corpus-import.py
    # takes their names from the re-saved copy instead.
    $styleName = $null
    $builtIn = $null
    try {
        $style = $Paragraph.Style
        $builtIn = [bool]$style.BuiltIn
        if (-not $builtIn) { $styleName = [string]$style.NameLocal }
    } catch { }
    if ($styleName -eq '') { $styleName = $null }
    $list = $range.ListFormat
    $listString = [string]$list.ListString
    $level = $null
    if ([int]$list.ListType -ne 0) { $level = [int]$list.ListLevelNumber }
    return [ordered]@{
        index = $Index
        text = Get-TextExcerpt ([string]$range.Text)
        style = $styleName
        style_builtin = $builtIn
        left_indent = ConvertTo-PointValue $format.LeftIndent
        right_indent = ConvertTo-PointValue $format.RightIndent
        first_line_indent = ConvertTo-PointValue $format.FirstLineIndent
        space_before = ConvertTo-PointValue $format.SpaceBefore
        space_after = ConvertTo-PointValue $format.SpaceAfter
        space_before_auto = ConvertTo-Flag $format.SpaceBeforeAuto
        space_after_auto = ConvertTo-Flag $format.SpaceAfterAuto
        line_spacing = ConvertTo-PointValue $format.LineSpacing
        line_spacing_rule = Get-LineRuleName ([int]$format.LineSpacingRule)
        alignment = Get-AlignmentName ([int]$format.Alignment)
        list_string = $listString
        list_level = $level
        font = Get-FontRecord $range.Characters.First.Font $Themes
    }
}

function Get-RunRecords($Range, [DateTime] $Deadline, $Themes) {
    # The paragraph's stretches of one character formatting, as Word's wdCharacterFormatting
    # unit (13) moves over them; $null once $Deadline has passed. The paragraph's ends are
    # read once, and the cursor is put at each run's start with SetRange, so neither how
    # Word's Collapse takes its direction nor whether Duplicate shares state with the
    # paragraph's range can make the reading repeat a run.
    $runs = New-Object System.Collections.ArrayList
    $start = [int]$Range.Start
    $end = [int]$Range.End
    $cursor = $Range.Duplicate
    $position = $start
    while ($position -lt $end) {
        if ([DateTime]::UtcNow -gt $Deadline) { return $null }
        # Each run is at least a character long, so more runs than characters is a fault.
        if ($runs.Count -ge $end - $start) { throw 'Word gave more runs than characters' }
        $cursor.SetRange($position, $position)
        [void]$cursor.MoveEnd(13, 1)
        $next = [int]$cursor.End
        if ($next -le $position) { throw 'Word did not move over a run of formatting' }
        if ($next -gt $end) {
            $next = $end
            $cursor.SetRange($position, $next)
        }
        [void]$runs.Add([ordered]@{
                length = $next - $position
                font = Get-FontRecord $cursor.Font $Themes
            })
        $position = $next
    }
    return , $runs.ToArray()
}

function Export-Formatting($Document, [int] $BudgetSeconds, $Themes, $Progress) {
    # Every paragraph of every story with the formatting Word computes for it. Then, in a
    # second pass, its runs of formatting, kept only if that pass takes no longer than the
    # first, so that the whole export takes at most twice as long as without runs.
    $deadline = [DateTime]::UtcNow.AddSeconds($BudgetSeconds)
    $clock = [System.Diagnostics.Stopwatch]::StartNew()
    $stories = New-Object System.Collections.ArrayList
    $visited = New-Object System.Collections.ArrayList
    $complete = $true
    foreach ($story in $Document.StoryRanges) {
        $range = $story
        $instance = 0
        while ($null -ne $range) {
            $name = Get-StoryName ([int]$range.StoryType)
            if ($null -eq $name) { break }
            $records = New-Object System.Collections.ArrayList
            $index = 0
            foreach ($paragraph in $range.Paragraphs) {
                if ([DateTime]::UtcNow -gt $deadline) { $complete = $false; break }
                $Progress.Step = "reading the formatting of paragraph $index of the $name story"
                $record = Get-ParagraphRecord $paragraph $index $Themes
                [void]$records.Add($record)
                [void]$visited.Add(@($paragraph, $record))
                $index++
            }
            [void]$stories.Add([ordered]@{ story = $name; instance = $instance; paragraphs = $records.ToArray() })
            if (-not $complete) { break }
            $range = $range.NextStoryRange
            $instance++
        }
        if (-not $complete) { break }
    }
    $paragraphSeconds = $clock.Elapsed.TotalSeconds
    # A floor of five seconds keeps small documents from losing their runs to timer noise.
    $runBudget = [math]::Max($paragraphSeconds, 5)
    $runDeadline = [DateTime]::UtcNow.AddSeconds($runBudget)
    $clipped = $runDeadline -gt $deadline
    if ($clipped) { $runDeadline = $deadline }
    $runClock = [System.Diagnostics.Stopwatch]::StartNew()
    $runsKept = $complete
    # The notes are fixed English sentences, which corpus-import.py checks: Word's own
    # error messages are in the language of its user interface and go to the console only.
    $note = ''
    $runError = $null
    if (-not $complete) { $note = 'not collected: the paragraphs did not fit in the time budget' }
    $collected = New-Object System.Collections.ArrayList
    if ($runsKept) {
        $Progress.Step = 'reading the runs of formatting'
        try {
            foreach ($pair in $visited) {
                $runs = Get-RunRecords $pair[0].Range $runDeadline $Themes
                if ($null -eq $runs) {
                    $runsKept = $false
                    $note = 'left out: they took longer than the paragraphs'
                    if ($clipped) { $note = 'left out: they did not fit in the time budget' }
                    break
                }
                [void]$collected.Add($runs)
            }
        } catch {
            $runsKept = $false
            $note = 'left out: reading them failed'
            $runError = $_.Exception.Message
        }
    }
    if ($runsKept) {
        for ($i = 0; $i -lt $visited.Count; $i++) { $visited[$i][1].runs = $collected[$i] }
    }
    $export = [ordered]@{
        format = 1
        complete = $complete
        runs = $runsKept
        runs_note = $note
        paragraph_seconds = [math]::Round($paragraphSeconds, 1)
        run_seconds = [math]::Round($runClock.Elapsed.TotalSeconds, 1)
        stories = $stories.ToArray()
    }
    $unresolved = @($Themes.Seen.Keys | Where-Object { -not $Themes.Seen[$_] })
    return New-Object PSObject -Property @{ Export = $export; RunError = $runError; Unresolved = $unresolved }
}

function Write-FormattingJson($Export, [string] $Path) {
    # On the main thread: ConvertTo-Json of Windows PowerShell 5.1 is slow on a large export,
    # and Word, idle by then, must not be stopped for it.
    $json = ConvertTo-Json -InputObject $Export -Depth 10 -Compress
    $utf8 = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::WriteAllText($Path, $json + "`n", $utf8)
}

function Read-DocumentFormatting($Record, [string] $Resaved, [string] $FormattingPath, $DocTokens) {
    # Runs the formatting step on the re-saved copy, writes the export to $FormattingPath and
    # fills the record's formatting fields and status; returns the step. Uses the worker and
    # the settings of the main script.
    Write-Host '  reading the formatting Word computes ...'
    $formattingStarted = Get-Date
    $step = Invoke-WordStep $worker $FormattingScript @($Resaved, $FormattingSeconds, $FormattingFunctions) ($FormattingSeconds + 120) $targets $progress
    if ($step.Status -eq 'done') {
        $summary = $step.Output[0]
        $export = $summary.Export
        Write-FormattingJson $export $FormattingPath
        $Record.formatting_seconds = [math]::Round(((Get-Date) - $formattingStarted).TotalSeconds, 1)
        $Record.formatting_sha256 = Get-Sha256 $FormattingPath
        $paragraphs = 0
        foreach ($story in $export.stories) { $paragraphs += @($story.paragraphs).Count }
        $Record.formatting_paragraphs = $paragraphs
        $Record.formatting_runs = [bool]$export.runs
        $Record.formatting_complete = [bool]$export.complete
        $formattingText = [System.IO.File]::ReadAllText($FormattingPath)
        $leaks = @($DocTokens | Where-Object { Test-Token $formattingText $_.Value } | ForEach-Object { "formatting: $($_.Kind)" })
        $Record.privacy_findings = $leaks
        if ($leaks.Count -gt 0) {
            $Record.status = 'privacy-failed'
            $Record.error = 'personal data in the formatting export; corpus-import.py refuses this document'
        } else {
            $Record.status = 'rendered'
            $Record.error = ''
        }
        if ($export.runs_note) { Write-Host "  runs of formatting $($export.runs_note)" -ForegroundColor Yellow }
        if ($summary.RunError) { Write-Host "  $($summary.RunError)" -ForegroundColor Yellow }
        foreach ($placeholder in @($summary.Unresolved)) {
            Write-Host "  theme font not resolved: $placeholder" -ForegroundColor Yellow
        }
    } elseif ($step.Status -eq 'error') {
        $Record.status = 'formatting-failed'
        $Record.error = "reading the formatting failed: $($step.Message)"
    } else {
        $Record.status = 'formatting-failed'
        $Record.error = "Word stopped while reading the formatting: $($step.Message)"
    }
    return $step
}

function Get-FormattingFunctions {
    # The text of the functions above: the worker runs in a runspace of its own, which has
    # none of this script's functions.
    $names = @(
        'ConvertTo-PointValue', 'ConvertTo-Flag', 'ConvertTo-HexColor', 'Get-ThemeColorName', 'Get-AlignmentName',
        'Get-LineRuleName', 'Get-StoryName', 'Get-TextExcerpt', 'Get-ThemeKind', 'Get-FontRecord',
        'Get-ParagraphRecord', 'Get-RunRecords', 'Export-Formatting'
    )
    return ($names | ForEach-Object {
            "function $_ {`n$((Get-Item "function:$_").ScriptBlock.ToString())`n}"
        }) -join "`n"
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

$FormattingScript = @'
param($Resaved, $BudgetSeconds, $Functions)
$ErrorActionPreference = 'Stop'
. ([ScriptBlock]::Create($Functions))
$word = $global:word
$progress.Step = 'Documents.Open (formatting)'
$doc = $word.Documents.Open($Resaved, $false, $true, $false)
try {
    # The fonts of the document's theme by ST_Theme value. MsoFontLanguageIndex: 1 Latin,
    # 2 complex script, 3 East Asian.
    $fonts = @{}
    try {
        $scheme = $doc.DocumentTheme.ThemeFontScheme
        foreach ($pair in @(@('minor', $scheme.MinorFont), @('major', $scheme.MajorFont))) {
            $latin = [string]$pair[1].Item(1).Name
            $fonts["$($pair[0])HAnsi"] = $latin
            $fonts["$($pair[0])Ascii"] = $latin
            $fonts["$($pair[0])Bidi"] = [string]$pair[1].Item(2).Name
            $fonts["$($pair[0])EastAsia"] = [string]$pair[1].Item(3).Name
        }
    } catch { }
    # A placeholder of Word's font box set on the text of a scratch document makes Word
    # write the theme font it stands for into that document's XML.
    $classify = {
        param($Placeholder)
        $step = $progress.Step
        $progress.Step = 'Documents.Add (classifying a theme font)'
        $scratch = $global:word.Documents.Add()
        try {
            $scratch.Range().InsertAfter('x')
            $range = $scratch.Range(0, 1)
            $range.Font.Name = $Placeholder
            $xml = [string]$range.WordOpenXML
        } finally {
            try {
                $scratch.Close(0)
            } catch {
                # Left open, it would stop the next document as a document of the user's.
                throw "the scratch document for classifying a theme font did not close: $($_.Exception.Message)"
            } finally {
                [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($scratch)
            }
            $progress.Step = $step
        }
        Get-ThemeKind $xml
    }
    $themes = @{ Fonts = $fonts; Seen = @{}; Classify = $classify }
    $result = Export-Formatting $doc $BudgetSeconds $themes $progress
} finally {
    $progress.Step = 'closing the re-saved copy (formatting)'
    $doc.Close(0)
    [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($doc)
}
# The main thread writes the JSON, outside this step's time-out.
$result
'@

$FormattingFunctions = Get-FormattingFunctions

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
    $why = 'rendered before; -Overwrite renders it again'
    $again = $null
    $json = Join-Path $outDir ($file.BaseName + '.json')
    if ($FormattingOnly) {
        $again = Get-FormattingOnlyState $file.FullName $outDir $ScriptVersion
        $skip = -not $again.Ready
        $why = $again.Reason
    } elseif (-not $Overwrite -and (Test-Path -LiteralPath $json)) {
        try {
            $previous = Get-Content -LiteralPath $json -Raw | ConvertFrom-Json
            $skip = $previous.status -eq 'rendered' -and $previous.script_version -eq $ScriptVersion
        } catch {
            $skip = $false
        }
    }
    $plan += New-Object PSObject -Property @{
        File = $file
        Fonts = $fonts
        NotInWindows = @(Get-MissingFonts $fonts $windowsFonts)
        Skip = $skip
        SkipReason = $why
        Again = $again
    }
}

Write-Host "Documents: $($inputs.Count) in, results to $outDir"
foreach ($entry in $plan) {
    $state = 'render'
    if ($FormattingOnly) { $state = 'read the formatting again' }
    if ($entry.Skip) { $state = "skip ($($entry.SkipReason))" }
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
        $formattingPath = Join-Path $outDir ($base + '.formatting.json')
        if ($FormattingOnly) {
            # The re-saved copy is read again; the earlier files stay until the new export is
            # known to be good.
            Write-Host "Reading the formatting of $($file.Name) again ..."
            $record = $entry.Again.Record
            $inputText = Get-PackageText $file.FullName
            $docTokens = @($tokens | Where-Object { $_.Always -or -not (Test-Token $inputText $_.Value) })
            $newPath = $formattingPath + '.new'
            $step = Read-DocumentFormatting $record $resaved $newPath $docTokens
            if ($record.status -eq 'rendered') {
                Move-Item -LiteralPath $newPath -Destination $formattingPath -Force
                Write-JsonFile $jsonPath $record
                $runs = 'without runs'
                if ($record.formatting_runs) { $runs = 'with runs' }
                $detail = "formatting of $($record.formatting_paragraphs) paragraphs $runs"
                if (-not $record.formatting_complete) { $detail += ' (incomplete: time budget)' }
                Write-Host "  rendered: $detail" -ForegroundColor Green
            } else {
                Remove-Item -LiteralPath $newPath -Force -ErrorAction SilentlyContinue
                Write-Host "  $($record.status): $($record.error); the earlier formatting is kept" -ForegroundColor Red
                foreach ($finding in $record.privacy_findings) { Write-Host "    $finding" -ForegroundColor Red }
            }
            $results += New-Object PSObject -Property @{ File = $file.Name; Status = $record.status; Missing = @() }
            if ($step.Status -eq 'stuck') { Exit-Stuck $step.Message $staging }
            if (($step.Status -eq 'error' -and @(Get-OwnWordProcess $since).Count -eq 0) -or $step.Status -eq 'timeout') {
                Write-Host 'Stopping: the Word instance is gone. Start the script again to read the remaining documents.' -ForegroundColor Red
                break
            }
            continue
        }
        foreach ($old in @($resaved, $pdf, $jsonPath, $formattingPath)) {
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
            formatting_file = $base + '.formatting.json'
            formatting_sha256 = ''
            formatting_paragraphs = 0
            formatting_runs = $false
            formatting_complete = $false
            formatting_seconds = 0
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
                $step = Read-DocumentFormatting $record $resaved $formattingPath $docTokens
                if ($step.Status -eq 'error') {
                    if (@(Get-OwnWordProcess $since).Count -eq 0) { $aborted = $true }
                } elseif ($step.Status -ne 'done') {
                    $aborted = $true
                }
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
        $runs = 'without runs'
        if ($record.formatting_runs) { $runs = 'with runs' }
        $detail = "$($record.pages) pages, compatibility mode $($record.compatibility_mode), formatting of $($record.formatting_paragraphs) paragraphs $runs"
        if ($record.formatting_paragraphs -gt 0 -and -not $record.formatting_complete) { $detail += ' (incomplete: time budget)' }
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
$verb = 'Rendered'
if ($FormattingOnly) { $verb = 'Formatting read again' }
Write-Host "$($verb): $rendered, skipped: $skipped, failed: $($failed.Count), not started: $notRun"
foreach ($item in $failed) { Write-Host "  failed: $($item.File) ($($item.Status))" -ForegroundColor Red }
if ($missing.Count -gt 0) {
    Write-Host "Fonts Word substituted: $($missing -join ', ')" -ForegroundColor Yellow
} else {
    Write-Host 'Fonts Word substituted: none'
}
Write-Host "Results: $outDir"
if ($failed.Count -gt 0 -or $notRun -gt 0 -or $userDocuments) { exit 1 }
exit 0
