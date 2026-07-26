$ErrorActionPreference = "SilentlyContinue"
$Utf8NoBom = [System.Text.UTF8Encoding]::new($false, $true)
[Console]::InputEncoding = $Utf8NoBom
[Console]::OutputEncoding = $Utf8NoBom

function Read-MngrUtf8Stdin {
  $inputStream = [Console]::OpenStandardInput()
  $memory = [System.IO.MemoryStream]::new()
  $inputStream.CopyTo($memory)
  return $Utf8NoBom.GetString($memory.ToArray())
}

function Write-MngrUsageCache {
  param($Payload)

  if ($null -eq $Payload.rate_limits) { return }

  $root = Join-Path $env:LOCALAPPDATA "mngr"
  $path = Join-Path $root "claude-usage.json"
  $next = $Payload.rate_limits | ConvertTo-Json -Compress -Depth 20
  if ([string]::IsNullOrWhiteSpace($next)) { return }

  $current = ""
  if (Test-Path -LiteralPath $path) {
    $current = Get-Content -Raw -Encoding UTF8 -LiteralPath $path -ErrorAction SilentlyContinue
  }

  if (($current.Trim()) -eq $next) { return }

  New-Item -ItemType Directory -Force -Path $root | Out-Null
  [System.IO.File]::WriteAllText($path, $next, $Utf8NoBom)
}

function Get-MngrDefaultStatusLine {
  param($Payload)

  $model = "Claude"
  if ($null -ne $Payload.model -and $null -ne $Payload.model.display_name) {
    $model = [string]$Payload.model.display_name
  }

  $context = $null
  if ($null -ne $Payload.context_window -and $null -ne $Payload.context_window.used_percentage) {
    $context = [math]::Round([double]$Payload.context_window.used_percentage)
  }

  if ($null -ne $context) {
    return "[$model] $context% context"
  }

  return "[$model]"
}

function Invoke-MngrOriginalStatusLine {
  param([string]$PayloadText)

  $scriptDir = Split-Path -Parent $PSCommandPath
  $commandPath = Join-Path $scriptDir "claude-statusline-original.txt"
  if (-not (Test-Path -LiteralPath $commandPath)) { return $null }

  $originalCommand = Get-Content -Raw -Encoding UTF8 -LiteralPath $commandPath -ErrorAction SilentlyContinue
  if ([string]::IsNullOrWhiteSpace($originalCommand)) { return $null }

  try {
    $output = $PayloadText | & $env:ComSpec /d /s /c $originalCommand 2>$null
    $line = ($output | Out-String).Trim()
    if (-not [string]::IsNullOrWhiteSpace($line)) {
      return $line
    }
  } catch {}

  return $null
}

$payloadText = ""
$payload = $null

try {
  $payloadText = Read-MngrUtf8Stdin
  if (-not [string]::IsNullOrWhiteSpace($payloadText)) {
    $payload = $payloadText | ConvertFrom-Json -ErrorAction Stop
    Write-MngrUsageCache -Payload $payload
  }
} catch {}

try {
  $wrapped = Invoke-MngrOriginalStatusLine -PayloadText $payloadText
  if (-not [string]::IsNullOrWhiteSpace($wrapped)) {
    [Console]::Out.WriteLine($wrapped)
    exit 0
  }
} catch {}

try {
  if ($null -ne $payload) {
    [Console]::Out.WriteLine((Get-MngrDefaultStatusLine -Payload $payload))
  }
} catch {}

exit 0
