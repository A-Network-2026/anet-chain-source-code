param(
    [string]$ConfigPath = ".\config.json",
    [string]$QueuePath = ".\queue.json",
    [string]$StatePath = ".\state.json",
    [string]$WalletListPath = "",
    [switch]$DryRun = $true,
    [int]$LimitCount = 0
)

$ErrorActionPreference = "Stop"

function Load-JsonFile {
    param([string]$Path)
    if (-not (Test-Path $Path)) {
        throw "Missing file: $Path"
    }
    return (Get-Content -Path $Path -Raw | ConvertFrom-Json)
}

function Save-JsonFile {
    param([string]$Path, [object]$Object)
    ($Object | ConvertTo-Json -Depth 50) | Set-Content -Path $Path -Encoding UTF8
}

function Get-Web2AccountInfo {
    param([string]$BaseUrl, [string]$Wallet)
    
    try {
        $response = Invoke-RestMethod -Method Get -Uri "$BaseUrl/web2/account/$Wallet" -ErrorAction Stop
        return [pscustomobject]@{
            found = $true
            wallet = $Wallet
            sessions = [int64]$response.sessions
            is_eligible = [bool]$response.is_eligible
        }
    } catch {
        return [pscustomobject]@{
            found = $false
            wallet = $Wallet
            sessions = 0
            is_eligible = $false
            error = $_.Exception.Message
        }
    }
}

function Get-UserDayKey {
    param([string]$Wallet)
    $today = (Get-Date).ToUniversalTime().ToString("yyyy-MM-dd")
    return "$today|$($Wallet.ToUpperInvariant())"
}

$config = Load-JsonFile -Path $ConfigPath
$queue = Load-JsonFile -Path $QueuePath
$state = Load-JsonFile -Path $StatePath

$baseUrl = [string]$config.base_url
$minSessions = [int64]$config.min_sessions
$maxPerUserPerDay = [double]$config.max_payout_usdc_per_user_per_day
$maxPerHour = [double]$config.max_total_payout_usdc_per_hour
$hourlyMax = $maxPerHour

# Default payout amount per user (can be overridden by config)
$defaultPayoutAmount = 1.0
if ($config.PSObject.Properties.Name -contains "default_payout_usdc_per_user") {
    $defaultPayoutAmount = [double]$config.default_payout_usdc_per_user
}

Write-Host "=== Queue Populate from Web2 ===" -ForegroundColor Cyan
Write-Host "Base URL: $baseUrl"
Write-Host "Min Sessions: $minSessions"
Write-Host "Max per User/Day: $maxPerUserPerDay USDC"
Write-Host "Max per Hour: $maxPerHour USDC"
Write-Host "Default Payout: $defaultPayoutAmount USDC"
Write-Host "Dry-Run Mode: $DryRun"
Write-Host ""

# If wallet list provided, use it; otherwise ask for input
$walletList = @()
if (-not [string]::IsNullOrWhiteSpace($WalletListPath)) {
    if (Test-Path $WalletListPath) {
        $walletList = @(Get-Content $WalletListPath | Where-Object { $_ -and -not $_.StartsWith("#") } | ForEach-Object { $_.Trim() })
        Write-Host "Loaded $($walletList.Count) wallets from $WalletListPath" -ForegroundColor Green
    } else {
        Write-Host "Wallet list file not found: $WalletListPath" -ForegroundColor Yellow
    }
}

# If no wallets from file, prompt
if ($walletList.Count -eq 0) {
    Write-Host "Enter eligible wallet addresses (one per line, empty line to finish):" -ForegroundColor Yellow
    $input_wallets = @()
    while ($true) {
        $wallet = Read-Host
        if ([string]::IsNullOrWhiteSpace($wallet)) { break }
        $input_wallets += $wallet.Trim()
    }
    $walletList = $input_wallets
    Write-Host "Collected $($walletList.Count) wallets" -ForegroundColor Green
}

if ($walletList.Count -eq 0) {
    Write-Host "No wallets provided. Exiting." -ForegroundColor Yellow
    exit 0
}

$addedCount = 0
$skippedCount = 0
$currentHourlyTotal = [double]$state.hourly_paid_total_usdc
$now = (Get-Date).ToUniversalTime().ToString("o")

foreach ($wallet in $walletList) {
    if ($LimitCount -gt 0 -and $addedCount -ge $LimitCount) {
        Write-Host "Reached limit of $LimitCount. Stopping." -ForegroundColor Yellow
        break
    }

    $wallet = $wallet.ToUpperInvariant()

    # Check if already processed
    if ($state.processed_request_ids -contains "payout_$wallet") {
        Write-Host "  [$wallet] SKIP: already processed" -ForegroundColor Gray
        $skippedCount++
        continue
    }

    # Check Web2 eligibility
    $info = Get-Web2AccountInfo -BaseUrl $baseUrl -Wallet $wallet
    if (-not $info.found) {
        Write-Host "  [$wallet] SKIP: not found in web2 ($($info.error))" -ForegroundColor Gray
        $skippedCount++
        continue
    }

    if ($info.sessions -lt $minSessions) {
        Write-Host "  [$wallet] SKIP: insufficient sessions ($($info.sessions) < $minSessions)" -ForegroundColor Gray
        $skippedCount++
        continue
    }

    if (-not $info.is_eligible) {
        Write-Host "  [$wallet] SKIP: not eligible flag" -ForegroundColor Gray
        $skippedCount++
        continue
    }

    # Check daily cap
    $userDayKey = Get-UserDayKey -Wallet $wallet
    $userPaidToday = 0.0
    if ($state.user_daily_paid_usdc.PSObject.Properties.Name -contains $userDayKey) {
        $userPaidToday = [double]$state.user_daily_paid_usdc.$userDayKey
    }

    if (($userPaidToday + $defaultPayoutAmount) -gt $maxPerUserPerDay) {
        Write-Host "  [$wallet] SKIP: daily per-user cap would be exceeded ($userPaidToday + $defaultPayoutAmount > $maxPerUserPerDay)" -ForegroundColor Gray
        $skippedCount++
        continue
    }

    # Check hourly cap
    if (($currentHourlyTotal + $defaultPayoutAmount) -gt $hourlyMax) {
        Write-Host "  [$wallet] SKIP: hourly cap would be exceeded ($currentHourlyTotal + $defaultPayoutAmount > $hourlyMax)" -ForegroundColor Gray
        $skippedCount++
        continue
    }

    # Create queue item
    $requestId = "payout_$(Get-Date -UFormat '%s')_$wallet"
    $swapRef = "swap_$(Get-Date -UFormat '%s')_$wallet"

    $item = [pscustomobject]@{
        request_id = $requestId
        user_wallet = $wallet
        destination_bsc_address = "0x1111111111111111111111111111111111111111"  # Placeholder; user provides real BSC address
        usdc_amount = $defaultPayoutAmount
        swap_reference = $swapRef
        status = "pending"
        retries = 0
        created_at = $now
        last_error = ""
        updated_at = $now
    }

    if ($DryRun) {
        Write-Host "  [$wallet] DRY-RUN: would add $defaultPayoutAmount USDC (sessions=$($info.sessions), eligible=$($info.is_eligible))" -ForegroundColor Cyan
    } else {
        $queue.items += $item
        $currentHourlyTotal += $defaultPayoutAmount
        Write-Host "  [$wallet] ADDED: $defaultPayoutAmount USDC" -ForegroundColor Green
    }

    $addedCount++
}

Write-Host ""
Write-Host "Summary:" -ForegroundColor Cyan
Write-Host "  Added: $addedCount"
Write-Host "  Skipped: $skippedCount"
Write-Host "  Total items in queue: $($queue.items.Count)"
Write-Host "  Hourly total (projected): $currentHourlyTotal USDC"
Write-Host ""

if (-not $DryRun) {
    Save-JsonFile -Path $QueuePath -Object $queue
    Write-Host "Queue saved to $QueuePath" -ForegroundColor Green
} else {
    Write-Host "DRY-RUN mode: no files saved. Run with -DryRun `$false to apply changes." -ForegroundColor Yellow
}
