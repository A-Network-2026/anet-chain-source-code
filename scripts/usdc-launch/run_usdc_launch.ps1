param(
    [string]$BaseUrl = "https://anet-private-mainnet.onrender.com",
    [string]$PayloadDir = "."
)

$ErrorActionPreference = "Stop"

function Test-HasPlaceholder {
    param(
        [object]$Object
    )

    if ($null -eq $Object) {
        return $false
    }

    if ($Object -is [string]) {
        return $Object.Contains("<") -and $Object.Contains(">")
    }

    if ($Object -is [System.Collections.IEnumerable] -and -not ($Object -is [string])) {
        foreach ($item in $Object) {
            if (Test-HasPlaceholder -Object $item) {
                return $true
            }
        }
        return $false
    }

    foreach ($property in $Object.PSObject.Properties) {
        if (Test-HasPlaceholder -Object $property.Value) {
            return $true
        }
    }

    return $false
}

function Invoke-JsonPost {
    param(
        [string]$Name,
        [string]$Endpoint,
        [string]$FileName
    )

    $path = Join-Path $PayloadDir $FileName
    if (-not (Test-Path $path)) {
        Write-Error "Missing payload file: $path"
    }

    Write-Host "\n==> $Name"
    Write-Host "POST $Endpoint"

    $raw = Get-Content -Path $path -Raw
    $parsed = $raw | ConvertFrom-Json
    if (Test-HasPlaceholder -Object $parsed) {
        Write-Error "Payload file still contains placeholder values: $path"
    }
    $response = Invoke-RestMethod -Method Post -Uri "$BaseUrl$Endpoint" -ContentType "application/json" -Body ($parsed | ConvertTo-Json -Depth 20 -Compress)
    $response | ConvertTo-Json -Depth 20
}

Write-Host "Base URL: $BaseUrl"
Write-Host "Payload dir: $PayloadDir"

Invoke-JsonPost -Name "ANRC20 Create USDC" -Endpoint "/tokens/anrc20/create" -FileName "01_anrc20_create.usdc.json"
Invoke-JsonPost -Name "ANRC20 Mint USDC" -Endpoint "/tokens/anrc20/mint" -FileName "02_anrc20_mint.usdc.json"
Invoke-JsonPost -Name "DEX Create ANET/USDC Pool" -Endpoint "/dex/pools/create" -FileName "03_dex_create_pool.anet_usdc.json"
Invoke-JsonPost -Name "DEX Add Liquidity ANET/USDC" -Endpoint "/dex/pools/add-liquidity" -FileName "04_dex_add_liquidity.anet_usdc.json"

Write-Host "\n==> Final pool check"
$pool = Invoke-RestMethod -Method Get -Uri "$BaseUrl/dex/pools/USDC"
$pool | ConvertTo-Json -Depth 20
