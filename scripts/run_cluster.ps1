# CyxCloud Cluster Test Script (Windows PowerShell)
#
# Usage:
#   .\run_cluster.ps1           # Start 3-node cluster
#   .\run_cluster.ps1 -Nodes 5  # Start 5-node cluster
#   .\run_cluster.ps1 -Test     # Start cluster and run tests
#   .\run_cluster.ps1 -Build    # Build first, then start cluster

param(
    [int]$Nodes = 3,
    [switch]$Test,
    [switch]$Build,
    [switch]$Clean
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent $ScriptDir
$DataRoot = Join-Path $ProjectRoot "cluster_data"
$BasePort = 50051

Write-Host "CyxCloud Cluster Test" -ForegroundColor Cyan
Write-Host "=====================" -ForegroundColor Cyan
Write-Host "Nodes: $Nodes"
Write-Host "Data directory: $DataRoot"
Write-Host ""

# Clean up old data if requested
if ($Clean) {
    Write-Host "Cleaning up old cluster data..." -ForegroundColor Yellow
    if (Test-Path $DataRoot) {
        Remove-Item -Recurse -Force $DataRoot
    }
}

# Build if requested
if ($Build) {
    Write-Host "Building cluster_node example..." -ForegroundColor Yellow
    Push-Location $ProjectRoot
    cargo build --example cluster_node --release
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Build failed!" -ForegroundColor Red
        Pop-Location
        exit 1
    }
    Pop-Location
    Write-Host "Build complete!" -ForegroundColor Green
    Write-Host ""
}

# Check if binary exists
$BinaryPath = Join-Path $ProjectRoot "target\release\examples\cluster_node.exe"
if (-not (Test-Path $BinaryPath)) {
    Write-Host "Binary not found. Run with -Build flag first." -ForegroundColor Red
    Write-Host "  .\run_cluster.ps1 -Build"
    exit 1
}

# Create data directories
New-Item -ItemType Directory -Force -Path $DataRoot | Out-Null

# Start nodes
$Jobs = @()
$Ports = @()

for ($i = 1; $i -le $Nodes; $i++) {
    $Port = $BasePort + $i - 1
    $Ports += $Port
    $NodeId = "node$i"
    $NodeDataDir = Join-Path $DataRoot $NodeId
    New-Item -ItemType Directory -Force -Path $NodeDataDir | Out-Null

    $TestFlag = ""
    if ($Test -and $i -eq 1) {
        $TestFlag = "--test"
    }

    # First node has no bootstrap, others bootstrap from first node
    $BootstrapFlag = ""
    if ($i -gt 1) {
        $BootstrapPort = $BasePort
        $BootstrapFlag = "--bootstrap 127.0.0.1:$BootstrapPort"
    }

    Write-Host "Starting $NodeId on port $Port..." -ForegroundColor Green

    $Arguments = "--port $Port --data-dir `"$NodeDataDir`" --node-id $NodeId $TestFlag $BootstrapFlag"

    $Job = Start-Process -FilePath $BinaryPath -ArgumentList $Arguments -PassThru -NoNewWindow
    $Jobs += $Job

    # Give each node time to start
    Start-Sleep -Milliseconds 500
}

Write-Host ""
Write-Host "Cluster started with $Nodes nodes!" -ForegroundColor Cyan
Write-Host "Ports: $($Ports -join ', ')" -ForegroundColor Cyan
Write-Host ""
Write-Host "Press Ctrl+C to stop all nodes..." -ForegroundColor Yellow

# Wait for user interrupt
try {
    while ($true) {
        $RunningCount = ($Jobs | Where-Object { -not $_.HasExited }).Count
        if ($RunningCount -eq 0) {
            Write-Host "All nodes have stopped." -ForegroundColor Yellow
            break
        }
        Start-Sleep -Seconds 1
    }
}
finally {
    # Cleanup: stop all running nodes
    Write-Host ""
    Write-Host "Stopping all nodes..." -ForegroundColor Yellow
    foreach ($Job in $Jobs) {
        if (-not $Job.HasExited) {
            Stop-Process -Id $Job.Id -Force -ErrorAction SilentlyContinue
        }
    }
    Write-Host "Cluster stopped." -ForegroundColor Green
}
