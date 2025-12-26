# CyxCloud Cluster Integration Test (Windows PowerShell)
#
# This script starts a 3-node cluster, runs tests, and verifies:
# 1. All nodes can store chunks
# 2. Chunks can be retrieved from any node
# 3. Replication works across nodes

param(
    [switch]$Build
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent $ScriptDir
$DataRoot = Join-Path $ProjectRoot "cluster_test_data"
$BasePort = 50081

Write-Host "CyxCloud Cluster Integration Test" -ForegroundColor Cyan
Write-Host "==================================" -ForegroundColor Cyan
Write-Host ""

# Clean up old data
if (Test-Path $DataRoot) {
    Remove-Item -Recurse -Force $DataRoot
}
New-Item -ItemType Directory -Force -Path $DataRoot | Out-Null

# Build if requested
if ($Build) {
    Write-Host "Building project..." -ForegroundColor Yellow
    Push-Location $ProjectRoot
    cargo build --example cluster_node --release
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Build failed!" -ForegroundColor Red
        Pop-Location
        exit 1
    }
    Pop-Location
    Write-Host ""
}

# Check if binary exists
$BinaryPath = Join-Path $ProjectRoot "target\release\examples\cluster_node.exe"
if (-not (Test-Path $BinaryPath)) {
    Write-Host "Binary not found. Run with -Build flag first." -ForegroundColor Red
    exit 1
}

# Start 3 nodes
$Nodes = @()
$Ports = @(50081, 50082, 50083)

Write-Host "Starting 3-node cluster..." -ForegroundColor Yellow

foreach ($i in 1..3) {
    $Port = $Ports[$i - 1]
    $NodeId = "test-node-$i"
    $NodeDataDir = Join-Path $DataRoot $NodeId
    New-Item -ItemType Directory -Force -Path $NodeDataDir | Out-Null

    $BootstrapFlag = ""
    if ($i -gt 1) {
        $BootstrapFlag = "--bootstrap 127.0.0.1:$($Ports[0])"
    }

    $TestFlag = if ($i -eq 1) { "--test" } else { "" }

    $Arguments = "--port $Port --data-dir `"$NodeDataDir`" --node-id $NodeId $TestFlag $BootstrapFlag"
    $Job = Start-Process -FilePath $BinaryPath -ArgumentList $Arguments -PassThru -NoNewWindow
    $Nodes += $Job

    Start-Sleep -Milliseconds 500
}

Write-Host "Cluster started!" -ForegroundColor Green
Write-Host ""

# Wait for all nodes to be ready
Start-Sleep -Seconds 2

# Run the integration tests
Write-Host "Running integration tests..." -ForegroundColor Yellow
Push-Location $ProjectRoot
cargo test -p cyxcloud-network --test integration_test -- --test-threads=1 2>&1 | ForEach-Object { Write-Host $_ }
$TestResult = $LASTEXITCODE
Pop-Location

# Cleanup: stop all nodes
Write-Host ""
Write-Host "Stopping cluster..." -ForegroundColor Yellow
foreach ($Node in $Nodes) {
    if (-not $Node.HasExited) {
        Stop-Process -Id $Node.Id -Force -ErrorAction SilentlyContinue
    }
}

# Clean up test data
Remove-Item -Recurse -Force $DataRoot -ErrorAction SilentlyContinue

# Report result
Write-Host ""
if ($TestResult -eq 0) {
    Write-Host "All tests passed!" -ForegroundColor Green
    exit 0
} else {
    Write-Host "Some tests failed!" -ForegroundColor Red
    exit 1
}
