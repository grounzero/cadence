# SPDX-License-Identifier: GPL-3.0-or-later
#
# Installs the release binary for this system once its checksum and bench match the release's, or
# builds Cadence from source at the release's tag where no binary fits or the check fails, on Windows
# with PowerShell alone; install.sh does the same in Git Bash, WSL, Linux and macOS. It asks before
# building, installing a compiler or Rust, replacing an install, changing the PATH or downloading
# tablebases, unless --yes is given.

Set-StrictMode -Version 2.0
# Native programs report through their exit codes, which are read one by one, so their error output
# never stops the script by itself.
$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'

$Repo = 'grounzero/cadence'
$RepoUrl = "https://github.com/$Repo.git"
$IssuesUrl = "https://github.com/$Repo/issues"
# The servers below are replaced only as Test-Urls allows, so a test can serve its own.
$ApiUrl = "https://api.github.com/repos/$Repo/releases"
$ReleasesUrl = "https://github.com/$Repo/releases/download"
$AssetsUrl = $ReleasesUrl
$TablesUrl = 'https://tablebase.lichess.ovh/tables/standard'
# How long a binary's bench, and its answer to uci, may take before it is stopped, in seconds.
$BenchSeconds = 600
$UciSeconds = 30
# The 290 tables of five pieces or fewer, in bytes, so their progress has a total before any arrives.
$TablesBytes = 983957920
# The workspace version these scripts ship with, which CI holds to Cargo.toml, so a version cut
# changes it here too.
$InstallerVersion = '0.6.0'

$script:Yes = $false
$script:KeepLog = $false
$script:Mode = 'install'
$script:Version = ''
$script:AskedVersion = ''
$script:Prefix = ''
$script:Build = ''
$script:Asked = ''
$script:Source = $false
$script:Tables = $false
$script:TestUrls = $false
$script:Stopped = $false
$script:Failed = $false
$script:Why = ''
$script:Built = ''
$script:Id = ''
$script:Attested = ''
$script:From = ''
$script:Made = ''
$script:Limited = $false
$script:UpToDate = $false
$script:Replace = $false
$script:Installed = ''
$script:HeldBuild = ''
$script:HeldFrom = ''
$script:Released = ''
$script:Unlisted = ''
$script:Earlier = $false
$script:Named = ''
$script:Unread = ''
$script:UnreadLimit = $false
$script:Where = 'default'
$script:Dest = ''
$script:Target = ''

function Show-Usage {
    @'
Usage: install.ps1 [options]

Installs Cadence's release binary for this system once its checksum and bench match the
release's, or builds it from source where no binary fits. It asks before building, installing a
compiler or Rust, replacing an install, changing your PATH or downloading tablebases. Without
--version it takes the latest stable release, or where none is published the newest pre-release,
asked first; a build from source names the tag it builds.

  --version V      this release rather than the default; with --source, also a test base x.y.z+n
  --dir DIR        install into DIR rather than the default folder; --prefix DIR is the same
  --generic        on x86-64, the generic build, for any processor with POPCNT
  --build TIER     on x86-64, the avx2 or the generic build; avx2 only where the processor runs it
  --source         build from source even where a release binary fits
  --update         as a plain run, which upgrades an older install, but also replace a file at
                   the install path that this script did not install, unless declined
  --uninstall      remove Cadence, and the PATH entry and tablebases if this script added them
  --tables         download the 3-4-5 piece tablebases without asking
  --yes            take every default without asking
  --keep-log       keep the log even when everything succeeds
  --test-urls      for tests: let CADENCE_RELEASES_URL, CADENCE_API_URL and CADENCE_TABLES_URL
                   name servers of their own; without it, CADENCE_RELEASES_URL must be this
                   repository's releases on github.com and the other two are refused
  --help           show this and stop
'@
}

# First on every run and first in the log, so any output says which installer wrote it.
Write-Host "Cadence installer $InstallerVersion"

$i = 0
while ($i -lt $args.Count) {
    $arg = [string]$args[$i]
    switch -Regex ($arg) {
        '^-{1,2}(yes|y)$' { $script:Yes = $true }
        '^-{1,2}keep-?log$' { $script:KeepLog = $true }
        '^-{1,2}update$' { $script:Mode = 'update' }
        '^-{1,2}uninstall$' { $script:Mode = 'uninstall' }
        '^-{1,2}source$' { $script:Source = $true }
        '^-{1,2}tables$' { $script:Tables = $true }
        '^-{1,2}test-urls$' { $script:TestUrls = $true }
        '^-{1,2}generic$' { $script:Build = 'generic' }
        '^-{1,2}(version|dir|prefix|build)$' {
            if ($i + 1 -ge $args.Count) { Write-Host "$arg needs a value"; exit 2 }
            $i++
            $value = [string]$args[$i]
            switch -Regex ($arg) {
                'version' { $script:Version = $value }
                '(dir|prefix)$' { $script:Prefix = $value }
                'build' { $script:Build = $value }
            }
        }
        '^-{1,2}(help|h)$' { Show-Usage; exit 0 }
        default { Write-Host "Unknown option: $arg"; Show-Usage; exit 2 }
    }
    $i++
}
if ($script:Build -notin @('', 'avx2', 'generic')) { Write-Host '--build takes avx2 or generic'; exit 2 }
# The build named on the command line, which an install of another build at the same version yields to.
$script:Asked = $script:Build
$script:AskedVersion = $script:Version
# --dir as one absolute spelling without trailing separators, so the state records one name for a folder.
if ($script:Prefix) {
    $full = [System.IO.Path]::GetFullPath($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($script:Prefix))
    while ($full.Length -gt 3 -and ($full.EndsWith('\') -or $full.EndsWith('/'))) { $full = $full.Substring(0, $full.Length - 1) }
    $script:Prefix = $full
}

# --- The system --------------------------------------------------------------------------------

$machine = $env:PROCESSOR_ARCHITECTURE
if ($env:PROCESSOR_ARCHITEW6432) { $machine = $env:PROCESSOR_ARCHITEW6432 }
switch ($machine) {
    'AMD64' { $Arch = 'x86_64' }
    'ARM64' { $Arch = 'aarch64' }
    default { Write-Host "This processor, $machine, is not one Cadence is built for."; exit 1 }
}
if (-not $env:LOCALAPPDATA) { Write-Host 'LOCALAPPDATA is not set, so give --dir.'; exit 1 }
$AppDir = Join-Path $env:LOCALAPPDATA 'Cadence'
$DefaultPrefix = $AppDir
$DataDir = $AppDir
$State = Join-Path $DataDir 'install-state'
$Work = Join-Path $DataDir "build.$PID"
$CargoBin = Join-Path $env:USERPROFILE '.cargo\bin'

# --- The log -----------------------------------------------------------------------------------

try {
    New-Item -ItemType Directory -Force -Path $AppDir -ErrorAction Stop | Out-Null
} catch {
    Write-Host "Cannot create $AppDir for the log."
    exit 1
}
$Log = Join-Path $AppDir ("install-{0}.log" -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
Set-Content -Path $Log -Value $null -Encoding UTF8

# The home folder is written as ~ in both slash directions, so the log names no user.
function Hide-Home([string]$Text) {
    $homeDir = $env:USERPROFILE
    if (-not $homeDir) { return $Text }
    foreach ($form in @($homeDir, $homeDir.Replace('\', '/'))) {
        $Text = [regex]::Replace($Text, [regex]::Escape($form), '~', 'IgnoreCase')
    }
    return $Text
}

function Write-Log([string]$Text) { Add-Content -Path $Log -Value (Hide-Home $Text) -Encoding UTF8 }
function Say([string]$Text) { Write-Host $Text; Write-Log $Text }
function Step([string]$Text) { Say ''; Say "== $Text" }

function Fail([string]$Text) {
    Say "Error: $Text"
    $script:Failed = $true
    throw 'cadence-install-stopped'
}

function Stop-Install([string]$Text) {
    Say $Text
    $script:Stopped = $true
    throw 'cadence-install-stopped'
}

# Shown as it runs and written to the log with its exit status, which is returned.
function Invoke-Logged([string]$File, [string[]]$Arguments) {
    Write-Log ('$ ' + $File + ' ' + ($Arguments -join ' '))
    & $File @Arguments 2>&1 | ForEach-Object {
        $line = "$_"
        Write-Host $line
        Write-Log $line
    }
    $code = $LASTEXITCODE
    Write-Log "(exit $code)"
    return $code
}

# Yes or no, the default in capitals; --yes takes the default.
function Ask([string]$Question, [bool]$Default) {
    $hint = if ($Default) { '[Y/n]' } else { '[y/N]' }
    if ($script:Yes) {
        Say ("$Question ${hint}: " + $(if ($Default) { 'y' } else { 'n' }) + ' (--yes)')
        return $Default
    }
    while ($true) {
        $reply = Read-Host "$Question $hint"
        Write-Log ("$Question ${hint}: " + $(if ($reply) { $reply } else { '<enter>' }))
        if (-not $reply) { return $Default }
        if ($reply -match '^(y|yes)$') { return $true }
        if ($reply -match '^(n|no)$') { return $false }
        Write-Host 'Please answer y or n.'
    }
}

# A numbered choice; --yes takes the first.
function Select-Choice([string]$Question, [string[]]$Options) {
    Say $Question
    for ($n = 0; $n -lt $Options.Count; $n++) { Say ("  {0}. {1}" -f ($n + 1), $Options[$n]) }
    if ($script:Yes) { Say 'Choice: 1 (--yes)'; return 1 }
    while ($true) {
        $reply = Read-Host 'Choice [1]'
        Write-Log ('Choice: ' + $(if ($reply) { $reply } else { '<enter>' }))
        if (-not $reply) { return 1 }
        $number = 0
        if ([int]::TryParse($reply, [ref]$number) -and $number -ge 1 -and $number -le $Options.Count) {
            return $number
        }
        Write-Host 'Please answer with one of the numbers.'
    }
}

# The state's lines, read as UTF-8 past any byte order mark this script wrote before the two scripts
# shared one format.
function Get-StateLines {
    if (-not (Test-Path -LiteralPath $State)) { return @() }
    return @([System.IO.File]::ReadAllLines($State, [System.Text.Encoding]::UTF8))
}

function Get-StateValue([string]$Key) {
    foreach ($line in Get-StateLines) {
        if ($line.StartsWith("$Key=")) { return $line.Substring($Key.Length + 1) }
    }
    return ''
}

# A path from the state as Windows names it; install.sh before this one wrote it as /c/....
function Get-StatePath([string]$Key) {
    $value = Get-StateValue $Key
    if ($value -match '^/([A-Za-z])/(.*)$') { return $Matches[1].ToUpper() + ':\' + $Matches[2].Replace('/', '\') }
    return $value
}

# A value already held is not written again, so a second run leaves the file as it was.
# The file is UTF-8 without a byte order mark and with one line feed a line, as install.sh writes it.
function Set-StateValue([string]$Key, [string]$Value) {
    if ((Test-Path -LiteralPath $State) -and (Get-StateValue $Key) -ceq $Value) { return }
    $lines = @(Get-StateLines | Where-Object { -not $_.StartsWith("$Key=") })
    $lines += "$Key=$Value"
    [System.IO.File]::WriteAllText($State, (($lines -join "`n") + "`n"), (New-Object System.Text.UTF8Encoding $false))
}

function Remove-StateValue([string]$Key) {
    if (-not (Get-StateValue $Key)) { return }
    $lines = @(Get-StateLines | Where-Object { -not $_.StartsWith("$Key=") })
    [System.IO.File]::WriteAllText($State, (($lines -join "`n") + "`n"), (New-Object System.Text.UTF8Encoding $false))
}

# Runs a program in the work folder for at most Seconds, with Feed as its input, and returns its exit
# code and output, or in Failed why Windows would not start it. On expiry taskkill ends it and every
# process it started, and Limited is set.
function Invoke-Limited([string]$Binary, [string]$Arguments, [string]$Feed, [int]$Seconds) {
    $script:Limited = $false
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $Binary
    $info.Arguments = $Arguments
    $info.WorkingDirectory = $Work
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardInput = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    try {
        $process = [System.Diagnostics.Process]::Start($info)
    } catch {
        $cause = $_.Exception
        while ($cause.InnerException) { $cause = $cause.InnerException }
        Write-Log "starting $Binary failed: $($cause.Message)"
        $name = Split-Path -Leaf $Binary
        $number = if ($cause -is [System.ComponentModel.Win32Exception]) { $cause.NativeErrorCode } else { 0 }
        # Access denied, a virus, or a block by group policy or by application control, which stops the
        # run with the log kept; any other failure is the binary's.
        if ($number -in @(5, 225, 1260, 4551)) {
            $script:KeepLog = $true
            Say "Windows blocked $name from running. This is most likely application control or antivirus software, which managed machines often have."
            Stop-Install "Installing Cadence here needs this machine's administrator, or a machine without that policy."
        }
        # Not a valid program, or one for another processor.
        if ($number -in @(193, 216)) { return @{ Code = -1; Out = ''; Failed = 'which does not take it for a program' } }
        return @{ Code = -1; Out = ''; Failed = "which answered: $($cause.Message)" }
    }
    $out = $process.StandardOutput.ReadToEndAsync()
    $err = $process.StandardError.ReadToEndAsync()
    try {
        $bytes = [System.Text.Encoding]::ASCII.GetBytes($Feed)
        $process.StandardInput.BaseStream.Write($bytes, 0, $bytes.Length)
        $process.StandardInput.BaseStream.Close()
    } catch { }
    if (-not $process.WaitForExit($Seconds * 1000)) {
        $script:Limited = $true
        & taskkill.exe /F /T /PID $process.Id 2>&1 | ForEach-Object { Write-Log "$_" }
        $process.WaitForExit(10000) | Out-Null
    }
    $code = if ($process.HasExited) { $process.ExitCode } else { -1 }
    $text = if ($out.Wait(10000)) { $out.Result } else { '' }
    if ($err.Wait(10000) -and $err.Result) { Write-Log $err.Result.TrimEnd() }
    return @{ Code = $code; Out = $text; Failed = '' }
}

function Get-IdName([string]$Binary) {
    # The engine answered only its banner when Windows PowerShell piped this in, as if the first
    # command carried a byte order mark, so an empty line goes first to take one.
    try { Write-Log ('console input encoding: ' + [Console]::InputEncoding.WebName) } catch { }
    $run = Invoke-Limited $Binary '' "`nuci`nquit`n" $UciSeconds
    $lines = @($run.Out -split "`r?`n" | Where-Object { $_ })
    Write-Log ('uci answered: ' + ($lines -join ' | '))
    foreach ($line in $lines) {
        if ($line -match '^id name (.*)$') { return $Matches[1].Trim() }
    }
    return ''
}

function Write-SystemReport {
    Write-Log "Cadence installer $InstallerVersion"
    Write-Log ("installer: $script:Mode " + $script:Version)
    Write-Log ('system: ' + [Environment]::OSVersion.VersionString + ", $machine")
    try { Write-Log ('os: ' + (Get-CimInstance Win32_OperatingSystem -ErrorAction Stop).Caption) } catch { }
    try { Write-Log ('cpu: ' + (Get-CimInstance Win32_Processor -ErrorAction Stop | Select-Object -First 1).Name) } catch { }
    Write-Log ('powershell: ' + $PSVersionTable.PSVersion)
}

# CADENCE_RELEASES_URL may name only this repository's releases, and the other two servers stay as
# they are, unless --test-urls lets a test name its own; whichever is in use is said.
function Test-Urls {
    if ($env:CADENCE_RELEASES_URL) {
        if ($env:CADENCE_RELEASES_URL -cnotin @($ReleasesUrl, "$ReleasesUrl/") -and -not $script:TestUrls) {
            Fail "CADENCE_RELEASES_URL is $env:CADENCE_RELEASES_URL, which is not $ReleasesUrl; only a test may name another server, with --test-urls"
        }
        $script:AssetsUrl = $env:CADENCE_RELEASES_URL.TrimEnd('/')
    }
    if ($env:CADENCE_API_URL) {
        if (-not $script:TestUrls) { Fail 'CADENCE_API_URL is set, which only a test may do, with --test-urls' }
        $script:ApiUrl = $env:CADENCE_API_URL
        Say "GitHub's API is replaced by $($script:ApiUrl), as CADENCE_API_URL and --test-urls ask."
    }
    if ($env:CADENCE_TABLES_URL) {
        if (-not $script:TestUrls) { Fail 'CADENCE_TABLES_URL is set, which only a test may do, with --test-urls' }
        $script:TablesUrl = $env:CADENCE_TABLES_URL
    }
}

# A release's x.y.z or x.y.z-rc.n, or with --source a test base's x.y.z+n, which has no release.
function Test-Version {
    if (-not $script:Version) { return }
    if ($script:Version -match '^\d+\.\d+\.\d+\+\d+$') {
        if (-not $script:Source) { Fail "$script:Version is a test base and not a release, so it has no binary; --source builds it" }
        return
    }
    if ($script:Version -notmatch '^\d+\.\d+\.\d+(-rc\.\d+)?$') {
        Fail "$script:Version is not a release's version, which reads x.y.z or x.y.z-rc.n"
    }
}

function Test-ProcessorFeature([uint32]$Feature) {
    try {
        if (-not ('CadenceInstall.Cpu' -as [type])) {
            Add-Type -Namespace CadenceInstall -Name Cpu -MemberDefinition @'
[DllImport("kernel32.dll")]
public static extern bool IsProcessorFeaturePresent(uint feature);
'@
        }
        # A Windows that does not know the feature answers no.
        return [CadenceInstall.Cpu]::IsProcessorFeaturePresent($Feature)
    } catch {
        return $false
    }
}

function Confirm-System {
    Step 'This system'
    if ($Arch -ne 'x86_64') {
        $script:Build = 'native'
        Say 'Windows on arm64, which Cadence has one build for.'
        return
    }
    # PF_AVX2_INSTRUCTIONS_AVAILABLE. Windows names no flag for BMI1, LZCNT or POPCNT, which every
    # processor with AVX2 also has, and the bench run catches a processor without POPCNT.
    $avx2 = Test-ProcessorFeature 40
    Write-Log "AVX2: $avx2"
    if ($avx2) {
        Say 'Windows on x86-64, with AVX2, BMI1, LZCNT and POPCNT: the AVX2 build runs here.'
    } else {
        Say 'Windows on x86-64, without AVX2: the generic build runs here, for any x86-64 with POPCNT.'
    }
    if ($script:Build -eq 'avx2' -and -not $avx2) {
        Fail '--build avx2 needs AVX2, BMI1, LZCNT and POPCNT, and this processor lacks AVX2'
    } elseif ($script:Build) {
        Say "The $script:Build build, as asked."
    } elseif ($avx2) {
        $script:Build = 'avx2'
        Say '--generic takes the generic build instead, for any x86-64 with POPCNT.'
    } else {
        $script:Build = 'generic'
    }
}

# --- The release binary ------------------------------------------------------------------------

# A version as five numbers that sort in release order: a candidate before its release, and a test
# base after the release it follows.
function Get-VersionKey([string]$Version) {
    if ($Version -match '^(\d+)\.(\d+)\.(\d+)-rc\.(\d+)') { return @([int]$Matches[1], [int]$Matches[2], [int]$Matches[3], 0, [int]$Matches[4]) }
    if ($Version -match '^(\d+)\.(\d+)\.(\d+)\+(\d+)') { return @([int]$Matches[1], [int]$Matches[2], [int]$Matches[3], 2, [int]$Matches[4]) }
    if ($Version -match '^(\d+)\.(\d+)\.(\d+)') { return @([int]$Matches[1], [int]$Matches[2], [int]$Matches[3], 1, 0) }
    return @(0, 0, 0, 0, 0)
}

# -1, 0 or 1 as the first version is older than, the same as or newer than the second.
function Compare-Version([string]$First, [string]$Second) {
    $a = Get-VersionKey $First
    $b = Get-VersionKey $Second
    for ($n = 0; $n -lt 5; $n++) {
        if ($a[$n] -lt $b[$n]) { return -1 }
        if ($a[$n] -gt $b[$n]) { return 1 }
    }
    return 0
}

# GitHub's latest release is the newest stable one, never a pre-release, so without one the newest
# pre-release by version is offered instead, and with neither nothing is built.
function Find-Release {
    Step 'Finding the release'
    if ($script:Version) {
        Say "Release $script:Version, as asked."
        return
    }
    $tag = ''
    # GitHub's API limits requests by address unless they carry a token, so GITHUB_TOKEN is sent there.
    $headers = @{}
    if ($env:GITHUB_TOKEN) { $headers['Authorization'] = "Bearer $env:GITHUB_TOKEN" }
    Write-Log "downloading $ApiUrl/latest"
    try {
        $latest = Invoke-RestMethod -UseBasicParsing -Uri "$ApiUrl/latest" -Headers $headers -ErrorAction Stop
        $tag = "$($latest.tag_name)"
    } catch {
        Test-RateLimit (Get-DownloadTrouble $_) $_
    }
    if ($tag -match '^\d+\.\d+\.\d+$') {
        $script:Version = $tag
        Say "The latest stable release is $tag."
    } else {
        Write-Log "downloading ${ApiUrl}?per_page=100"
        try {
            $releases = Invoke-RestMethod -UseBasicParsing -Uri "${ApiUrl}?per_page=100" -Headers $headers -ErrorAction Stop
        } catch {
            $trouble = Get-DownloadTrouble $_
            Test-RateLimit $trouble $_
            Fail "could not read Cadence's releases from GitHub$trouble"
        }
        foreach ($release in @($releases)) {
            $name = "$($release.tag_name)"
            if ($name -notmatch '^\d+\.\d+\.\d+-rc\.\d+$') { continue }
            if (-not $script:Version -or (Compare-Version $name $script:Version) -gt 0) { $script:Version = $name }
        }
        if (-not $script:Version) {
            if ($script:Source) { Stop-Install 'With no published release, --source needs --version.' }
            Say 'Cadence has no published release yet, so there is no binary to install.'
            Stop-Install '--source --version X.Y.Z builds a tagged version from source.'
        }
        Say "No stable release of Cadence exists yet. The newest pre-release is $script:Version."
        if (-not $script:Source) {
            Say 'To install it without this question:'
            $update = if ($script:Mode -eq 'update') { ' --update' } else { '' }
            Say "powershell -ExecutionPolicy Bypass -File install.ps1$update --version $script:Version"
            if (-not (Ask "Install pre-release $($script:Version)?" $true)) { Stop-Install 'Nothing was installed.' }
        }
    }
    # The installer's own version is offered only where a release of it is published.
    if ($script:Version -ne $InstallerVersion) {
        Write-Log "downloading $ApiUrl/tags/$InstallerVersion"
        try {
            Invoke-RestMethod -UseBasicParsing -Uri "$ApiUrl/tags/$InstallerVersion" -Headers $headers -ErrorAction Stop | Out-Null
            Say "This installer comes with Cadence $InstallerVersion; --version $InstallerVersion installs that one instead."
        } catch {
            Write-Log "release $InstallerVersion is not offered$(Get-DownloadTrouble $_)"
        }
    }
}

# Whether a failed request's answer says it is GitHub's rate limit, as its headers do; a refusal for
# any other reason is not explained as one.
function Test-GitHubLimit($Failure) {
    try {
        $answer = $Failure.Exception.Response.Headers
        if ($answer -is [System.Net.WebHeaderCollection]) {
            return ($answer['X-RateLimit-Remaining'] -eq '0' -or [bool]$answer['Retry-After'])
        }
        $values = $null
        if ($answer.TryGetValues('X-RateLimit-Remaining', [ref]$values) -and @($values) -contains '0') { return $true }
        return [bool]$answer.Contains('Retry-After')
    } catch {
        return $false
    }
}

# GitHub refusing the lookup for its hourly limit is said as such, never read as there being no release.
function Test-RateLimit([string]$Trouble, $Failure) {
    if (($Trouble -eq ' (the server answered 403)' -or $Trouble -eq ' (the server answered 429)') -and (Test-GitHubLimit $Failure)) {
        Fail "GitHub refused to name Cadence's releases$($Trouble): it limits how often one address asks. Try again in an hour, or set GITHUB_TOKEN to a GitHub token and run this again."
    }
}

# Why a download failed, in plain words and parentheses; the exception's own text goes to the log.
function Get-DownloadTrouble($Failure) {
    $exception = $Failure.Exception
    Write-Log "$($exception.Message)"
    $code = 0
    if ($exception.PSObject.Properties['Response'] -and $exception.Response) {
        try { $code = [int]$exception.Response.StatusCode } catch { $code = 0 }
    }
    if (-not $code -and "$($exception.Message)" -match '\((\d{3})\)') { $code = [int]$Matches[1] }
    if ($code -eq 404) { return ' (not found)' }
    if ($code) { return " (the server answered $code)" }
    $message = "$($exception.Message)"
    if ($message -match 'could not be resolved|Unable to connect|No such host|actively refused|network') { return ' (no connection to the server)' }
    if ($message -match 'timed out') { return ' (timed out)' }
    if ($message -match 'closed unexpectedly|connection was closed|was aborted') { return ' (the server closed the connection)' }
    return ' (the log has the details)'
}

# The empty string once the file is in the work folder, or why it is not, in parentheses.
function Save-Asset([string]$Name) {
    $url = "$AssetsUrl/$($script:Version)/$Name"
    Write-Log "downloading $url"
    try {
        Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile (Join-Path $Work $Name) -ErrorAction Stop
        return ''
    } catch {
        return (Get-DownloadTrouble $_)
    }
}

# The release's SHA256SUMS in the work folder, downloaded once, or false with the reason in Why.
function Get-ReleaseSums {
    $sums = Join-Path $Work 'SHA256SUMS'
    if ((Test-Path -LiteralPath $sums) -and (Get-Item -LiteralPath $sums).Length -gt 0) { return $true }
    $failed = Save-Asset 'SHA256SUMS'
    if ($failed) {
        Remove-Item -Force -LiteralPath $sums -ErrorAction SilentlyContinue
        $script:Why = "Release $script:Version publishes no checksum file$failed, so none of its binaries is used."
        return $false
    }
    return $true
}

# The file the release's SHA256SUMS gives a SHA-256 to, or the empty string.
function Get-ListedAs([string]$Sum) {
    foreach ($line in Get-Content -Path (Join-Path $Work 'SHA256SUMS')) {
        $parts = @("$line".Trim() -split '\s+')
        if ($parts.Count -ge 2 -and $parts[0] -eq $Sum) { return $parts[1].TrimStart('*') }
    }
    return ''
}

# The SHA-256 the release's SHA256SUMS gives a file, or the empty string.
function Get-Listed([string]$Name) {
    foreach ($line in Get-Content -Path (Join-Path $Work 'SHA256SUMS')) {
        $parts = @("$line".Trim() -split '\s+')
        if ($parts.Count -ge 2 -and ($parts[1] -eq $Name -or $parts[1] -eq "*$Name")) { return $parts[0].ToLower() }
    }
    return ''
}

# A file's or folder's identity as Windows keeps it, its volume's serial number and its file index,
# or the empty string where it cannot be opened; links, junctions, subst drives and UNC names share it.
function Get-FileIdentity([string]$Path) {
    try {
        if (-not ('CadenceInstall.FileId' -as [type])) {
            Add-Type -Namespace CadenceInstall -Name FileId -MemberDefinition @'
[StructLayout(LayoutKind.Sequential)]
public struct Info {
    public uint Attributes, CreatedLow, CreatedHigh, AccessedLow, AccessedHigh, WrittenLow, WrittenHigh;
    public uint Volume, SizeHigh, SizeLow, Links, IndexHigh, IndexLow;
}
[DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern Microsoft.Win32.SafeHandles.SafeFileHandle CreateFile(string name, uint access, uint share, IntPtr security, uint disposition, uint flags, IntPtr template);
[DllImport("kernel32.dll", SetLastError = true)]
public static extern bool GetFileInformationByHandle(Microsoft.Win32.SafeHandles.SafeFileHandle handle, out Info info);
public static string Of(string path) {
    using (var handle = CreateFile(path, 0, 7, IntPtr.Zero, 3, 0x02000000, IntPtr.Zero)) {
        Info info;
        if (handle.IsInvalid || !GetFileInformationByHandle(handle, out info)) { return ""; }
        return info.Volume.ToString("x8") + ":" + info.IndexHigh.ToString("x8") + info.IndexLow.ToString("x8");
    }
}
'@
        }
        return [CadenceInstall.FileId]::Of($Path)
    } catch {
        Write-Log "the identity of $Path was not read: $_"
        return ''
    }
}

# Whether two paths are one file or folder, by the identity Windows keeps where both can be opened,
# and otherwise by their full spelling.
function Test-SameFile([string]$First, [string]$Second) {
    if (-not $First -or -not $Second) { return $false }
    $one = Get-FileIdentity $First
    $two = Get-FileIdentity $Second
    if ($one -and $two) { return $one -eq $two }
    return [System.IO.Path]::GetFullPath($First).TrimEnd('\') -eq [System.IO.Path]::GetFullPath($Second).TrimEnd('\')
}

function Get-Sha256([string]$Path) { return (Get-FileHash -Algorithm SHA256 -Path $Path).Hash.ToLower() }

# The release workflow attests every binary it builds, run at the version's tag, and gh checks that
# attestation where it is installed and signed in. Where gh cannot check, that is said and the install
# goes on; an attestation that fails leaves Why.
function Test-Attestation([string]$File) {
    $script:Attested = 'no'
    if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
        Say "Its build attestation was not verified: gh, GitHub's command line tool, is not installed."
        return $true
    }
    $ghArgs = @('attestation', 'verify', (Join-Path $Work $File), '--repo', $Repo,
        '--cert-identity', "https://github.com/$Repo/.github/workflows/release.yml@refs/tags/$($script:Version)")
    Write-Log ('$ gh ' + ($ghArgs -join ' '))
    $said = @(& gh @ghArgs 2>&1 | ForEach-Object { "$_".Trim() })
    $code = $LASTEXITCODE
    foreach ($line in $said) { Write-Log $line }
    Write-Log "(exit $code)"
    $last = "$(@($said | Where-Object { $_ }) | Select-Object -Last 1)"
    if ($code -eq 0) {
        $script:Attested = 'yes'
        Say "Its build attestation verifies: GitHub's release workflow built it at the tag $($script:Version)."
        return $true
    }
    # gh exits 4 when it is not signed in, and an older gh does not know the command or its options.
    if ($code -eq 4) {
        Say 'Its build attestation was not verified: gh is installed but not signed in (gh auth login).'
        return $true
    }
    if (($said -join "`n") -match 'unknown (command|flag|shorthand flag)') {
        Say 'Its build attestation was not verified: this gh is too old to check one.'
        return $true
    }
    # A verifier that never loaded its trust root reports a failed verification too, so it is read
    # first, as gh failing to check. Otherwise a verdict is no attestation for this file, a signature
    # that is not the release workflow's at the tag, or a certificate naming something else.
    if (($said -join "`n") -cmatch 'failed to choose verifier|initialization may have failed') {
        Say "Its build attestation was not verified: gh could not check it ($last)."
        return $true
    }
    if (($said -join "`n") -cmatch 'HTTP 404.*/attestations/|[Nn]o attestations found|expected [A-Za-z]+ to be|verifying with issuer|[Vv]erification failed') {
        $script:Why = "$File's build attestation does not verify ($last), so it is not used."
        return $false
    }
    Say "Its build attestation was not verified: gh could not check it ($last)."
    return $true
}

# Leaves the checked binary in Built, or the reason none is used in Why.
function Test-Binary {
    if ($Arch -ne 'x86_64') {
        $script:Why = "Cadence's releases have no binary for Windows on arm64."
        return
    }
    $file = "cadence-$($script:Version)-windows-x86-64-$($script:Build).exe"
    if (-not (Get-ReleaseSums)) { return }
    $want = Get-Listed $file
    if (-not $want) {
        $script:Why = "Release $script:Version's SHA256SUMS lists no $file, so there is no binary for this system."
        return
    }
    Say "Release $script:Version has a binary for this system: $file."

    Step "Checking $file"
    $failed = Save-Asset $file
    if ($failed) {
        $script:Why = "Could not download $file$failed, so it is not used."
        return
    }
    $path = Join-Path $Work $file
    $got = Get-Sha256 $path
    Write-Log "${file}: SHA-256 $got; SHA256SUMS: $want"
    if ($got -ne $want) {
        $script:Why = "$file's SHA-256 is $got and SHA256SUMS says $want, so it is not used."
        return
    }
    Say "Its SHA-256 matches the release's SHA256SUMS."
    if (-not (Test-Attestation $file)) { return }
    $want = Get-Listed 'bench.txt'
    if (-not $want) {
        $script:Why = "Release $script:Version's SHA256SUMS lists no bench.txt to check $file against, so it is not used."
        return
    }
    $failed = Save-Asset 'bench.txt'
    if ($failed) {
        $script:Why = "Could not download the release's bench.txt$failed, so $file is not used."
        return
    }
    $bench = Join-Path $Work 'bench.txt'
    if ((Get-Sha256 $bench) -ne $want) {
        $script:Why = "The release's bench.txt does not match SHA256SUMS, so $file is not used."
        return
    }
    $benchWant = "$(Get-Content -Raw -Path $bench)".Trim()
    if (-not (Test-BenchAndName $path $benchWant "Cadence $script:Version")) {
        $script:Why = 'I' + $script:Why.Substring(1) + ', so it is not used.'
        return
    }
    $script:Built = $path
    $script:From = 'release'
}

# --- The build ---------------------------------------------------------------------------------

function Request-Source {
    if ($script:Source) {
        Step 'Building from source, as --source asks'
    } else {
        Say $script:Why
        Step 'Building from source instead'
    }
    Say 'Building takes several minutes and about 2 GB of disk space, for the source, the Rust'
    Say 'version Cadence pins if it is not installed yet, and the build. Both are estimates.'
    New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
    try {
        $drive = Get-PSDrive -Name ($DataDir.Substring(0, 1)) -ErrorAction Stop
        $free = [math]::Floor($drive.Free / 1MB)
        Say "$DataDir has $free MB free."
        if ($free -lt 2048) { Say 'That may not be enough.' }
    } catch { }
    if (-not (Ask "Build Cadence $($script:Version) from source, at its tag $($script:Version)?" $true)) { Stop-Install 'Nothing was installed.' }
}

function Test-Git {
    if (Get-Command git -ErrorAction SilentlyContinue) {
        Write-Log ('git: ' + (git --version))
        return
    }
    Say "Git is needed to fetch Cadence's source, and it is not installed."
    Say 'Install Git for Windows from https://git-scm.com/download/win, or with: winget install --id Git.Git -e'
    Fail 'git is not installed'
}

function Get-MsvcVersion {
    $component = 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64'
    if ($Arch -ne 'x86_64') { $component = 'Microsoft.VisualStudio.Component.VC.Tools.ARM64' }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) { return '' }
    $found = & $vswhere -latest -products '*' -requires $component -property installationVersion 2>$null
    return "$($found | Select-Object -First 1)".Trim()
}

function Test-Linker {
    $found = Get-MsvcVersion
    if ($found) {
        Write-Log "linker: Visual Studio C++ build tools $found"
        return
    }
    Say "Rust needs Microsoft's C++ build tools to link with, and they are not installed."
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
        Fail 'winget is not available: install the Visual Studio Build Tools with C++ from https://visualstudio.microsoft.com/visual-cpp-build-tools/, then run this again'
    }
    if (-not (Ask 'Install them now with winget (several GB, and Windows may ask for permission)?' $true)) {
        Stop-Install 'Install the Visual Studio Build Tools with C++, then run this again.'
    }
    $code = Invoke-Logged 'winget' @('install', '--id', 'Microsoft.VisualStudio.2022.BuildTools', '-e',
        '--accept-package-agreements', '--accept-source-agreements', '--override',
        '--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended')
    if ($code -ne 0) { Fail "the build tools' install failed" }
    $found = Get-MsvcVersion
    if (-not $found) { Fail 'the C++ build tools are still not found after the install' }
    Write-Log "linker: Visual Studio C++ build tools $found"
}

function Install-Rustup {
    $rustupArch = 'x86_64'
    if ($Arch -ne 'x86_64') { $rustupArch = 'aarch64' }
    $init = Join-Path $Work 'rustup-init.exe'
    Write-Log "downloading https://win.rustup.rs/$rustupArch"
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "https://win.rustup.rs/$rustupArch" -OutFile $init -ErrorAction Stop
    } catch {
        Fail "could not download rustup-init.exe$(Get-DownloadTrouble $_)"
    }
    if ((Invoke-Logged $init @('-y', '--default-toolchain', 'none')) -ne 0) { Fail "rustup's install failed" }
    $env:Path = "$CargoBin;$env:Path"
    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) { Fail 'rustup is still not found after its install' }
}

function Test-Rust {
    if (Test-Path $CargoBin) { $env:Path = "$CargoBin;$env:Path" }
    if (Get-Command rustup -ErrorAction SilentlyContinue) {
        Write-Log ('rustup: ' + "$(rustup --version 2>$null | Select-Object -First 1)")
        return
    }
    if (Get-Command cargo -ErrorAction SilentlyContinue) {
        Say "This Rust came without rustup, as a package manager's does, so it cannot fetch the Rust version Cadence pins."
    } else {
        Say 'Rust is not installed.'
    }
    $choice = Select-Choice 'How would you like to go on?' @(
        'Install rustup now, from rust-lang.org',
        'Show the steps to install it by hand',
        'Quit')
    switch ($choice) {
        1 { Install-Rustup }
        2 {
            Say '1. Download rustup-init.exe from https://rustup.rs and run it.'
            Say '2. Open a new terminal, so cargo is on your PATH.'
            Say '3. Run this script again. The Rust version Cadence pins installs at its first build.'
            Stop-Install 'Nothing was installed.'
        }
        default { Stop-Install 'Nothing was installed.' }
    }
}

# The tag the source is fetched at, the version asked for or found, checked against the repository's.
function Resolve-Tag {
    Write-Log "`$ git ls-remote --tags --refs $RepoUrl"
    $listing = git ls-remote --tags --refs $RepoUrl 2>&1
    if ($LASTEXITCODE -ne 0) {
        foreach ($line in $listing) { Write-Log "$line" }
        Fail "could not list the releases at $RepoUrl"
    }
    $tags = @($listing | ForEach-Object { "$_" -replace '^.*refs/tags/', '' })
    if ($tags -notcontains $script:Version) {
        if ($script:Version.Contains('+')) { Fail "there is no test base $script:Version at $RepoUrl" }
        Fail "there is no release $script:Version at $RepoUrl"
    }
}

function Get-Source {
    Step "Fetching Cadence $script:Version"
    $script:Src = Join-Path $Work 'cadence'
    if (Test-Path $script:Src) { Remove-Item -Recurse -Force $script:Src }
    $code = Invoke-Logged 'git' @('-c', 'init.defaultBranch=main', 'init', '--quiet', $script:Src)
    if ($code -ne 0) { Fail "could not create $($script:Src)" }
    $code = Invoke-Logged 'git' @('-C', $script:Src, 'remote', 'add', 'origin', $RepoUrl)
    if ($code -ne 0) { Fail "could not name the repository in $($script:Src)" }
    # A release names itself from its annotated tag, so the tag is fetched as itself.
    $code = Invoke-Logged 'git' @('-C', $script:Src, 'fetch', '--quiet', '--depth', '1', 'origin',
        "+refs/tags/$($script:Version):refs/tags/$($script:Version)")
    if ($code -ne 0) { Fail "could not fetch Cadence $script:Version" }
    $code = Invoke-Logged 'git' @('-C', $script:Src, '-c', 'advice.detachedHead=false', 'checkout', '--quiet',
        '--detach', "refs/tags/$($script:Version)")
    if ($code -ne 0) { Fail "could not check out Cadence $script:Version" }
}

# The commit the release's notes say it was built from, in Named, or nothing where no release names
# one: a test base, or a version from before releases had pages. Where GitHub's API does not answer,
# Unread says why, and the build goes on without the check.
function Get-ReleaseCommit {
    $script:Named = ''
    $script:Unread = ''
    $script:UnreadLimit = $false
    if ($script:Version.Contains('+')) { return }
    $headers = @{}
    if ($env:GITHUB_TOKEN) { $headers['Authorization'] = "Bearer $env:GITHUB_TOKEN" }
    Write-Log "downloading $ApiUrl/tags/$($script:Version)"
    try {
        $release = Invoke-RestMethod -UseBasicParsing -Uri "$ApiUrl/tags/$($script:Version)" -Headers $headers -ErrorAction Stop
    } catch {
        $trouble = Get-DownloadTrouble $_
        if ($trouble -ne ' (not found)') { $script:Unread = $trouble }
        $script:UnreadLimit = Test-GitHubLimit $_
        return
    }
    if ("$($release.body)" -match 'built from ([0-9a-f]{40}) at the tag') { $script:Named = $Matches[1] }
    if (-not $script:Named) { Fail "release $($script:Version)'s notes name no commit, so its source cannot be checked against them" }
}

# The fetched tag must be the commit the release names before anything is built from it.
function Test-Commit {
    $got = "$(git -C $script:Src rev-parse HEAD 2>$null)".Trim()
    if ($LASTEXITCODE -ne 0 -or -not $got) { Fail "could not read the commit Cadence $($script:Version) was fetched at" }
    Write-Log "fetched commit: $got; the release names: $(if ($script:Named) { $script:Named } else { 'none' })"
    if ($script:Unread) {
        Say "GitHub's API did not answer$($script:Unread), so the commit release $($script:Version) names could not be read. It is built at its tag, commit $got, without that check."
        if ($script:UnreadLimit) { Say 'GitHub limits how often one address asks; set GITHUB_TOKEN to a GitHub token to check it next time.' }
        return
    }
    if (-not $script:Named) {
        if ($script:Version.Contains('+')) {
            Say "Test base $($script:Version) is commit $got; a test base has no release to name one."
        } else {
            Say "Version $($script:Version) has no release page naming its commit, so it is built at its tag, commit $got."
        }
        return
    }
    if ($got -ne $script:Named) {
        Fail "the tag $($script:Version) is commit $got, and the release names $($script:Named), so it is not built"
    }
    Say "The tag $($script:Version) is commit $got, the one the release names."
}

function Initialize-Toolchain {
    Step 'The Rust toolchain'
    Push-Location $script:Src
    try {
        if ((Invoke-Logged 'rustup' @('toolchain', 'install')) -ne 0) {
            Say 'This rustup installs the Rust version Cadence pins as the build starts instead.'
        }
        Write-Log ('rustc: ' + "$(rustc --version 2>&1)")
        Write-Log ('cargo: ' + "$(cargo --version 2>&1)")
        $script:Triple = "$(rustc -vV 2>$null | Where-Object { $_ -match '^host: ' })" -replace '^host: ', ''
    } finally {
        Pop-Location
    }
    if (-not $script:Triple) { Fail 'rustc does not name the system it builds for' }
    Write-Log "target: $script:Triple"
}

# The rustflags a config file of the repository gives the target, from an array of strings or a string
# of words, with Unreadable set where a rustflags key has any other form.
function Get-RepoFlags([string]$Path) {
    $result = @{ Flags = @(); Unreadable = $false }
    if (-not (Test-Path -LiteralPath $Path)) { return $result }
    $on = $false
    $grab = $false
    $key = $false
    $raw = ''
    foreach ($line in Get-Content -LiteralPath $Path) {
        if ($line -match '^\s*#') { continue }
        $line = $line -replace '\s*#[^"]*$', ''
        if ($line -eq "[target.$($script:Triple)]") { $on = $true; continue }
        if ($line -match '^\[') { $on = $false }
        if ($on -and $line -match '^\s*rustflags\s*=') {
            $key = $true
            $grab = $true
            $line = $line.Substring($line.IndexOf('=') + 1)
        }
        if ($grab) {
            $raw += ' ' + $line
            if ($raw -match '^\s*"' -or $line.Contains(']')) { $grab = $false }
        }
    }
    if (-not $key) { return $result }
    if ($raw -match '^\s*\[') {
        foreach ($match in [regex]::Matches($raw, '"([^"]*)"')) { $result.Flags += $match.Groups[1].Value }
        if ([regex]::Replace($raw, '"[^"]*"', '') -match '[^\[\]\s,]') { $result.Unreadable = $true }
    } elseif ($raw -match '^\s*"([^"]*)"\s*$') {
        $result.Flags = @($Matches[1] -split '\s+' | Where-Object { $_ })
    } else {
        $result.Unreadable = $true
    }
    return $result
}

# Sets a variable for this process and the programs it starts, an empty value included: .NET deletes a
# variable set empty, so kernel32 sets that one.
function Set-ProcessVariable([string]$Name, $Value) {
    if ($null -eq $Value -or "$Value") {
        [Environment]::SetEnvironmentVariable($Name, $Value, 'Process')
        return
    }
    try {
        if (-not ('CadenceInstall.Env' -as [type])) {
            Add-Type -Namespace CadenceInstall -Name Env -MemberDefinition @'
[DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
public static extern bool SetEnvironmentVariable(string name, string value);
'@
        }
        [void][CadenceInstall.Env]::SetEnvironmentVariable($Name, '')
    } catch {
        Write-Log "could not set $Name empty: $_"
    }
}

function Invoke-Build {
    Step "Building Cadence $script:Version"
    $cargoArgs = @('build', '--release', '--locked', '-p', 'cadence-engine', '--bin', 'cadence')
    $files = @('.cargo\config.toml')
    $section = '^\[target\.' + [regex]::Escape($script:Triple) + '\]'
    $script:Made = $script:Build
    if ($script:Build -eq 'generic') {
        $generic = Join-Path $script:Src '.cargo\x86-64-generic.toml'
        if ((Test-Path $generic) -and (Select-String -Path $generic -Pattern $section -Quiet)) {
            $cargoArgs += @('--config', '.cargo/x86-64-generic.toml')
            $files += '.cargo\x86-64-generic.toml'
        } else {
            Say "Cadence $script:Version has no generic settings for $script:Triple, so it is built for that system's baseline."
            $script:Made = 'baseline'
        }
    } elseif ($script:Build -eq 'avx2') {
        if (-not (Select-String -Path (Join-Path $script:Src '.cargo\config.toml') -Pattern $section -Quiet)) {
            Say "Cadence $script:Version has no AVX2 settings for $script:Triple, so it is built for that system's baseline."
            $script:Made = 'baseline'
        }
    }
    # The repository's own flags, in the order Cargo would join them, given as CARGO_ENCODED_RUSTFLAGS,
    # which takes the place of every other source: RUSTFLAGS, a target's variable and any config file.
    $flags = @()
    foreach ($file in $files) {
        $read = Get-RepoFlags (Join-Path $script:Src $file)
        # A rustflags key this script cannot read would build by flags it never saw, so nothing is built.
        if ($read.Unreadable) { Fail "$file sets rustflags for $($script:Triple) in a form this script cannot read, so nothing is built" }
        $flags += $read.Flags
    }
    $encoded = $flags -join [char]0x1f
    Write-Log ('rustflags: ' + ($flags -join ' '))
    $names = @('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', ('CARGO_TARGET_' + $script:Triple.ToUpper().Replace('-', '_') + '_RUSTFLAGS'))
    $saved = @{}
    foreach ($name in $names) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        [Environment]::SetEnvironmentVariable($name, $null, 'Process')
    }
    Set-ProcessVariable 'CARGO_ENCODED_RUSTFLAGS' $encoded
    Push-Location $script:Src
    try {
        $code = Invoke-Logged 'cargo' $cargoArgs
    } finally {
        Pop-Location
        foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
    }
    if ($code -ne 0) { Fail 'the build failed' }
    $script:Built = Join-Path $script:Src 'target\release\cadence.exe'
    if (-not (Test-Path $script:Built)) { Fail "the build left no binary at $script:Built" }
    $script:From = 'source'
}

# Runs a binary's bench and asks its name, leaving the reason in Why if either is not the one wanted.
function Test-BenchAndName([string]$Binary, [string]$Want, [string]$Name) {
    Say 'Running its bench.'
    Write-Log '$ cadence bench'
    $run = Invoke-Limited $Binary 'bench' '' $BenchSeconds
    if ($run.Failed) {
        $script:Why = "it cannot start on this system, $($run.Failed)"
        return $false
    }
    if ($script:Limited) {
        $script:Why = "its bench did not finish within $BenchSeconds seconds, so it was stopped"
        return $false
    }
    $code = $run.Code
    $lines = @($run.Out -split "`r?`n" | Where-Object { $_.Trim() })
    $got = ''
    if ($lines.Count -gt 0) { $got = ("$($lines[-1])".Trim() -split '\s+')[0] }
    Write-Log "bench: exit $code, $got; bench.txt: $Want"
    if ($code -ne 0) {
        # STATUS_ILLEGAL_INSTRUCTION.
        $what = if ($code -eq -1073741795) { ' (an illegal instruction, one this processor lacks)' } else { '' }
        $script:Why = "it stopped in its bench with exit $code$what"
        return $false
    }
    if (-not $got -or $got -ne $Want) {
        $script:Why = "its bench counts $(if ($got) { $got } else { 'nothing' }) and bench.txt says $Want"
        return $false
    }
    $script:Id = Get-IdName $Binary
    Write-Log "id name: $script:Id"
    if ($script:Limited) {
        $script:Why = "it did not answer uci within $UciSeconds seconds, so it was stopped"
        return $false
    }
    if ($script:Id -ne $Name) {
        $script:Why = "it calls itself '$script:Id' and not '$Name'"
        return $false
    }
    Say "The bench counts $got nodes, as bench.txt says, and the engine calls itself $script:Id."
    return $true
}

# A release calls itself by its version; a test base's build is the version's -dev- at its commit.
function Test-Build {
    Step 'Checking the build'
    $name = "Cadence $script:Version"
    if ($script:Version.Contains('+')) {
        $commit = "$(git -C $script:Src rev-parse HEAD)".Trim()
        $name = 'Cadence ' + $script:Version.Split('+')[0] + '-dev-' + $commit.Substring(0, 7)
    }
    $want = (Get-Content -Raw -Path (Join-Path $script:Src 'bench.txt')).Trim()
    if (-not (Test-BenchAndName $script:Built $want $name)) { Fail "$script:Why, so it is not installed" }
}

# --- The install -------------------------------------------------------------------------------

# Where the binary goes, in Dest and Target: --dir's folder, the folder the state records the binary
# in while it is still there, or the default.
function Get-TargetPath {
    $previous = Get-StatePath 'binary'
    $script:Where = 'default'
    $script:Dest = $DefaultPrefix
    if ($script:Prefix) {
        $script:Where = 'dir'
        $script:Dest = $script:Prefix
    } elseif ($previous -and (Test-Path -LiteralPath $previous)) {
        $script:Where = 'before'
        $script:Dest = Split-Path -Parent $previous
    }
    $script:Target = Join-Path $script:Dest 'cadence.exe'
}

# The version this script recorded installing at Target, in Installed, and its build and whether it
# came from a release binary or source in HeldBuild and HeldFrom, trusted only while the file there
# has the SHA-256 recorded with it. The file itself is never run.
function Get-InstalledVersion {
    $script:Installed = ''
    $script:HeldBuild = ''
    $script:HeldFrom = ''
    $script:Earlier = $false
    if (-not (Test-Path -LiteralPath $script:Target -PathType Leaf)) { return }
    if (-not (Test-SameFile (Get-StatePath 'binary') $script:Target)) { return }
    $sum = Get-StateValue 'sha256'
    if (-not $sum) {
        $script:Earlier = $true
        return
    }
    if ((Get-Sha256 $script:Target) -ne $sum) { return }
    $script:Installed = Get-StateValue 'version'
    $script:HeldBuild = Get-StateValue 'build'
    $script:HeldFrom = Get-StateValue 'from'
}

# The published release binary with this SHA-256 in Released, by the digests GitHub's API lists for
# every release's files; where that list cannot be read it returns false, with the reason in Unlisted.
function Find-ReleasedAs([string]$Sum) {
    $script:Released = ''
    $script:Unlisted = ''
    $headers = @{}
    if ($env:GITHUB_TOKEN) { $headers['Authorization'] = "Bearer $env:GITHUB_TOKEN" }
    Write-Log "downloading ${ApiUrl}?per_page=100"
    try {
        $releases = Invoke-RestMethod -UseBasicParsing -Uri "${ApiUrl}?per_page=100" -Headers $headers -ErrorAction Stop
    } catch {
        $script:Unlisted = Get-DownloadTrouble $_
        return $false
    }
    foreach ($release in @($releases)) {
        foreach ($asset in @($release.assets)) {
            if ($asset -and $asset.PSObject.Properties['digest'] -and "$($asset.digest)" -eq "sha256:$Sum") {
                $script:Released = "$($asset.name)"
                return $true
            }
        }
    }
    return $true
}

# The installers before this one recorded the path alone, so their install is known by its SHA-256
# among the release's binaries, and recorded as this script records it; otherwise it may be replaced.
function Find-EarlierInstall {
    Step 'The installed Cadence'
    $sum = Get-Sha256 $script:Target
    $name = ''
    $sums = $false
    if (-not $script:Version.Contains('+') -and (Get-ReleaseSums)) {
        $sums = $true
        $name = Get-ListedAs $sum
    }
    if ($name.StartsWith("cadence-$($script:Version)-")) {
        Say "An earlier installer put Cadence at $($script:Target) without recording its version; its SHA-256 is release $($script:Version)'s $name."
        $script:Installed = $script:Version
        $script:HeldBuild = if ($name -match '-generic(\.exe)?$') { 'generic' } elseif ($name -match '-avx2(\.exe)?$') { 'avx2' } else { 'native' }
        $script:HeldFrom = 'release'
        Set-StateValue 'version' $script:Installed
        Set-StateValue 'sha256' $sum
        Set-StateValue 'build' $script:HeldBuild
        Set-StateValue 'from' 'release'
        return
    }
    if ($sums) {
        Say "An earlier installer put Cadence at $($script:Target) without recording its version, and its SHA-256 is none of release $($script:Version)'s."
    } else {
        Say "An earlier installer put Cadence at $($script:Target) without recording its version."
    }
    # Its version unknown, it is replaced by default only with the newest release, and only where it
    # is some other release's binary rather than a file put there by hand.
    $default = $false
    if (-not $script:AskedVersion) {
        if (-not (Find-ReleasedAs $sum)) {
            Say "The list of published releases could not be read$($script:Unlisted), so whether it is a release's binary is not known."
        } elseif ($script:Released) {
            Say "Its SHA-256 is that of $($script:Released), another release's binary."
            $default = $true
        } else {
            Say "Its SHA-256 is that of no published release's binary."
        }
    }
    # --yes with --version installs that version over whatever is there.
    if ($script:Yes -and $script:AskedVersion) {
        Say "Replacing it with Cadence $($script:Version), as --yes and --version $($script:Version) ask."
    } elseif (-not (Ask "Replace it with Cadence $($script:Version)?" $default)) {
        Stop-Install 'Kept the one there; nothing was installed.'
    }
    $script:Replace = $true
}

# The same version installed stops the run as up to date, unless another build or a build from source
# is asked for; an older one is upgraded unless declined, and a newer one is kept unless a downgrade
# is asked for.
function Compare-Installed {
    Get-TargetPath
    Get-InstalledVersion
    if ($script:Earlier) { Find-EarlierInstall }
    if (-not $script:Installed) { return }
    if (-not $script:Earlier) { Step 'The installed Cadence' }
    $order = Compare-Version $script:Installed $script:Version
    if ($order -eq 0) {
        # A baseline build was made where the target has no AVX2 or generic settings, so every build
        # asked for there is that one.
        if ($Arch -eq 'x86_64' -and $script:Asked -and $script:HeldBuild -ne $script:Build -and $script:HeldBuild -ne 'baseline') {
            $held = if ($script:HeldBuild) { " in the $($script:HeldBuild) build" } else { '' }
            Say "Cadence $($script:Version) is installed at $($script:Target)$held, and the $($script:Build) build is asked for, so it is replaced."
            $script:Replace = $true
        } elseif ($script:Source -and $script:HeldFrom -ne 'source') {
            Say "Cadence $($script:Version) is installed at $($script:Target), and --source asks for it built from source, so it is replaced."
            $script:Replace = $true
        } else {
            Say "Cadence $($script:Version) is already installed at $($script:Target), so it is up to date."
            $script:UpToDate = $true
        }
    } elseif ($order -lt 0) {
        Say "Cadence $($script:Installed) is installed at $($script:Target), and $($script:Version) is available."
        if (-not (Ask "Upgrade it to $($script:Version)?" $true)) { Stop-Install "Kept Cadence $($script:Installed); nothing was installed." }
        $script:Replace = $true
    } else {
        Say "Cadence $($script:Installed) is installed at $($script:Target). It is newer than $($script:Version), so installing $($script:Version) would be a downgrade."
        # --yes takes a downgrade only where --version names the release.
        if ($script:Yes -and $script:AskedVersion) {
            Say "Downgrading it to $($script:Version), as --yes and --version $($script:Version) ask."
        } elseif (-not (Ask "Downgrade it to $($script:Version)?" $false)) {
            Stop-Install "Kept Cadence $($script:Installed); nothing was installed."
        }
        $script:Replace = $true
    }
}

function Install-Binary {
    Step 'Installing'
    switch ($script:Where) {
        'dir' { Say "Installing into $($script:Dest), as --dir asks." }
        'before' { Say "Installing into $($script:Dest), where Cadence was installed before." }
        default { Say "Installing into $($script:Dest), the default folder; --dir DIR installs into DIR instead." }
    }
    New-Item -ItemType Directory -Force -Path $script:Dest | Out-Null
    # A file this script has no record of is never run to ask its name, since it may not be Cadence.
    if ((Test-Path -LiteralPath $script:Target) -and -not $script:Replace) {
        Say "There is already a file at $($script:Target), whose version this script has no record of."
        if (-not (Ask 'Replace it?' ($script:Mode -eq 'update'))) { Stop-Install 'Kept the one there; nothing was installed.' }
    }
    try {
        Copy-Item -Force -Path $script:Built -Destination $script:Target -ErrorAction Stop
    } catch {
        Fail "could not write $($script:Target); if Cadence is running, close it and run this again"
    }
    $previous = Get-StatePath 'binary'
    if ($previous -and (Test-Path -LiteralPath $previous) -and -not (Test-SameFile $previous $script:Target)) { Remove-Previous $previous }
    # Whatever the steps above did, the install ends with the checked binary at the target.
    $want = Get-Sha256 $script:Built
    if (-not (Test-Path -LiteralPath $script:Target -PathType Leaf) -or (Get-Sha256 $script:Target) -ne $want) {
        Say "The binary at $($script:Target) was missing or changed after the install, so it is copied again."
        try {
            Copy-Item -Force -Path $script:Built -Destination $script:Target -ErrorAction Stop
        } catch {
            Fail "could not write $($script:Target)"
        }
        if ((Get-Sha256 $script:Target) -ne $want) { Fail "the binary at $($script:Target) does not match the one checked" }
    }
    Set-StateValue 'binary' $script:Target
    Set-StateValue 'version' $script:Version
    Set-StateValue 'sha256' (Get-Sha256 $script:Target)
    Set-StateValue 'build' $(if ($script:Made) { $script:Made } else { $script:Build })
    Set-StateValue 'from' $script:From
    Say "Installed $script:Target."
}

# The user PATH's entries as the registry holds them, with any %VARIABLE% unexpanded.
function Get-UserPath {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')
    if (-not $key) { return @() }
    try {
        return @([string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames') -split ';' | Where-Object { $_ })
    } finally {
        $key.Close()
    }
}

# Writes the user PATH back as the kind of value it was, so a %VARIABLE% in it stays one, or removes it
# once empty. Clearing a variable never set then tells running programs the environment changed.
function Set-UserPath([string[]]$Entries) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
    try {
        $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
        if ($key.GetValueNames() -contains 'Path') { $kind = $key.GetValueKind('Path') }
        $kept = @($Entries | Where-Object { $_ })
        if ($kept.Count -gt 0) { $key.SetValue('Path', ($kept -join ';'), $kind) } else { $key.DeleteValue('Path', $false) }
    } finally {
        $key.Close()
    }
    [Environment]::SetEnvironmentVariable('CADENCE_INSTALLER_UNSET', $null, 'User')
}

# Removes a folder this script added to the user PATH, asking first unless Now is given.
function Remove-FromPath([string]$Added, [switch]$Now) {
    $entries = @(Get-UserPath)
    if ($entries -notcontains $Added) {
        Say "$Added is not on your user PATH, so there is nothing to remove there."
    } elseif ($Now -or (Ask "Remove $Added from your PATH, which this script added?" $true)) {
        Set-UserPath @($entries | Where-Object { $_ -ne $Added })
        Say "Removed $Added from your PATH."
    }
}

# A copy an earlier run installed in another folder, offered for removal with the PATH entry this
# script added for it, so the state names one install and the uninstall leaves nothing behind.
function Remove-Previous([string]$Previous) {
    if (Test-SameFile $Previous $script:Target) { return }
    $folder = Split-Path -Parent $Previous
    $sum = Get-StateValue 'sha256'
    $default = $true
    Say "An earlier run installed Cadence at $Previous."
    # A copy that is no longer the file recorded there may be the user's own, so it is kept by default.
    if (-not $sum) {
        Say 'No SHA-256 was recorded for it, so it may not be the file installed there.'
        $default = $false
    } elseif ((Get-Sha256 $Previous) -ne $sum) {
        Say 'It has changed since it was installed: its SHA-256 is not the one recorded.'
        $default = $false
    }
    if (-not (Ask 'Remove that copy, and the PATH entry this script added for it?' $default)) {
        Say 'Kept it. This script now records only the new install, so its uninstall will not remove that copy.'
        return
    }
    try {
        Remove-Item -Force -LiteralPath $Previous -ErrorAction Stop
    } catch {
        Fail "could not remove $Previous; if Cadence is running, close it and run this again"
    }
    Say "Removed $Previous."
    $added = Get-StateValue 'path_windows'
    if ($added -and (Test-SameFile $added $folder)) {
        Remove-FromPath $added -Now
        Remove-StateValue 'path_windows'
    }
}

# A folder other than the default is the caller's to manage, so the offer defaults to no there.
function Add-ToPath {
    $dest = Split-Path -Parent $script:Target
    if (@($env:Path -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -eq $dest.TrimEnd('\') }).Count -gt 0) {
        Say "$dest is already on your PATH."
        return
    }
    # The user PATH a new terminal reads, which this one does not see until it is opened again.
    $entries = @(Get-UserPath)
    if (@($entries | Where-Object { [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') -eq $dest.TrimEnd('\') }).Count -gt 0) {
        Say "$dest is already on your user PATH, for each new terminal."
        return
    }
    if (-not (Ask "Add $dest to your PATH, so a terminal finds cadence?" (Test-SameFile $dest $DefaultPrefix))) { return }
    Set-UserPath ($entries + $dest)
    Set-StateValue 'path_windows' $dest
    # Each command on a line of its own, so it can be copied whole; a path with a space or a quote runs
    # through &.
    Say 'Added. This window will not find cadence until you open a new one. To run it here now:'
    Say $(if ($script:Target -match "[ ']") { "& '" + $script:Target.Replace("'", "''") + "'" } else { $script:Target })
    Say "or put it on this window's PATH with:"
    Say ("`$env:Path = '" + $dest.Replace("'", "''") + ";' + `$env:Path")
    Say 'If a new Windows Terminal tab still does not find cadence, close all of its windows and start it again.'
}

# How many tables a download left in the folder the state records; all 290 is a finished one.
function Get-TablesHeld {
    $dir = Get-StatePath 'tables'
    if (-not $dir -or -not (Test-Path -LiteralPath $dir -PathType Container)) { return 0 }
    return @(Get-ChildItem -LiteralPath $dir -File | Where-Object { $_.Name -cmatch '^K[QRBNP]*vK[QRBNP]*\.rtb[wz]$' }).Count
}

# A download the state records that stopped part way: some tables, or a table half fetched, in its
# folder. A folder emptied or removed since is not offered again.
function Test-TablesUnfinished {
    if (-not (Get-StateValue 'tables')) { return $false }
    $held = Get-TablesHeld
    if ($held -ge 290) { return $false }
    if ($held -gt 0) { return $true }
    $dir = Get-StatePath 'tables'
    return [bool]((Test-Path -LiteralPath $dir -PathType Container) -and @(Get-ChildItem -LiteralPath $dir -File -Filter '*.part').Count -gt 0)
}

function Get-Tables {
    Step 'Syzygy tablebases, if you want them'
    Say 'Cadence can read the 3-4-5 piece Syzygy tablebases, 290 files and about 1 GB, to play every'
    Say "ending of five pieces or fewer perfectly. They are downloaded from $TablesUrl."
    $default = $false
    if (Test-TablesUnfinished) {
        Say "An earlier download stopped with $(Get-TablesHeld) of the 290 tables in $(Get-StatePath 'tables')."
        $default = $true
    }
    if ($script:Tables) {
        Say 'Downloading them, as --tables asks.'
    } elseif (-not (Ask 'Download them now?' $default)) {
        Say 'Skipped.'
        return
    }
    $dir = Join-Path $DataDir 'syzygy\3-4-5'
    # Recorded before anything is fetched, so a download that stops is offered again.
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Set-StateValue 'tables' $dir
    $sums = Join-Path $Work 'sha256'
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$TablesUrl/sha256" -OutFile $sums -ErrorAction Stop
    } catch {
        Fail "could not download the tables' published checksums$(Get-DownloadTrouble $_)"
    }
    # Every name becomes a path below, so each must be a table's name and each sum a SHA-256.
    $wanted = @(Get-Content -Path $sums | ForEach-Object {
        $parts = @("$_".Trim() -split '\s+')
        if ($parts.Count -ge 2) {
            $pieces = ($parts[1] -replace '\.rtb[wz]$', '') -replace 'v', ''
            if ($pieces.Length -le 5) { [pscustomobject]@{ Sum = $parts[0].ToLower(); File = $parts[1]; Fields = $parts.Count } }
        }
    })
    if ($wanted.Count -ne 290) { Fail "the published checksums list $($wanted.Count) tables of five pieces or fewer, not 290" }
    foreach ($table in $wanted) {
        if ($table.Fields -ne 2 -or $table.Sum -cnotmatch '^[0-9a-f]{64}$' -or $table.File -cnotmatch '^K[QRBNP]*vK[QRBNP]*\.rtb[wz]$') {
            Fail "the published checksums name '$($table.File)', which is not a table's file name, so nothing was downloaded"
        }
    }
    $checked = 0
    $fetched = 0
    $bytes = 0
    foreach ($table in $wanted) {
        $group = if ($table.File.EndsWith('.rtbw')) { '3-4-5-wdl' } else { '3-4-5-dtz' }
        $path = Join-Path $dir $table.File
        # A table already here and matching its checksum is kept as it is.
        $have = (Test-Path -LiteralPath $path) -and ((Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLower() -eq $table.Sum)
        if (-not $have) {
            $fetched++
            $part = "$path.part"
            try {
                Invoke-WebRequest -UseBasicParsing -Uri "$TablesUrl/$group/$($table.File)" -OutFile $part -ErrorAction Stop
            } catch {
                Fail "could not download $($table.File)$(Get-DownloadTrouble $_)"
            }
            if ((Get-FileHash -Algorithm SHA256 -Path $part).Hash.ToLower() -ne $table.Sum) {
                Remove-Item -Force $part
                Fail "$($table.File) does not match its published checksum"
            }
            Move-Item -Force -Path $part -Destination $path
        }
        $checked++
        $bytes += (Get-Item -LiteralPath $path).Length
        if ($checked % 29 -eq 0) {
            Say ("  $checked of 290 downloaded and checked, {0} of {1} MB" -f [math]::Floor($bytes / 1MB), [math]::Floor($TablesBytes / 1MB))
        }
    }
    Say "All 290 tables are in $dir, each matching its published checksum: $fetched downloaded now, $(290 - $fetched) already there."
    Say 'To use them, set the engine option SyzygyPath in your chess program to this folder:'
    Say $dir
}

function Show-Done {
    Step 'Done'
    Say "Cadence is at $script:Target and calls itself $script:Id."
    if ($script:From -eq 'source') {
        Say "It was built here from source at the tag $($script:Version), so its bench is the check it passed."
    } elseif ($script:Attested -eq 'no') {
        Say 'Its build attestation was not verified, so its SHA-256 and its bench are the checks it passed.'
    }
    Say ''
    Say 'To play it in a chess program, add it as a UCI engine and choose that file:'
    Say '  Arena: Engines, Install New Engine, choose the file, then UCI when asked.'
    Say '  Cute Chess: Tools, Settings, Engines, the + button, the file as the command, UCI as the protocol.'
    Say '  Banksia: add an engine in its engine manager and choose the file.'
    if (Test-SameFile (Split-Path -Parent $script:Target) $DefaultPrefix) {
        Say 'AppData is hidden, so paste the path above into the GUI''s file dialog, or type %LOCALAPPDATA%\Cadence into its address bar.'
    }
    Say ''
    Say "Report bugs as GitHub issues: $IssuesUrl"
}

function Uninstall-Cadence {
    Step 'Uninstalling Cadence'
    $bin = Get-StatePath 'binary'
    if (-not $bin) {
        $folder = if ($script:Prefix) { $script:Prefix } else { $DefaultPrefix }
        $bin = Join-Path $folder 'cadence.exe'
    }
    if (Test-Path $bin) {
        if (Ask "Remove $($bin)?" $true) {
            try {
                Remove-Item -Force -Path $bin -ErrorAction Stop
            } catch {
                Fail "could not remove $($bin); if Cadence is running, close it and run this again"
            }
            Say 'Removed it.'
        }
    } else {
        Say "There is no Cadence at $bin."
    }
    $added = Get-StateValue 'path_windows'
    if ($added) { Remove-FromPath $added }
    $tables = Get-StatePath 'tables'
    if ($tables -and (Test-Path -LiteralPath $tables) -and (Ask "Remove the tablebases at $tables, about 1 GB?" $true)) {
        Remove-Item -Recurse -Force -LiteralPath $tables -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $tables) { Fail "could not remove $tables; if a chess program has them open, close it and run this again" }
        Say 'Removed them.'
    }
    if (Test-Path $State) { Remove-Item -Force -Path $State }
    Say 'Done.'
}

# --- The run -----------------------------------------------------------------------------------

$code = 0
try {
    Write-SystemReport
    Test-Urls
    if ($script:Mode -eq 'uninstall') {
        Uninstall-Cadence
    } else {
        Step "Cadence's installer"
        Say "This installs Cadence's release binary for this system once its checksum and bench match the"
        Say "release's, or builds it from source where no binary fits. It asks before building, installing a"
        Say 'compiler or Rust, replacing an install, changing your PATH or downloading tablebases.'
        Say "Release files come from $AssetsUrl."
        Test-Version
        New-Item -ItemType Directory -Force -Path $Work | Out-Null
        Confirm-System
        if (-not $script:Source -or -not $script:Version) { Find-Release }
        Compare-Installed
        if ($script:UpToDate) {
            # Up to date, the run stops, and only tablebases asked for by name or a download that stopped
            # are still fetched.
            if ($script:Tables -or (Test-TablesUnfinished)) { Get-Tables }
        } else {
            if (-not $script:Source) { Test-Binary }
            if (-not $script:Built) {
                Request-Source
                Test-Git
                Resolve-Tag
                Get-ReleaseCommit
                Get-Source
                Test-Commit
                Test-Linker
                Test-Rust
                Initialize-Toolchain
                Invoke-Build
                Test-Build
            }
            Install-Binary
            Add-ToPath
            Get-Tables
            Show-Done
        }
    }
} catch {
    if ("$_" -ne 'cadence-install-stopped') {
        Say "Error: $_"
        $script:Failed = $true
    }
    $code = 1
} finally {
    if (Test-Path $Work) { Remove-Item -Recurse -Force -Path $Work -ErrorAction SilentlyContinue }
    if ($script:Failed) {
        Write-Host ''
        Write-Host "The log is at $Log."
        Write-Host "Attach this file to a GitHub issue: $IssuesUrl"
    } elseif ($script:KeepLog) {
        Write-Host "The log is at $Log."
    } else {
        Remove-Item -Force -Path $Log -ErrorAction SilentlyContinue
    }
}
exit $code
