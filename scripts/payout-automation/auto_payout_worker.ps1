param(
    [string]$ConfigPath = ".\config.json",
    [string]$QueuePath = ".\queue.json",
    [string]$StatePath = ".\state.json"
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

function Set-ObjectField {
    param(
        [object]$Object,
        [string]$Name,
        $Value
    )

    if ($Object.PSObject.Properties.Name -contains $Name) {
        $Object.$Name = $Value
    } else {
        $Object | Add-Member -NotePropertyName $Name -NotePropertyValue $Value
    }
}

function Get-Web2Eligibility {
    param([string]$BaseUrl, [string]$Wallet)

    try {
        $response = Invoke-RestMethod -Method Get -Uri "$BaseUrl/web2/account/$Wallet"
        return [pscustomobject]@{
            found = $true
            sessions = [int64]$response.sessions
            is_eligible = [bool]$response.is_eligible
        }
    } catch {
        return [pscustomobject]@{
            found = $false
            sessions = 0
            is_eligible = $false
        }
    }
}

function Get-ReserveRatio {
    param($Config)

    $liabilities = [double]$Config.current_pending_liabilities_usdc
    $reserve = [double]$Config.current_reserve_usdc

    if ($liabilities -le 0) {
        return 999999.0
    }

    return ($reserve / $liabilities)
}

function Ensure-HourlyWindow {
    param($State)

    $now = (Get-Date).ToUniversalTime()
    $start = [DateTime]::Parse($State.hourly_window_start).ToUniversalTime()
    if (($now - $start).TotalHours -ge 1) {
        $State.hourly_window_start = $now.ToString("o")
        $State.hourly_paid_total_usdc = 0
    }
}

function Get-UserDayKey {
    param([string]$Wallet)
    $today = (Get-Date).ToUniversalTime().ToString("yyyy-MM-dd")
    return "$today|$($Wallet.ToUpperInvariant())"
}

function Invoke-PayoutExecutor {
    param($Config, $Item)

    if ([string]::IsNullOrWhiteSpace($Config.payout_executor_url)) {
        return [pscustomobject]@{
            ok = $false
            error = "payout_executor_url is not configured"
            tx_hash = ""
        }
    }

    $headers = @{}
    if (-not [string]::IsNullOrWhiteSpace($Config.payout_executor_api_key)) {
        $headers["X-Api-Key"] = $Config.payout_executor_api_key
    }

    $payload = [ordered]@{
        request_id = $Item.request_id
        user_wallet = $Item.user_wallet
        destination_bsc_address = $Item.destination_bsc_address
        usdc_amount = [double]$Item.usdc_amount
        asset = "USDC"
        network = "BSC"
        swap_reference = $Item.swap_reference
    }

    try {
        $response = Invoke-RestMethod -Method Post -Uri $Config.payout_executor_url -Headers $headers -ContentType "application/json" -Body ($payload | ConvertTo-Json -Depth 10 -Compress)

        $ok = $false
        $txHash = ""
        if ($null -ne $response) {
            if ($response.status -eq "ok" -or $response.status -eq "sent" -or $response.ok -eq $true) {
                $ok = $true
            }
            if ($null -ne $response.tx_hash) {
                $txHash = [string]$response.tx_hash
            }
        }

        return [pscustomobject]@{
            ok = $ok
            error = if ($ok) { "" } else { "executor returned non-ok response" }
            tx_hash = $txHash
        }
    } catch {
        return [pscustomobject]@{
            ok = $false
            error = $_.Exception.Message
            tx_hash = ""
        }
    }
}

function Invoke-OnchainPayoutActivity {
    param($Config, $Item, [string]$TxHash)

    $enabled = $true
    if ($Config.PSObject.Properties.Name -contains "chain_activity_enabled") {
        $enabled = [bool]$Config.chain_activity_enabled
    }

    if (-not $enabled) {
        return [pscustomobject]@{ ok = $true; error = "" }
    }

    $activityUrl = ""
    if ($Config.PSObject.Properties.Name -contains "chain_activity_url") {
        $activityUrl = [string]$Config.chain_activity_url
    }
    if ([string]::IsNullOrWhiteSpace($activityUrl)) {
        $baseUrl = ([string]$Config.base_url).TrimEnd('/')
        if ([string]::IsNullOrWhiteSpace($baseUrl)) {
            return [pscustomobject]@{ ok = $false; error = "base_url is empty for chain activity" }
        }
        $activityUrl = "$baseUrl/app/activity"
    }

    $source = "inapp"
    if ($Config.PSObject.Properties.Name -contains "chain_activity_source") {
        $candidate = [string]$Config.chain_activity_source
        if ($candidate -eq "web" -or $candidate -eq "inapp") {
            $source = $candidate
        }
    }

    $payload = [ordered]@{
        source = $source
        action = "payout_sent"
        status = "success"
        detail = "request_id=$($Item.request_id);wallet=$($Item.user_wallet);destination=$($Item.destination_bsc_address);usdc=$($Item.usdc_amount);tx_hash=$TxHash"
    }

    try {
        $response = Invoke-RestMethod -Method Post -Uri $activityUrl -ContentType "application/json" -Body ($payload | ConvertTo-Json -Depth 10 -Compress)
        if ($null -ne $response -and $response.status -eq "accepted") {
            return [pscustomobject]@{ ok = $true; error = "" }
        }
        return [pscustomobject]@{ ok = $false; error = "chain activity returned non-accepted response" }
    } catch {
        return [pscustomobject]@{ ok = $false; error = $_.Exception.Message }
    }
}

$config = Load-JsonFile -Path $ConfigPath
$queue = Load-JsonFile -Path $QueuePath
$state = Load-JsonFile -Path $StatePath

if ($null -eq $queue.items) {
    throw "Queue file must contain items array"
}

if ($null -eq $state.processed_request_ids) { $state | Add-Member -NotePropertyName processed_request_ids -NotePropertyValue @() }
if ($null -eq $state.processed_swap_references) { $state | Add-Member -NotePropertyName processed_swap_references -NotePropertyValue @() }
if ($null -eq $state.user_daily_paid_usdc) { $state | Add-Member -NotePropertyName user_daily_paid_usdc -NotePropertyValue @{} }
if ($null -eq $state.hourly_paid_total_usdc) { $state | Add-Member -NotePropertyName hourly_paid_total_usdc -NotePropertyValue 0 }
if ($null -eq $state.hourly_window_start) { $state | Add-Member -NotePropertyName hourly_window_start -NotePropertyValue ((Get-Date).ToUniversalTime().ToString("o")) }

Ensure-HourlyWindow -State $state

$maxRetries = [int]$config.max_retries
$hourlyMax = [double]$config.max_total_payout_usdc_per_hour
$perUserDailyMax = [double]$config.max_payout_usdc_per_user_per_day
$minSessions = [int64]$config.min_sessions
$reserveRatioMin = [double]$config.reserve_ratio_min
$reserveRatio = Get-ReserveRatio -Config $config

$nowIso = (Get-Date).ToUniversalTime().ToString("o")
$processedCount = 0

foreach ($item in $queue.items) {
    if ($item.status -ne "pending") {
        continue
    }

    $requestId = [string]$item.request_id
    $swapRef = [string]$item.swap_reference
    $wallet = ([string]$item.user_wallet).ToUpperInvariant()
    $amount = [double]$item.usdc_amount

    if ([string]::IsNullOrWhiteSpace($requestId) -or [string]::IsNullOrWhiteSpace($wallet) -or $amount -le 0) {
        Set-ObjectField -Object $item -Name "status" -Value "rejected"
        Set-ObjectField -Object $item -Name "last_error" -Value "invalid payout request fields"
        Set-ObjectField -Object $item -Name "updated_at" -Value $nowIso
        continue
    }

    if ($state.processed_request_ids -contains $requestId) {
        Set-ObjectField -Object $item -Name "status" -Value "duplicate"
        Set-ObjectField -Object $item -Name "last_error" -Value "request_id already processed"
        Set-ObjectField -Object $item -Name "updated_at" -Value $nowIso
        continue
    }

    if (-not [string]::IsNullOrWhiteSpace($swapRef) -and ($state.processed_swap_references -contains $swapRef)) {
        Set-ObjectField -Object $item -Name "status" -Value "duplicate"
        Set-ObjectField -Object $item -Name "last_error" -Value "swap_reference already processed"
        Set-ObjectField -Object $item -Name "updated_at" -Value $nowIso
        continue
    }

    # Skip Web2 eligibility check in test mode
    $testMode = $false
    if ($config.PSObject.Properties.Name -contains "test_mode_skip_web2_eligibility") {
        $testMode = [bool]$config.test_mode_skip_web2_eligibility
    }

    if (-not $testMode) {
        $eligibility = Get-Web2Eligibility -BaseUrl $config.base_url -Wallet $wallet
        if (-not $eligibility.found) {
            Set-ObjectField -Object $item -Name "last_error" -Value "wallet not found in web2 ledger"
            Set-ObjectField -Object $item -Name "updated_at" -Value $nowIso
            continue
        }

        if (($eligibility.sessions -lt $minSessions) -or (-not $eligibility.is_eligible)) {
            Set-ObjectField -Object $item -Name "last_error" -Value "wallet not eligible yet"
            Set-ObjectField -Object $item -Name "updated_at" -Value $nowIso
            continue
        }
    }

    if ($reserveRatio -lt $reserveRatioMin) {
        Set-ObjectField -Object $item -Name "last_error" -Value "reserve ratio below minimum threshold"
        Set-ObjectField -Object $item -Name "updated_at" -Value $nowIso
        continue
    }

    $userDayKey = Get-UserDayKey -Wallet $wallet
    $userPaidToday = 0.0
    if ($state.user_daily_paid_usdc.PSObject.Properties.Name -contains $userDayKey) {
        $userPaidToday = [double]$state.user_daily_paid_usdc.$userDayKey
    }

    if (($userPaidToday + $amount) -gt $perUserDailyMax) {
        Set-ObjectField -Object $item -Name "last_error" -Value "daily per-user payout cap exceeded"
        Set-ObjectField -Object $item -Name "updated_at" -Value $nowIso
        continue
    }

    if (([double]$state.hourly_paid_total_usdc + $amount) -gt $hourlyMax) {
        Set-ObjectField -Object $item -Name "last_error" -Value "hourly total payout cap exceeded"
        Set-ObjectField -Object $item -Name "updated_at" -Value $nowIso
        continue
    }

    if ([bool]$config.dry_run) {
        Set-ObjectField -Object $item -Name "status" -Value "simulated_paid"
        Set-ObjectField -Object $item -Name "payout_tx_hash" -Value "dryrun-$requestId"
        Set-ObjectField -Object $item -Name "paid_at" -Value $nowIso
        Set-ObjectField -Object $item -Name "updated_at" -Value $nowIso
    } else {
        $result = Invoke-PayoutExecutor -Config $config -Item $item
        if (-not $result.ok) {
            Set-ObjectField -Object $item -Name "retries" -Value ([int]$item.retries + 1)
            Set-ObjectField -Object $item -Name "last_error" -Value $result.error
            Set-ObjectField -Object $item -Name "updated_at" -Value $nowIso

            if ($item.retries -ge $maxRetries) {
                Set-ObjectField -Object $item -Name "status" -Value "failed"
            }
            continue
        }

        Set-ObjectField -Object $item -Name "status" -Value "paid"
        Set-ObjectField -Object $item -Name "payout_tx_hash" -Value $result.tx_hash
        Set-ObjectField -Object $item -Name "paid_at" -Value $nowIso
        Set-ObjectField -Object $item -Name "updated_at" -Value $nowIso

        # Emit ANET on-chain audit event for successful live payouts without blocking settlement.
        $chainActivity = Invoke-OnchainPayoutActivity -Config $config -Item $item -TxHash $result.tx_hash
        if (-not $chainActivity.ok) {
            Set-ObjectField -Object $item -Name "chain_activity_error" -Value $chainActivity.error
        } else {
            Set-ObjectField -Object $item -Name "chain_activity_error" -Value ""
        }
    }

    $state.processed_request_ids += $requestId
    if (-not [string]::IsNullOrWhiteSpace($swapRef)) {
        $state.processed_swap_references += $swapRef
    }

    $state.hourly_paid_total_usdc = [double]$state.hourly_paid_total_usdc + $amount
    $state.user_daily_paid_usdc | Add-Member -NotePropertyName $userDayKey -NotePropertyValue ($userPaidToday + $amount) -Force

    $processedCount += 1
}

Save-JsonFile -Path $QueuePath -Object $queue
Save-JsonFile -Path $StatePath -Object $state

Write-Host "Processed payout items: $processedCount"
Write-Host "Hourly total paid (USDC): $($state.hourly_paid_total_usdc)"
Write-Host "Reserve ratio: $reserveRatio"
