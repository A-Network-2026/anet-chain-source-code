param(
    [Parameter(Mandatory = $true)]
    [string]$Wallet,

    [Parameter(Mandatory = $true)]
    [int]$BaseNonce,

    [string]$ChainId = "anet-private-mainnet-1",
    [string]$OutDir = ".",
    [ValidateSet("custom", "pilot100", "phase1")]
    [string]$Preset = "custom",

    # Phase-1 conservative launch profile
    [UInt64]$UsdcMintUnits = 75000000000,
    [UInt64]$InitialAnetAnts = 2500000000000,
    [UInt64]$InitialUsdcUnits = 25000000000,
    [UInt64]$AddAnetAnts = 1000000000000,
    [UInt64]$AddUsdcUnits = 10000000000
)

$ErrorActionPreference = "Stop"

function New-Timestamp {
    return (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
}

if (-not (Test-Path $OutDir)) {
    New-Item -ItemType Directory -Path $OutDir | Out-Null
}

if ($Preset -eq "pilot100") {
    # $100 USDC launch profile (internal pilot)
    $UsdcMintUnits = 100000000
    $InitialAnetAnts = 5000000000
    $InitialUsdcUnits = 50000000
    $AddAnetAnts = 5000000000
    $AddUsdcUnits = 50000000
}

if ($Preset -eq "phase1") {
    # Existing conservative phase-1 launch profile
    $UsdcMintUnits = 75000000000
    $InitialAnetAnts = 2500000000000
    $InitialUsdcUnits = 25000000000
    $AddAnetAnts = 1000000000000
    $AddUsdcUnits = 10000000000
}

$walletUpper = $Wallet.Trim().ToUpperInvariant()

$payload1 = [ordered]@{
    auth = [ordered]@{
        wallet = $walletUpper
        nonce = $BaseNonce
        timestamp = New-Timestamp
        chain_id = $ChainId
        payload = [ordered]@{
            route = "anrc20_create"
            symbol = "USDC"
            name = "USD Coin"
        }
        signature = "<FILL_SIGNATURE_HEX_65_BYTES>"
        action_hash = "<FILL_ACTION_HASH_HEX>"
    }
    symbol = "USDC"
    name = "USD Coin"
    decimals = 6
    initial_supply = 0
    mintable = $true
}

$payload2 = [ordered]@{
    auth = [ordered]@{
        wallet = $walletUpper
        nonce = ($BaseNonce + 1)
        timestamp = New-Timestamp
        chain_id = $ChainId
        payload = [ordered]@{
            route = "anrc20_mint"
            symbol = "USDC"
            to = $walletUpper
            amount = $UsdcMintUnits
        }
        signature = "<FILL_SIGNATURE_HEX_65_BYTES>"
        action_hash = "<FILL_ACTION_HASH_HEX>"
    }
    to = $walletUpper
    symbol = "USDC"
    amount = $UsdcMintUnits
}

$payload3 = [ordered]@{
    provider = $walletUpper
    auth = [ordered]@{
        wallet = $walletUpper
        nonce = ($BaseNonce + 2)
        timestamp = New-Timestamp
        chain_id = $ChainId
        payload = [ordered]@{
            route = "dex_create_pool"
            token_symbol = "USDC"
            anet_amount_ants = $InitialAnetAnts
            token_amount_units = $InitialUsdcUnits
            fee_bps = 30
        }
        signature = "<FILL_SIGNATURE_HEX_65_BYTES>"
        action_hash = "<FILL_ACTION_HASH_HEX>"
    }
    token_symbol = "USDC"
    anet_amount_ants = $InitialAnetAnts
    token_amount_units = $InitialUsdcUnits
    fee_bps = 30
}

$payload4 = [ordered]@{
    provider = $walletUpper
    auth = [ordered]@{
        wallet = $walletUpper
        nonce = ($BaseNonce + 3)
        timestamp = New-Timestamp
        chain_id = $ChainId
        payload = [ordered]@{
            route = "dex_add_liquidity"
            token_symbol = "USDC"
            anet_amount_ants = $AddAnetAnts
            token_amount_units = $AddUsdcUnits
        }
        signature = "<FILL_SIGNATURE_HEX_65_BYTES>"
        action_hash = "<FILL_ACTION_HASH_HEX>"
    }
    token_symbol = "USDC"
    anet_amount_ants = $AddAnetAnts
    token_amount_units = $AddUsdcUnits
}

$files = @(
    @{ Name = "01_anrc20_create.usdc.json"; Payload = $payload1 },
    @{ Name = "02_anrc20_mint.usdc.json"; Payload = $payload2 },
    @{ Name = "03_dex_create_pool.anet_usdc.json"; Payload = $payload3 },
    @{ Name = "04_dex_add_liquidity.anet_usdc.json"; Payload = $payload4 }
)

foreach ($entry in $files) {
    $path = Join-Path $OutDir $entry.Name
    ($entry.Payload | ConvertTo-Json -Depth 20) | Set-Content -Path $path -Encoding UTF8
    Write-Host "Generated: $path"
}

Write-Host "\nNext steps:"
Write-Host "1) Fill signature and action_hash in each file"
Write-Host "2) Run: .\\run_usdc_launch.ps1 -PayloadDir $OutDir"
Write-Host "3) Run: .\\quick_check.ps1"
Write-Host "\nApplied profile: $Preset"
Write-Host "USDC mint units: $UsdcMintUnits"
Write-Host "Create pool: ANET ants=$InitialAnetAnts, USDC units=$InitialUsdcUnits"
Write-Host "Add liquidity: ANET ants=$AddAnetAnts, USDC units=$AddUsdcUnits"
