$ErrorActionPreference = "SilentlyContinue"

function ConvertTo-MngrHookPayload {
  param([string]$PayloadText)

  $hook = $PayloadText | ConvertFrom-Json
  Add-Content "$env:LOCALAPPDATA\mngr\hook-events.log" "$(Get-Date -Format o) $($hook.hook_event_name)"
  if ($hook.hook_event_name -eq "PermissionRequest") {
    $requestId = [guid]::NewGuid().ToString()
    $hook | Add-Member -NotePropertyName "request_id" -NotePropertyValue $requestId -Force
  }

  return $hook
}

function Read-MngrApprovalResponse {
  param([string]$Path)

  for ($i = 0; $i -lt 10; $i++) {
    try {
      if (Test-Path -LiteralPath $Path) {
        $content = Get-Content -Raw -LiteralPath $Path -ErrorAction Stop
        if (-not [string]::IsNullOrWhiteSpace($content)) {
          return ($content | ConvertFrom-Json -ErrorAction Stop)
        }
      }
    } catch {
      Start-Sleep -Milliseconds 50
    }
  }

  return $null
}

function ConvertTo-ClaudePermissionOutput {
  param(
    [string]$Decision,
    [string]$Reason,
    $UpdatedPermissions
  )

  $validDecision = if (@("allow", "deny") -contains $Decision) { $Decision } else { "deny" }
  $decisionObject = @{
    behavior = $validDecision
  }

  if ($validDecision -eq "deny") {
    $decisionObject.message = if ($null -eq $Reason) { "" } else { $Reason }
    $decisionObject.interrupt = $false
  } elseif ($null -ne $UpdatedPermissions) {
    $decisionObject.updatedPermissions = $UpdatedPermissions
  }

  @{
    "continue" = $true
    suppressOutput = $true
    hookSpecificOutput = @{
      hookEventName = "PermissionRequest"
      decision = $decisionObject
    }
  } | ConvertTo-Json -Compress -Depth 10
}

$payload = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($payload)) {
  exit 0
}

try {
  $mngrPayload = ConvertTo-MngrHookPayload -PayloadText $payload.Trim()
} catch {
  exit 0
}

if ($env:MNGR_HOOK_TEST_MODE -eq "payload") {
  $mngrPayload | ConvertTo-Json -Compress -Depth 100
  exit 0
}

if ($env:MNGR_HOOK_TEST_MODE -eq "permission-output") {
  $decision = if ([string]::IsNullOrWhiteSpace($env:MNGR_HOOK_TEST_DECISION)) { "allow" } else { $env:MNGR_HOOK_TEST_DECISION }
  $reason = if ([string]::IsNullOrWhiteSpace($env:MNGR_HOOK_TEST_REASON)) { "" } else { $env:MNGR_HOOK_TEST_REASON }
  $updatedPermissions = if ($decision -eq "allow") { $mngrPayload.permission_suggestions } else { $null }
  ConvertTo-ClaudePermissionOutput -Decision $decision -Reason $reason -UpdatedPermissions $updatedPermissions
  exit 0
}

$jsonLine = ($mngrPayload | ConvertTo-Json -Compress -Depth 100)

try {
  $client = [System.IO.Pipes.NamedPipeClientStream]::new(".", "mngr", [System.IO.Pipes.PipeDirection]::Out)
  $client.Connect(50)

  $writer = [System.IO.StreamWriter]::new($client, [System.Text.UTF8Encoding]::new($false))
  $writer.NewLine = "`n"
  $writer.WriteLine($jsonLine)
  $writer.Flush()
  $writer.Dispose()
  $client.Dispose()
} catch {
  exit 0
}

if ($mngrPayload.hook_event_name -ne "PermissionRequest") {
  exit 0
}

$responsesRoot = Join-Path $env:LOCALAPPDATA "mngr\responses"
$responsePath = Join-Path $responsesRoot "$($mngrPayload.request_id).json"
$deadline = (Get-Date).AddSeconds(3600)

while ((Get-Date) -lt $deadline) {
  $response = Read-MngrApprovalResponse -Path $responsePath
  if ($null -ne $response) {
    Remove-Item -LiteralPath $responsePath -Force -ErrorAction SilentlyContinue
    ConvertTo-ClaudePermissionOutput -Decision ([string]$response.decision) -Reason ([string]$response.reason) -UpdatedPermissions $response.updatedPermissions
    exit 0
  }

  Start-Sleep -Milliseconds 200
}

exit 0
