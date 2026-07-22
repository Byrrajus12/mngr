$ErrorActionPreference = "SilentlyContinue"

$GatedTools = @("Bash", "Write", "Edit", "MultiEdit", "NotebookEdit")

function ConvertTo-MngrHookPayload {
  param([string]$PayloadText)

  $hook = $PayloadText | ConvertFrom-Json
  $toolName = [string]$hook.tool_name
  $requestId = [guid]::NewGuid().ToString()
  $gated = $hook.hook_event_name -eq "PreToolUse" -and $GatedTools -contains $toolName

  $hook | Add-Member -NotePropertyName "request_id" -NotePropertyValue $requestId -Force
  $hook | Add-Member -NotePropertyName "gated" -NotePropertyValue $gated -Force

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
    [string]$Reason
  )

  $validDecision = if (@("allow", "deny", "ask") -contains $Decision) { $Decision } else { "ask" }
  return @{
    hookSpecificOutput = @{
      hookEventName = "PreToolUse"
      permissionDecision = $validDecision
      permissionDecisionReason = if ($null -eq $Reason) { "" } else { $Reason }
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

if (-not $mngrPayload.gated) {
  exit 0
}

$responsesRoot = Join-Path $env:LOCALAPPDATA "mngr\responses"
$responsePath = Join-Path $responsesRoot "$($mngrPayload.request_id).json"
$deadline = (Get-Date).AddSeconds(570)

while ((Get-Date) -lt $deadline) {
  $response = Read-MngrApprovalResponse -Path $responsePath
  if ($null -ne $response) {
    Remove-Item -LiteralPath $responsePath -Force -ErrorAction SilentlyContinue
    ConvertTo-ClaudePermissionOutput -Decision ([string]$response.decision) -Reason ([string]$response.reason)
    exit 0
  }

  Start-Sleep -Milliseconds 200
}

ConvertTo-ClaudePermissionOutput -Decision "ask" -Reason "mngr approval timed out"
exit 0
