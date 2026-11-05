# SPDX-License-Identifier: GPL-3.0-or-later
#
# Builds Cadence from its source at a release tag, checks the build and installs it, on Windows with
# PowerShell alone; install.sh does the same in Git Bash, WSL, Linux and macOS. Every step asks first
# unless --yes is given.

Set-StrictMode -Version 2.0
# Native programs report through their exit codes, which are read one by one, so their error output
# never stops the script by itself.
$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'

$RepoUrl = 'https://github.com/grounzero/cadence.git'
$IssuesUrl = 'https://github.com/grounzero/cadence/issues'
$TablesUrl = 'https://tablebase.lichess.ovh/tables/standard'

$script:Yes = $false
$script:KeepLog = $false
$script:Mode = 'install'
$script:Version = ''
$script:Prefix = ''
$script:Build = ''
$script:Stopped = $false
$script:Failed = $false

function Show-Usage {
    @'
Usage: install.ps1 [options]

Builds Cadence from source, checks the build and installs it. Every step asks first.

  --version X.Y.Z  build this release rather than the latest
  --prefix DIR     install into DIR rather than the default folder
  --build TIER     on x86-64, make the avx2 or the generic build whatever the processor has
  --update         rebuild and replace an installed Cadence
  --uninstall      remove Cadence, and the PATH entry and tablebases if this script added them
  --yes            take every default without asking
  --keep-log       keep the log even when everything succeeds
  --help           show this and stop
'@
}

$i = 0
while ($i -lt $args.Count) {
    $arg = [string]$args[$i]
    switch -Regex ($arg) {
        '^-{1,2}(yes|y)$' { $script:Yes = $true }
        '^-{1,2}keep-?log$' { $script:KeepLog = $true }
        '^-{1,2}update$' { $script:Mode = 'update' }
        '^-{1,2}uninstall$' { $script:Mode = 'uninstall' }
        '^-{1,2}(version|prefix|build)$' {
            if ($i + 1 -ge $args.Count) { Write-Host "$arg needs a value"; exit 2 }
            $i++
            $value = [string]$args[$i]
            switch -Regex ($arg) {
                'version' { $script:Version = $value }
                'prefix' { $script:Prefix = $value }
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
if (-not $env:LOCALAPPDATA) { Write-Host 'LOCALAPPDATA is not set, so give --prefix.'; exit 1 }
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

function Set-StateValue([string]$Key, [string]$Value) {
    $lines = @()
    if (Test-Path $State) { $lines = @(Get-Content -Path $State | Where-Object { -not $_.StartsWith("$Key=") }) }
    $lines += "$Key=$Value"
    Set-Content -Path $State -Value $lines -Encoding UTF8
}

function Get-IdName([string]$Binary) {
    # A process of its own, written to as bytes: what Windows PowerShell pipes into a native program
    # reached the engine as no command it answered.
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $Binary
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardInput = $true
    $info.RedirectStandardOutput = $true
    $process = [System.Diagnostics.Process]::Start($info)
    $bytes = [System.Text.Encoding]::ASCII.GetBytes("uci`nquit`n")
    $process.StandardInput.BaseStream.Write($bytes, 0, $bytes.Length)
    $process.StandardInput.BaseStream.Close()
    $out = $process.StandardOutput.ReadToEnd()
    $process.WaitForExit()
    $lines = @($out -split "`r?`n" | Where-Object { $_ })
    Write-Log ('uci answered: ' + ($lines -join ' | '))
    foreach ($line in $lines) {
        if ($line -match '^id name (.*)$') { return $Matches[1].Trim() }
    }
    return ''
}

function Write-SystemReport {
    Write-Log ("installer: $script:Mode " + $script:Version)
    Write-Log ('system: ' + [Environment]::OSVersion.VersionString + ", $machine")
    try { Write-Log ('os: ' + (Get-CimInstance Win32_OperatingSystem -ErrorAction Stop).Caption) } catch { }
    try { Write-Log ('cpu: ' + (Get-CimInstance Win32_Processor -ErrorAction Stop | Select-Object -First 1).Name) } catch { }
    Write-Log ('powershell: ' + $PSVersionTable.PSVersion)
}

# --- The prerequisites -------------------------------------------------------------------------

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
    New-Item -ItemType Directory -Force -Path $Work | Out-Null
    $init = Join-Path $Work 'rustup-init.exe'
    Write-Log "downloading https://win.rustup.rs/$rustupArch"
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "https://win.rustup.rs/$rustupArch" -OutFile $init -ErrorAction Stop
    } catch {
        Fail "could not download rustup-init.exe: $($_.Exception.Message)"
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

# --- The release -------------------------------------------------------------------------------

function Resolve-Release {
    Step 'Finding the release'
    Write-Log "`$ git ls-remote --tags --refs $RepoUrl"
    $listing = git ls-remote --tags --refs $RepoUrl 2>&1
    if ($LASTEXITCODE -ne 0) {
        foreach ($line in $listing) { Write-Log "$line" }
        Fail "could not list the releases at $RepoUrl"
    }
    $tags = @($listing | ForEach-Object { "$_" -replace '^.*refs/tags/', '' })
    if (-not $script:Version) {
        $releases = @($tags | Where-Object { $_ -match '^\d+\.\d+\.\d+$' } | Sort-Object { [version]$_ })
        if ($releases.Count -eq 0) { Fail "there is no release at $RepoUrl" }
        $script:Version = $releases[-1]
        Say "The latest release is $script:Version."
    } else {
        if ($script:Version -notmatch '^\d+\.\d+\.\d+(-rc\.\d+)?$') {
            Fail "$script:Version is not a release's version, which reads x.y.z or x.y.z-rc.n"
        }
        if ($tags -notcontains $script:Version) { Fail "there is no release $script:Version at $RepoUrl" }
        Say "Building release $script:Version, as asked."
    }
}

function Test-Avx2 {
    try {
        if (-not ('CadenceInstall.Cpu' -as [type])) {
            Add-Type -Namespace CadenceInstall -Name Cpu -MemberDefinition @'
[DllImport("kernel32.dll")]
public static extern bool IsProcessorFeaturePresent(uint feature);
'@
        }
        # PF_AVX2_INSTRUCTIONS_AVAILABLE; a Windows that does not know it answers no.
        return [CadenceInstall.Cpu]::IsProcessorFeaturePresent(40)
    } catch {
        return $false
    }
}

function Select-Build {
    if ($Arch -ne 'x86_64') {
        $script:Build = 'native'
        Say 'This processor has one build, which will be made.'
        return
    }
    if ($script:Build) {
        Say "The $script:Build build will be made, as --build asks."
    } elseif (Test-Avx2) {
        Say 'This processor has AVX2, so the AVX2 build will be made.'
        if (Ask 'Make the AVX2 build? No makes the generic build, for any x86-64 with POPCNT.' $true) {
            $script:Build = 'avx2'
        } else {
            $script:Build = 'generic'
        }
    } else {
        Say 'This processor has no AVX2, so the generic build will be made.'
        if (Ask 'Make the generic build? No makes the AVX2 build, which this processor may not run.' $true) {
            $script:Build = 'generic'
        } else {
            $script:Build = 'avx2'
        }
    }
}

function Show-Cost {
    Step 'Before anything is downloaded'
    Say 'Building takes several minutes and about 2 GB of disk space, for the source, the Rust'
    Say 'version Cadence pins if it is not installed yet, and the build. Both are estimates.'
    New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
    try {
        $drive = Get-PSDrive -Name ($DataDir.Substring(0, 1)) -ErrorAction Stop
        $free = [math]::Floor($drive.Free / 1MB)
        Say "$DataDir has $free MB free."
        if ($free -lt 2048) { Say 'That may not be enough.' }
    } catch { }
    if (-not (Ask 'Go on?' $true)) { Stop-Install 'Nothing was installed.' }
}

# --- The build ---------------------------------------------------------------------------------

function Get-Source {
    Step "Fetching Cadence $script:Version"
    if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
    New-Item -ItemType Directory -Force -Path $Work | Out-Null
    $script:Src = Join-Path $Work 'cadence'
    $code = Invoke-Logged 'git' @('-c', 'init.defaultBranch=main', 'init', '--quiet', $script:Src)
    if ($code -ne 0) { Fail "could not create $($script:Src)" }
    $code = Invoke-Logged 'git' @('-C', $script:Src, 'remote', 'add', 'origin', $RepoUrl)
    if ($code -ne 0) { Fail "could not name the repository in $($script:Src)" }
    # The build names itself from the annotated tag, so the tag is fetched as itself.
    $code = Invoke-Logged 'git' @('-C', $script:Src, 'fetch', '--quiet', '--depth', '1', 'origin',
        "+refs/tags/$($script:Version):refs/tags/$($script:Version)")
    if ($code -ne 0) { Fail "could not fetch Cadence $script:Version" }
    $code = Invoke-Logged 'git' @('-C', $script:Src, '-c', 'advice.detachedHead=false', 'checkout', '--quiet',
        '--detach', "refs/tags/$($script:Version)")
    if ($code -ne 0) { Fail "could not check out Cadence $script:Version" }
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

function Test-Build {
    Step 'Checking the build'
    $want = (Get-Content -Raw -Path (Join-Path $script:Src 'bench.txt')).Trim()
    Write-Log '$ cadence bench'
    $lines = @(& $script:Built bench 2>$null)
    $got = ''
    if ($lines.Count -gt 0) { $got = ("$($lines[-1])".Trim() -split '\s+')[0] }
    Write-Log "bench: $got; bench.txt: $want"
    if (-not $got -or $got -ne $want) {
        Fail "the build's bench is $(if ($got) { $got } else { 'nothing' }) and bench.txt says $want, so it is not installed"
    }
    $script:Id = Get-IdName $script:Built
    Write-Log "id name: $script:Id"
    if ($script:Id -ne "Cadence $script:Version") {
        Fail "the build calls itself '$script:Id' and not 'Cadence $script:Version', so it is not installed"
    }
    Say "The bench counts $got nodes, as bench.txt says, and the engine calls itself $script:Id."
}

# --- The install -------------------------------------------------------------------------------

function Install-Binary {
    Step 'Installing'
    $previous = Get-StateValue 'binary'
    if ($script:Prefix) {
        $dest = $script:Prefix
        Say "Installing into $dest, as --prefix asks."
    } elseif ($script:Mode -eq 'update' -and $previous) {
        $dest = Split-Path -Parent $previous
        Say "Installing into $dest, where Cadence was installed before."
    } else {
        Say "The default folder is $DefaultPrefix."
        if (Ask 'Install Cadence there?' $true) {
            $dest = $DefaultPrefix
        } else {
            $dest = Read-Host 'Folder to install in'
            Write-Log "Folder to install in: $dest"
            if (-not $dest) { Stop-Install 'No folder was given; nothing was installed.' }
        }
    }
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    $script:Target = Join-Path $dest 'cadence.exe'
    if (Test-Path $script:Target) {
        $old = Get-IdName $script:Target
        $named = if ($old) { ", calling itself $old" } else { '' }
        Say "There is already a Cadence at $script:Target$named."
        if (-not (Ask 'Replace it?' ($script:Mode -eq 'update'))) { Stop-Install 'Kept the one there; nothing was installed.' }
    }
    try {
        Copy-Item -Force -Path $script:Built -Destination $script:Target -ErrorAction Stop
    } catch {
        Fail "could not write $($script:Target); if Cadence is running, close it and run this again"
    }
    Set-StateValue 'binary' $script:Target
    Say "Installed $script:Target."
}

function Add-ToPath {
    $dest = Split-Path -Parent $script:Target
    $user = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $user) { $user = '' }
    $entries = @($user -split ';' | Where-Object { $_ })
    if ($entries -contains $dest) {
        Say "$dest is already on your PATH."
        return
    }
    if (-not (Ask "Add $dest to your PATH, so a terminal finds cadence?" $true)) { return }
    [Environment]::SetEnvironmentVariable('Path', (($entries + $dest) -join ';'), 'User')
    Set-StateValue 'path_windows' $dest
    Say 'Added. Terminals opened from now on find cadence.'
}

function Get-Tables {
    Step 'Syzygy tablebases, if you want them'
    Say 'Cadence can read the 3-4-5 piece Syzygy tablebases, 290 files and about 1 GB, to play every'
    Say "ending of five pieces or fewer perfectly. They are downloaded from $TablesUrl."
    if (-not (Ask 'Download them now?' $false)) { Say 'Skipped.'; return }
    $dir = Join-Path $DataDir 'syzygy\3-4-5'
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $sums = Join-Path $Work 'sha256'
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$TablesUrl/sha256" -OutFile $sums -ErrorAction Stop
    } catch {
        Fail "could not download the tables' published checksums: $($_.Exception.Message)"
    }
    $wanted = @(Get-Content -Path $sums | ForEach-Object {
        $parts = $_ -split '\s+'
        if ($parts.Count -ge 2) {
            $pieces = ($parts[1] -replace '\.rtb[wz]$', '') -replace 'v', ''
            if ($pieces.Length -le 5) { [pscustomobject]@{ Sum = $parts[0].ToLower(); File = $parts[1] } }
        }
    })
    if ($wanted.Count -ne 290) { Fail "the published checksums list $($wanted.Count) tables of five pieces or fewer, not 290" }
    Set-StateValue 'tables' $dir
    $checked = 0
    foreach ($table in $wanted) {
        $group = switch -Regex ($table.File) {
            '\.rtbw$' { '3-4-5-wdl' }
            '\.rtbz$' { '3-4-5-dtz' }
            default { Fail "the published checksums name $($table.File), which is not a table" }
        }
        $path = Join-Path $dir $table.File
        $have = (Test-Path $path) -and ((Get-FileHash -Algorithm SHA256 -Path $path).Hash.ToLower() -eq $table.Sum)
        if (-not $have) {
            $part = "$path.part"
            try {
                Invoke-WebRequest -UseBasicParsing -Uri "$TablesUrl/$group/$($table.File)" -OutFile $part -ErrorAction Stop
            } catch {
                Fail "could not download $($table.File)"
            }
            if ((Get-FileHash -Algorithm SHA256 -Path $part).Hash.ToLower() -ne $table.Sum) {
                Remove-Item -Force $part
                Fail "$($table.File) does not match its published checksum"
            }
            Move-Item -Force -Path $part -Destination $path
        }
        $checked++
        if ($checked % 29 -eq 0) { Say "  $checked of 290 checked" }
    }
    Say "All 290 tables are in $dir, each matching its published checksum."
    Say 'To use them, set the engine option SyzygyPath to that folder in your chess program.'
}

function Show-Done {
    Step 'Done'
    Say "Cadence is at $script:Target and calls itself $script:Id."
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
    if ($script:Mode -eq 'uninstall') {
        Uninstall-Cadence
    } else {
        Step "Cadence's installer"
        Say 'This builds Cadence from its source, checks the build and installs it. Each step asks first.'
        Show-Cost
        Test-Git
        Test-Linker
        Test-Rust
        Resolve-Release
        Select-Build
        Get-Source
        Initialize-Toolchain
        Invoke-Build
        Test-Build
        Install-Binary
        Add-ToPath
        Get-Tables
        Show-Done
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
