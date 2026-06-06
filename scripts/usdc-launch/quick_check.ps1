param(
    [string]$BaseUrl = "https://anet-private-mainnet.onrender.com"
)

$ErrorActionPreference = "Stop"

Write-Host "==> Health"
Invoke-RestMethod -Method Get -Uri "$BaseUrl/health" | ConvertTo-Json -Depth 10

Write-Host "\n==> USDC token"
try {
    Invoke-RestMethod -Method Get -Uri "$BaseUrl/tokens/anrc20/USDC" | ConvertTo-Json -Depth 10
} catch {
    Write-Host "USDC token not found or not accessible"
}

Write-Host "\n==> DEX pools"
Invoke-RestMethod -Method Get -Uri "$BaseUrl/dex/pools" | ConvertTo-Json -Depth 10

Write-Host "\n==> USDC pool detail"
try {
    Invoke-RestMethod -Method Get -Uri "$BaseUrl/dex/pools/USDC" | ConvertTo-Json -Depth 10
} catch {
    Write-Host "USDC pool not found yet"
}
