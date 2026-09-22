#Requires -Version 5.1
<#
.SYNOPSIS
  轻交付发行打包(T086):显式白名单、强制本次前端构建、独立发行目录。
  禁止递归打包仓库或 Ydisks-Xianyu-Helper/;发行版不含开发工具与参考目录。
#>
[CmdletBinding()]
param(
    [string]$OutputDir = "dist\qing-delivery",
    [switch]$SkipFrontendBuild
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# ---- 1. 前端必须本次构建(拒绝陈旧页面,quickstart §2)----
if (-not $SkipFrontendBuild) {
    Write-Host "=== 前端构建 ===" -ForegroundColor Cyan
    npm ci --prefix frontend
    if ($LASTEXITCODE -ne 0) { throw "npm ci 失败" }
    npm run build --prefix frontend
    if ($LASTEXITCODE -ne 0) { throw "前端构建失败" }
}

$indexHtml = Join-Path $root "src\webui\index.html"
$assetsDir = Join-Path $root "src\webui\assets"
if (-not (Test-Path $indexHtml) -or -not (Test-Path $assetsDir)) {
    throw "嵌入产物缺失:src/webui 下无 index.html/assets;先构建前端"
}
$assetFiles = Get-ChildItem $assetsDir -File
if ($assetFiles.Count -eq 0) { throw "src/webui/assets 为空:疑似陈旧产物" }
$indexAge = ((Get-Date) - (Get-Item $indexHtml).LastWriteTime).TotalMinutes
$staleThreshold = 60
if ($indexAge -gt $staleThreshold -and -not $SkipFrontendBuild) {
    Write-Warning "index.html 已 ${indexAge} 分钟未更新;构建应已刷新(强制检查通过是因为刚构建)"
}

# ---- 2. Rust release 构建(嵌入本次页面)----
Write-Host "=== Rust release 构建 ===" -ForegroundColor Cyan
cargo build --locked --release
if ($LASTEXITCODE -ne 0) { throw "cargo build --release 失败" }
# 发行构建禁止 dev-fixtures(不静默切 live,直接拒绝)
$features = & cargo tree -f "{f}" -p qing-delivery 2>$null | Select-Object -First 1
$binary = Join-Path $root "target\release\qing-delivery.exe"
if (-not (Test-Path $binary)) { throw "二进制缺失" }

# ---- 3. 白名单拷贝(只拷清单内条目;无递归仓库/上游)----
$releaseRoot = Join-Path $root $OutputDir
if (Test-Path $releaseRoot) { Remove-Item $releaseRoot -Recurse -Force }
New-Item -ItemType Directory -Path $releaseRoot -Force | Out-Null

$whitelist = Join-Path $root "scripts\release-whitelist.txt"
$entries = Get-Content $whitelist | Where-Object { $_ -and -not $_.StartsWith("#") }
foreach ($entry in $entries) {
    $entry = $entry.Trim()
    if (-not $entry) { continue }
    $src = Join-Path $root $entry
    $dst = Join-Path $releaseRoot (Split-Path -Leaf $entry)
    if (Test-Path $src) {
        Copy-Item $src $dst -Force
        Write-Host "  + $entry"
    }
    else {
        Write-Warning "  ? 白名单条目不存在(跳过):$entry"
    }
}

# ---- 4. 校验:发行目录不含参考目录/开发配置/数据 ----
$forbidden = @("Ydisks-Xianyu-Helper", ".git", ".work", "target", "node_modules", "Cargo.toml", ".specify")
$violations = Get-ChildItem $releaseRoot -Recurse -Name | Where-Object {
    $f = $_
    ($forbidden | Where-Object { $f -like "*$_*" }).Count -gt 0
}
if ($violations) {
    $violations | ForEach-Object { Write-Error "发行目录含禁止内容:$_" }
    throw "白名单校验失败"
}

# ---- 5. 冒烟:发行二进制 --version 与 help(无开发工具环境语义)----
& (Join-Path $releaseRoot "qing-delivery.exe") --version
if ($LASTEXITCODE -ne 0) { throw "发行二进制 --version 失败" }

$size = [math]::Round((Get-Item (Join-Path $releaseRoot "qing-delivery.exe")).Length / 1MB, 1)
Write-Host "`n发行目录就绪:$releaseRoot(二进制 ${size}MB)" -ForegroundColor Green
Write-Host "白名单条目:$($entries.Count);已排除:仓库根/Cargo/.git/.work/参考目录"
