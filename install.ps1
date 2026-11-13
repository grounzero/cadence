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
$AssetsUrl = "https://github.com/$Repo/releases/download"
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
$script:Prefix = ''
$script:Build = ''
$script:Source = $false
$script:Tables = $false
$script:TestUrls = $false
$script:Stopped = $false
$script:Failed = $false
$script:Why = ''
$script:Built = ''
$script:Id = ''
$script:Attested = ''
$script:Limited = $false
$script:UpToDate = $false
$script:Replace = $false
$script:Installed = ''
$script:Named = ''
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
  --update         install the default release, or --version's, over an installed Cadence
  --uninstall      remove Cadence, and the PATH entry and tablebases if this script added them
  --tables         download the 3-4-5 piece tablebases without asking
  --yes            take every default without asking
  --keep-log       keep the log even when everything succeeds
  --test-urls      for tests: let CADENCE_RELEASES_URL, CADENCE_API_URL and CADENCE_TABLES_URL
                   name servers of their own; without it, CADENCE_RELEASES_URL must be HTTPS on
                   github.com and the other two are refused
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

function Get-StateValue([string]$Key) {
    if (-not (Test-Path $State)) { return '' }
    foreach ($line in Get-Content -Path $State) {
        if ($line.StartsWith("$Key=")) { return $line.Substring($Key.Length + 1) }
    }
    return ''
}

# A value already held is not written again, so a second run leaves the file as it was.
function Set-StateValue([string]$Key, [string]$Value) {
    if ((Test-Path $State) -and (Get-StateValue $Key) -ceq $Value) { return }
    $lines = @()
    if (Test-Path $State) { $lines = @(Get-Content -Path $State | Where-Object { -not $_.StartsWith("$Key=") }) }
    $lines += "$Key=$Value"
    Set-Content -Path $State -Value $lines -Encoding UTF8
}

# Runs a program in the work folder for at most Seconds, with Feed as its input, and returns its exit
# code and output. On expiry taskkill ends it and every process it started, and Limited is set.
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
    $process = [System.Diagnostics.Process]::Start($info)
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
    return @{ Code = $code; Out = $text }
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

# CADENCE_RELEASES_URL may name HTTPS on github.com, and the other two servers stay as they are,
# unless --test-urls lets a test name its own; whichever is in use is said.
function Test-Urls {
    if ($env:CADENCE_RELEASES_URL) {
        if (-not $env:CADENCE_RELEASES_URL.StartsWith('https://github.com/', [StringComparison]::Ordinal) -and -not $script:TestUrls) {
            Fail "CADENCE_RELEASES_URL is $env:CADENCE_RELEASES_URL, which is not HTTPS on github.com; only a test may name another server, with --test-urls"
        }
        $script:AssetsUrl = $env:CADENCE_RELEASES_URL
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
        Test-RateLimit (Get-DownloadTrouble $_)
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
            Test-RateLimit $trouble
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
    if ($script:Version -ne $InstallerVersion) {
        Say "This installer comes with Cadence $InstallerVersion; --version $InstallerVersion installs that one instead."
    }
}

# GitHub refusing the lookup for its hourly limit is said as such, never read as there being no release.
function Test-RateLimit([string]$Trouble) {
    if ($Trouble -eq ' (the server answered 403)' -or $Trouble -eq ' (the server answered 429)') {
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

# The SHA-256 the release's SHA256SUMS gives a file, or the empty string.
function Get-Listed([string]$Name) {
    foreach ($line in Get-Content -Path (Join-Path $Work 'SHA256SUMS')) {
        $parts = @("$line".Trim() -split '\s+')
        if ($parts.Count -ge 2 -and ($parts[1] -eq $Name -or $parts[1] -eq "*$Name")) { return $parts[0].ToLower() }
    }
    return ''
}

function Get-Sha256([string]$Path) { return (Get-FileHash -Algorithm SHA256 -Path $Path).Hash.ToLower() }

# The release workflow attests every binary it builds, and gh checks that attestation where it is
# installed and signed in. Where it cannot, that is said, and an attestation that fails leaves Why.
function Test-Attestation([string]$File) {
    $script:Attested = 'no'
    if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
        Say "Its build attestation was not verified: gh, GitHub's command line tool, is not installed."
        return $true
    }
    $ghArgs = @('attestation', 'verify', (Join-Path $Work $File), '--repo', $Repo,
        '--signer-workflow', "$Repo/.github/workflows/release.yml", '--source-ref', "refs/tags/$($script:Version)")
    Write-Log ('$ gh ' + ($ghArgs -join ' '))
    $said = @(& gh @ghArgs 2>&1 | ForEach-Object { "$_" })
    $code = $LASTEXITCODE
    foreach ($line in $said) { Write-Log $line }
    Write-Log "(exit $code)"
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
    $last = @($said | Where-Object { "$_".Trim() }) | Select-Object -Last 1
    $script:Why = "$File's build attestation does not verify ($("$last".Trim())), so it is not used."
    return $false
}

# Leaves the checked binary in Built, or the reason none is used in Why.
function Test-Binary {
    if ($Arch -ne 'x86_64') {
        $script:Why = "Cadence's releases have no binary for Windows on arm64."
        return
    }
    $file = "cadence-$($script:Version)-windows-x86-64-$($script:Build).exe"
    $failed = Save-Asset 'SHA256SUMS'
    if ($failed) {
        $script:Why = "Release $script:Version publishes no checksum file$failed, so none of its binaries is used."
        return
    }
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
# one: a test base, or a version from before releases had pages.
function Get-ReleaseCommit {
    $script:Named = ''
    if ($script:Version.Contains('+')) { return }
    $headers = @{}
    if ($env:GITHUB_TOKEN) { $headers['Authorization'] = "Bearer $env:GITHUB_TOKEN" }
    Write-Log "downloading $ApiUrl/tags/$($script:Version)"
    try {
        $release = Invoke-RestMethod -UseBasicParsing -Uri "$ApiUrl/tags/$($script:Version)" -Headers $headers -ErrorAction Stop
    } catch {
        $trouble = Get-DownloadTrouble $_
        Test-RateLimit $trouble
        if ($trouble -ne ' (not found)') { Fail "could not read release $($script:Version) from GitHub$trouble" }
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

function Invoke-Build {
    Step "Building Cadence $script:Version"
    $cargoArgs = @('build', '--release', '--locked', '-p', 'cadence-engine', '--bin', 'cadence')
    $section = '^\[target\.' + [regex]::Escape($script:Triple) + '\]'
    if ($script:Build -eq 'generic') {
        $generic = Join-Path $script:Src '.cargo\x86-64-generic.toml'
        if ((Test-Path $generic) -and (Select-String -Path $generic -Pattern $section -Quiet)) {
            $cargoArgs += @('--config', '.cargo/x86-64-generic.toml')
        } else {
            Say "Cadence $script:Version has no generic settings for $script:Triple, so it is built for that system's baseline."
        }
    } elseif ($script:Build -eq 'avx2') {
        if (-not (Select-String -Path (Join-Path $script:Src '.cargo\config.toml') -Pattern $section -Quiet)) {
            Say "Cadence $script:Version has no AVX2 settings for $script:Triple, so it is built for that system's baseline."
        }
    }
    # RUSTFLAGS would replace the repository's own per-target flags, so it is left out of the build.
    $saved = @($env:RUSTFLAGS, $env:CARGO_ENCODED_RUSTFLAGS)
    Remove-Item Env:RUSTFLAGS, Env:CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue
    Push-Location $script:Src
    try {
        $code = Invoke-Logged 'cargo' $cargoArgs
    } finally {
        Pop-Location
        if ($null -ne $saved[0]) { $env:RUSTFLAGS = $saved[0] }
        if ($null -ne $saved[1]) { $env:CARGO_ENCODED_RUSTFLAGS = $saved[1] }
    }
    if ($code -ne 0) { Fail 'the build failed' }
    $script:Built = Join-Path $script:Src 'target\release\cadence.exe'
    if (-not (Test-Path $script:Built)) { Fail "the build left no binary at $script:Built" }
}

# Runs a binary's bench and asks its name, leaving the reason in Why if either is not the one wanted.
function Test-BenchAndName([string]$Binary, [string]$Want, [string]$Name) {
    Say 'Running its bench.'
    Write-Log '$ cadence bench'
    $run = Invoke-Limited $Binary 'bench' '' $BenchSeconds
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

# Where the binary goes, in Dest and Target: --dir's folder, the earlier install's under --update, or
# the default.
function Get-TargetPath {
    $previous = Get-StateValue 'binary'
    $script:Where = 'default'
    $script:Dest = $DefaultPrefix
    if ($script:Prefix) {
        $script:Where = 'dir'
        $script:Dest = $script:Prefix
    } elseif ($script:Mode -eq 'update' -and $previous) {
        $script:Where = 'before'
        $script:Dest = Split-Path -Parent $previous
    }
    $script:Target = Join-Path $script:Dest 'cadence.exe'
}

# The version this script recorded installing at Target, in Installed, trusted only while the file
# there has the SHA-256 recorded with it. The file itself is never run.
function Get-InstalledVersion {
    $script:Installed = ''
    if (-not (Test-Path -LiteralPath $script:Target -PathType Leaf)) { return }
    if ((Get-StateValue 'binary') -ne $script:Target) { return }
    $sum = Get-StateValue 'sha256'
    if (-not $sum -or (Get-Sha256 $script:Target) -ne $sum) { return }
    $script:Installed = Get-StateValue 'version'
}

# The same version installed stops the run as up to date, an older one is upgraded unless declined,
# and a newer one is kept unless a downgrade is asked for.
function Compare-Installed {
    Get-TargetPath
    Get-InstalledVersion
    if (-not $script:Installed) { return }
    Step 'The installed Cadence'
    $order = Compare-Version $script:Installed $script:Version
    if ($order -eq 0) {
        Say "Cadence $($script:Version) is already installed at $($script:Target), so it is up to date."
        $script:UpToDate = $true
    } elseif ($order -lt 0) {
        Say "Cadence $($script:Installed) is installed at $($script:Target), and $($script:Version) is available."
        if (-not (Ask "Upgrade it to $($script:Version)?" $true)) { Stop-Install "Kept Cadence $($script:Installed); nothing was installed." }
        $script:Replace = $true
    } else {
        Say "Cadence $($script:Installed) is installed at $($script:Target). It is newer than $($script:Version), so installing $($script:Version) would be a downgrade."
        if (-not (Ask "Downgrade it to $($script:Version)?" $false)) { Stop-Install "Kept Cadence $($script:Installed); nothing was installed." }
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
    Set-StateValue 'binary' $script:Target
    Set-StateValue 'version' $script:Version
    Set-StateValue 'sha256' (Get-Sha256 $script:Target)
    Say "Installed $script:Target."
}

# A folder named with --dir is the caller's to manage, so the offer defaults to no there.
function Add-ToPath {
    $dest = Split-Path -Parent $script:Target
    $user = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $user) { $user = '' }
    $entries = @($user -split ';' | Where-Object { $_ })
    if ($entries -contains $dest) {
        Say "$dest is already on your PATH."
        return
    }
    if (-not (Ask "Add $dest to your PATH, so a terminal finds cadence?" (-not $script:Prefix))) { return }
    [Environment]::SetEnvironmentVariable('Path', (($entries + $dest) -join ';'), 'User')
    Set-StateValue 'path_windows' $dest
    # Each command on a line of its own, so it can be copied whole; a path with a space or a quote runs
    # through &.
    Say 'Added. This window will not find cadence until you open a new one. To run it here now:'
    Say $(if ($script:Target -match "[ ']") { "& '" + $script:Target.Replace("'", "''") + "'" } else { $script:Target })
    Say "or put it on this window's PATH with:"
    Say ("`$env:Path = '" + $dest.Replace("'", "''") + ";' + `$env:Path")
    Say 'If a new Windows Terminal tab still does not find cadence, close all of its windows and start it again.'
}

function Get-Tables {
    Step 'Syzygy tablebases, if you want them'
    Say 'Cadence can read the 3-4-5 piece Syzygy tablebases, 290 files and about 1 GB, to play every'
    Say "ending of five pieces or fewer perfectly. They are downloaded from $TablesUrl."
    if ($script:Tables) {
        Say 'Downloading them, as --tables asks.'
    } elseif (-not (Ask 'Download them now?' $false)) {
        Say 'Skipped.'
        return
    }
    $dir = Join-Path $DataDir 'syzygy\3-4-5'
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
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Set-StateValue 'tables' $dir
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
    if ($script:Attested -eq 'no') {
        Say 'Its build attestation was not verified, so its SHA-256 and its bench are the checks it passed.'
    }
    Say ''
    Say 'To play it in a chess program, add it as a UCI engine and choose that file:'
    Say '  Arena: Engines, Install New Engine, choose the file, then UCI when asked.'
    Say '  Cute Chess: Tools, Settings, Engines, the + button, the file as the command, UCI as the protocol.'
    Say '  Banksia: add an engine in its engine manager and choose the file.'
    Say ''
    Say "Report bugs as GitHub issues: $IssuesUrl"
}

function Uninstall-Cadence {
    Step 'Uninstalling Cadence'
    $bin = Get-StateValue 'binary'
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
    if ($added -and (Ask "Remove $added from your PATH, which this script added?" $true)) {
        $user = [Environment]::GetEnvironmentVariable('Path', 'User')
        if ($user) {
            $kept = @($user -split ';' | Where-Object { $_ -and $_ -ne $added })
            [Environment]::SetEnvironmentVariable('Path', ($kept -join ';'), 'User')
        }
        Say 'Removed it.'
    }
    $tables = Get-StateValue 'tables'
    if ($tables -and (Test-Path $tables) -and (Ask "Remove the tablebases at $tables, about 1 GB?" $true)) {
        Remove-Item -Recurse -Force -Path $tables
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
            # Up to date, the run stops, and only tablebases asked for by name are still fetched.
            if ($script:Tables) { Get-Tables }
        } else {
            if (-not $script:Source) { Test-Binary }
            if (-not $script:Built) {
                Request-Source
                Test-Git
                Resolve-Tag
                Get-ReleaseCommit
                Test-Linker
                Test-Rust
                Get-Source
                Test-Commit
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
