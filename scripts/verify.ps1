#Requires -Version 5.1
<#
.SYNOPSIS
  轻交付质量门禁:串联 quickstart §2 的全部验证命令。
  任一命令非零退出即失败;测试报告必须证明关键测试实际运行,
  过滤后零测试不得视为通过(quickstart §2 约定)。
#>
[CmdletBinding()]
param(
    [switch]$SkipFrontend,
    [switch]$SkipRust
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$failed = $false

function Invoke-Gate {
    param([string]$Name, [scriptblock]$Action)
    Write-Host "`n=== $Name ===" -ForegroundColor Cyan
    & $Action
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[FAIL] $Name (exit $LASTEXITCODE)" -ForegroundColor Red
        $script:failed = $true
    } else {
        Write-Host "[PASS] $Name" -ForegroundColor Green
    }
}

Write-Host "工具链版本:" -ForegroundColor Cyan
rustc --version
cargo --version
node --version
npm --version

if (-not $SkipRust) {
    Invoke-Gate 'cargo fmt --check' { cargo fmt --all -- --check }
    Invoke-Gate 'cargo clippy' { cargo clippy --locked --all-targets -- -D warnings }
    Invoke-Gate 'cargo test' { cargo test --locked --all-targets }
    Invoke-Gate 'cargo build --release' { cargo build --locked --release }
}

if (-not $SkipFrontend) {
    Invoke-Gate 'npm ci' { npm ci --prefix frontend }
    Invoke-Gate 'frontend typecheck' { npm run typecheck --prefix frontend }
    Invoke-Gate 'frontend test' { npm run test --prefix frontend -- --run }
    Invoke-Gate 'frontend build' { npm run build --prefix frontend }
}

if ($failed) {
    Write-Host "`n验证失败:存在未通过的门禁。" -ForegroundColor Red
    exit 1
}
Write-Host "`n全部门禁通过。" -ForegroundColor Green
exit 0
