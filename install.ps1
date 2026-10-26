# Install aistudio (Google AI Studio CLI) from GitHub Releases. Windows.
#
#   irm https://raw.githubusercontent.com/igarinpiano/ai-studio-cli/main/install.ps1 | iex
#
# Environment variables:
#   AISTUDIO_VERSION      Release tag to install, e.g. v0.1.0 (default: latest)
#   AISTUDIO_INSTALL_DIR  Where to put aistudio.exe (default: %LOCALAPPDATA%\Programs\aistudio)
#   GITHUB_TOKEN / GH_TOKEN  Token for a private repository (also taken from
#                         `gh auth token` when the GitHub CLI is logged in)
#   AISTUDIO_GITHUB_API / AISTUDIO_GITHUB_URL  API and web base URLs
#                         (default: https://api.github.com, https://github.com)

# (Everything runs in a script block so `irm | iex` leaves no variables or
# preferences behind in the caller's session.)
& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'  # Invoke-WebRequest is very slow with the progress bar on PS 5.1

    $Repo = 'igarinpiano/ai-studio-cli'
    $Bin = 'aistudio'
    $Version = if ($env:AISTUDIO_VERSION) { $env:AISTUDIO_VERSION } else { 'latest' }
    $InstallDir = if ($env:AISTUDIO_INSTALL_DIR) { $env:AISTUDIO_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\aistudio' }
    $Api = if ($env:AISTUDIO_GITHUB_API) { $env:AISTUDIO_GITHUB_API } else { 'https://api.github.com' }
    $Web = if ($env:AISTUDIO_GITHUB_URL) { $env:AISTUDIO_GITHUB_URL } else { 'https://github.com' }

    $Token = if ($env:GITHUB_TOKEN) { $env:GITHUB_TOKEN } elseif ($env:GH_TOKEN) { $env:GH_TOKEN } else { '' }
    if (-not $Token -and (Get-Command gh -ErrorAction SilentlyContinue)) {
        $Token = (& gh auth token 2>$null | Out-String).Trim()
        if ($LASTEXITCODE -ne 0) { $Token = '' }
    }

    # Windows PowerShell 5.1 may default to TLS 1.0, which GitHub rejects.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    function Get-Headers([string]$Accept) {
        $h = @{ Accept = $Accept; 'User-Agent' = 'aistudio-installer' }
        if ($Token) { $h['Authorization'] = "Bearer $Token" }
        return $h
    }

    $Arch = $null
    try { $Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() } catch { }
    if (-not $Arch) {
        $Arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    }
    $Target = switch -Regex ($Arch) {
        '^arm64$' { 'aarch64-pc-windows-msvc' }
        '^(x64|amd64)$' { 'x86_64-pc-windows-msvc' }
        default { throw "Unsupported CPU architecture: $Arch" }
    }

    $ReleaseUrl = if ($Version -eq 'latest') { "$Api/repos/$Repo/releases/latest" } else { "$Api/repos/$Repo/releases/tags/$Version" }
    try {
        $Release = Invoke-RestMethod -Uri $ReleaseUrl -Headers (Get-Headers 'application/vnd.github+json') -UseBasicParsing
    } catch {
        $Hint = if (-not $Token) { " (for a private repository, set GITHUB_TOKEN or run 'gh auth login')" } else { '' }
        throw "Could not find release '$Version' of $Repo$Hint. $($_.Exception.Message)"
    }
    $Tag = $Release.tag_name
    $Name = "$Bin-$($Tag -replace '^v', '')-$Target"
    $Asset = "$Name.zip"

    function Get-Asset([string]$AssetName, [string]$OutFile) {
        if ($Token) {
            # Private repositories serve assets only through the API. The token is
            # not forwarded on the redirect to the storage host.
            $a = $Release.assets | Where-Object { $_.name -eq $AssetName } | Select-Object -First 1
            if (-not $a) { throw "Release $Tag has no asset $AssetName" }
            Invoke-WebRequest -Uri $a.url -Headers (Get-Headers 'application/octet-stream') -OutFile $OutFile -UseBasicParsing
        } else {
            Invoke-WebRequest -Uri "$Web/$Repo/releases/download/$Tag/$AssetName" -OutFile $OutFile -UseBasicParsing
        }
    }

    $Tmp = Join-Path ([IO.Path]::GetTempPath()) ("aistudio-install-" + [Guid]::NewGuid())
    New-Item -ItemType Directory -Path $Tmp | Out-Null
    try {
        $Zip = Join-Path $Tmp $Asset
        Write-Host "Downloading $Asset..."
        Get-Asset $Asset $Zip

        $Sums = Join-Path $Tmp 'SHA256SUMS'
        $HaveSums = $true
        try { Get-Asset 'SHA256SUMS' $Sums } catch { $HaveSums = $false }
        if ($HaveSums) {
            $Expected = $null
            foreach ($line in Get-Content $Sums) {
                $fields = $line.Trim() -split '\s+'
                if ($fields.Count -ge 2 -and $fields[1].TrimStart('*') -eq $Asset) { $Expected = $fields[0].ToLower() }
            }
            $Actual = (Get-FileHash -Algorithm SHA256 $Zip).Hash.ToLower()
            if (-not $Expected) {
                Write-Warning "$Asset is not listed in SHA256SUMS; skipping verification."
            } elseif ($Expected -ne $Actual) {
                throw "Checksum mismatch for $Asset (expected $Expected, got $Actual)"
            }
        } else {
            Write-Warning "No SHA256SUMS in release $Tag; skipping verification."
        }

        Expand-Archive -Path $Zip -DestinationPath $Tmp -Force
        $Exe = Join-Path (Join-Path $Tmp $Name) "$Bin.exe"
        if (-not (Test-Path $Exe)) { throw "The archive did not contain $Name\$Bin.exe" }

        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
        Copy-Item $Exe (Join-Path $InstallDir "$Bin.exe") -Force
    } finally {
        Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
    }

    $Installed = Join-Path $InstallDir "$Bin.exe"
    Write-Host "Installed $(& $Installed --version) to $Installed"

    $UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $Entries = @($UserPath -split ';' | Where-Object { $_ -ne '' })
    if ($Entries -notcontains $InstallDir) {
        [Environment]::SetEnvironmentVariable('Path', (($Entries + $InstallDir) -join ';'), 'User')
        $env:Path = "$env:Path;$InstallDir"
        Write-Host "Added $InstallDir to your user PATH. Open a new terminal to use '$Bin' everywhere."
    }

    Write-Host ''
    Write-Host "Next: get an API key at https://aistudio.google.com/apikey, then run: $Bin login"
}
